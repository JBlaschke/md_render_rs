//! GitLab page chrome: `<html>` classes and colour-mode head bits (GL §11.3), stylesheet order
//! and media per theme mode, the blob template (GL §12), the note wrapper (DESIGN D16),
//! client configuration, banners, listing and error fragments.

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
const LIGHT: &str = "(prefers-color-scheme: light)";
const DARK: &str = "(prefers-color-scheme: dark)";

/// `(href, media)` of the bundle stylesheet links.
fn sheets(doc: &Document) -> Vec<(String, Option<String>)> {
    doc.select("link[rel=stylesheet][data-mdr-style=bundle]")
        .nodes()
        .iter()
        .map(|n| {
            (
                n.attr("href").unwrap_or_default().to_string(),
                n.attr("media").map(|m| m.to_string()),
            )
        })
        .collect()
}

#[test]
fn system_mode_skeleton() {
    let (_dir, bundle) = bundle();
    let page = Page::new(Flavor::Gitlab);
    let html = page.render(&bundle);
    let doc = Document::from(html.as_str());

    let root = doc.select("html");
    assert_eq!(root.attr("class").as_deref(), Some("gl-system ui-neutral"));
    assert_eq!(root.attr("lang").as_deref(), Some("en"));
    assert_eq!(
        root.attr("data-icon-sprite").as_deref(),
        Some("/__/md-render/bundle/gitlab/assets/icons-0123456789abcdef.svg")
    );
    assert_eq!(
        attr(&doc, "meta[name=color-scheme]", "content").as_deref(),
        Some("light dark")
    );
    // The gl-dark bootstrap runs in <head> before the stylesheets (GL §11.3).
    let head = doc.select("head").inner_html().to_string();
    let script = head
        .find("classList.add('gl-dark')")
        .expect("gl-dark bootstrap");
    assert!(script < head.find("rel=\"stylesheet\"").unwrap());

    assert_eq!(
        sheets(&doc),
        [
            (
                format!("{B}gitlab/assets/application-1111.css"),
                Some(LIGHT.into())
            ),
            (
                format!("{B}gitlab/assets/application_dark-2222.css"),
                Some(DARK.into())
            ),
            (
                format!("{B}gitlab/assets/tailwind-3333.css"),
                Some("all".into())
            ),
            (
                format!("{B}gitlab/assets/fonts-4444.css"),
                Some("all".into())
            ),
            (
                format!("{B}gitlab/assets/highlight/themes/white-6666.css"),
                Some(LIGHT.into())
            ),
            (
                format!("{B}gitlab/assets/highlight/themes/dark-5555.css"),
                Some(DARK.into())
            ),
        ]
    );

    assert_eq!(attr(&doc, "body", "class").as_deref(), Some("tab-width-8"));
    let content = doc.select(
        "body > div.container-fluid.container-limited.gl-pt-5 > #fileHolder.file-holder > \
         div.blob-viewer[data-type=rich][data-loaded=true] > \
         div.blob-viewer[data-path=\"README.md\"][data-rich-type=markup] > \
         div#md-content.file-content.js-markup-content.md",
    );
    assert_eq!(content.length(), 1, "{html}");
    assert_eq!(
        doc.select("#fileHolder > .js-file-title.file-title-flex-parent strong")
            .text()
            .as_ref(),
        "README.md"
    );
    assert!(content.select("copy-code").exists());

    assert_eq!(script_srcs(&doc), ["/__/md-render/static/gitlab.js"]);
    assert!(
        !html.contains("snippet-clipboard-copy-button"),
        "no GitHub template on GitLab pages"
    );
    let cfg = config(&doc);
    assert_eq!(cfg["flavor"], "gitlab");
    assert_eq!(cfg["theme"], "auto");
    assert!(cfg["live"].is_null());
}

