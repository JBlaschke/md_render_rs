//! Local file dependencies of a rendered document (images, video, audio), GRIP §11.1.
//!
//! After each render the HTML is scanned for `src`, `srcset` and `poster` attributes of
//! media elements. Relative URLs (resolved against the document's directory) and
//! root-relative URLs (resolved against the served root) that stay inside the root are
//! dependencies: when one changes on disk the server sends an `assets` event so the client
//! can cache-bust just that element.

use percent_encoding::percent_decode_str;

use crate::PREFIX;

/// One local dependency of a document.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DepRef {
    /// Root-relative path with `/` separators (decoded).
    pub rel: String,
    /// The URL path the browser requests for it (`/`-prefixed, percent-encoded).
    pub url: String,
}

/// Media elements whose URL attributes are dependencies.
const SELECTOR: &str = "img, source, video, audio, track, input[type=image]";

/// Scan rendered HTML for local dependencies of the document at root-relative `doc_rel`.
/// The result is sorted and free of duplicates.
pub fn scan(html: &str, doc_rel: &str) -> Vec<DepRef> {
    let doc = dom_query::Document::fragment(html);
    let mut urls: Vec<String> = Vec::new();
    for node in doc.select(SELECTOR).nodes() {
        for attr in ["src", "poster"] {
            if let Some(v) = node.attr(attr) {
                urls.push(v.to_string());
            }
        }
        if let Some(set) = node.attr("srcset") {
            // `srcset="a.png 1x, b.png 2x"`: the URL is the first token of each candidate.
            urls.extend(
                set.split(',')
                    .filter_map(|c| c.split_whitespace().next())
                    .map(str::to_owned),
            );
        }
    }
    let base_dir: Vec<&str> = {
        let mut segs: Vec<&str> = doc_rel.split('/').filter(|s| !s.is_empty()).collect();
        segs.pop(); // the document's own file name
        segs
    };
    let mut out: Vec<DepRef> = urls
        .iter()
        .filter_map(|u| resolve_local(u, &base_dir))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Resolve one URL attribute value to a root-relative path, or None when it is not a local
/// file inside the root (absolute URL, scheme, fragment, internal route, escaping `..`).
fn resolve_local(url: &str, base_dir: &[&str]) -> Option<DepRef> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('#') || url.starts_with("//") || has_scheme(url) {
        return None;
    }
    if url == PREFIX || url.starts_with(&format!("{PREFIX}/")) {
        return None;
    }
    // Drop query and fragment.
    let path = url.split(['?', '#']).next().unwrap_or("");
    if path.is_empty() {
        return None;
    }
    let decoded = percent_decode_str(path).decode_utf8().ok()?;
    if decoded.contains('\0') || decoded.contains('\\') {
        return None;
    }
    let mut segs: Vec<String> = if decoded.starts_with('/') {
        Vec::new()
    } else {
        base_dir.iter().map(|s| (*s).to_owned()).collect()
    };
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop()?; // escaping the root: not a dependency we may watch
            }
            s => {
                if s.eq_ignore_ascii_case(".git") {
                    return None;
                }
                segs.push(s.to_owned());
            }
        }
    }
    if segs.is_empty() || decoded.ends_with('/') {
        return None;
    }
    let rel = segs.join("/");
    let url = super::paths::url_for(&rel, false);
    Some(DepRef { rel, url })
}

/// Does `url` start with a URL scheme (`http:`, `data:`, `mailto:` ...)?
fn has_scheme(url: &str) -> bool {
    let Some(colon) = url.find(':') else {
        return false;
    };
    let scheme = &url[..colon];
    let mut chars = scheme.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rels(html: &str, doc: &str) -> Vec<String> {
        scan(html, doc).into_iter().map(|d| d.rel).collect()
    }

    #[test]
    fn finds_relative_and_root_relative_media() {
        let html = r#"<p><img src="img/a.png" alt=""> <img src="/logo.svg?x=1#f"></p>
            <video src="clip.mp4" poster="poster.jpg"><source src="clip.webm"></video>
            <picture><source srcset="w1.png 1x, w2.png 2x"><img src="w.png"></picture>"#;
        assert_eq!(
            rels(html, "docs/README.md"),
            [
                "docs/clip.mp4",
                "docs/clip.webm",
                "docs/img/a.png",
                "docs/poster.jpg",
                "docs/w.png",
                "docs/w1.png",
                "docs/w2.png",
                "logo.svg",
            ]
        );
    }

    #[test]
    fn skips_remote_data_internal_and_escaping_urls() {
        let html = r##"<img src="https://example.com/a.png"><img src="//cdn/x.png">
            <img src="data:image/png;base64,AAAA"><img src="/__/md-render/bundle/github/emoji/shipit.png">
            <img src="../../outside.png"><img src="#frag"><img src=""><img src="../.git/x.png">
            <a href="link.png">not media</a>"##;
        assert!(
            rels(html, "docs/README.md").is_empty(),
            "{:?}",
            rels(html, "docs/README.md")
        );
    }

    #[test]
    fn decodes_and_reencodes() {
        let deps = scan(
            r#"<img src="my%20pic.png"><img src="my pic.png"><img src="./a/../b.png">"#,
            "README.md",
        );
        assert_eq!(deps.len(), 2);
        assert_eq!(
            deps[0],
            DepRef {
                rel: "b.png".into(),
                url: "/b.png".into()
            }
        );
        assert_eq!(
            deps[1],
            DepRef {
                rel: "my pic.png".into(),
                url: "/my%20pic.png".into()
            }
        );
    }

    #[test]
    fn scheme_detection() {
        assert!(has_scheme("http://x"));
        assert!(has_scheme("data:,x"));
        assert!(has_scheme("git+ssh:x"));
        assert!(!has_scheme("a/b:c")); // ':' after a '/' is part of the path
        assert!(!has_scheme("img.png"));
        assert!(!has_scheme("1x:y"));
    }
}
