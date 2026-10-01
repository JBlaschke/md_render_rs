//! GitHub code highlighting: Linguist lookup + PrettyLights classes (GH §7, D5).
//!
//! Output shape (GH §7.1/§7.6): the code is HTML-escaped (`&`, `<`, `>` only; quotes stay
//! literal), every pushed TextMate scope that maps to a class opens a `<span class="pl-*">` that
//! nests inside the spans of its enclosing scopes, the syntax's root scope is skipped, and at
//! every newline all open spans are closed and re-opened on the next line (lazily: a span that
//! ends right at a line end is not re-opened as an empty span). The final newline is dropped.

mod classes;
mod diff;
pub(crate) mod linguist;
mod session;

use syntect::parsing::{Scope, SyntaxReference};

use super::GithubCode;
use super::engine::{self, Event, Limits};

use classes::{Class, Overlay, Scheme, TextIs};
pub(crate) use classes::{DEFAULT_VARIANT, Variant};

/// How a Linguist language is tokenized.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Grammar {
    /// A two-face syntax.
    Syntect(&'static SyntaxReference),
    /// `text.shell-session`: a port of Linguist's tiny shell-session grammar with the shell
    /// syntax for commands (GH §7.2 `console`, CQ §1.7).
    ShellSession,
    /// `source.diff`: a port of Linguist's line-based diff grammar.
    Diff,
}

/// Linguist `tm_scope` → two-face syntax scope, for grammars two-face has under another name
/// (CQ §1.7; reviewed by hand: only grammars for the same language, or a true superset/dialect).
const SCOPE_ALIASES: &[(&str, &str)] = &[
    ("source.actionscript.3", "source.actionscript.2"),
    ("source.assembly", "source.asm.x86_64"),
    ("source.batchfile", "source.dosbatch"),
    ("source.cfscript", "source.cfml.script"),
    ("source.commonlisp", "source.lisp"),
    ("source.css.less", "source.less"),
    ("source.css.scss", "source.scss"),
    ("source.csv", "text.csv"),
    ("source.cython", "source.python"),
    ("source.desktop", "source.ini"),
    ("source.dotenv", "source.env"),
    ("source.editorconfig", "source.ini"),
    ("source.emacs.lisp", "source.lisp"),
    ("source.fish", "source.shell.fish"),
    ("source.fortran", "source.fixedform-fortran"),
    ("source.fortran.modern", "source.modern-fortran"),
    ("source.gitattributes", "text.git.attributes"),
    ("source.gitconfig", "text.git.config"),
    ("source.gitignore", "text.git.ignore"),
    ("source.groovy.gradle", "source.groovy"),
    ("source.hcl", "source.terraform"),
    ("source.httpspec", "text.http-request-response"),
    ("source.idris", "source.idris2"),
    ("source.java-properties", "source.java-props"),
    ("source.json.comments", "source.json"),
    ("source.kotlin", "source.Kotlin"),
    ("source.lean", "source.lean4"),
    ("source.mdx", "text.html.markdown"),
    ("source.miniyaml", "source.yaml"),
    ("source.ml", "source.sml"),
    ("source.renpy", "source.python"),
    ("source.scheme", "source.lisp"),
    ("source.ssh-config", "source.ssh_config"),
    ("source.svelte", "text.html.svelte"),
    ("source.textproto", "text.prototxt"),
    ("source.tsql", "source.sql"),
    ("source.tsv", "text.csv.tab"),
    ("go.mod", "source.gomod"),
    ("go.sum", "source.gosum"),
    ("text.crontab", "source.crontab"),
    ("text.eml.basic", "text.eml"),
    ("text.git-commit", "text.git.commit"),
    ("text.html.asciidoc", "text.asciidoc"),
    ("text.html.cfm", "embedding.cfml"),
    ("text.html.django", "text.html.jinja2"),
    ("text.html.erb", "text.html.ruby"),
    ("text.html.php", "embedding.php"),
    ("text.md", "text.html.markdown"),
    ("text.robot", "source.robot"),
    ("text.roff", "text.groff"),
    ("text.xml.svg", "text.xml"),
    ("text.xml.xsl", "text.xml"),
];

/// Linguist scopes whose same-named two-face syntax is a worse match than another one.
const SCOPE_OVERRIDES: &[(&str, &str)] = &[("source.shell", "source.shell.bash")];

