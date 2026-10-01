//! The smaller post-sanitizer DOM filters (CQ §3.1 step 5): emoji, tables, images, links,
//! `<details>` summaries, comment-mode `notranslate` + color chips, and `dir="auto"`.

use std::sync::OnceLock;

use dom_query::{Document, NodeRef};
use regex::Regex;

use super::emoji::{self, Seg};
use super::sanitize::{url_allowed, url_scheme};
use super::{Ctx, dom};

// ------------------------------------------------------------------------------------------
// emoji (GH §8.5)

/// Elements whose text is never emoji-processed.
const NO_EMOJI: &[&str] = &[
    "code",
    "pre",
    "math-renderer",
    "svg",
    "g-emoji",
    "script",
    "style",
    "textarea",
];

pub(super) fn emoji(doc: &Document, ctx: &Ctx) {
    let texts: Vec<NodeRef<'_>> = dom::root(doc)
        .descendants()
        .into_iter()
        .filter(NodeRef::is_text)
        .collect();
    for t in texts {
        let Some(s) = dom::text_of(&t) else { continue };
        if !s.contains(':') && !s.contains('\u{fe0f}') {
            continue;
        }
        if dom::has_ancestor(&t, |a| {
            dom::name(a).is_some_and(|n| NO_EMOJI.contains(&&*n))
        }) {
            continue;
        }
        let Some(segs) = emoji::convert(&s) else {
            continue;
        };
        if let [Seg::Text(only)] = segs.as_slice() {
            t.set_text(only.as_str());
            continue;
        }
        for seg in segs {
            let node = match seg {
                Seg::Text(s) => doc.tree.new_text(s.as_str()),
                Seg::GEmoji { alias, chars } => {
                    let g = doc.tree.new_element("g-emoji");
                    g.set_attr("class", "g-emoji");
                    g.set_attr("alias", &alias);
                    g.append_child(&doc.tree.new_text(chars.as_str()));
                    g
                }
                Seg::Custom(name) => {
                    let img = doc.tree.new_element("img");
                    let code = format!(":{name}:");
                    img.set_attr("class", "emoji");
                    img.set_attr("title", &code);
                    img.set_attr("alt", &code);
                    img.set_attr(
                        "src",
                        &format!("{}/github/emoji/{name}.png", ctx.bundle_url),
                    );
                    img.set_attr("height", "20");
                    img.set_attr("width", "20");
                    img.set_attr("align", "absmiddle");
                    img
                }
            };
            t.insert_before(&node);
        }
        t.remove_from_parent();
    }
}

// ------------------------------------------------------------------------------------------
// tables (GH §8.3)

pub(super) fn tables(doc: &Document, ctx: &Ctx) {
    for table in dom::elements_named(doc, "table") {
        if ctx.comment && !table.has_attr("role") {
            table.set_attr("role", "table");
        }
        // Only outermost tables are wrapped (`S/pages/lifeparticle_*`: nested raw tables).
        if table
            .parent()
            .is_some_and(|p| p.has_name("markdown-accessiblity-table"))
            || dom::has_ancestor(&table, |a| a.has_name("table"))
        {
            continue;
        }
        let wrapper = doc.tree.new_element("markdown-accessiblity-table");
        if let Some(sp) = table.attr("data-sourcepos") {
            wrapper.set_attr("data-sourcepos", &sp);
            table.remove_attr("data-sourcepos");
        }
        dom::wrap(&table, &wrapper);
    }
}

// ------------------------------------------------------------------------------------------
// links and images (GH §8.8, §8.9)

/// Host of an absolute `http(s)://` or protocol-relative URL, as written.
fn url_host(url: &str) -> Option<&str> {
    let rest = if let Some(r) = url.strip_prefix("//") {
        r
    } else {
        let (scheme, r) = url.split_once("://")?;
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return None;
        }
        r
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    Some(host.split(':').next().unwrap_or(host))
}

/// `rel="nofollow"` rule (GH §8.9): absolute or protocol-relative URL whose host is neither
/// `github.com` nor a subdomain of it (case-insensitive).
pub(super) fn is_external(url: &str) -> bool {
    url_host(url).is_some_and(|h| {
        let h = h.to_ascii_lowercase();
        h != "github.com" && !h.ends_with(".github.com")
    })
}

/// Whether github.com would route the image through camo (GH §8.8): external http(s)
/// images except GitHub's own content hosts (case-sensitive match, `S/probes/links`).
fn would_camo(url: &str) -> bool {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    url_host(url).is_some_and(|h| {
        !(h == "github.com" || h == "gist.github.com" || h.ends_with(".githubusercontent.com"))
    })
}