#[test]
fn light_and_dark_modes() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Gitlab);

    page.theme = theme(Theme::Light, "light", "dark");
    let doc = Document::from(page.render(&bundle).as_str());
    assert_eq!(
        attr(&doc, "html", "class").as_deref(),
        Some("gl-light ui-neutral")
    );
    assert!(!doc.select("meta[name=color-scheme]").exists());
    assert!(
        !doc.select("head")
            .inner_html()
            .contains("classList.add('gl-dark')")
    );
    assert_eq!(
        sheets(&doc),
        [
            (format!("{B}gitlab/assets/application-1111.css"), None),
            (
                format!("{B}gitlab/assets/tailwind-3333.css"),
                Some("all".into())
            ),
            (
                format!("{B}gitlab/assets/fonts-4444.css"),
                Some("all".into())
            ),
            (
                format!("{B}gitlab/assets/highlight/themes/white-6666.css"),
                None
            ),
        ]
    );

    page.theme = theme(Theme::Dark, "light", "dark");
    let doc = Document::from(page.render(&bundle).as_str());
    assert_eq!(
        attr(&doc, "html", "class").as_deref(),
        Some("gl-dark ui-neutral")
    );
    assert_eq!(
        attr(&doc, "meta[name=color-scheme]", "content").as_deref(),
        Some("dark light")
    );
    assert_eq!(
        sheets(&doc),
        [
            (format!("{B}gitlab/assets/application_dark-2222.css"), None),
            (
                format!("{B}gitlab/assets/tailwind-3333.css"),
                Some("all".into())
            ),
            (
                format!("{B}gitlab/assets/fonts-4444.css"),
                Some("all".into())
            ),
            (
                format!("{B}gitlab/assets/highlight/themes/dark-5555.css"),
                None
            ),
        ]
    );
}

#[test]
fn wrappers_per_mode_and_layout() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Gitlab);

    page.layout = Layout::Bare;
    let doc = Document::from(page.render(&bundle).as_str());
    assert!(!doc.select("#fileHolder").exists());
    assert_eq!(
        doc.select(".container-limited > #md-content.file-content.md")
            .length(),
        1
    );

    page.mode = Mode::Comment;
    page.layout = Layout::Readme;
    page.file_name = Some("note.md");
    let doc = Document::from(page.render(&bundle).as_str());
    assert_eq!(
        doc.select(".mdr-note > .note-body > #md-content.note-text.md")
            .length(),
        1
    );
    assert_eq!(
        doc.select(".mdr-note > .mdr-note-header strong")
            .text()
            .as_ref(),
        "note.md"
    );

    page.layout = Layout::Bare;
    let doc = Document::from(page.render(&bundle).as_str());
    assert!(!doc.select(".mdr-note").exists());
    assert_eq!(doc.select("#md-content.note-text.md").length(), 1);
}

#[test]
fn libraries_follow_features() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Gitlab);

    page.content.features = Features {
        math: true,
        mermaid: false,
    };
    let doc = Document::from(page.render(&bundle).as_str());
    assert_eq!(
        config(&doc)["scripts"],
        serde_json::json!({
            "katex": format!("{B}gitlab/lib/katex/katex.min.js"),
            "katex-css": format!("{B}gitlab/lib/katex/katex.min.css"),
        })
    );
    // KaTeX CSS is linked up front when the page has math (GL §12), after the site CSS.
    let lib = doc.select("link[data-mdr-lib=katex-css]");
    assert_eq!(
        lib.attr("href").as_deref(),
        Some(format!("{B}gitlab/lib/katex/katex.min.css").as_str())
    );
    let hrefs = stylesheet_hrefs(&doc);
    assert!(
        hrefs.iter().position(|h| h.ends_with("katex.min.css"))
            > hrefs.iter().position(|h| h.ends_with("dark-5555.css"))
    );

    page.content.features = Features {
        math: false,
        mermaid: true,
    };
    let doc = Document::from(page.render(&bundle).as_str());
    assert_eq!(
        config(&doc)["scripts"],
        serde_json::json!({ "mermaid": format!("{B}gitlab/lib/mermaid/mermaid.min.js") })
    );
    assert!(!doc.select("link[data-mdr-lib]").exists());

    // Live: every library is listed for lazy loading, the KaTeX CSS only when needed.
    page.content.features = Features::default();
    page.live = Some(live_info());
    let doc = Document::from(page.render(&bundle).as_str());
    let cfg = config(&doc);
    for key in ["katex", "katex-css", "mermaid"] {
        assert!(cfg["scripts"][key].is_string(), "{key}");
    }
    assert!(!doc.select("link[data-mdr-lib]").exists());
    assert_eq!(
        script_srcs(&doc),
        [
            "/__/md-render/static/live.js",
            "/__/md-render/static/gitlab.js"
        ]
    );
    assert_eq!(
        attr(&doc, "#md-content", "data-version").as_deref(),
        Some("7")
    );
}

