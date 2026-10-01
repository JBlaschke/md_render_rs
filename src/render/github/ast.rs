//! comrak options and the AST passes that run before formatting (CQ §3.1 step 2).
//!
//! * tagfilter on raw HTML literals (GH §8.11; comrak's own `tagfilter` is deprecated).
//!
//! Alerts, dollar math and task lists are found later, on the HTML tree, as github.com does
//! ([`super::alerts`], [`super::math`], [`super::tasks`]).

use std::collections::HashMap;

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
    dedupe_footnotes(root);
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
// duplicate footnote definitions (GH-FN-1)

/// cmark-gfm's reference map (`src/references.c`) folds footnote labels: it trims, collapses
/// internal whitespace runs to one space and case-folds. We mirror that to group duplicates.
fn normalize_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.trim().chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            prev_ws = false;
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The label of a footnote definition (`None` for other nodes).
fn definition_label(node: &AstNode<'_>) -> Option<String> {
    match &node.data().value {
        NodeValue::FootnoteDefinition(d) => Some(d.name.clone()),
        _ => None,
    }
}

/// Collapses duplicate footnote definitions the way cmark-gfm (and therefore github.com)
/// does: the **first** definition of a label wins, later duplicates — and case variants such
/// as `[^a]`/`[^A]` — are dropped, and the body blocks that sit between them stay in the body
/// (GH-FN-1). comrak 0.55 instead keeps the **last** definition (its `fd_map` inserts win),
/// moves only that one to the document end, and leaves earlier duplicates inline, so without
/// this pass the formatter folds the text that follows the first duplicate into the footnote
/// list and emits two `<li>` with the same id.
fn dedupe_footnotes<'a>(root: &'a AstNode<'a>) {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<&'a AstNode<'a>>> = HashMap::new();
    for node in root.descendants() {
        if let Some(name) = definition_label(node) {
            let label = normalize_label(&name);
            groups
                .entry(label.clone())
                .or_insert_with(|| {
                    order.push(label);
                    Vec::new()
                })
                .push(node);
        }
    }

    for label in &order {
        let defs = &groups[label];
        if defs.len() < 2 {
            continue;
        }
        let first = defs[0];
        let canonical = definition_label(first).unwrap_or_default();
        // The surviving node is the one comrak resolved references to (it alone carries a
        // non-zero reference count and sits at the document end).
        let winner = defs.iter().copied().find(|n| {
            matches!(&n.data().value, NodeValue::FootnoteDefinition(d) if d.total_references > 0)
        });
        let Some(winner) = winner else {
            // No reference anywhere: every definition is an orphan github.com drops.
            for d in defs {
                d.detach();
            }
            continue;
        };
        if !std::ptr::eq(winner, first) {
            // Give the survivor the first definition's body and label (its id uses the
            // first-seen spelling, e.g. `a`, not `A`).
            for c in winner.children().collect::<Vec<_>>() {
                c.detach();
            }
            for c in first.children().collect::<Vec<_>>() {
                c.detach();
                winner.append(c);
            }
            if let NodeValue::FootnoteDefinition(d) = &mut winner.data_mut().value {
                d.name = canonical.clone();
            }
        }
        for d in defs {
            if !std::ptr::eq(*d, winner) {
                d.detach();
            }
        }
    }

    // An unreferenced footnote renders no section on github.com. comrak drops a lone one
    // while parsing, but keeps the first of several same-label duplicates inline (the others
    // are gone by now), which would otherwise pull the following body into a `<section>`.
    let unreferenced: Vec<&'a AstNode<'a>> = root
        .descendants()
        .filter(|n| {
            matches!(&n.data().value, NodeValue::FootnoteDefinition(d) if d.total_references == 0)
        })
        .collect();
    for node in unreferenced {
        node.detach();
    }

    if order.is_empty() {
        return;
    }
    // Point every reference at the first-seen spelling of its (folded) label, so ids match.
    let canonical: HashMap<String, String> = order
        .iter()
        .filter_map(|label| {
            let defs = &groups[label];
            (defs.len() > 1).then(|| (label.clone(), definition_label(defs[0]).unwrap_or_default()))
        })
        .collect();
    if canonical.is_empty() {
        return;
    }
    for node in root.descendants() {
        if let NodeValue::FootnoteReference(r) = &mut node.data_mut().value {
            if let Some(name) = canonical.get(&normalize_label(&r.name)) {
                r.name = name.clone();
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
