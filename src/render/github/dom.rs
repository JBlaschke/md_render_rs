//! Thin helpers over `dom_query` used by the DOM filters (CQ §3.1 steps 4-6); the ones the
//! GitLab pipeline and `--api` share live in `render::dom` and are re-exported here.

use dom_query::{Document, NodeRef};

pub(super) use crate::render::dom::{has_ancestor, text_of, unwrap};

/// The content root (`<html>` element of the parsed fragment).
pub(super) fn root(doc: &Document) -> NodeRef<'_> {
    doc.html_root()
}

/// All element descendants of the content root, in document order.
pub(super) fn elements(doc: &Document) -> Vec<NodeRef<'_>> {
    root(doc)
        .descendants()
        .into_iter()
        .filter(NodeRef::is_element)
        .collect()
}

/// All descendants named `name` (HTML local name), in document order.
pub(super) fn elements_named<'a>(doc: &'a Document, name: &str) -> Vec<NodeRef<'a>> {
    root(doc)
        .descendants()
        .into_iter()
        .filter(|n| n.has_name(name))
        .collect()
}

/// Local name of an element (`None` for other nodes).
pub(super) fn name(n: &NodeRef<'_>) -> Option<dom_query::LocalName> {
    n.qual_name_ref().map(|q| q.local.clone())
}

/// Whether the element has any attributes.
pub(super) fn has_attrs(n: &NodeRef<'_>) -> bool {
    n.element_ref().is_some_and(|e| !e.attrs.is_empty())
}

/// Wraps `n` in `wrapper` (which must be detached).
pub(super) fn wrap(n: &NodeRef<'_>, wrapper: &NodeRef<'_>) {
    n.insert_before(wrapper);
    wrapper.append_child(n);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_and_text() {
        let doc = Document::fragment("<pre><code>x</code></pre>");
        let code = elements_named(&doc, "code")[0];
        assert!(has_ancestor(&code, |p| p.has_name("pre")));
        let t = code.first_child().unwrap();
        assert_eq!(text_of(&t).as_deref(), Some("x"));
        assert!(!has_attrs(&code));
        assert_eq!(name(&code).as_deref(), Some("code"));
    }
}
