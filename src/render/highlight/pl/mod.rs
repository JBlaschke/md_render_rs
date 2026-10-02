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
use crate::html::push_text;

use classes::{Class, Overlay, Scheme, TextRules, WordRules};
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
    let (grammar, overlay_scope) = with_jsx(grammar, tm_scope, code);
    let scheme = Scheme::get(variant);
    let overlay = scheme.overlay(overlay_scope);
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
    let mut html = html.unwrap_or_else(|_| crate::html::text(code).into_owned());
    trim_trailing_newlines(&mut html, tm_scope);
    GithubCode {
        scope,
        html: Some(html),
    }
}

/// The grammar and the overlay scope for a block: JavaScript containing JSX is tokenized with
/// the TypeScriptReact grammar and classified like TSX (github.com's tree-sitter output for JSX
/// is the same in `source.js` and `source.tsx` blocks), see [`jsx`](super::jsx). The block keeps
/// its Linguist scope (`highlight-source-js`).
fn with_jsx<'a>(grammar: Grammar, tm_scope: &'a str, code: &str) -> (Grammar, &'a str) {
    const JSX_SCOPE: &str = "source.tsx";
    if tm_scope == "source.js" && super::jsx::contains_jsx(code) {
        if let Some(tsx) = engine::syntax_by_scope(JSX_SCOPE) {
            return (Grammar::Syntect(tsx), JSX_SCOPE);
        }
    }
    (grammar, tm_scope)
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

/// One scope on the writer's stack.
#[derive(Clone, Copy, Debug)]
struct Entry<'s> {
    class: Class,
    /// Replace `class` (possibly by no class) with the first alternative whose condition the
    /// scope's first text satisfies (tree-sitter-style text predicates, GH §7.7); resolved when
    /// the span is first opened.
    by_text: Option<&'s TextRules>,
}

