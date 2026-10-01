//! Thin helpers over `dom_query` used by the DOM filters (CQ §3.1 steps 4-6).

use dom_query::{Document, NodeData, NodeRef};

/// The content root (`<html>` element of the parsed fragment).
pub(super) fn root(doc: &Document) -> NodeRef<'_> {
    doc.html_root()
}

/// All element descendants of the content root, in document order.
pub(super) fn elements(doc: &Document) -> Vec<NodeRef<'_>> {
    root(doc).descendants().into_iter().filter(NodeRef::is_element).collect()
}

/// All descendants named `name` (HTML local name), in document order.
pub(super) fn elements_named<'a>(doc: &'a Document, name: &str) -> Vec<NodeRef<'a>> {
    root(doc).descendants().into_iter().filter(|n| n.has_name(name)).collect()
}

/// Local name of an element (`None` for other nodes).
pub(super) fn name(n: &NodeRef<'_>) -> Option<dom_query::LocalName> {
    n.qual_name_ref().map(|q| q.local.clone())
}

/// Whether `n` has an ancestor element for which `pred` holds.
pub(super) fn has_ancestor(n: &NodeRef<'_>, pred: impl Fn(&NodeRef<'_>) -> bool) -> bool {
    let mut cur = n.parent();
    while let Some(p) = cur {
        if p.is_element() && pred(&p) {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// Replaces `n` by its children (and, for `<template>`, its template contents).
pub(super) fn unwrap(n: &NodeRef<'_>) {
    let template = n.element_ref().and_then(|e| e.template_contents);
    if let Some(t) = template {
        let frag = NodeRef::new(t, n.tree);
        for c in frag.children() {
            n.insert_before(&c);
        }
    }
    for c in n.children() {
        n.insert_before(&c);
    }
    n.remove_from_parent();
}

/// Text content of a text node (empty for other nodes).
pub(super) fn text_of(n: &NodeRef<'_>) -> Option<String> {
    n.query(|t| match &t.data {
        NodeData::Text { contents } => Some(contents.to_string()),
        _ => None,
    })
    .flatten()
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
    fn unwrap_keeps_children_and_template_contents() {
        let doc = Document::fragment("<p>a<span>b<i>c</i></span>d</p><template>t<b>u</b></template>");
        for n in elements_named(&doc, "span") {
            unwrap(&n);
        }
        for n in elements_named(&doc, "template") {
            unwrap(&n);
        }
        assert_eq!(root(&doc).inner_html().to_string(), "<p>ab<i>c</i>d</p>t<b>u</b>");
    }

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