/// `https://github.com/O/R/blob/REF/path` image URLs are served from `/raw/` (GH §2.3).
fn blob_to_raw(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.splitn(4, '/');
    let (owner, repo, kind, tail) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    (kind == "blob" && !owner.is_empty() && !repo.is_empty())
        .then(|| format!("https://github.com/{owner}/{repo}/raw/{tail}"))
}

/// GitHub user-attachment URLs (`/user-attachments/assets/<uuid>`, `/O/R/assets/<n>/<uuid>`).
fn is_attachment(url: &str) -> bool {
    url.strip_prefix("https://github.com/").is_some_and(|rest| {
        rest.starts_with("user-attachments/assets/") || rest.split('/').nth(2) == Some("assets")
    })
}

pub(super) fn images(doc: &Document) {
    let root = dom::root(doc);
    // In README and comment contexts only user-attachment videos survive (GH §8.11;
    // `S/api_gfm_ctx.html` drops `<video src="https://example.com/v.mp4">`).
    for video in dom::elements_named(doc, "video") {
        let src = video.attr("src").map(|s| s.to_string()).unwrap_or_default();
        if !is_attachment(&src) {
            dom::unwrap(&video);
        }
    }
    for source in dom::elements_named(doc, "source") {
        if let Some(set) = source.attr("srcset").map(|s| s.to_string()) {
            if let Some(r) = blob_to_raw(&set) {
                source.set_attr("srcset", &r);
            }
        }
    }
    for img in dom::elements_named(doc, "img") {
        if img.has_class("emoji") {
            continue;
        }
        let mut href = None;
        if let Some(src) = img.attr("src").map(|s| s.to_string()) {
            if !url_allowed(&src, &["http", "https"]) {
                img.remove_attr("src");
            } else if src.starts_with("//") {
                // camo cannot proxy protocol-relative URLs; GitHub empties them.
                img.set_attr("src", "");
            } else if let Some(r) = blob_to_raw(&src) {
                img.set_attr("src", &r);
                href = Some(src);
            }
        }
        if dom::has_ancestor(&img, |a| a.has_name("picture")) {
            continue;
        }
        let w = img.attr("width").map(|s| s.to_string());
        let h = img.attr("height").map(|s| s.to_string());
        let numeric = |v: &Option<String>| {
            v.as_ref()
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        };
        if numeric(&w) && numeric(&h) {
            let (w, h) = (w.unwrap_or_default(), h.unwrap_or_default());
            img.set_attr(
                "style",
                &format!(
                    "max-width: 100%; height: auto; max-height: {h}px;; aspect-ratio: {w} / {h}; background-color: var(--bgColor-muted); border-radius: 6px"
                ),
            );
            img.add_class("js-gh-image-fallback");
        } else {
            img.set_attr("style", "max-width: 100%;");
        }
        if dom::has_ancestor(&img, |a| a.has_name("a")) {
            continue;
        }
        let src = href
            .or_else(|| img.attr("src").map(|s| s.to_string()))
            .unwrap_or_default();
        let a = doc.tree.new_element("a");
        a.set_attr("target", "_blank");
        let nofollow = would_camo(&src) || is_external(&src);
        a.set_attr(
            "rel",
            if nofollow {
                "noopener noreferrer nofollow"
            } else {
                "noopener noreferrer"
            },
        );
        a.set_attr("href", &src);
        let top_level = img.parent().is_some_and(|p| p.id == root.id);
        dom::wrap(&img, &a);
        // A raw `<img>` that is a whole HTML block ends up in a paragraph on github.com
        // (`S/pages/lifeparticle_*` after `<!-- … -->`, `S/probes/attrs` CASE-img), except
        // user attachments, which GitHub turns into its own media markup.
        if top_level && !is_attachment(&src) {
            let p = doc.tree.new_element("p");
            dom::wrap(&a, &p);
        }
    }
    for picture in dom::elements_named(doc, "picture") {
        if picture
            .parent()
            .is_some_and(|p| p.has_name("themed-picture"))
        {
            continue;
        }
        let tp = doc.tree.new_element("themed-picture");
        tp.set_attr("data-catalyst-inline", "true");
        dom::wrap(&picture, &tp);
    }
}

pub(super) fn links(doc: &Document) {
    for a in dom::elements_named(doc, "a") {
        if let Some(href) = a.attr("href").map(|s| s.to_string()) {
            if !url_allowed(&href, &["http", "https", "mailto", "xmpp"]) {
                a.remove_attr("href");
            } else {
                if let Some(rest) = href.strip_prefix("http://github.com") {
                    if rest.is_empty() || rest.starts_with(['/', '?', '#']) {
                        a.set_attr("href", &format!("https://github.com{rest}"));
                    }
                }
                if !a.has_attr("rel")
                    && url_scheme(&href).is_none_or(|s| s != "mailto")
                    && is_external(&href)
                {
                    a.set_attr("rel", "nofollow");
                }
            }
        }
        if !dom::has_attrs(&a) {
            dom::unwrap(&a);
        }
    }
}