/// Pick the grammar for a Linguist `tm_scope` (CQ §1.7): explicit override → same scope in
/// two-face → alias table → `find_syntax_by_token` on the language name. `None`: no grammar
/// (the block is still wrapped as `highlight-<scope>`, without token spans).
pub(crate) fn grammar(tm_scope: &str, language_name: &str) -> Option<Grammar> {
    match tm_scope {
        "text.shell-session" => return Some(Grammar::ShellSession),
        "source.diff" => return Some(Grammar::Diff),
        _ => {}
    }
    let lookup = |table: &[(&str, &'static str)]| {
        table
            .iter()
            .find(|(from, _)| *from == tm_scope)
            .and_then(|(_, to)| engine::syntax_by_scope(to))
    };
    lookup(SCOPE_OVERRIDES)
        .or_else(|| engine::syntax_by_scope(tm_scope))
        .or_else(|| lookup(SCOPE_ALIASES))
        .or_else(|| name_fallback(language_name))
        .map(Grammar::Syntect)
}

/// `find_syntax_by_token(language name)`, restricted to an exact (case-insensitive) syntax-name
/// match: matching the name against file extensions, or using aliases/extensions, picks
/// unrelated grammars far too often (Prolog → Perl, `cls` → TeX, ...).
fn name_fallback(language_name: &str) -> Option<&'static SyntaxReference> {
    engine::syntax_by_token(language_name).filter(|s| s.name.eq_ignore_ascii_case(language_name))
}

/// The shell syntax used for shell-session commands.
pub(crate) fn shell_syntax() -> Option<&'static SyntaxReference> {
    match grammar("source.shell", "Shell") {
        Some(Grammar::Syntect(s)) => Some(s),
        _ => None,
    }
}

/// `highlight::github` with an explicit class-scheme variant.
pub(crate) fn highlight_with(code: &str, lang: &str, variant: Variant) -> GithubCode {
    let Some(language) = linguist::find(lang) else {
        return GithubCode::default();
    };
    let Some(tm_scope) = language.tm_scope.as_deref() else {
        return GithubCode::default();
    };
    let scope = Some(tm_scope.to_owned());
    let Some(grammar) = grammar(tm_scope, &language.name) else {
        return GithubCode { scope, html: None };
    };
    // GH §7.1: CRLF in code is normalized.
    let normalized;
    let code = if code.contains('\r') {
        normalized = code.replace("\r\n", "\n");
        normalized.as_str()
    } else {
        code
    };
    let scheme = Scheme::get(variant);
    let overlay = scheme.overlay(tm_scope);
    let html = match grammar {
        Grammar::Syntect(syntax) => render(code, syntax, scheme, overlay),
        Grammar::ShellSession => session::render(code, scheme, overlay),
        Grammar::Diff if Limits::DEFAULT.admits(code) => {
            let mut w = SpanWriter::new(scheme, overlay, code.len() * 2);
            diff::render(code, &mut w);
            Ok(w.finish())
        }
        Grammar::Diff => Err(engine::Error),
    };
    // Tokenizer failure (limits, parse error, internal panic): same wrapper, no token spans.
    let mut html = html.unwrap_or_else(|_| escape_text(code));
    trim_trailing_newlines(&mut html, tm_scope);
    GithubCode {
        scope,
        html: Some(html),
    }
}

/// Linguist scopes github.com highlights with tree-sitter (GH §7.7, confirmed by output shape:
/// `pl-kos` punctuation, no `pl-pds`/nested comment markers, trailing blank lines dropped).
const TREE_SITTER_SCOPES: &[&str] = &[
    "source.rust",
    "source.python",
    "source.js",
    "source.ts",
    "source.tsx",
    "source.c",
    "source.go",
    "source.css",
    "text.html.basic",
    "source.ruby",
    "source.java",
    "text.html.php",
    "source.cs",
    "source.swift",
    "source.elixir",
    "source.nix",
    "source.regexp",
];

/// GitHub removes the final newline of the code; its tree-sitter highlighter also drops a
/// trailing blank line (every trailing newline), its TextMate path keeps it. Spans are closed
/// at each newline, so trailing newlines are bare text.
fn trim_trailing_newlines(html: &mut String, tm_scope: &str) {
    if TREE_SITTER_SCOPES.contains(&tm_scope) {
        html.truncate(html.trim_end_matches('\n').len());
    } else if html.ends_with('\n') {
        html.pop();
    }
}

/// Escape code text like GitHub (GH §7.1): `&`, `<`, `>` only.
pub(crate) fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 8);
    push_escaped(&mut out, s);
    out
}

