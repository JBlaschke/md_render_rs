//! GitHub page chrome: `<html>` attributes, stylesheet cascade, wrappers per mode × layout,
//! client configuration, banners, listing and error fragments (GH §3, §4.4-4.5, §9.A/§9.B).

mod page_support;

use dom_query::Document;
use md_render::page::{Banner, BannerKind, Layout, ListingEntry, error_html, listing_html};
use md_render::render::Features;
use md_render::styles::Bundle;
use md_render::{Flavor, Mode, Theme};
use page_support::*;

fn bundle() -> (tempfile::TempDir, Bundle) {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = full_bundle(dir.path());
    (dir, bundle)
}

const B: &str = "/__/md-render/bundle/";

#[test]
fn readme_auto_skeleton() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Github);
    page.extra_css = vec!["/extra.css".into()];
    let html = page.render(&bundle);
    assert!(html.starts_with("<!DOCTYPE html>\n<html "), "{}", &html[..80]);
    let doc = Document::from(html.as_str());

    // <html> attributes (GH §3.1): must be on the root element.
    let root = doc.select("html");
    assert_eq!(root.attr("lang").as_deref(), Some("en"));
    assert_eq!(root.attr("data-color-mode").as_deref(), Some("auto"));
    assert_eq!(root.attr("data-light-theme").as_deref(), Some("light"));
    assert_eq!(root.attr("data-dark-theme").as_deref(), Some("dark"));
    assert_eq!(root.attr("data-a11y-animated-images").as_deref(), Some("system"));
    assert_eq!(root.attr("data-a11y-link-underlines").as_deref(), Some("true"));

    // Cascade order (GH §4.5): themes for both slots, then the core roles, then extra CSS.
    assert_eq!(
        stylesheet_hrefs(&doc),
        [
            format!("{B}github/assets/light-1167.css"),
            format!("{B}github/assets/dark-d4a4.css"),
            format!("{B}github/assets/primer-primitives-0001.css"),
            format!("{B}github/assets/primer-0002.css"),
            format!("{B}github/assets/global-0003.css"),
            format!("{B}github/assets/github-0004.css"),
            format!("{B}github/assets/repository-0005.css"),
            format!("{B}github/assets/code-c0de.css"),
            "/extra.css".to_owned(),
        ]
    );
    assert_eq!(doc.select("link[data-mdr-style=bundle]").length(), 8);
    assert_eq!(doc.select("head > title").text().as_ref(), "README.md");

    // The page-level rules (tab size, .color-fg-success) come after the bundle CSS and before
    // the user's extra CSS.
    let chrome = doc.select("style[data-mdr-style=chrome]");
    assert!(chrome.text().contains("--tab-size-preference:4"));
    assert!(chrome.text().contains(".color-fg-success{color:var(--fgColor-success)}"));
    let head = doc.select("head").inner_html().to_string();
    let pos = |needle: &str| head.find(needle).unwrap_or_else(|| panic!("{needle} in head"));
    assert!(pos("code-c0de.css") < pos("data-mdr-style=\"chrome\""));
    assert!(pos("data-mdr-style=\"chrome\"") < pos("/extra.css"));

    // README box (GH §3.2 / §9.A).
    let article = doc.select(
        "body > div.container-xl.px-3.px-md-4.px-lg-5.mt-4 > #readme.mdr-box.md > \
         div.js-snippet-clipboard-copy-unpositioned.mdr-box-body > article#md-content",
    );
    assert_eq!(article.length(), 1, "{html}");
    for class in ["markdown-body", "entry-content", "container-lg"] {
        assert!(article.has_class(class), "{class}");
    }
    assert_eq!(article.attr("itemprop").as_deref(), Some("text"));
    assert!(article.attr("data-version").is_none(), "not a live page");
    let header = doc.select("#readme > .mdr-box-header > h2.mdr-box-title");
    assert_eq!(header.text().trim(), "README.md");
    assert_eq!(header.select("svg.octicon-book").length(), 1);
    assert!(article.select("h1.heading-element").exists());

    // Clipboard template (GH §5.3) and the client scripts.
    let template = doc.select("template#snippet-clipboard-copy-button-unpositioned");
    assert_eq!(template.length(), 1);
    // (html5ever keeps template contents in a separate fragment; check the source.)
    let template_src = &html[html.find("<template id=\"snippet-clipboard").unwrap()..];
    let template_src = &template_src[..template_src.find("</template>").unwrap()];
    assert!(template_src.contains("js-clipboard-copy-icon"));
    assert!(template_src.contains("js-clipboard-check-icon color-fg-success d-none"));
    assert_eq!(script_srcs(&doc), ["/__/md-render/static/github.js"]);
    assert_eq!(attr(&doc, "link[rel=icon]", "href").as_deref(), Some("/__/md-render/static/favicon.svg"));

    let cfg = config(&doc);
    assert_eq!(cfg["flavor"], "github");
    assert_eq!(cfg["mode"], "readme");
    assert_eq!(cfg["theme"], "auto");
    assert_eq!(cfg["themes"]["dark"], "dark");
    assert!(cfg["live"].is_null());
    assert_eq!(cfg["scripts"], serde_json::json!({}), "no features, no libraries");
}