// ------------------------------------------------------------------------------------------
// misc

/// `<details>` without a `<summary>` gets GitHub's default one (`S/probes/tags.gfm.html` T28).
pub(super) fn details(doc: &Document) {
    for d in dom::elements_named(doc, "details") {
        if d.element_children().iter().any(|c| c.has_name("summary")) {
            continue;
        }
        let s = doc.tree.new_element("summary");
        s.append_child(&doc.tree.new_text("Details"));
        d.prepend_child(&s);
    }
}

fn color_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?ix)^(?:
                \#[0-9a-f]{6}
              | rgba?\(\s*\d{1,3}%?\s*,\s*\d{1,3}%?\s*,\s*\d{1,3}%?\s*(?:,\s*(?:\d*\.)?\d+%?\s*)?\)
              | hsla?\(\s*\d{1,3}(?:deg)?\s*,\s*\d{1,3}%\s*,\s*\d{1,3}%\s*(?:,\s*(?:\d*\.)?\d+%?\s*)?\)
            )$",
        )
        .expect("static color regex")
    })
}

/// Comment context: `notranslate` on all `pre`/`code` and color chips in inline code
/// (GH §8.13, §9.B, `S/probes/colors.gfm.html`).
pub(super) fn comment_code(doc: &Document) {
    for n in dom::elements(doc) {
        let is_pre = n.has_name("pre");
        if !is_pre && !n.has_name("code") {
            continue;
        }
        n.add_class("notranslate");
        if is_pre || dom::has_ancestor(&n, |a| a.has_name("pre")) {
            continue;
        }
        let text = n.text().to_string();
        if n.element_children().is_empty() && color_regex().is_match(&text) {
            let chip = doc.tree.new_element("span");
            chip.set_attr(
                "class",
                "ml-1 d-inline-block border circle color-border-subtle",
            );
            chip.set_attr(
                "style",
                &format!("background-color: {text}; height: 8px; width: 8px;"),
            );
            n.append_child(&chip);
        }
    }
}