fn push_escaped(out: &mut String, s: &str) {
    let mut last = 0;
    for (i, b) in s.bytes().enumerate() {
        let rep = match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            _ => continue,
        };
        out.push_str(&s[last..i]);
        out.push_str(rep);
        last = i + 1;
    }
    out.push_str(&s[last..]);
}

/// One scope on the writer's stack.
#[derive(Clone, Copy, Debug)]
struct Entry {
    class: Class,
    /// Replaces `class` if the scope's first text satisfies the condition (tree-sitter-style
    /// text predicates, GH §7.7); resolved when the span is first opened.
    by_text: Option<(TextIs, &'static str)>,
}

/// Builds nested-span HTML from a scope event stream (GH §7.6).
pub(crate) struct SpanWriter<'s> {
    scheme: &'s Scheme,
    overlay: Option<&'s Overlay>,
    out: String,
    /// One entry per scope on the tokenizer's stack.
    entries: Vec<Entry>,
    /// Number of entries with a class.
    some: usize,
    /// How many of the entries with a class (outermost first) are currently open in `out`.
    opened: usize,
    /// A scope pushed while `entries` has this length is a syntax's root scope: it gets no
    /// class (GH §7.6 "skip the root scope", which would otherwise map to `pl-s1`).
    root_depth: usize,
    /// Innermost scope of the previous non-blank text (for `PREVIOUS +` rules).
    previous: Option<Scope>,
}

impl<'s> SpanWriter<'s> {
    pub(crate) fn new(scheme: &'s Scheme, overlay: Option<&'s Overlay>, cap: usize) -> Self {
        SpanWriter {
            scheme,
            overlay,
            out: String::with_capacity(cap),
            entries: Vec::new(),
            some: 0,
            opened: 0,
            root_depth: 0,
            previous: None,
        }
    }

    /// Start embedded code: the next syntax root scope is pushed on top of the current stack.
    pub(crate) fn embed(&mut self) {
        self.root_depth = self.entries.len();
        self.previous = None;
    }

    pub(crate) fn event(&mut self, event: Event<'_>, stack: &[Scope]) {
        match event {
            Event::Push(scope) => {
                let entry = if self.entries.len() == self.root_depth {
                    Entry {
                        class: None,
                        by_text: None,
                    }
                } else {
                    let parents = stack.get(..stack.len().saturating_sub(1)).unwrap_or(&[]);
                    let (class, by_text) =
                        self.scheme
                            .classify(self.overlay, scope, parents, self.previous);
                    if class.is_some() && Scheme::absorbs(self.overlay, scope, parents) {
                        self.absorb_parent();
                    }
                    Entry { class, by_text }
                };
                self.push_entry(entry);
            }
            Event::Pop => self.pop_class(),
            Event::Text(text) => {
                self.text(text);
                if !text.trim().is_empty() {
                    self.previous = stack.last().copied();
                }
            }
        }
    }

    /// Drop the class of the innermost scope if its span has not been opened yet (no text since
    /// it was pushed), so that the scope being pushed takes its place.
    fn absorb_parent(&mut self) {
        if self.opened < self.some {
            if let Some(last) = self.entries.last_mut().filter(|e| e.class.is_some()) {
                last.class = None;
                self.some -= 1;
            }
        }
    }

    pub(crate) fn push_class(&mut self, class: Class) {
        self.push_entry(Entry {
            class,
            by_text: None,
        });
    }

    fn push_entry(&mut self, entry: Entry) {
        if entry.class.is_some() {
            self.some += 1;
        }
        self.entries.push(entry);
    }

    pub(crate) fn pop_class(&mut self) {
        // syntect reports pops on an already empty stack as well; ignore those.
        if let Some(Entry { class: Some(_), .. }) = self.entries.pop() {
            if self.opened == self.some {
                self.out.push_str("</span>");
                self.opened -= 1;
            }
            self.some -= 1;
        }
    }

    /// Emit text (at most one `\n`, as its last byte).
    pub(crate) fn text(&mut self, text: &str) {
        let (body, newline) = match text.strip_suffix('\n') {
            Some(body) => (body, true),
            None => (text, false),
        };
        if !body.is_empty() {
            self.open_pending(body);
            push_escaped(&mut self.out, body);
        }
        if newline {
            self.close_all();
            self.out.push('\n');
        }
    }