#[test]
fn banners_use_gitlab_alerts() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Gitlab);
    page.banners = vec![Banner {
        kind: BannerKind::Error,
        message: "read error: <denied>".into(),
    }];
    page.content.warnings = vec!["include outside the root".into()];
    let html = page.render(&bundle);
    let doc = Document::from(html.as_str());
    let alerts = doc.select("#mdr-banners > .gl-alert");
    assert_eq!(alerts.length(), 2);
    assert!(alerts.nodes()[0].has_class("gl-alert-danger"));
    assert!(alerts.nodes()[1].has_class("gl-alert-warning"));
    assert_eq!(
        doc.select("#mdr-banners .gl-alert-danger .gl-alert-body")
            .text()
            .as_ref(),
        "read error: <denied>"
    );
    assert!(html.contains("read error: &lt;denied&gt;"));
}

#[test]
fn fallback_styles() {
    let page = Page::new(Flavor::Gitlab);
    let doc = Document::from(page.render(&Bundle::fallback()).as_str());
    assert_eq!(stylesheet_hrefs(&doc), [format!("{B}gitlab/fallback.css")]);
    assert_eq!(
        attr(&doc, "html", "class").as_deref(),
        Some("gl-system ui-neutral")
    );
    assert!(attr(&doc, "html", "data-icon-sprite").is_none());

    let dir = tempfile::tempdir().unwrap();
    let github_only = github(BundleBuilder::new()).open(dir.path());
    let doc = Document::from(page.render(&github_only).as_str());
    assert_eq!(
        stylesheet_hrefs(&doc),
        ["/__/md-render/static/fallback-gitlab.css"]
    );
}

/// PAGE-3: a bundle whose GitLab flavor has no dark application/highlight CSS (vendoring treats
/// both as optional) links the light sheets in every colour mode, without the light media
/// query, and renders as `gl-light`: no gl-dark bootstrap, so light CSS never meets dark tokens.
#[test]
fn bundle_without_dark_css_renders_light() {
    let dir = tempfile::tempdir().unwrap();
    let light_only = BundleBuilder::new()
        .stylesheet(
            Flavor::Gitlab,
            "gitlab/assets/application-1111.css",
            "app",
            Some("light"),
            ".md{color:#333}",
        )
        .stylesheet(
            Flavor::Gitlab,
            "gitlab/assets/tailwind-3333.css",
            "tailwind",
            None,
            ".gl-sr-only{position:absolute}",
        )
        .stylesheet(
            Flavor::Gitlab,
            "gitlab/assets/highlight/themes/white-6666.css",
            "highlight",
            Some("light"),
            ".code.white{color:#3a383d}",
        )
        .open(dir.path());
    for mode in [Theme::Auto, Theme::Dark, Theme::Light] {
        let mut page = Page::new(Flavor::Gitlab);
        page.theme = theme(mode, "light", "dark");
        let html = page.render(&light_only);
        let doc = Document::from(html.as_str());
        assert_eq!(
            sheets(&doc),
            [
                (format!("{B}gitlab/assets/application-1111.css"), None),
                (
                    format!("{B}gitlab/assets/tailwind-3333.css"),
                    Some("all".into())
                ),
                (
                    format!("{B}gitlab/assets/highlight/themes/white-6666.css"),
                    None
                ),
            ],
            "{mode:?}"
        );
        assert_eq!(
            attr(&doc, "html", "class").as_deref(),
            Some("gl-light ui-neutral"),
            "{mode:?}"
        );
        assert!(!html.contains("gl-dark"), "{mode:?}: no gl-dark anywhere");
        assert_eq!(doc.select("meta[name=color-scheme]").length(), 0);
        assert_eq!(config(&doc)["theme"], "light");
    }
}