/// `dir="auto"` placement (GH §8.13): every `p`, `h1`–`h6`, `ul`/`ol` except task lists, and
/// every `div` except the plain clipboard wrapper; never overriding an existing `dir`.
pub(super) fn dir_auto(doc: &Document) {
    for n in dom::elements(doc) {
        let Some(name) = dom::name(&n) else { continue };
        let wanted = match &*name {
            "p" => true,
            // An empty heading stays a bare `<hN></hN>` (GH §6.1).
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => n.first_child().is_some(),
            "ul" | "ol" => !n.has_class("contains-task-list"),
            "div" => !n.has_class("snippet-clipboard-content"),
            _ => false,
        };
        if wanted && !n.has_attr("dir") {
            n.set_attr("dir", "auto");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(html: &str, f: impl Fn(&Document)) -> String {
        let doc = Document::fragment(html);
        f(&doc);
        dom::root(&doc).inner_html().to_string()
    }

    #[test]
    fn nofollow_rule() {
        for url in [
            "https://example.com/x",
            "//example.com/x",
            "https://github.com.evil.com/x",
            "https://raw.githubusercontent.com/a",
            "https://user.github.io/x",
        ] {
            assert!(is_external(url), "{url}");
        }
        for url in [
            "https://github.com/a/b",
            "https://GITHUB.COM/x",
            "https://docs.github.com/x",
            "rel/x",
            "#frag",
            "mailto:a@b.c",
            "/abs",
        ] {
            assert!(!is_external(url), "{url}");
        }
        assert!(would_camo("https://www.github.com/x/img.png"));
        assert!(would_camo("https://GITHUB.COM/x/img.png"));
        assert!(!would_camo("https://github.com/a/b/img.png"));
        assert!(!would_camo(
            "https://raw.githubusercontent.com/a/b/c/img.png"
        ));
        assert!(!would_camo("rel.png"));
    }

    #[test]
    fn links_filter() {
        let out = apply(
            r#"<a href="https://example.com/x">a</a><a href="http://github.com/x">b</a><a href="javascript:alert(1)">c</a><a>d</a><a href="mailto:a@b.c">e</a>"#,
            links,
        );
        assert_eq!(
            out,
            r#"<a href="https://example.com/x" rel="nofollow">a</a><a href="https://github.com/x">b</a>cd<a href="mailto:a@b.c">e</a>"#
        );
    }

    #[test]
    fn images_filter() {
        let out = apply(r#"<p><img src="x.png" alt="a"></p>"#, images);
        assert_eq!(
            out,
            r#"<p><a target="_blank" rel="noopener noreferrer" href="x.png"><img src="x.png" alt="a" style="max-width: 100%;"></a></p>"#
        );
        let out = apply(
            r#"<a href="u"><img src="https://img.shields.io/x"></a>"#,
            images,
        );
        assert_eq!(
            out,
            r#"<a href="u"><img src="https://img.shields.io/x" style="max-width: 100%;"></a>"#
        );
        let out = apply(
            r#"<div><img src="https://e.com/a.png" width="100" height="50"></div>"#,
            images,
        );
        assert!(out.contains(r#"style="max-width: 100%; height: auto; max-height: 50px;; aspect-ratio: 100 / 50; background-color: var(--bgColor-muted); border-radius: 6px" class="js-gh-image-fallback""#));
        assert!(out.starts_with(r#"<div><a target="_blank" rel="noopener noreferrer nofollow" href="https://e.com/a.png">"#));
        let out = apply(
            r#"<p><img src="//x.com/p.png"><img src="data:x"></p>"#,
            images,
        );
        assert_eq!(
            out,
            r#"<p><a target="_blank" rel="noopener noreferrer" href=""><img src="" style="max-width: 100%;"></a><a target="_blank" rel="noopener noreferrer" href=""><img style="max-width: 100%;"></a></p>"#
        );
        // A top-level raw image becomes a paragraph, except user attachments.
        let out = apply(
            r#"<img src="a.png"><img src="https://github.com/user-attachments/assets/x">"#,
            images,
        );
        assert_eq!(
            out,
            r#"<p><a target="_blank" rel="noopener noreferrer" href="a.png"><img src="a.png" style="max-width: 100%;"></a></p><a target="_blank" rel="noopener noreferrer" href="https://github.com/user-attachments/assets/x"><img src="https://github.com/user-attachments/assets/x" style="max-width: 100%;"></a>"#
        );
        // `/blob/` image URLs load from `/raw/`; the link keeps the blob URL.
        let out = apply(
            r#"<p><img src="https://github.com/o/r/blob/main/i.png"></p>"#,
            images,
        );
        assert_eq!(
            out,
            r#"<p><a target="_blank" rel="noopener noreferrer" href="https://github.com/o/r/blob/main/i.png"><img src="https://github.com/o/r/raw/main/i.png" style="max-width: 100%;"></a></p>"#
        );
        assert_eq!(
            apply(
                r#"<p><video src="https://e.com/v.mp4"></video></p>"#,
                images
            ),
            "<p></p>"
        );
        let out = apply(
            r#"<picture><source srcset="a.png"><img src="b.png"></picture>"#,
            images,
        );
        assert_eq!(
            out,
            r#"<themed-picture data-catalyst-inline="true"><picture><source srcset="a.png"><img src="b.png"></picture></themed-picture>"#
        );
    }

    #[test]
    fn tables_details_dir() {
        let doc = Document::fragment("<table><tr><td>x</td></tr></table>");
        tables(&doc, &Ctx::for_tests(true));
        assert_eq!(
            dom::root(&doc).inner_html().to_string(),
            r#"<markdown-accessiblity-table><table role="table"><tbody><tr><td>x</td></tr></tbody></table></markdown-accessiblity-table>"#
        );
        assert_eq!(
            apply("<details>x</details>", details),
            "<details><summary>Details</summary>x</details>"
        );
        assert_eq!(
            apply(
                r#"<p>a</p><ul class="contains-task-list"></ul><div class="snippet-clipboard-content"></div><div dir="rtl"></div><blockquote></blockquote>"#,
                dir_auto
            ),
            r#"<p dir="auto">a</p><ul class="contains-task-list"></ul><div class="snippet-clipboard-content"></div><div dir="rtl"></div><blockquote></blockquote>"#
        );
    }

    #[test]
    fn colors() {
        let out = apply(
            "<p><code>#00ff00</code> <code>#fff</code> <code>rgba(0, 255, 0, 1)</code> <code>hsl(120, 100%, 50%)</code></p>",
            comment_code,
        );
        assert_eq!(out.matches("color-border-subtle").count(), 3);
        assert!(out.contains(r#"<code class="notranslate">#fff</code>"#));
        assert!(out.contains(r#"style="background-color: #00ff00; height: 8px; width: 8px;""#));
    }

    #[test]
    fn emoji_dom() {
        let ctx = Ctx::for_tests(false);
        let out = apply(
            "<p>:smile: <code>:smile:</code> <em>:warning:</em> :octocat:</p>",
            |d| emoji(d, &ctx),
        );
        assert_eq!(
            out,
            r#"<p>😄 <code>:smile:</code> <em><g-emoji class="g-emoji" alias="warning">⚠️</g-emoji></em> <img class="emoji" title=":octocat:" alt=":octocat:" src="/b/github/emoji/octocat.png" height="20" width="20" align="absmiddle"></p>"#
        );
    }
}
