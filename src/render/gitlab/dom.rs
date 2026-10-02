//! Small DOM helpers over `dom_query` with Nokogiri-like semantics, shared by the GitLab filters
//! (the ones the GitHub pipeline and `--api` share live in `render::dom` and are re-exported).
//!
//! Every helper is iterative (never recursive), so arbitrarily deep documents cannot overflow the
//! stack.

use dom_query::{NodeData, NodeId, NodeRef};

pub(super) use crate::render::dom::{add_class, classes, new_element, new_text, text_of, unwrap};

/// Whether `n` is an element with local name `name` (any namespace, like Nokogiri's `#name`).
#[inline]
pub(super) fn is(n: &NodeRef<'_>, name: &str) -> bool {
    n.has_name(name)
}

/// Whether `n` is an element whose local name is one of `names`.
pub(super) fn is_any(n: &NodeRef<'_>, names: &[&str]) -> bool {
    n.element_ref()
        .is_some_and(|el| names.contains(&el.name.local.as_ref()))
}

/// Local name of an element; `None` for other nodes.
pub(super) fn tag(n: &NodeRef<'_>) -> Option<String> {
    n.element_ref().map(|el| el.name.local.to_string())
}

/// Attribute value (`None` when absent or not an element).
#[inline]
pub(super) fn attr(n: &NodeRef<'_>, name: &str) -> Option<String> {
    n.element_ref()
        .and_then(|el| el.attr_ref(name.into()).map(str::to_owned))
}

/// Whether the attribute equals `value`.
pub(super) fn attr_is(n: &NodeRef<'_>, name: &str, value: &str) -> bool {
    n.element_ref()
        .is_some_and(|el| el.attr_ref(name.into()) == Some(value))
}

/// Concatenated text of all descendant text nodes (Nokogiri `#text` / `#content`).
pub(super) fn text(n: &NodeRef<'_>) -> String {
    n.text().to_string()
}

/// Replaces the contents of a text node.
pub(super) fn set_text_node_contents(n: &NodeRef<'_>, value: &str) {
    n.update(|tn| {
        if let NodeData::Text { contents } = &mut tn.data {
            contents.clear();
            contents.push_slice(value);
        }
    });
}

/// Element children (Nokogiri `#elements`).
pub(super) fn element_children<'a>(n: &NodeRef<'a>) -> Vec<NodeRef<'a>> {
    n.children_it(false).filter(|c| c.is_element()).collect()
}

/// Whether `n` is attached below `root` (follows parents; iterative).
pub(super) fn is_attached(n: &NodeRef<'_>, root: &NodeRef<'_>) -> bool {
    let mut cur = n.parent();
    while let Some(p) = cur {
        if p.id == root.id {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// Whether any ancestor of `n` below `root` is an element named one of `names`.
pub(super) fn has_ancestor(n: &NodeRef<'_>, names: &[&str], root: &NodeRef<'_>) -> bool {
    let mut cur = n.parent();
    while let Some(p) = cur {
        if p.id == root.id {
            return false;
        }
        if is_any(&p, names) {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// Nearest ancestor element named `name` (below `root`).
pub(super) fn ancestor<'a>(n: &NodeRef<'a>, name: &str, root: &NodeRef<'_>) -> Option<NodeRef<'a>> {
    let mut cur = n.parent();
    while let Some(p) = cur {
        if p.id == root.id {
            return None;
        }
        if is(&p, name) {
            return Some(p);
        }
        cur = p.parent();
    }
    None
}

/// All nodes below `root` in document order (pre-order), excluding `root`.
pub(super) fn descendants(root: &NodeRef<'_>) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut next = root.first_child();
    while let Some(n) = next {
        out.push(n.id);
        next = if let Some(c) = n.first_child() {
            Some(c)
        } else {
            following_skip_children(&n, root)
        };
    }
    out
}

/// Elements below `root` in document order whose local name is one of `names`.
pub(super) fn elements_named(root: &NodeRef<'_>, names: &[&str]) -> Vec<NodeId> {
    descendants(root)
        .into_iter()
        .filter(|id| is_any(&root.tree.get_unchecked(id), names))
        .collect()
}

/// The first element below `root` in document order whose local name is one of `names`
/// (Nokogiri `root.css(name).first`). Stops at the match instead of collecting the subtree.
pub(super) fn first_element_named<'a>(root: &NodeRef<'a>, names: &[&str]) -> Option<NodeRef<'a>> {
    let mut next = root.first_child();
    while let Some(n) = next {
        if is_any(&n, names) {
            return Some(n);
        }
        next = n
            .first_child()
            .or_else(|| following_skip_children(&n, root));
    }
    None
}

/// Text nodes below `root` in document order.
pub(super) fn text_nodes(root: &NodeRef<'_>) -> Vec<NodeId> {
    descendants(root)
        .into_iter()
        .filter(|id| root.tree.get_unchecked(id).is_text())
        .collect()
}

/// The next node in document order after `n`'s subtree (stays below `root`).
pub(super) fn following_skip_children<'a>(
    n: &NodeRef<'a>,
    root: &NodeRef<'_>,
) -> Option<NodeRef<'a>> {
    let mut cur = *n;
    loop {
        if cur.id == root.id {
            return None;
        }
        if let Some(s) = cur.next_sibling() {
            return Some(s);
        }
        cur = cur.parent()?;
    }
}

/// Ruby `String#strip`: removes leading/trailing ASCII whitespace and NUL.
pub(super) fn ruby_strip(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, '\0' | '\t' | '\n' | '\x0B' | '\x0C' | '\r' | ' '))
}

/// ActiveSupport `blank?` for strings: empty or only Unicode whitespace.
pub(super) fn is_blank(s: &str) -> bool {
    s.chars().all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dom_query::Document;

    #[test]
    fn descendants_document_order() {
        let doc = Document::fragment("<p>a<b>c</b></p><ul><li>x</li></ul>");
        let root = doc.html_root();
        let names: Vec<String> = descendants(&root)
            .iter()
            .map(|id| {
                let n = root.tree.get_unchecked(id);
                tag(&n).unwrap_or_else(|| format!("#{}", text(&n)))
            })
            .collect();
        assert_eq!(names, ["p", "#a", "b", "#c", "ul", "li", "#x"]);
    }

    #[test]
    fn first_element_in_document_order() {
        let doc = Document::fragment(
            "<table><caption><b>c</b></caption><thead><tr id=\"h\"><th>a</th></tr></thead>\
             <tbody><tr id=\"b\"><td>x</td></tr></tbody></table><p id=\"after\">p</p>",
        );
        let root = doc.html_root();
        let table = doc.select("table").nodes()[0];
        let first = first_element_named(&table, &["tr"]).unwrap();
        assert_eq!(attr(&first, "id").as_deref(), Some("h"));
        assert_eq!(
            Some(first.id),
            elements_named(&table, &["tr"]).first().copied()
        );
        // Stays below its root: the `p` after the table is not found from the table.
        assert!(first_element_named(&table, &["p"]).is_none());
        let p = first_element_named(&root, &["p", "b"]).unwrap();
        assert_eq!(tag(&p).as_deref(), Some("b"));
        assert!(first_element_named(&first, &["tr"]).is_none());
    }

    #[test]
    fn strip_and_blank() {
        assert_eq!(ruby_strip("\0 \t x \n"), "x");
        assert_eq!(ruby_strip("\u{a0}x"), "\u{a0}x");
        assert!(is_blank(" \u{a0}\n"));
        assert!(!is_blank(" x"));
    }
}
