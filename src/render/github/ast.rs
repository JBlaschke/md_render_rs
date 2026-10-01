//! comrak options and the AST passes that run before formatting (CQ §3.1 step 2).
//!
//! * tagfilter on raw HTML literals (GH §8.11; comrak's own `tagfilter` is deprecated),
//! * task-list fixes (GH §8.1: `- [ ]` alone is not a task).
//!
//! Alerts and dollar math are found later, on the HTML tree, as github.com does
//! ([`super::alerts`], [`super::math`]).

use std::borrow::Cow;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options};

/// comrak options for the GitHub flavor (GH §10): GFM extensions without comrak's alerts,
/// dollar math, shortcodes and heading ids, which we implement ourselves.
pub(super) fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.footnotes = true;
    o.extension.math_code = true;
    o.render.r#unsafe = true;
    o
}

/// Runs all AST passes.
pub(super) fn transform<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>) {
    let nodes: Vec<&'a AstNode<'a>> = root.descendants().collect();

    for &node in &nodes {
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

    for &node in &nodes {
        if matches!(node.data().value, NodeValue::TaskItem(_)) {
            fix_task_item(arena, node);
        }
    }
    for &node in &nodes {
        let is_list = matches!(node.data().value, NodeValue::List(_));
        if is_list {
            let any_task = node
                .children()
                .any(|c| matches!(c.data().value, NodeValue::TaskItem(_)));
            if let NodeValue::List(l) = &mut node.data_mut().value {
                l.is_task_list = any_task;
            }
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

// ------------------------------------------------------------------------------------------
// task lists (GH §8.1)

/// A `TaskItem` without text (`- [ ]` alone) is a plain item showing the brackets.
fn fix_task_item<'a>(arena: &'a Arena<'a>, node: &'a AstNode<'a>) {
    let first_is_para = node
        .first_child()
        .is_some_and(|c| matches!(c.data().value, NodeValue::Paragraph));
    if first_is_para {
        return;
    }
    let symbol = match node.data().value {
        NodeValue::TaskItem(ref t) => t.symbol,
        _ => return,
    };
    let list = node.parent().and_then(|p| match p.data().value {
        NodeValue::List(ref l) => Some(*l),
        _ => None,
    });
    let Some(list) = list else { return };
    let start = node.data().sourcepos.start;
    node.data_mut().value = NodeValue::Item(list);
    let para = arena.alloc(AstNode::from(NodeValue::Paragraph));
    para.data_mut().sourcepos.start = start;
    let text = format!("[{}]", symbol.unwrap_or(' '));
    para.append(arena.alloc(AstNode::from(NodeValue::Text(Cow::Owned(text)))));
    node.prepend(para);
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
