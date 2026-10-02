//! GitLab code highlighting: Rouge registry lookup + Rouge short classes (GL §8, D5).
//!
//! GitLab highlights with Rouge 5.1.0 and formats with `Rouge::Formatters::HTMLGitlab`
//! (`R/gitlab-src/lib/rouge/formatters/html_gitlab.rb`, GL §8.2): one entry per source line,
//! tokens as `<span class="{short}">escaped</span>` (plain `Text` unwrapped), a token spanning
//! lines split per line, empty pieces dropped. We tokenize with syntect + two-face and map scopes
//! to Rouge classes ([`classes`]); `diff`, `console` and `docker` are ports of the Rouge
//! lexers' line logic ([`diff`], [`console`], [`docker`]).

mod classes;
mod console;
mod diff;
mod docker;
pub(crate) mod registry;
mod words;

use std::ops::Range;

use syntect::parsing::SyntaxReference;

use super::GitlabCode;
use super::engine::{self, Event, Limits};
pub(crate) use classes::{DEFAULT_VARIANT, Variant};
use classes::{ForLang, Scheme};
use registry::{PLAINTEXT, Resolved};

/// A classified piece of the (CRLF-normalized) code: `class` `None` = plain text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    pub class: Option<&'static str>,
    pub range: Range<usize>,
}

impl Segment {
    fn new(class: Option<&'static str>, range: Range<usize>) -> Self {
        Segment { class, range }
    }
}

/// Rouge tag → two-face syntax scope (`None`: no grammar, plain lines), for tags where the
/// generic selection below picks nothing or the wrong grammar (a shared file extension:
/// `apex` → TeX via `.cls`, `prolog` → Pascal via `.pro`, ...). Reviewed against the full table
/// of 234 Rouge lexers.
const TAG_SYNTAX: &[(&str, Option<&str>)] = &[
    ("apache", Some("source.apacheconf")),
    ("apex", None),
    ("cisco_ios", None),
    ("cobol", None),
    ("conf", None),
    ("fsharp", Some("source.fsharp")),
    ("json-doc", Some("source.json")),
    ("jsx", Some("source.js")),
    ("kick_assembler", None),
    ("mason", None),
    ("mathematica", None),
    ("openedge", None),
    ("plsql", Some("source.sql")),
    ("prolog", None),
    ("reasonml", None),
    ("supercollider", None),
    ("systemverilog", Some("source.systemverilog")),
];

/// Pick the syntect syntax for a Rouge tag (GL §8.1: tag, then the user's string, then the
/// lexer's file extensions).
pub(crate) fn select_syntax(tag: &str, user: &str) -> Option<&'static SyntaxReference> {
    if let Some((_, scope)) = TAG_SYNTAX.iter().find(|(t, _)| *t == tag) {
        return scope.and_then(engine::syntax_by_scope);
    }
    engine::syntax_by_token(tag)
        .or_else(|| engine::syntax_by_token(user))
        .or_else(|| {
            registry::extensions(tag)
                .iter()
                .find_map(|e| engine::syntax_by_extension(e))
        })
        .filter(|s| s.scope.build_string() != "text.plain")
}