#[test]
fn single_themes_and_variants() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Github);

    page.theme = theme(Theme::Light, "light", "dark");
    let doc = Document::from(page.render(&bundle).as_str());
    let hrefs = stylesheet_hrefs(&doc);
    assert_eq!(hrefs[0], format!("{B}github/assets/light-1167.css"));
    assert!(hrefs[1].ends_with("primer-primitives-0001.css"), "only the light theme file: {hrefs:?}");
    assert_eq!(attr(&doc, "html", "data-color-mode").as_deref(), Some("light"));

    page.theme = theme(Theme::Dark, "light", "dark_dimmed");
    let doc = Document::from(page.render(&bundle).as_str());
    let hrefs = stylesheet_hrefs(&doc);
    assert_eq!(hrefs[0], format!("{B}github/assets/dark_dimmed-d1d1.css"));
    assert!(hrefs[1].ends_with("primer-primitives-0001.css"));
    assert_eq!(attr(&doc, "html", "data-color-mode").as_deref(), Some("dark"));
    assert_eq!(attr(&doc, "html", "data-dark-theme").as_deref(), Some("dark_dimmed"));
    assert_eq!(config(&doc)["themes"]["dark"], "dark_dimmed");

    // Ids that are not vendored fall back to light / dark, attributes included (GH §4.4: the
    // theme file for a slot must be loaded).
    page.theme = theme(Theme::Auto, "light_tritanopia", "dark_high_contrast");
    let doc = Document::from(page.render(&bundle).as_str());
    let hrefs = stylesheet_hrefs(&doc);
    assert_eq!(hrefs[..2], [format!("{B}github/assets/light-1167.css"), format!("{B}github/assets/dark-d4a4.css")]);
    assert_eq!(attr(&doc, "html", "data-light-theme").as_deref(), Some("light"));
    assert_eq!(attr(&doc, "html", "data-dark-theme").as_deref(), Some("dark"));
}

#[test]
fn wrappers_per_mode_and_layout() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Github);

    // readme × bare: no box, same article.
    page.layout = Layout::Bare;
    let doc = Document::from(page.render(&bundle).as_str());
    assert!(!doc.select("#readme").exists());
    assert_eq!(doc.select(".js-snippet-clipboard-copy-unpositioned > article#md-content.markdown-body").length(), 1);

    // comment × readme: GH §3.3 Box with the title header.
    page.mode = Mode::Comment;
    page.layout = Layout::Readme;
    page.file_name = Some("issue <draft>.md");
    let html = page.render(&bundle);
    let doc = Document::from(html.as_str());
    let body = doc.select(
        "body > div.container-lg.px-3.mt-4 > div.Box > div.js-snippet-clipboard-copy-unpositioned > \
         div.comment-body.markdown-body#md-content",
    );
    assert_eq!(body.length(), 1, "{html}");
    assert_eq!(doc.select(".Box > .Box-header > strong").text().as_ref(), "issue <draft>.md");
    assert!(html.contains("issue &lt;draft&gt;.md"));
    assert_eq!(config(&doc)["mode"], "comment");

    // comment × bare: content only.
    page.layout = Layout::Bare;
    let doc = Document::from(page.render(&bundle).as_str());
    assert!(!doc.select(".Box").exists());
    assert_eq!(doc.select(".js-snippet-clipboard-copy-unpositioned > .comment-body#md-content").length(), 1);

    // No file name: no header at all.
    page.mode = Mode::Readme;
    page.layout = Layout::Readme;
    page.file_name = None;
    let doc = Document::from(page.render(&bundle).as_str());
    assert!(doc.select("#readme").exists());
    assert!(!doc.select(".mdr-box-header").exists());
}

