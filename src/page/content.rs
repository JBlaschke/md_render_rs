//! Export-time post-processing of the rendered content (Inline / Directory asset modes):
//! drop `data-sourcepos` (CQ §3.5: export must strip it) and point references into the bundle
//! (custom emoji `<img src="/__/md-render/bundle/github/emoji/x.png">`, DESIGN D6) at data URIs
//! or copied files.

use dom_query::Document;

use super::assets::Assets;
use super::css::BUNDLE_ROUTE;

/// URL-valued attributes that may point into the bundle.
const URL_ATTRS: [&str; 3] = ["src", "href", "poster"];

/// Rewrite `html` (a content fragment) for an export. Returns the input unchanged when there is
/// nothing to do, so the common case does not re-serialize the document.
pub(crate) fn for_export(
    html: &str,
    assets: &mut Assets<'_>,
    directory_href: Option<&str>,
) -> anyhow::Result<String> {
    let href_prefix = directory_href
        .map(|h| h.trim_end_matches('/'))
        .filter(|h| !h.is_empty());
    let mentions_bundle =
        html.contains(BUNDLE_ROUTE) || href_prefix.is_some_and(|h| html.contains(h));
    if !html.contains("data-sourcepos") && !mentions_bundle {
        return Ok(html.to_owned());
    }

    let doc = Document::fragment(html);
    doc.select("[data-sourcepos]").remove_attr("data-sourcepos");
    if mentions_bundle {
        for attr in URL_ATTRS {
            for node in doc.select(&format!("[{attr}]")).nodes() {
                let Some(value) = node.attr(attr) else {
                    continue;
                };
                match assets.content_url(&value)? {
                    Some(new) if new.is_empty() => node.remove_attr(attr),
                    Some(new) => node.set_attr(attr, &new),
                    None => {}
                }
            }
        }
    }
    Ok(doc.html_root().inner_html().to_string())
}