    /// Open the spans of all classified entries not open yet; `first` is the text that follows.
    fn open_pending(&mut self, first: &str) {
        if self.opened == self.some {
            return;
        }
        let mut seen = 0;
        for entry in self.entries.iter_mut() {
            let Some(class) = entry.class else { continue };
            seen += 1;
            if seen <= self.opened {
                continue;
            }
            // A span opened for the first time settles its text-dependent class.
            let class = match entry.by_text.take() {
                Some((cond, alt)) if cond.matches(first) => alt,
                _ => class,
            };
            entry.class = Some(class);
            self.out.push_str("<span class=\"");
            self.out.push_str(class);
            self.out.push_str("\">");
        }
        self.opened = self.some;
    }

    fn close_all(&mut self) {
        for _ in 0..self.opened {
            self.out.push_str("</span>");
        }
        self.opened = 0;
    }

    /// Close everything still open and return the HTML.
    pub(crate) fn finish(mut self) -> String {
        self.close_all();
        self.out
    }
}

/// Tokenize `code` with `syntax` and render nested PrettyLights spans.
fn render(
    code: &str,
    syntax: &'static SyntaxReference,
    scheme: &Scheme,
    overlay: Option<&Overlay>,
) -> Result<String, engine::Error> {
    let mut writer = SpanWriter::new(scheme, overlay, code.len() * 2);
    engine::tokenize(code, syntax, Limits::DEFAULT, |event, stack| {
        writer.event(event, stack)
    })?;
    Ok(writer.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh(code: &str, lang: &str) -> GithubCode {
        highlight_with(code, lang, DEFAULT_VARIANT)
    }

    #[test]
    fn unknown_and_scopeless_languages() {
        assert_eq!(gh("x\n", ""), GithubCode::default());
        assert_eq!(gh("x\n", "no-such-lang"), GithubCode::default());
        assert_eq!(gh("x\n", "text"), GithubCode::default());
        assert_eq!(gh("x\n", "plaintext"), GithubCode::default());
    }

    #[test]
    fn scope_without_grammar() {
        // PowerShell has a Linguist tm_scope but no two-face grammar (CR §2).
        let r = gh("Get-Item\n", "powershell");
        assert_eq!(r.scope.as_deref(), Some("source.powershell"));
        assert_eq!(r.html, None);
    }

    #[test]
    fn spans_close_and_reopen_at_newlines() {
        let r = gh("/* a\nb */\nint x;\n", "c");
        assert_eq!(r.scope.as_deref(), Some("source.c"));
        let html = r.html.unwrap();
        for line in html.split('\n') {
            assert_eq!(
                line.matches("<span").count(),
                line.matches("</span>").count(),
                "{line}"
            );
        }
        assert!(html.starts_with("<span class=\"pl-c\">"), "{html}");
        assert!(html.contains("\n<span class=\"pl-c\">b "), "{html}");
        assert!(!html.ends_with('\n'));
        assert!(!html.contains("<span class=\"pl-c\"></span>"), "{html}");
    }

    #[test]
    fn escaping_matches_github() {
        let r = gh("a = \"<&>'\"\n", "python");
        let html = r.html.unwrap();
        assert!(html.contains("&lt;&amp;&gt;'"), "{html}");
        assert!(html.contains('"'), "quotes stay literal: {html}");
        assert_eq!(
            escape_text("<a href=\"x\">&</a>"),
            "&lt;a href=\"x\"&gt;&amp;&lt;/a&gt;"
        );
    }

    #[test]
    fn crlf_is_normalized() {
        let a = gh("let x = 1;\r\nlet y = 2;\r\n", "rust");
        let b = gh("let x = 1;\nlet y = 2;\n", "rust");
        assert_eq!(a, b);
    }

    #[test]
    fn grammar_selection() {
        let name = |scope: &str, lang: &str| match grammar(scope, lang) {
            Some(Grammar::Syntect(s)) => Some(s.name.clone()),
            Some(Grammar::ShellSession) => Some("<session>".into()),
            Some(Grammar::Diff) => Some("<diff>".into()),
            None => None,
        };
        assert_eq!(name("source.rust", "Rust").as_deref(), Some("Rust"));
        assert_eq!(name("text.md", "Markdown").as_deref(), Some("Markdown"));
        assert_eq!(name("source.kotlin", "Kotlin").as_deref(), Some("Kotlin"));
        assert_eq!(
            name("text.shell-session", "ShellSession").as_deref(),
            Some("<session>")
        );
        assert_eq!(name("source.powershell", "PowerShell"), None);
        assert_eq!(name("source.prolog", "Prolog"), None);
        assert!(shell_syntax().is_some());
    }
}
