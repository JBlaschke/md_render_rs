//! Offline equivalents of what GitLab's JavaScript does to the server HTML (DESIGN D3, GL §0.3,
//! §6.8, §6.11, §6.12), plus md-render's own annotations (`data-mdr-key`, DESIGN §5/§7) and the
//! optional removal of source positions.

use dom_query::NodeRef;
use sha2::{Digest, Sha256};

use super::dom;

/// Applies the post-JavaScript transforms.
pub(super) fn apply(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["img", "pre", "table"]) {
        let n = root.tree.get_unchecked(&id);
        match dom::tag(&n).as_deref() {
            Some("img") => lazy_loaded_image(&n),
            Some("pre") => {
                // syntax_highlight.js
                if dom::classes(&n).iter().any(|c| c == "js-syntax-highlight") {
                    dom::add_class(&n, "code-syntax-highlight-theme");
                }
            }
            Some("table") => table_wrapper(root, &n),
            _ => {}
        }
    }
}

/// ImageLazyLoadFilter + the client lazy loader, minus the placeholder (GL §6.12): the image keeps
/// its real `src`, gains `decoding="async"` and `loading="lazy"`, and never gets class `lazy`.
fn lazy_loaded_image(img: &NodeRef<'_>) {
    if dom::classes(img).iter().any(|c| c == "js-render-iframe") {
        return;
    }
    img.set_attr("decoding", "async");
    img.set_attr("loading", "lazy");
}

/// `renderMarkdownTables` (GL §6.11): tables with a header row get GitLab's scroll/shadow
/// wrapper; tables without `<thead>` are left alone, as the client does.
fn table_wrapper(root: &NodeRef<'_>, table: &NodeRef<'_>) {
    let Some(thead) = dom::element_children(table)
        .into_iter()
        .find(|c| dom::is(c, "thead"))
    else {
        return;
    };
    let Some(row) = dom::element_children(&thead)
        .into_iter()
        .find(|c| dom::is(c, "tr"))
    else {
        return;
    };
    if !dom::element_children(&row)
        .iter()
        .any(|c| dom::is_any(c, &["th", "td"]))
    {
        return;
    }
    let outer = dom::new_element(
        root.tree,
        "div",
        &[("class", "gl-table-shadow-overlay gl-my-5")],
    );
    let inner = dom::new_element(
        root.tree,
        "div",
        &[
            (
                "class",
                "gl-max-h-[70vh] gl-w-fit gl-max-w-full gl-overflow-x-auto gl-overflow-y-auto",
            ),
            ("data-sticky-header", "true"),
        ],
    );
    table.insert_before(&outer);
    outer.append_child(&inner);
    inner.append_child(table);
    dom::add_class(table, "gl-min-w-full gl-overflow-y-hidden");
}

/// Stable content key: the first 16 hex digits of SHA-256 over the parts.
pub(super) fn content_key(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    let digest = h.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Adds `data-mdr-key` to code-block wrappers (including math and mermaid blocks) and to inline
/// or display math elements, so the live client can keep already rendered nodes (DESIGN §7).
pub(super) fn add_keys(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["div", "span", "code", "pre"]) {
        let n = root.tree.get_unchecked(&id);
        let classes = dom::classes(&n);
        if dom::is(&n, "div") && classes.iter().any(|c| c == "markdown-code-block") {
            let pre = dom::element_children(&n)
                .into_iter()
                .find(|c| dom::is(c, "pre"));
            let (lang, params, text) = match &pre {
                Some(p) => (
                    dom::attr(p, "data-canonical-lang").unwrap_or_default(),
                    dom::attr(p, "data-lang-params").unwrap_or_default(),
                    dom::text(p),
                ),
                None => (String::new(), String::new(), dom::text(&n)),
            };
            n.set_attr(
                "data-mdr-key",
                &content_key(&["code", &lang, &params, &text]),
            );
        } else if !dom::is(&n, "pre")
            && classes.iter().any(|c| c == "js-render-math")
            && !dom::has_ancestor(&n, &["pre"], root)
        {
            let style = dom::attr(&n, "data-math-style").unwrap_or_default();
            n.set_attr(
                "data-mdr-key",
                &content_key(&["math", &style, &dom::text(&n)]),
            );
        }
    }
}

/// Removes `data-sourcepos` and `data-checkbox-sourcepos` everywhere (`opts.sourcepos == false`).
pub(super) fn strip_sourcepos(root: &NodeRef<'_>) {
    for id in dom::descendants(root) {
        let n = root.tree.get_unchecked(&id);
        if n.is_element() {
            n.remove_attrs(&["data-sourcepos", "data-checkbox-sourcepos"]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dom_query::Document;

    fn run(html: &str, f: impl Fn(&NodeRef<'_>)) -> String {
        let doc = Document::fragment(html);
        let root = doc.html_root();
        f(&root);
        root.inner_html().to_string()
    }

    #[test]
    fn images_tables_code() {
        assert_eq!(
            run("<img src=\"a.png\" alt=\"\">", apply),
            "<img src=\"a.png\" alt=\"\" decoding=\"async\" loading=\"lazy\">"
        );
        assert_eq!(
            run(
                "<table dir=\"auto\"><thead><tr><th>h</th></tr></thead></table>",
                apply
            ),
            "<div class=\"gl-table-shadow-overlay gl-my-5\"><div class=\"gl-max-h-[70vh] gl-w-fit gl-max-w-full gl-overflow-x-auto gl-overflow-y-auto\" data-sticky-header=\"true\"><table dir=\"auto\" class=\"gl-min-w-full gl-overflow-y-hidden\"><thead><tr><th>h</th></tr></thead></table></div></div>"
        );
        assert_eq!(
            run("<table><tbody><tr><td>x</td></tr></tbody></table>", apply),
            "<table><tbody><tr><td>x</td></tr></tbody></table>"
        );
        assert_eq!(
            run(
                "<pre class=\"code highlight js-syntax-highlight language-c\"><code></code></pre>",
                apply
            ),
            "<pre class=\"code highlight js-syntax-highlight language-c code-syntax-highlight-theme\"><code></code></pre>"
        );
    }

    #[test]
    fn keys_are_content_based() {
        let a = run(
            "<div class=\"markdown-code-block\"><pre data-canonical-lang=\"c\"><code>x</code></pre></div><p><span class=\"js-render-math\" data-math-style=\"inline\">a</span></p>",
            add_keys,
        );
        let b = run(
            "<p>other</p><div class=\"markdown-code-block\"><pre data-canonical-lang=\"c\"><code>x</code></pre></div>",
            add_keys,
        );
        let key = |s: &str| {
            s.split("data-mdr-key=\"")
                .nth(1)
                .map(|r| r[..16].to_owned())
        };
        assert_eq!(key(&a), key(&b));
        assert_eq!(a.matches("data-mdr-key").count(), 2);
        assert_ne!(
            content_key(&["code", "c", "", "x"]),
            content_key(&["code", "c", "", "y"])
        );
    }

    #[test]
    fn sourcepos_stripped() {
        assert_eq!(
            run(
                "<p data-sourcepos=\"1:1-1:3\">x<input data-checkbox-sourcepos=\"1:2-1:2\"></p>",
                strip_sourcepos
            ),
            "<p>x<input></p>"
        );
    }
}
