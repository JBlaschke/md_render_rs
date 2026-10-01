//! `source.diff` for GitHub: a port of the line rules of Linguist's diff grammar
//! (textmate/diff.tmbundle), rendered with the PrettyLights map (GH §7.4: `diff --git` header
//! `pl-c1`, `---`/`+++` file headers `pl-md`/`pl-mi1`, `@@ … @@` `pl-mdr`, changed lines with a
//! nested marker span). Sublime's Diff syntax leaves `diff --git`, `index` and `#` lines
//! unscoped, hence the port.
//!
//! ```text
//! ^((\*{15})|(={67})|(-{3}))$                       meta.separator.diff             pl-ms
//! ^\d+(,\d+)*(a|d|c)\d+(,\d+)*$                     meta.diff.range.normal          pl-mdr
//! ^(@@)\s*(.+?)\s*(@@)                              meta.diff.range.unified         pl-mdr
//! ^(((\-{3}) .+ (\-{4}))|((\*{3}) .+ (\*{4})))$     meta.diff.range.context         pl-mdr
//! ^diff --git a/.*$  |  ^diff (-|\S+\s+\S+).*$     meta.diff.header.{git,command}  pl-c1
//! ^(((-{3}) .+)|((\*{3}) .+))$                      meta.diff.header.from-file      pl-md
//! ^(\+{3}) .+$                                      meta.diff.header.to-file        pl-mi1
//! ^(((>)( .*)?)|((\+).*))$                          markup.inserted.diff (+marker)  pl-mi1
//! ^(!).*$                                           markup.changed.diff (+marker)   pl-mc
//! ^(((<)( .*)?)|((-).*))$                           markup.deleted.diff (+marker)   pl-md
//! ^(#) … \n                                         comment.line.number-sign.diff   pl-c
//! ^index …, ^Index: …, ^Only in …                   meta.diff.index / only-in       (none)
//! ```
//! The Perforce `==== … - ` header variants need look-around and are not ported.

use std::sync::OnceLock;

use regex::Regex;

use super::SpanWriter;

/// How one line is rendered: an outer class and the length of a leading marker that gets a
/// nested span of the same class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LineClass {
    class: &'static str,
    /// Bytes of the line covered by the outer span (the rest is plain).
    len: usize,
    /// Bytes of the leading marker with a nested span (0: none).
    marker: usize,
}

struct Rules {
    separator: Regex,
    range_normal: Regex,
    range_unified: Regex,
    range_context: Regex,
    header: Regex,
    from_file: Regex,
    to_file: Regex,
}

fn rules() -> Option<&'static Rules> {
    static RULES: OnceLock<Option<Rules>> = OnceLock::new();
    RULES
        .get_or_init(|| {
            Some(Rules {
                separator: Regex::new(r"^((\*{15})|(={67})|(-{3}))$").ok()?,
                range_normal: Regex::new(r"^\d+(,\d+)*(a|d|c)\d+(,\d+)*$").ok()?,
                range_unified: Regex::new(r"^(@@)\s*(.+?)\s*(@@)").ok()?,
                range_context: Regex::new(r"^(((-{3}) .+ (-{4}))|((\*{3}) .+ (\*{4})))$").ok()?,
                header: Regex::new(r"^diff (--git a/|-|\S+\s+\S+)").ok()?,
                from_file: Regex::new(r"^(((-{3}) .+)|((\*{3}) .+))$").ok()?,
                to_file: Regex::new(r"^(\+{3}) .+$").ok()?,
            })
        })
        .as_ref()
}