#[test]
fn listing_fragment() {
    let entries = vec![
        ListingEntry {
            name: "src".into(),
            href: "/src/".into(),
            is_dir: true,
            size: None,
        },
        ListingEntry {
            name: "<README>.md".into(),
            href: "/%3CREADME%3E.md".into(),
            is_dir: false,
            size: Some(10),
        },
    ];
    let html = listing_html(Flavor::Gitlab, "/", &entries);
    let doc = Document::fragment(html.as_str());
    let rows = doc.select(".mdr-listing > .tree-item");
    assert_eq!(rows.length(), 2);
    assert!(
        doc.select(".tree-item:nth-child(2) svg.folder-icon")
            .exists()
    );
    assert!(doc.select(".tree-item:nth-child(3) svg.file-icon").exists());
    assert_eq!(
        doc.select(".tree-item:nth-child(3) a.tree-item-link")
            .text()
            .as_ref(),
        "<README>.md"
    );
    assert!(rows.nodes()[1].text().contains("10 B"));
    assert!(html.contains("&lt;README&gt;.md"));
    assert!(listing_html(Flavor::Gitlab, "x", &[]).contains("This directory is empty."));
}

#[test]
fn error_fragment() {
    let html = error_html(Flavor::Gitlab, 403, "outside the served root");
    let doc = Document::fragment(html.as_str());
    let alert = doc.select(".gl-alert.gl-alert-danger[role=alert]");
    assert_eq!(alert.length(), 1);
    assert_eq!(
        alert.select(".gl-alert-title").text().trim(),
        "403 Forbidden"
    );
    assert_eq!(
        alert.select(".gl-alert-body").text().as_ref(),
        "outside the served root"
    );
}

/// PAGE-2: a self-contained GitLab export carries GitLab's own copy-to-clipboard icon (served
/// and `--no-inline` pages reference the vendored sprite) instead of falling back to the
/// Octicon; a sprite without that symbol keeps the fallback.
#[test]
fn inline_export_carries_the_copy_icon() {
    let dir = tempfile::tempdir().unwrap();
    let with_sprite = BundleBuilder::new()
        .stylesheet(
            Flavor::Gitlab,
            "gitlab/assets/application-1.css",
            "app",
            Some("light"),
            ".md{}",
        )
        .asset(
            Flavor::Gitlab,
            "gitlab/assets/icons-0123abcd.svg",
            GITLAB_SPRITE,
        )
        .open(dir.path());
    let mut page = Page::new(Flavor::Gitlab);
    page.assets = md_render::page::AssetMode::Inline;
    let html = page.render(&with_sprite);
    assert_self_contained(&html);
    let doc = Document::from(html.as_str());
    assert_eq!(attr(&doc, "html", "data-icon-sprite").as_deref(), Some(""));
    let symbol = doc.select("body > svg[aria-hidden] > symbol");
    assert_eq!(symbol.length(), 1, "only the copy icon is carried");
    assert_eq!(symbol.attr("id").as_deref(), Some("copy-to-clipboard"));
    assert!(html.contains("<symbol id=\"copy-to-clipboard\" viewBox=\"0 0 16 16\">"));
    assert!(html.contains(
        "d=\"M4 9.5H2.5v-7h7V4H11V2a1 1 0 0 0-1-1H2a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h2V9.5z\""
    ));
    assert!(!html.contains("id=\"abuse\""));

    // Served pages reference the sprite and carry no icons.
    page.assets = server();
    let html = page.render(&with_sprite);
    let doc = Document::from(html.as_str());
    assert_eq!(
        attr(&doc, "html", "data-icon-sprite").as_deref(),
        Some("/__/md-render/bundle/gitlab/assets/icons-0123abcd.svg")
    );
    assert!(!html.contains("<symbol"));

    // The test bundle's sprite has no such symbol: no attribute, so gitlab.js uses its fallback.
    let (_dir, full) = bundle();
    page.assets = md_render::page::AssetMode::Inline;
    let html = page.render(&full);
    let doc = Document::from(html.as_str());
    assert_eq!(attr(&doc, "html", "data-icon-sprite"), None);
    assert!(!html.contains("<symbol"));
}
