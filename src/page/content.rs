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
pub(crate) fn for_export(html: &str, assets: &mut Assets<'_>) -> anyhow::Result<String> {
    let mentions_bundle = html.contains(BUNDLE_ROUTE);
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

/// Is `url` a document-relative reference (`img/a.png`, `../x.md#s`)? Not: empty, fragment-only,
/// query-only, root- or scheme-relative (`/x`, `//host/x`), or with a scheme (`https:`, `data:`,
/// `mailto:`).
fn is_document_relative(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() || u.starts_with(['#', '?', '/', '\\']) {
        return false;
    }
    let scheme_end = u.find([':', '/', '?', '#']);
    !matches!(scheme_end, Some(i) if u.as_bytes()[i] == b':'
        && u[..i].starts_with(|c: char| c.is_ascii_alphabetic())
        && u[..i].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
}

/// `prefix/url` for a document-relative `url`, else `None`.
fn rebase_one(url: &str, prefix: &str) -> Option<String> {
    is_document_relative(url).then(|| {
        let u = url.trim();
        let u = u.strip_prefix("./").unwrap_or(u);
        format!("{}/{u}", prefix.trim_end_matches('/'))
    })
}

/// `srcset` with each document-relative candidate URL rebased (HTML's candidate syntax: a URL
/// without whitespace, optional descriptors, comma separators).
fn rebase_srcset(srcset: &str, prefix: &str) -> Option<String> {
    let mut out = String::with_capacity(srcset.len() + 16);
    let mut changed = false;
    let mut rest = srcset;
    loop {
        let start = rest.len()
            - rest
                .trim_start_matches(|c: char| c.is_whitespace() || c == ',')
                .len();
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.is_empty() {
            break;
        }
        let url_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let mut url = &rest[..url_len];
        // A URL directly followed by a comma has no descriptors.
        let trailing_commas = url.len() - url.trim_end_matches(',').len();
        url = &url[..url.len() - trailing_commas];
        match rebase_one(url, prefix) {
            Some(new) => {
                out.push_str(&new);
                changed = true;
            }
            None => out.push_str(url),
        }
        rest = &rest[url.len()..];
        let desc_len = if trailing_commas > 0 {
            0
        } else {
            rest.find(',').unwrap_or(rest.len())
        };
        out.push_str(&rest[..desc_len]);
        rest = &rest[desc_len..];
    }
    changed.then_some(out)
}

/// Export to another directory than the source's: prefix every document-relative URL in the
/// content (`src`, `href`, `poster`, `srcset`) with `prefix`, the URL path from the output
/// directory to the source directory (e.g. `../docs`), so local images and links keep
/// resolving. Bundle references are root-relative and stay as they are.
pub(crate) fn rebase_relative_urls(html: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return html.to_owned();
    }
    let doc = Document::fragment(html);
    let mut changed = false;
    for attr in URL_ATTRS {
        for node in doc.select(&format!("[{attr}]")).nodes() {
            if let Some(new) = node.attr(attr).and_then(|v| rebase_one(&v, prefix)) {
                node.set_attr(attr, &new);
                changed = true;
            }
        }
    }
    for node in doc.select("[srcset]").nodes() {
        if let Some(new) = node.attr("srcset").and_then(|v| rebase_srcset(&v, prefix)) {
            node.set_attr("srcset", &new);
            changed = true;
        }
    }
    if changed {
        doc.html_root().inner_html().to_string()
    } else {
        html.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_relative_urls() {
        for rel in [
            "img/a.png",
            "./a.png",
            "../x.md#s",
            "a%20b.png",
            "x.md?raw=1",
            "a/b:c",
        ] {
            assert!(is_document_relative(rel), "{rel}");
        }
        for not in [
            "",
            "#top",
            "?q",
            "/abs.png",
            "//cdn/x.png",
            "https://e.com/x",
            "data:image/png;base64,AA",
            "mailto:a@b",
            "MAILTO:a@b",
            "\\\\share\\x",
        ] {
            assert!(!is_document_relative(not), "{not}");
        }
    }

    #[test]
    fn rebases_attributes_and_srcset() {
        let html = "<p><a href=\"img/a.png\"><img src=\"./img/a.png\" alt=\"a\" \
             srcset=\"img/a.png 1x,img/b.png 2x, https://e.com/c.png 3x\"></a> \
             <a href=\"#x\">x</a> <a href=\"https://e.com\">e</a> <video poster=\"p.jpg\" src=\"/v.mp4\"></video></p>";
        let out = rebase_relative_urls(html, "../docs");
        let doc = Document::fragment(out.as_str());
        let get = |sel: &str, attr: &str| doc.select(sel).attr(attr).map(|s| s.to_string());
        assert_eq!(get("a", "href").as_deref(), Some("../docs/img/a.png"));
        assert_eq!(get("img", "src").as_deref(), Some("../docs/img/a.png"));
        assert_eq!(
            get("img", "srcset").as_deref(),
            Some("../docs/img/a.png 1x,../docs/img/b.png 2x, https://e.com/c.png 3x")
        );
        assert_eq!(get("a[href^='#']", "href").as_deref(), Some("#x"));
        assert_eq!(get("video", "poster").as_deref(), Some("../docs/p.jpg"));
        assert_eq!(get("video", "src").as_deref(), Some("/v.mp4"));
        // Nothing relative: the input comes back unchanged (no re-serialization).
        let plain = "<p><a href=\"#x\">x</a></p>";
        assert_eq!(rebase_relative_urls(plain, "../docs"), plain);
        assert_eq!(rebase_relative_urls(html, ""), html);
    }

    #[test]
    fn srcset_edge_cases() {
        assert_eq!(
            rebase_srcset("a.png, b.png 2x", "p").as_deref(),
            Some("p/a.png, p/b.png 2x")
        );
        // A comma inside a URL (not followed by whitespace) belongs to it (HTML srcset rules);
        // trailing commas end a candidate without descriptors.
        assert_eq!(
            rebase_srcset("a.png,b.png 2x", "p").as_deref(),
            Some("p/a.png,b.png 2x")
        );
        assert_eq!(
            rebase_srcset("a.png,, b.png", "p").as_deref(),
            Some("p/a.png,, p/b.png")
        );
        assert_eq!(rebase_srcset("  ", "p"), None);
        assert_eq!(rebase_srcset("data:image/png;base64,AA 1x", "p"), None);
    }
}