/// Normalized code and its classified segments, for `highlight::gitlab` and the evaluation.
pub(crate) fn segments_for(code: &str, lang: &str, variant: Variant) -> (String, Vec<Segment>) {
    // CRLF → LF (Rouge drops every `\r` in its escaping anyway).
    let code = code.replace("\r\n", "\n");
    let resolved = registry::resolve(lang);
    let plain = |code: &str| vec![Segment::new(None, 0..code.len())];
    let segments = match resolved {
        Resolved::Verbatim(_) | Resolved::Rouge(PLAINTEXT) => plain(&code),
        Resolved::Rouge("diff") => diff::tokens(&code)
            .into_iter()
            .scan(0usize, |pos, (class, text)| {
                let start = *pos;
                *pos += text.len();
                Some(Segment::new(class, start..*pos))
            })
            .collect(),
        Resolved::Rouge(tag) => {
            let scheme = Scheme::get(variant);
            let mut out = Vec::new();
            // The console lexer hands commands to the shell lexer.
            let lexer = if tag == "console" { "shell" } else { tag };
            let result = if tag == "console" {
                console::segments(&code, &scheme.for_lang(lexer), &mut out)
            } else if tag == "docker" {
                docker::segments(&code, &scheme.for_lang("shell"), &mut out)
            } else {
                match select_syntax(tag, lang) {
                    Some(syntax) => tokenize(&code, syntax, &scheme.for_lang(tag), &mut out),
                    None => Err(engine::Error),
                }
            };
            match result {
                Ok(()) if scheme.words => {
                    let out = match words::for_tag(lexer) {
                        Some(words) => words.refine(&code, out),
                        None => out,
                    };
                    if words::WHITESPACE_TAGS.contains(&tag) {
                        words::mark_whitespace(&code, out)
                    } else {
                        out
                    }
                }
                Ok(()) => out,
                Err(engine::Error) => plain(&code),
            }
        }
    };
    (code, segments)
}

/// The language-independent class scheme of `variant` (evaluation only).
#[cfg(test)]
pub(crate) fn classes_for_eval(variant: Variant) -> ForLang<'static> {
    Scheme::get(variant).for_lang("")
}

/// `highlight::gitlab` with an explicit class-scheme variant.
pub(crate) fn highlight_with(code: &str, lang: &str, variant: Variant) -> GitlabCode {
    let tag = registry::resolve(lang).tag().to_owned();
    let (code, segments) = segments_for(code, lang, variant);
    GitlabCode {
        tag,
        lines: build_lines(&code, &segments),
    }
}

/// Tokenize with syntect and classify each text segment.
fn tokenize(
    code: &str,
    syntax: &'static SyntaxReference,
    scheme: &ForLang<'_>,
    out: &mut Vec<Segment>,
) -> Result<(), engine::Error> {
    let mut pos = 0usize;
    engine::tokenize(code, syntax, Limits::DEFAULT, |event, stack| {
        if let Event::Text(text) = event {
            let range = pos..pos + text.len();
            pos = range.end;
            out.push(Segment::new(scheme.classify(stack), range));
        }
    })
}

/// `HTMLGitlab#stream` + `token_lines` (GL §8.2): merge adjacent pieces of the same class
/// (Rouge's lexers consolidate equal adjacent tokens), split at `\n`, drop empty pieces, and
/// emit one entry per line. A trailing newline does not start another line; empty code has no
/// lines.
pub(crate) fn build_lines(code: &str, segments: &[Segment]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    // The piece being accumulated: (class, start, end) in `code`.
    let mut piece: Option<(Option<&'static str>, usize, usize)> = None;
    let mut line_has_text = false;
    for seg in segments {
        let Some(text) = code.get(seg.range.clone()) else {
            continue;
        };
        let mut start = seg.range.start;
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 {
                flush(code, &mut piece, &mut line);
                lines.push(std::mem::take(&mut line));
                line_has_text = false;
            }
            if part.is_empty() {
                start += 1;
                continue;
            }
            let end = start + part.len();
            match &mut piece {
                Some((class, _, piece_end)) if *class == seg.class && *piece_end == start => {
                    *piece_end = end;
                }
                _ => {
                    flush(code, &mut piece, &mut line);
                    piece = Some((seg.class, start, end));
                }
            }
            line_has_text = true;
            start = end + 1;
        }
    }
    flush(code, &mut piece, &mut line);
    if line_has_text {
        lines.push(line);
    }
    lines
}

