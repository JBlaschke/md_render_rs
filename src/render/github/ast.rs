//! comrak options and the AST passes that run before formatting (CQ §3.1 step 2).
//!
//! * tagfilter on raw HTML literals (GH §8.11; comrak's own `tagfilter` is deprecated).
//!
//! Alerts, dollar math and task lists are found later, on the HTML tree, as github.com does
//! ([`super::alerts`], [`super::math`], [`super::tasks`]).

use comrak::Options;
use comrak::nodes::{AstNode, NodeValue};

/// comrak options for the GitHub flavor (GH §10): GFM extensions without comrak's alerts,
/// dollar math, task lists, shortcodes and heading ids, which we implement on the HTML tree.
pub(super) fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.footnotes = true;
    o.extension.math_code = true;
    o.render.r#unsafe = true;
    o
}

/// Runs all AST passes.
pub(super) fn transform<'a>(root: &'a AstNode<'a>) {
    for node in root.descendants() {
        let mut ast = node.data_mut();
        match &mut ast.value {
            NodeValue::HtmlBlock(b) => {
                if let Some(f) = tagfilter(&b.literal) {
                    b.literal = f;
                }
            }
            NodeValue::HtmlInline(lit) => {
                if let Some(f) = tagfilter(lit) {
                    *lit = f;
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------------------------------
// tagfilter (GH §8.11)

/// Raw tags renamed before HTML parsing: github.com's HTML parser predates the rule that
/// `<dialog>` / `<search>` close an open `<p>`; both are removed by the sanitizer anyway, so
/// an unknown name reproduces GitHub's tree (`S/probes/tags.gfm.html` T30, T101).
const LEGACY_INLINE: [&str; 2] = ["dialog", "search"];

const TAGFILTER: [&str; 9] = [
    "title",
    "textarea",
    "style",
    "xmp",
    "iframe",
    "noembed",
    "noframes",
    "script",
    "plaintext",
];

fn tag_at(b: &[u8], j: usize, names: &[&str]) -> bool {
    names.iter().any(|t| {
        let end = j + t.len();
        end <= b.len()
            && b[j..end].eq_ignore_ascii_case(t.as_bytes())
            && match b.get(end) {
                None => false,
                Some(&c) => {
                    c.is_ascii_whitespace()
                        || c == b'>'
                        || (c == b'/' && b.get(end + 1) == Some(&b'>'))
                }
            }
    })
}

/// Escapes the `<` of every filtered tag (any case) and renames [`LEGACY_INLINE`] tags;
/// `None` when nothing matched.
fn tagfilter(literal: &str) -> Option<String> {
    let b = literal.as_bytes();
    let mut out: Option<String> = None;
    let mut last = 0;
    for (i, _) in literal.match_indices('<') {
        let mut j = i + 1;
        if b.get(j) == Some(&b'/') {
            j += 1;
        }
        let replacement = if tag_at(b, j, &TAGFILTER) {
            "&lt;"
        } else if tag_at(b, j, &LEGACY_INLINE) {
            if j == i + 1 {
                "<mdr-legacy-"
            } else {
                "</mdr-legacy-"
            }
        } else {
            continue;
        };
        let s = out.get_or_insert_with(|| String::with_capacity(literal.len() + 16));
        s.push_str(&literal[last..i]);
        s.push_str(replacement);
        last = j;
        if replacement == "&lt;" {
            last = i + 1;
        }
    }
    out.map(|mut s| {
        s.push_str(&literal[last..]);
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagfilter_cases() {
        assert_eq!(
            tagfilter("<script>x</script>").as_deref(),
            Some("&lt;script>x&lt;/script>")
        );
        assert_eq!(
            tagfilter("<IFRAME src=x>").as_deref(),
            Some("&lt;IFRAME src=x>")
        );
        assert_eq!(tagfilter("<style/>").as_deref(), Some("&lt;style/>"));
        assert_eq!(tagfilter("<scripts>"), None);
        assert_eq!(tagfilter("<div>ok</div>"), None);
        assert_eq!(tagfilter("<title"), None);
        assert_eq!(
            tagfilter("a <dialog>b</DIALOG>").as_deref(),
            Some("a <mdr-legacy-dialog>b</mdr-legacy-DIALOG>")
        );
    }
}