#[test]
fn libraries_follow_features() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Github);

    page.content.features = Features { math: true, mermaid: false };
    let cfg = config(&Document::from(page.render(&bundle).as_str()));
    assert_eq!(
        cfg["scripts"],
        serde_json::json!({
            "mathjax": format!("{B}github/lib/mathjax/es5/tex-chtml-full.js"),
            "mathjax-safe": format!("{B}github/lib/mathjax/es5/ui/safe.js"),
        })
    );

    page.content.features = Features { math: false, mermaid: true };
    let cfg = config(&Document::from(page.render(&bundle).as_str()));
    assert_eq!(cfg["scripts"], serde_json::json!({ "mermaid": format!("{B}github/lib/mermaid/mermaid.min.js") }));

    // Libraries are loaded lazily by github.js, never as <script src> in the page.
    page.content.features = Features { math: true, mermaid: true };
    let doc = Document::from(page.render(&bundle).as_str());
    assert_eq!(script_srcs(&doc), ["/__/md-render/static/github.js"]);
}

#[test]
fn live_page() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Github);
    page.live = Some(live_info());
    let doc = Document::from(page.render(&bundle).as_str());

    // live.js runs before the flavor script (it records pristine nodes first, DESIGN §7).
    assert_eq!(script_srcs(&doc), ["/__/md-render/static/live.js", "/__/md-render/static/github.js"]);
    assert_eq!(attr(&doc, "#md-content", "data-version").as_deref(), Some("7"));
    let cfg = config(&doc);
    assert_eq!(
        cfg["live"],
        serde_json::json!({
            "events_url": "/__/md-render/events?path=docs/README.md",
            "render_url": "/__/md-render/render?path=docs/README.md",
            "doc": "docs/README.md",
            "version": 7,
            "server_id": "abc123",
        })
    );
    // A live page may gain math or diagrams later, so every vendored library is listed.
    for key in ["mathjax", "mathjax-safe", "mermaid"] {
        assert!(cfg["scripts"][key].as_str().is_some_and(|u| u.starts_with(B)), "{key}");
    }
}

#[test]
fn banners_are_escaped_and_deduplicated() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Github);
    page.banners = vec![
        Banner { kind: BannerKind::Error, message: "render failed: <script>alert(1)</script>".into() },
        Banner { kind: BannerKind::Info, message: "API rate limit; rendered offline".into() },
    ];
    page.content.warnings = vec!["API rate limit; rendered offline".into(), "unknown emoji".into()];
    let html = page.render(&bundle);
    assert!(!html.contains("<script>alert(1)"));
    let doc = Document::from(html.as_str());
    let banners = doc.select("#mdr-banners > .flash");
    assert_eq!(banners.length(), 3, "warning duplicated by a banner is shown once");
    let classes: Vec<String> = banners.nodes().iter().map(|n| n.attr("class").unwrap_or_default().to_string()).collect();
    assert!(classes[0].contains("flash-error"));
    assert!(!classes[1].contains("flash-error") && !classes[1].contains("flash-warn"));
    assert!(classes[2].contains("flash-warn"));
    assert_eq!(banners.nodes()[0].text().as_ref(), "render failed: <script>alert(1)</script>");
    assert_eq!(attr(&doc, "#mdr-banners > .flash-error", "role").as_deref(), Some("alert"));
    // Banners sit above the content.
    assert!(html.find("id=\"mdr-banners\"").unwrap() < html.find("id=\"md-content\"").unwrap());
}

#[test]
fn untrusted_text_is_escaped() {
    // The fallback bundle keeps the requested theme ids (nothing to fall back to), so they
    // reach the <html> attributes verbatim.
    let bundle = Bundle::fallback();
    let mut page = Page::new(Flavor::Github);
    page.title = "</title><script>x()</script>";
    page.file_name = Some("a\"b<c>.md");
    let mut live = live_info();
    live.doc = "</script><script>alert(1)</script>.md".into();
    page.live = Some(live.clone());
    page.theme = theme(Theme::Auto, "light\"><x", "dark");
    page.extra_css = vec!["/x.css?a=1&b=\"2\"".into()];
    let html = page.render(&bundle);
    assert!(!html.contains("<script>x()"));
    assert!(!html.contains("</script><script>alert"));
    let doc = Document::from(html.as_str());
    assert_eq!(doc.select("title").text().as_ref(), "</title><script>x()</script>");
    assert_eq!(doc.select(".mdr-box-title").text().as_ref(), "a\"b<c>.md");
    assert_eq!(config(&doc)["live"]["doc"], live.doc);
    assert_eq!(attr(&doc, "html", "data-light-theme").as_deref(), Some("light\"><x"));
    assert_eq!(stylesheet_hrefs(&doc).last().map(String::as_str), Some("/x.css?a=1&b=\"2\""));
    // The JSON block never contains a raw '<'.
    let raw = html.split("id=\"mdr-config\" type=\"application/json\">").nth(1).unwrap();
    let raw = &raw[..raw.find("</script>").unwrap()];
    assert!(!raw.contains('<'), "{raw}");
}