fn flush(code: &str, piece: &mut Option<(Option<&'static str>, usize, usize)>, line: &mut String) {
    let Some((class, start, end)) = piece.take() else {
        return;
    };
    let text = &code[start..end];
    match class {
        Some(class) => {
            line.push_str("<span class=\"");
            line.push_str(class);
            line.push_str("\">");
            push_rouge_escaped(line, text);
            line.push_str("</span>");
        }
        None => push_rouge_escaped(line, text),
    }
}

/// `Gitlab::Unicode` bidi control characters (`\p{Bidi_Control}`).
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Non-ASCII `\p{Space_Separator}` characters, which GitLab replaces with plain spaces.
fn is_non_ascii_space(c: char) -> bool {
    matches!(
        c,
        '\u{00A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// The tooltip GitLab puts on bidi control characters (`Gitlab::Unicode.bidi_warning`).
const BIDI_WARNING: &str = "Potentially unwanted character detected: Unicode BiDi Control";

/// Rouge's `escape_special_html_chars` (`&`, `<`, `>`; `\r` removed) plus `HTMLGitlab`'s
/// `replace_space_characters` and `highlight_unicode_control_characters` (GL §6.8, §8.2).
fn push_rouge_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => {}
            c if is_non_ascii_space(c) => out.push(' '),
            c if is_bidi_control(c) => {
                out.push_str(
                    "<span class=\"unicode-bidi has-tooltip\" data-toggle=\"tooltip\" title=\"",
                );
                out.push_str(BIDI_WARNING);
                out.push_str("\">");
                out.push(c);
                out.push_str("</span>");
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gl(code: &str, lang: &str) -> GitlabCode {
        highlight_with(code, lang, DEFAULT_VARIANT)
    }

    fn seg(class: Option<&'static str>, r: Range<usize>) -> Segment {
        Segment::new(class, r)
    }

    #[test]
    fn line_building() {
        let code = "ab\ncd\n";
        let s = [seg(Some("k"), 0..1), seg(Some("k"), 1..4), seg(None, 4..6)];
        assert_eq!(
            build_lines(code, &s),
            ["<span class=\"k\">ab</span>", "<span class=\"k\">c</span>d"]
        );
        assert!(build_lines("", &[]).is_empty());
        assert_eq!(build_lines("\n", &[seg(None, 0..1)]), [""]);
        assert_eq!(build_lines("a\n\n", &[seg(None, 0..3)]), ["a", ""]);
        assert_eq!(build_lines("a\nb", &[seg(None, 0..3)]), ["a", "b"]);
    }

    #[test]
    fn escaping_matches_rouge() {
        let mut s = String::new();
        push_rouge_escaped(&mut s, "a<b>&\"c\"\r'\u{00A0}");
        assert_eq!(s, "a&lt;b&gt;&amp;\"c\"' ");
        let mut s = String::new();
        push_rouge_escaped(&mut s, "x\u{202E}y");
        assert!(
            s.starts_with("x<span class=\"unicode-bidi has-tooltip\""),
            "{s}"
        );
        assert!(s.ends_with("\u{202E}</span>y"), "{s}");
    }

    #[test]
    fn plaintext_and_verbatim() {
        let r = gl("a < b\n\nc\n", "");
        assert_eq!(r.tag, "plaintext");
        assert_eq!(r.lines, ["a &lt; b", "", "c"]);
        let r = gl("graph TD;\n  A-->B\n", "mermaid");
        assert_eq!(r.tag, "mermaid");
        assert_eq!(r.lines, ["graph TD;", "  A--&gt;B"]);
        let r = gl("x = 1\n", "Python");
        assert_eq!(r.tag, "plaintext");
        assert_eq!(r.lines, ["x = 1"]);
        assert!(gl("", "rust").lines.is_empty());
    }

    #[test]
    fn diff_lines() {
        let r = gl("--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n z\n", "diff");
        assert_eq!(r.tag, "diff");
        assert_eq!(
            r.lines,
            [
                "<span class=\"gd\">--- a</span>",
                "<span class=\"gi\">+++ b</span>",
                "<span class=\"p\">@@ -1 +1 @@</span>",
                "<span class=\"gd\">-x</span>",
                "<span class=\"gi\">+y</span>",
                " z",
            ]
        );
        assert_eq!(gl("-x\n", "patch").tag, "diff");
    }

    #[test]
    fn console_lines() {
        // R/rouge_samples/console.gitlab.html, lines 2, 3 and 5 (output), and the prompt.
        let r = gl(
            "$ cargo build --release\n   Compiling foo v0.1.0\n$ echo \"hi\"\nhi\n",
            "console",
        );
        assert_eq!(r.tag, "console");
        assert!(
            r.lines[0].starts_with("<span class=\"gp\">$</span><span class=\"w\"> </span>"),
            "{}",
            r.lines[0]
        );
        assert_eq!(
            r.lines[1],
            "<span class=\"go\">   Compiling foo v0.1.0</span>"
        );
        assert_eq!(r.lines[3], "<span class=\"go\">hi</span>");
    }

    /// GL-HL-1: lines as Rouge 5.1.0 with GitLab's formatter renders them
    /// (`R/tools/rouge_gitlab.rb`).
    #[test]
    fn rouge_lexer_fixes() {
        // YAML: a key the grammar reads as a boolean, flow sequences, opening quotes.
        let r = gl("on: [push, pull_request]\nenv:\n  A: \"q\"\n", "yaml");
        assert_eq!(
            r.lines,
            [
                "<span class=\"na\">on</span><span class=\"pi\">:</span> \
                 <span class=\"pi\">[</span><span class=\"nv\">push</span>\
                 <span class=\"pi\">,</span> <span class=\"nv\">pull_request</span>\
                 <span class=\"pi\">]</span>",
                "<span class=\"na\">env</span><span class=\"pi\">:</span>",
                "  <span class=\"na\">A</span><span class=\"pi\">:</span> \
                 <span class=\"s2\">\"</span><span class=\"s\">q\"</span>",
            ]
        );
        // Ruby: builtins, plain keywords, constants, operators, names, `0x` numbers.
        let r = gl(
            "require 'json'\nclass Bar < Base\n  attr_reader :name\n  def initialize(opts = \
             {})\n    puts x.inspect if true && !nil\n  end\nend\nn = 0x1f\n",
            "ruby",
        );
        assert_eq!(
            r.lines,
            [
                "<span class=\"nb\">require</span> <span class=\"s1\">'json'</span>",
                "<span class=\"k\">class</span> <span class=\"nc\">Bar</span> \
                 <span class=\"o\">&lt;</span> <span class=\"no\">Base</span>",
                "  <span class=\"nb\">attr_reader</span> <span class=\"ss\">:name</span>",
                "  <span class=\"k\">def</span> <span class=\"nf\">initialize</span>\
                 <span class=\"p\">(</span><span class=\"n\">opts</span> \
                 <span class=\"o\">=</span> <span class=\"p\">{})</span>",
                "    <span class=\"nb\">puts</span> <span class=\"n\">x</span>\
                 <span class=\"p\">.</span><span class=\"nf\">inspect</span> \
                 <span class=\"k\">if</span> <span class=\"kp\">true</span> \
                 <span class=\"o\">&amp;&amp;</span> <span class=\"o\">!</span>\
                 <span class=\"kp\">nil</span>",
                "  <span class=\"k\">end</span>",
                "<span class=\"k\">end</span>",
                "<span class=\"n\">n</span> <span class=\"o\">=</span> \
                 <span class=\"mh\">0x1f</span>",
            ]
        );
        // PHP: `<?php`, namespaces, variables, keywords, strings with interpolation.
        let r = gl(
            "<?php\nnamespace App\\Http;\nuse Foo\\Bar;\n  private $name = \"str\";\n  \
             public function __construct(int $a) { $this->a = $a; }\necho \"Hello {$name}\";\n",
            "php",
        );
        assert_eq!(
            r.lines,
            [
                "<span class=\"cp\">&lt;?php</span>",
                "<span class=\"kn\">namespace</span> <span class=\"nn\">App\\Http</span>\
                 <span class=\"p\">;</span>",
                "<span class=\"kn\">use</span> <span class=\"nc\">Foo\\Bar</span>\
                 <span class=\"p\">;</span>",
                "  <span class=\"k\">private</span> <span class=\"nv\">$name</span> \
                 <span class=\"o\">=</span> <span class=\"s2\">\"str\"</span>\
                 <span class=\"p\">;</span>",
                "  <span class=\"k\">public</span> <span class=\"k\">function</span> \
                 <span class=\"n\">__construct</span><span class=\"p\">(</span>\
                 <span class=\"kt\">int</span> <span class=\"nv\">$a</span>\
                 <span class=\"p\">)</span> <span class=\"p\">{</span> \
                 <span class=\"nv\">$this</span><span class=\"o\">-&gt;</span>\
                 <span class=\"n\">a</span> <span class=\"o\">=</span> \
                 <span class=\"nv\">$a</span><span class=\"p\">;</span> \
                 <span class=\"p\">}</span>",
                "<span class=\"k\">echo</span> <span class=\"s2\">\"Hello </span>\
                 <span class=\"si\">{</span><span class=\"nv\">$name</span>\
                 <span class=\"si\">}</span><span class=\"s2\">\"</span>\
                 <span class=\"p\">;</span>",
            ]
        );
        // Dockerfile: instruction arguments are strings, `RUN` is shell (continued after
        // `\`), comments only where a token starts.
        let r = gl(
            "FROM ubuntu:22.04 AS build\n# comment\nARG VERSION=1\nRUN apt-get update && \\\n    \
             apt-get install -y curl\nCOPY --from=build /app /app\nCMD [\"./run\"]\n\
             onbuild copy . .\n",
            "dockerfile",
        );
        assert_eq!(r.tag, "docker");
        assert_eq!(
            r.lines,
            [
                "<span class=\"k\">FROM</span><span class=\"w\"> </span>\
                 <span class=\"s\">ubuntu:22.04</span><span class=\"w\"> </span>\
                 <span class=\"k\">AS</span><span class=\"w\"> </span>\
                 <span class=\"s\">build</span>",
                "<span class=\"c\"># comment</span>",
                "<span class=\"k\">ARG</span><span class=\"s\"> VERSION=1</span>",
                "<span class=\"k\">RUN </span>apt-get update <span class=\"o\">&amp;&amp;</span> \
                 <span class=\"se\">\\</span>",
                "    apt-get <span class=\"nb\">install</span> <span class=\"nt\">-y</span> curl",
                "<span class=\"k\">COPY</span><span class=\"s\"> --from=build /app /app</span>",
                "<span class=\"k\">CMD</span><span class=\"s\"> [\"./run\"]</span>",
                "<span class=\"k\">onbuild</span><span class=\"w\"> </span>\
                 <span class=\"k\">copy</span><span class=\"s\"> . .</span>",
            ]
        );
        // Every byte is covered, also without a final newline and with a lone `\`.
        let r = gl("RUN a \\", "dockerfile");
        assert_eq!(
            r.lines,
            ["<span class=\"k\">RUN </span>a <span class=\"err\">\\</span>"]
        );
        let r = gl("RUN\n  echo x\nx = 1 # c", "dockerfile");
        assert_eq!(r.lines.len(), 3);
        assert!(
            r.lines[2].ends_with("<span class=\"c\"># c</span>"),
            "{:?}",
            r.lines
        );
    }

    #[test]
    fn rust_tokens() {
        let r = gl("fn main() { let x = \"s\"; } // c\n", "rs");
        assert_eq!(r.tag, "rust");
        assert_eq!(r.lines.len(), 1);
        assert!(
            r.lines[0].contains("<span class=\"c1\">// c</span>"),
            "{}",
            r.lines[0]
        );
        assert!(r.lines[0].contains("\"s\"</span>"), "{}", r.lines[0]);
    }
}