fn classify(line: &str, r: &Rules) -> Option<LineClass> {
    let whole = |class| {
        Some(LineClass {
            class,
            len: line.len(),
            marker: 0,
        })
    };
    let marked = |class| {
        Some(LineClass {
            class,
            len: line.len(),
            marker: 1,
        })
    };
    if r.separator.is_match(line) {
        return whole("pl-ms");
    }
    if r.range_normal.is_match(line) {
        return whole("pl-mdr");
    }
    if let Some(m) = r.range_unified.find(line) {
        return Some(LineClass {
            class: "pl-mdr",
            len: m.end(),
            marker: 0,
        });
    }
    if r.range_context.is_match(line) {
        return whole("pl-mdr");
    }
    if r.header.is_match(line) {
        return whole("pl-c1");
    }
    if r.from_file.is_match(line) {
        return whole("pl-md");
    }
    if r.to_file.is_match(line) {
        return whole("pl-mi1");
    }
    let first = line.as_bytes().first().copied();
    let rest_blank_or_space = line.len() == 1 || line.as_bytes().get(1) == Some(&b' ');
    match first {
        Some(b'+') => marked("pl-mi1"),
        Some(b'>') if rest_blank_or_space => marked("pl-mi1"),
        Some(b'!') => marked("pl-mc"),
        Some(b'-') => marked("pl-md"),
        Some(b'<') if rest_blank_or_space => marked("pl-md"),
        Some(b'#') => marked("pl-c"),
        _ => None,
    }
}

/// Render a diff.
pub(super) fn render(code: &str, w: &mut SpanWriter<'_>) {
    let Some(r) = rules() else {
        w.text(code);
        return;
    };
    for line in code.split_inclusive('\n') {
        let (content, newline) = match line.strip_suffix('\n') {
            Some(c) => (c, true),
            None => (line, false),
        };
        match classify(content, r) {
            Some(LineClass { class, len, marker }) => {
                w.push_class(Some(class));
                if marker > 0 {
                    w.push_class(Some(class));
                    w.text(&content[..marker]);
                    w.pop_class();
                }
                w.text(&content[marker..len]);
                w.pop_class();
                w.text(&content[len..]);
            }
            None => w.text(content),
        }
        if newline {
            w.text("\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{DEFAULT_VARIANT, highlight_with};

    fn diff(code: &str) -> String {
        highlight_with(code, "diff", DEFAULT_VARIANT).html.unwrap()
    }

    #[test]
    fn github_sample() {
        // S/api_gfm.html, the `diff` block.
        let code = "diff --git a/file.txt b/file.txt\nindex 83db48f..bf269f4 100644\n--- a/file.txt\n+++ b/file.txt\n@@ -1,3 +1,3 @@\n unchanged line\n-removed line\n+added line\n! changed line\n";
        let want = "<span class=\"pl-c1\">diff --git a/file.txt b/file.txt</span>\nindex 83db48f..bf269f4 100644\n<span class=\"pl-md\">--- a/file.txt</span>\n<span class=\"pl-mi1\">+++ b/file.txt</span>\n<span class=\"pl-mdr\">@@ -1,3 +1,3 @@</span>\n unchanged line\n<span class=\"pl-md\"><span class=\"pl-md\">-</span>removed line</span>\n<span class=\"pl-mi1\"><span class=\"pl-mi1\">+</span>added line</span>\n<span class=\"pl-mc\"><span class=\"pl-mc\">!</span> changed line</span>";
        assert_eq!(diff(code), want);
    }

    #[test]
    fn other_lines() {
        assert_eq!(diff("---\n"), "<span class=\"pl-ms\">---</span>");
        assert_eq!(
            diff("# note\n"),
            "<span class=\"pl-c\"><span class=\"pl-c\">#</span> note</span>"
        );
        assert_eq!(
            diff("@@ -1 +1 @@ fn f()\n"),
            "<span class=\"pl-mdr\">@@ -1 +1 @@</span> fn f()"
        );
        assert_eq!(diff("5c5\n"), "<span class=\"pl-mdr\">5c5</span>");
        assert_eq!(
            diff("> new\n>x\n"),
            "<span class=\"pl-mi1\"><span class=\"pl-mi1\">&gt;</span> new</span>\n&gt;x"
        );
        assert_eq!(diff("plain & <b>\n"), "plain &amp; &lt;b&gt;");
    }
}
