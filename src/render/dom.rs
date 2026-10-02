//! DOM helpers over `dom_query` shared by the offline GitHub and GitLab pipelines and the
//! `--api` post-processing.
//!
//! Flavour-specific helpers stay with their flavour: `github/dom.rs` (document-wide element
//! lists) and `gitlab/dom.rs` (Nokogiri semantics such as root-bounded ancestor walks and
//! Ruby string predicates). Every helper here is iterative, so deep documents cannot overflow
//! the stack.

use dom_query::{NodeData, NodeRef, Tree};

/// Class tokens, split on ASCII whitespace as HTML does (and `dom_query`'s `has_class`).
pub(crate) fn classes(n: &NodeRef<'_>) -> Vec<String> {
    n.attr("class")
        .map(|c| c.split_ascii_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Nokogiri `add_class`: appends each token of the space-separated `class` that is not present
/// yet. Creates the attribute when missing and leaves it untouched when nothing is added.
pub(crate) fn add_class(n: &NodeRef<'_>, class: &str) {
    let mut list = classes(n);
    let mut changed = !n.has_attr("class");
    for c in class.split_ascii_whitespace() {
        if !list.iter().any(|x| x == c) {
            list.push(c.to_owned());
            changed = true;
        }
    }
    if changed {
        n.set_attr("class", &list.join(" "));
    }
}

/// Removes the class token `class`, dropping the attribute when it becomes empty (untouched when
/// the token is absent).
pub(crate) fn remove_class(n: &NodeRef<'_>, class: &str) {
    let list = classes(n);
    if !list.iter().any(|c| c == class) {
        return;
    }
    let rest: Vec<String> = list.into_iter().filter(|c| c != class).collect();
    if rest.is_empty() {
        n.remove_attr("class");
    } else {
        n.set_attr("class", &rest.join(" "));
    }
}

/// Whether `n` has an ancestor element for which `pred` holds (up to the document).
pub(crate) fn has_ancestor(n: &NodeRef<'_>, pred: impl Fn(&NodeRef<'_>) -> bool) -> bool {
    let mut cur = n.parent();
    while let Some(p) = cur {
        if p.is_element() && pred(&p) {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// Replaces `n` by its children (Nokogiri `node.replace(node.children)`). For a `<template>`,
/// whose contents html5ever keeps in a separate fragment, those contents come first.
pub(crate) fn unwrap(n: &NodeRef<'_>) {
    let template = n.element_ref().and_then(|e| e.template_contents);
    if let Some(t) = template {
        let frag = NodeRef::new(t, n.tree);
        while let Some(c) = frag.first_child() {
            n.insert_before(&c);
        }
    }
    while let Some(c) = n.first_child() {
        n.insert_before(&c);
    }
    n.remove_from_parent();
}

/// Contents of a text node (`None` for other nodes).
pub(crate) fn text_of(n: &NodeRef<'_>) -> Option<String> {
    n.query(|t| match &t.data {
        NodeData::Text { contents } => Some(contents.to_string()),
        _ => None,
    })
    .flatten()
}

/// Creates a detached element with the given attributes (in order).
pub(crate) fn new_element<'a>(tree: &'a Tree, name: &str, attrs: &[(&str, &str)]) -> NodeRef<'a> {
    let el = tree.new_element(name);
    for (k, v) in attrs {
        el.set_attr(k, v);
    }
    el
}

/// Creates a detached text node.
pub(crate) fn new_text<'a>(tree: &'a Tree, text: &str) -> NodeRef<'a> {
    tree.new_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dom_query::Document;

    fn first<'a>(doc: &'a Document, sel: &str) -> NodeRef<'a> {
        doc.select(sel).nodes()[0]
    }

    #[test]
    fn class_helpers() {
        let doc = Document::fragment(r#"<p class="a  b">x</p><i>y</i><b class="c">z</b>"#);
        let p = first(&doc, "p");
        assert_eq!(classes(&p), ["a", "b"]);
        add_class(&p, "a");
        assert_eq!(p.attr("class").as_deref(), Some("a  b"), "unchanged");
        add_class(&p, "b c a");
        assert_eq!(p.attr("class").as_deref(), Some("a b c"));
        remove_class(&p, "x");
        assert_eq!(p.attr("class").as_deref(), Some("a b c"));
        remove_class(&p, "b");
        assert_eq!(p.attr("class").as_deref(), Some("a c"));
        let i = first(&doc, "i");
        add_class(&i, "");
        assert_eq!(
            i.attr("class").as_deref(),
            Some(""),
            "created, like Nokogiri"
        );
        let b = first(&doc, "b");
        remove_class(&b, "c");
        assert!(!b.has_attr("class"));
        // Non-ASCII spaces are part of a token, as in HTML.
        let doc = Document::fragment("<p class=\"a\u{a0}b\">x</p>");
        assert_eq!(classes(&first(&doc, "p")), ["a\u{a0}b"]);
    }

    #[test]
    fn unwrap_keeps_children_and_template_contents() {
        let doc =
            Document::fragment("<p>a<span>b<i>c</i></span>d</p><template>t<b>u</b></template>");
        unwrap(&first(&doc, "span"));
        unwrap(&first(&doc, "template"));
        assert_eq!(
            doc.html_root().inner_html().to_string(),
            "<p>ab<i>c</i>d</p>t<b>u</b>"
        );
    }

    #[test]
    fn ancestors_text_and_new_elements() {
        let doc = Document::fragment("<pre><code>x</code></pre>");
        let code = first(&doc, "code");
        assert!(has_ancestor(&code, |p| p.has_name("pre")));
        assert!(!has_ancestor(&code, |p| p.has_name("code")));
        let t = code.first_child().unwrap();
        assert_eq!(text_of(&t).as_deref(), Some("x"));
        assert_eq!(text_of(&code), None);
        let el = new_element(&doc.tree, "a", &[("href", "#x"), ("class", "y")]);
        code.append_child(&el);
        assert_eq!(
            code.html().to_string(),
            r##"<code>x<a href="#x" class="y"></a></code>"##
        );
    }
}
