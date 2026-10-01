//! Small DOM helpers over `dom_query` with Nokogiri-like semantics, shared by the GitLab filters.
//!
//! Every helper is iterative (never recursive), so arbitrarily deep documents cannot overflow the
//! stack.

use dom_query::{NodeData, NodeId, NodeRef, Tree};

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

/// Whitespace-separated class tokens.
pub(super) fn classes(n: &NodeRef<'_>) -> Vec<String> {
    attr(n, "class")
        .map(|c| c.split_ascii_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Nokogiri `add_class`: appends each token that is not present yet (creates the attribute).
pub(super) fn add_class(n: &NodeRef<'_>, class: &str) {
    let mut list = classes(n);
    let had = n.has_attr("class");
    let mut changed = !had;
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

/// Removes class tokens; drops the attribute when it becomes empty.
#[cfg(test)]
pub(super) fn remove_class_tokens(n: &NodeRef<'_>, remove: &[&str]) {
    if !n.has_attr("class") {
        return;
    }
    let list: Vec<String> = classes(n)
        .into_iter()
        .filter(|c| !remove.contains(&c.as_str()))
        .collect();
    if list.is_empty() {
        n.remove_attr("class");
    } else {
        n.set_attr("class", &list.join(" "));
    }
}

/// Concatenated text of all descendant text nodes (Nokogiri `#text` / `#content`).
pub(super) fn text(n: &NodeRef<'_>) -> String {
    n.text().to_string()
}

/// Contents of a text node (`None` for other nodes).
pub(super) fn text_node_contents(n: &NodeRef<'_>) -> Option<String> {
    n.query(|tn| match &tn.data {
        NodeData::Text { contents } => Some(contents.to_string()),
        _ => None,
    })
    .flatten()
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

/// Text nodes below `root` in document order.
pub(super) fn text_nodes(root: &NodeRef<'_>) -> Vec<NodeId> {
    descendants(root)
        .into_iter()
        .filter(|id| root.tree.get_unchecked(id).is_text())
        .collect()
}

/// The next node in document order after `n`'s subtree (stays below `root`).
pub(super) fn following_skip_children<'a>(n: &NodeRef<'a>, root: &NodeRef<'_>) -> Option<NodeRef<'a>> {
    let mut cur = n.clone();
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

/// Replaces `n` by its children (Nokogiri `node.replace(node.children)`).
pub(super) fn unwrap(n: &NodeRef<'_>) {
    while let Some(c) = n.first_child() {
        n.insert_before(&c);
    }
    n.remove_from_parent();
}

/// Creates a detached element with the given attributes (in order).
pub(super) fn new_element<'a>(tree: &'a Tree, name: &str, attrs: &[(&str, &str)]) -> NodeRef<'a> {
    let el = tree.new_element(name);
    for (k, v) in attrs {
        el.set_attr(k, v);
    }
    el
}

/// Creates a detached text node.
pub(super) fn new_text<'a>(tree: &'a Tree, text: &str) -> NodeRef<'a> {
    tree.new_text(text)
}

/// Ruby `String#strip`: removes leading/trailing ASCII whitespace and NUL.
pub(super) fn ruby_strip(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, '\0' | '\t' | '\n' | '\x0B' | '\x0C' | '\r' | ' '))
}

/// ActiveSupport `blank?` for strings: empty or only Unicode whitespace.
pub(super) fn is_blank(s: &str) -> bool {
    s.chars().all(char::is_whitespace)
}

/// Escapes `&`, `<`, `>` and `"` for HTML text and attribute values.
pub(super) fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use dom_query::Document;

    #[test]
    fn unwrap_keeps_children_in_order() {
        let doc = Document::fragment("<p>a<u>b<i>c</i>d</u>e</p>");
        let root = doc.html_root();
        let u = doc.select("u").nodes()[0].clone();
        unwrap(&u);
        assert_eq!(root.inner_html().to_string(), "<p>ab<i>c</i>de</p>");
    }

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
    fn class_helpers() {
        let doc = Document::fragment("<p class=\"a  b\">x</p>");
        let p = doc.select("p").nodes()[0].clone();
        add_class(&p, "b c");
        assert_eq!(attr(&p, "class").as_deref(), Some("a b c"));
        remove_class_tokens(&p, &["a", "b", "c"]);
        assert!(!p.has_attr("class"));
    }

    #[test]
    fn strip_and_blank() {
        assert_eq!(ruby_strip("\0 \t x \n"), "x");
        assert_eq!(ruby_strip("\u{a0}x"), "\u{a0}x");
        assert!(is_blank(" \u{a0}\n"));
        assert!(!is_blank(" x"));
    }
}