#[test]
fn fallback_styles() {
    // A fallback bundle links its own fallback sheet through the bundle route.
    let fallback = Bundle::fallback();
    let page = Page::new(Flavor::Github);
    let doc = Document::from(page.render(&fallback).as_str());
    assert_eq!(stylesheet_hrefs(&doc), [format!("{B}github/fallback.css")]);
    assert_eq!(attr(&doc, "html", "data-color-mode").as_deref(), Some("auto"));

    // A bundle without the flavor at all: the embedded static fallback.
    let dir = tempfile::tempdir().unwrap();
    let gitlab_only = gitlab(BundleBuilder::new()).open(dir.path());
    let doc = Document::from(page.render(&gitlab_only).as_str());
    assert_eq!(stylesheet_hrefs(&doc), ["/__/md-render/static/fallback-github.css"]);

    // A flavor entry without any stylesheet: also the embedded fallback.
    let dir = tempfile::tempdir().unwrap();
    let empty = BundleBuilder::new().empty_flavor(Flavor::Github).open(dir.path());
    let doc = Document::from(page.render(&empty).as_str());
    assert_eq!(stylesheet_hrefs(&doc), ["/__/md-render/static/fallback-github.css"]);
    assert!(config(&doc)["scripts"].as_object().is_some_and(|m| m.is_empty()));
}

#[test]
fn custom_prefix() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Github);
    page.assets = md_render::page::AssetMode::Server { prefix: "/mdr/".into() };
    let doc = Document::from(page.render(&bundle).as_str());
    assert!(stylesheet_hrefs(&doc)[0].starts_with("/mdr/bundle/github/assets/"));
    assert_eq!(script_srcs(&doc), ["/mdr/static/github.js"]);
}

#[test]
fn listing_fragment() {
    let entries = vec![
        ListingEntry { name: "docs".into(), href: "/docs/".into(), is_dir: true, size: None },
        ListingEntry { name: "a <b>.md".into(), href: "/a%20%3Cb%3E.md".into(), is_dir: false, size: Some(2048) },
        ListingEntry { name: "q\"uote".into(), href: "/q\"uote".into(), is_dir: false, size: Some(5) },
    ];
    let html = listing_html(Flavor::Github, "sub/dir", &entries);
    let doc = Document::fragment(html.as_str());
    let rows = doc.select(".Box.mdr-listing > .Box-row");
    assert_eq!(rows.length(), 3);
    assert_eq!(doc.select(".Box-row svg.octicon-file-directory-fill").length(), 1);
    assert_eq!(doc.select(".Box-row svg.octicon-file").length(), 2);
    let links: Vec<(String, String)> = doc
        .select(".Box-row a")
        .nodes()
        .iter()
        .map(|n| (n.attr("href").unwrap_or_default().to_string(), n.text().to_string()))
        .collect();
    assert_eq!(
        links,
        [
            ("/docs/".into(), "docs".into()),
            ("/a%20%3Cb%3E.md".into(), "a <b>.md".into()),
            ("/q\"uote".into(), "q\"uote".into())
        ]
    );
    assert!(doc.select(".Box-row").nodes()[1].text().contains("2.0 KB"));
    // Breadcrumbs link every ancestor.
    let crumbs: Vec<String> =
        doc.select(".Box-header a").nodes().iter().map(|n| n.attr("href").unwrap_or_default().to_string()).collect();
    assert_eq!(crumbs, ["/", "/sub/", "/sub/dir/"]);

    let empty = listing_html(Flavor::Github, "", &[]);
    assert!(empty.contains("This directory is empty."));
}

#[test]
fn error_fragment() {
    let html = error_html(Flavor::Github, 404, "No such file: <x>.md");
    let doc = Document::fragment(html.as_str());
    let flash = doc.select(".flash.flash-error[role=alert]");
    assert_eq!(flash.length(), 1);
    assert!(flash.text().contains("404 Not Found"));
    assert!(flash.text().contains("No such file: <x>.md"));
    assert!(html.contains("&lt;x&gt;"));
    assert!(error_html(Flavor::Github, 599, "x").contains("599 Error"));
}