/// Builds nested-span HTML from a scope event stream (GH §7.6).
pub(crate) struct SpanWriter<'s> {
    scheme: &'s Scheme,
    overlay: Option<&'s Overlay>,
    out: String,
    /// One entry per scope on the tokenizer's stack.
    entries: Vec<Entry<'s>>,
    /// Number of entries with a class.
    some: usize,
    /// How many of the entries with a class (outermost first) are currently open in `out`.
    opened: usize,
    /// A scope pushed while `entries` has this length is a syntax's root scope: it gets no
    /// class (GH §7.6 "skip the root scope", which would otherwise map to `pl-s1`).
    root_depth: usize,
    /// Innermost scope of the previous non-blank text (for `PREVIOUS +` rules).
    previous: Option<Scope>,
    /// The code being tokenized, when every text event is the next slice of it: enables the
    /// overlay's word rules, whose conditions look at the code around a word.
    source: Option<&'s str>,
    /// Byte offset in `source` of the next text event.
    pos: usize,
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
            source: None,
            pos: 0,
        }
    }

    /// Enable word rules: the text events will be the consecutive slices of `code`.
    fn with_source(mut self, code: &'s str) -> Self {
        self.source = Some(code);
        self
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
                let at = self.pos;
                self.pos += text.len();
                // Word rules need the text's position in the code; never guess it.
                if self
                    .source
                    .is_some_and(|s| s.get(at..self.pos) != Some(text))
                {
                    self.source = None;
                }
                let words = self.source.map(|_| (stack, at));
                self.text_with(text, words);
                if !text.trim().is_empty() {
                    self.previous = stack.last().copied();
                }
            }
        }
    }

    /// The word-rule tables that apply to text under `stack`, innermost first: from the
    /// innermost scope outwards, every scope's rules, up to the first scope with a class of its
    /// own.
    fn word_tables(&self, stack: &[Scope]) -> Vec<&'s WordRules> {
        let mut tables = Vec::new();
        let Some(overlay) = self.overlay.filter(|o| o.has_words()) else {
            return tables;
        };
        // One entry per scope on the stack (the writer mirrors the tokenizer's stack).
        if self.entries.len() != stack.len() {
            return tables;
        }
        for i in (0..stack.len()).rev() {
            if let Some(rules) = overlay.words(stack[i], &stack[..i]) {
                tables.push(rules);
            }
            if self.entries[i].class.is_some() {
                break;
            }
        }
        tables
    }

    /// Emit `body` (no newline) at byte `at` of `source`, wrapping the words that a rule in
    /// `tables` classifies.
    fn push_words(&mut self, body: &str, at: usize, source: &str, tables: &[&WordRules]) {
        let mut rest = body;
        let mut start = at;
        while let Some(c) = rest.chars().next() {
            let word_char = |c: char| c.is_alphanumeric() || c == '_';
            let len = if c.is_whitespace() {
                rest.find(|c: char| !c.is_whitespace())
                    .unwrap_or(rest.len())
            } else if word_char(c) {
                rest.find(|c: char| !word_char(c)).unwrap_or(rest.len())
            } else {
                c.len_utf8()
            };
            let (word, tail) = rest.split_at(len);
            let class = if c.is_whitespace() {
                None
            } else {
                let end = start + len;
                tables
                    .iter()
                    .flat_map(|rules| rules.iter())
                    .find(|(is, near, _)| is.matches(word) && near.holds(source, start, end))
                    .and_then(|&(_, _, class)| class)
            };
            match class {
                Some(class) => {
                    self.out.push_str("<span class=\"");
                    self.out.push_str(class);
                    self.out.push_str("\">");
                    push_text(&mut self.out, word);
                    self.out.push_str("</span>");
                }
                None => push_text(&mut self.out, word),
            }
            rest = tail;
            start += len;
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

    fn push_entry(&mut self, entry: Entry<'s>) {
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
        self.text_with(text, None);
    }

    /// [`SpanWriter::text`], applying the word rules for `stack` when `words` gives the text's
    /// stack and its byte offset in [`SpanWriter::source`].
    fn text_with(&mut self, text: &str, words: Option<(&[Scope], usize)>) {
        let (body, newline) = match text.strip_suffix('\n') {
            Some(body) => (body, true),
            None => (text, false),
        };
        if !body.is_empty() {
            self.open_pending(body);
            // After `open_pending`: entries settle their text-dependent class there.
            let tables = words.map(|(stack, at)| (self.word_tables(stack), at));
            match (tables, self.source) {
                (Some((tables, at)), Some(source)) if !tables.is_empty() => {
                    self.push_words(body, at, source, &tables)
                }
                _ => push_text(&mut self.out, body),
            }
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
        let mut dropped = 0;
        for entry in self.entries.iter_mut() {
            let Some(class) = entry.class else { continue };
            seen += 1;
            if seen <= self.opened {
                continue;
            }
            // A span opened for the first time settles its text-dependent class; an entry
            // whose class turns out to be `None` no longer counts as classified.
            let alternative = entry.by_text.take().and_then(|rules| {
                rules
                    .iter()
                    .find(|(cond, _)| cond.matches(first))
                    .map(|&(_, alt)| alt)
            });
            entry.class = alternative.unwrap_or(Some(class));
            let Some(class) = entry.class else {
                dropped += 1;
                continue;
            };
            self.out.push_str("<span class=\"");
            self.out.push_str(class);
            self.out.push_str("\">");
        }
        self.some -= dropped;
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
    let mut writer = SpanWriter::new(scheme, overlay, code.len() * 2).with_source(code);
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
            crate::html::text("<a href=\"x\">&</a>"),
            "&lt;a href=\"x\"&gt;&amp;&lt;/a&gt;"
        );
    }

    #[test]
    fn crlf_is_normalized() {
        let a = gh("let x = 1;\r\nlet y = 2;\r\n", "rust");
        let b = gh("let x = 1;\nlet y = 2;\n", "rust");
        assert_eq!(a, b);
    }

    /// GH-HL-1: JSX in ```jsx / ```js blocks. With two-face's JavaScript grammar `</div>` was a
    /// division plus a regex literal (string color over the rest of the block) and the closing
    /// brackets after JSX `invalid.illegal` (`pl-ii`).
    #[test]
    fn jsx_in_javascript() {
        for lang in ["jsx", "js", "javascript"] {
            let r = gh("const a = <div>x</div>;\nconst b = 1;\n", lang);
            assert_eq!(r.scope.as_deref(), Some("source.js"), "{lang}");
            let html = r.html.unwrap();
            assert!(!html.contains("pl-pds"), "{lang}: {html}");
            assert!(!html.contains("pl-ii"), "{lang}: {html}");
            // github.com (tree-sitter-javascript): `<`/`>` pl-c1, `</` pl-kos, names pl-s1.
            assert_eq!(
                html,
                "<span class=\"pl-k\">const</span> <span class=\"pl-s1\">a</span> \
                 <span class=\"pl-c1\">=</span> <span class=\"pl-c1\">&lt;</span>\
                 <span class=\"pl-s1\">div</span><span class=\"pl-c1\">&gt;</span>x\
                 <span class=\"pl-kos\">&lt;/</span><span class=\"pl-s1\">div</span>\
                 <span class=\"pl-c1\">&gt;</span><span class=\"pl-kos\">;</span>\n\
                 <span class=\"pl-k\">const</span> <span class=\"pl-s1\">b</span> \
                 <span class=\"pl-c1\">=</span> <span class=\"pl-c1\">1</span>\
                 <span class=\"pl-kos\">;</span>",
                "{lang}"
            );
        }
        let component = "export function App({ items }) {\n  return (\n    <ul className=\"x\">\
            \n      {items.map((i) => <Item key={i.id} {...i} />)}\n      <p>n: {n}</p>\n    \
            </ul>\n  );\n}\n";
        let html = gh(component, "jsx").html.unwrap();
        assert!(
            !html.contains("pl-ii") && !html.contains("pl-pds"),
            "{html}"
        );
        assert!(html.ends_with("<span class=\"pl-kos\">}</span>"), "{html}");
        assert!(
            html.contains(
                "<span class=\"pl-c1\">&lt;</span><span class=\"pl-v\">Item</span> \
                 <span class=\"pl-c1\">key</span>"
            ),
            "{html}"
        );
        assert!(
            html.contains("<span class=\"pl-kos\">{</span>...<span class=\"pl-s1\">i</span>"),
            "{html}"
        );
        assert!(
            html.contains("<span class=\"pl-kos\">/&gt;</span>"),
            "{html}"
        );
    }

    #[test]
    fn jsx_grammar_only_for_jsx() {
        let js = engine::syntax_by_scope("source.js").unwrap();
        let pick = |code: &str| match with_jsx(Grammar::Syntect(js), "source.js", code) {
            (Grammar::Syntect(s), overlay) => (s.scope.build_string(), overlay),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            pick("if (a < b) f(a);\n"),
            ("source.js".into(), "source.js")
        );
        assert_eq!(pick("x = '<b>';\n"), ("source.js".into(), "source.js"));
        assert_eq!(pick("x = <b>y</b>;\n"), ("source.tsx".into(), "source.tsx"));
        // Other scopes never switch.
        let ts = engine::syntax_by_scope("source.ts").unwrap();
        assert!(matches!(
            with_jsx(Grammar::Syntect(ts), "source.ts", "x = <b>y</b>;\n"),
            (Grammar::Syntect(s), "source.ts") if s.scope.build_string() == "source.ts"
        ));
    }

    /// Assert that `gh(code, lang)` contains every one of `parts`.
    #[track_caller]
    fn has(code: &str, lang: &str, parts: &[&str]) {
        let html = gh(code, lang).html.unwrap();
        for part in parts {
            assert!(html.contains(part), "{lang}: {part:?} not in\n{html}");
        }
    }

    /// GH-HL-2: classes github.com's tree-sitter (and TextMate) grammars give that the Sublime
    /// scopes alone got wrong.
    #[test]
    fn language_class_fixes() {
        // C: `typedef`/`struct` keywords, not types; `sizeof` a keyword, not an operator.
        has(
            "typedef struct node { int v; } node_t;\nint n = sizeof(node_t);\n",
            "c",
            &[
                "<span class=\"pl-k\">typedef</span> <span class=\"pl-k\">struct</span>",
                "<span class=\"pl-k\">sizeof</span>",
            ],
        );
        // Rust: imported names and derive names are constructors, `dyn` a keyword, lifetimes
        // `'` + label, enum variants constructors and their fields properties, path calls
        // functions, uppercase path segments types; `self` parameters builtins.
        has(
            "use std::fmt::Display;\n#[derive(Debug)]\nenum E<'a> { A(&'a dyn Display), \
             B { w: u8 } }\nfn f(&self) { let m = HashMap::new(); }\n",
            "rust",
            &[
                "<span class=\"pl-kos\">::</span><span class=\"pl-v\">Display</span>\
                 <span class=\"pl-kos\">;</span>",
                "derive<span class=\"pl-kos\">(</span><span class=\"pl-v\">Debug</span>",
                "<span class=\"pl-c1\">'</span><span class=\"pl-ent\">a</span> \
                 <span class=\"pl-k\">dyn</span> <span class=\"pl-smi\">Display</span>",
                "<span class=\"pl-v\">A</span><span class=\"pl-kos\">(</span>",
                "<span class=\"pl-v\">B</span> <span class=\"pl-kos\">{</span> \
                 <span class=\"pl-c1\">w</span>",
                "<span class=\"pl-c1\">&amp;</span><span class=\"pl-smi\">self</span>",
                "<span class=\"pl-smi\">HashMap</span><span class=\"pl-kos\">::</span>\
                 <span class=\"pl-en\">new</span>",
            ],
        );
        // Words in strings and comments are left alone.
        has(
            "let s = \"HashMap::new()\"; // foo(x)\n",
            "rust",
            &[
                "<span class=\"pl-s\">\"HashMap::new()\"</span>",
                "<span class=\"pl-c\">// foo(x)</span>",
            ],
        );
        // Makefile: the name inside `$(…)` is a variable, `$(func …)` a string.
        has(
            "OBJS = $(SRCS)\nall: $(wildcard *.c)\n",
            "makefile",
            &[
                "<span class=\"pl-s\">$(<span class=\"pl-smi\">SRCS</span>)</span>",
                "<span class=\"pl-s\">$(<span class=\"pl-c1\">wildcard</span> </span>",
            ],
        );
        // PHP: `echo`, `fn` and promoted-property modifiers are keywords, declared classes
        // plain, instantiated ones constructors, operators plain, namespaces per segment.
        has(
            "<?php\nnamespace App\\Http;\nclass A extends B { public function __construct(\
             private int $a) { echo $this->a; $f = fn($x) => $x * 2; throw new E(); } }\n",
            "php",
            &[
                "<span class=\"pl-v\">App</span>\\<span class=\"pl-v\">Http</span>",
                "<span class=\"pl-k\">class</span> A <span class=\"pl-k\">extends</span> B {",
                "(<span class=\"pl-k\">private</span> <span class=\"pl-smi\">int</span>",
                "<span class=\"pl-k\">echo</span> <span class=\"pl-s1\">\
                 <span class=\"pl-c1\">$</span><span class=\"pl-smi\">this</span></span>",
                "<span class=\"pl-k\">fn</span>(",
                ") =&gt; ",
                "<span class=\"pl-k\">new</span> <span class=\"pl-v\">E</span>",
            ],
        );
        // Python: decorator calls and exception constructors are calls, `in` an operator,
        // `%s` placeholders not looked into.
        has(
            "@dataclass(frozen=True)\nclass P:\n    def f(self, x: Optional[int]):\n        \
             for i in x: raise ValueError(\"%s\" % i)\n",
            "python",
            &[
                "<span class=\"pl-en\"><span class=\"pl-en\">dataclass</span></span>",
                "<span class=\"pl-v\">Optional</span>",
                "<span class=\"pl-c1\">in</span>",
                "<span class=\"pl-en\">ValueError</span>(",
                "<span class=\"pl-s\">%s\"</span>",
            ],
        );
        // Kotlin: constructor calls are entity names.
        has(
            "val u = listOf(User(\"a\"))\n",
            "kotlin",
            &["<span class=\"pl-c1\">listOf</span>(<span class=\"pl-en\">User</span>("],
        );
    }

    /// Word rules only run where every text event is the next slice of the code.
    #[test]
    fn word_rules_need_the_source() {
        let scheme = Scheme::get(DEFAULT_VARIANT);
        let overlay = scheme.overlay("source.rust");
        let syntax = engine::syntax_by_scope("source.rust").unwrap();
        let code = "fn f(x: &dyn T) {}\n";
        let mut with = SpanWriter::new(scheme, overlay, 0).with_source(code);
        let mut without = SpanWriter::new(scheme, overlay, 0);
        engine::tokenize(code, syntax, Limits::DEFAULT, |e, s| {
            with.event(e, s);
            without.event(e, s);
        })
        .unwrap();
        assert!(with.finish().contains("<span class=\"pl-k\">dyn</span>"));
        assert!(without.finish().contains("</span>dyn T"));
        // A source that does not match the events disables them instead of misreading it.
        let mut wrong = SpanWriter::new(scheme, overlay, 0).with_source("xx");
        engine::tokenize(code, syntax, Limits::DEFAULT, |e, s| wrong.event(e, s)).unwrap();
        assert!(wrong.finish().contains("</span>dyn T"));
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
