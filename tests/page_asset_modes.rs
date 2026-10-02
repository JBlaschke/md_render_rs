//! `AssetMode::Inline` (self-contained exports: CSS inlined with data: URIs, libraries and
//! client JS inlined) and `AssetMode::Directory` (files copied next to the export, linked
//! relatively), DESIGN §3.6.

mod page_support;

use std::path::Path;

use base64::Engine as _;
use dom_query::Document;
use md_render::page::AssetMode;
use md_render::render::Features;
use md_render::styles::Bundle;
use md_render::{Flavor, Theme};
use page_support::*;

fn bundle() -> (tempfile::TempDir, Bundle) {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = full_bundle(dir.path());
    (dir, bundle)
}

fn decode_data_uri(uri: &str) -> Vec<u8> {
    let (_, b64) = uri
        .split_once(";base64,")
        .unwrap_or_else(|| panic!("base64 data URI: {uri}"));
    let b64 = b64.split('#').next().unwrap_or(b64);
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("valid base64")
}

/// Text of the inlined stylesheet for bundle path `path`.
fn inlined_css(doc: &Document, path: &str) -> String {
    let sel = doc.select(&format!("style[data-mdr-path=\"{path}\"]"));
    assert_eq!(sel.length(), 1, "inlined {path}");
    sel.text().to_string()
}

#[test]
fn github_inline_is_self_contained() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Github);
    page.assets = AssetMode::Inline;
    page.live = Some(live_info()); // ignored outside server mode
    page.content.features = Features {
        math: true,
        mermaid: true,
    };
    let html = page.render(&bundle);
    assert_self_contained(&html);
    let doc = Document::from(html.as_str());

    // Stylesheets inlined in cascade order, marked like the links of a served page.
    let order: Vec<String> = doc
        .select("style[data-mdr-path]")
        .nodes()
        .iter()
        .filter_map(|n| n.attr("data-mdr-path"))
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        order,
        [
            "github/assets/light-1167.css",
            "github/assets/dark-d4a4.css",
            "github/assets/primer-primitives-0001.css",
            "github/assets/primer-0002.css",
            "github/assets/global-0003.css",
            "github/assets/github-0004.css",
            "github/assets/repository-0005.css",
            "github/assets/code-c0de.css",
        ]
    );
    assert_eq!(doc.select("style[data-mdr-style=bundle]").length(), 8);

    // url() rewriting, resolved against each file's bundle path.
    let global = inlined_css(&doc, "github/assets/global-0003.css");
    let urls = css_urls(&global);
    assert_eq!(urls.len(), 3, "{global}");
    assert!(urls[0].starts_with("data:image/gif;base64,"));
    assert_eq!(decode_data_uri(&urls[0]), GIF);
    assert_eq!(
        urls[1], "data:,",
        "root-relative URL outside the bundle is dropped"
    );
    assert_eq!(urls[2], "data:,", "external URL is dropped");
    let primer = inlined_css(&doc, "github/assets/primer-0002.css");
    assert_eq!(
        css_urls(&primer),
        ["data:image/svg+xml;base64,PHN2Zy8+"],
        "data: URIs are untouched"
    );
    let promo = inlined_css(&doc, "github/assets/github-0004.css");
    assert_eq!(
        css_urls(&promo),
        ["data:,"],
        "large site images are not inlined"
    );
    // @import is followed and inlined as CSS, with its own references resolved; only the
    // value changes (the import keeps its string form).
    let repo = inlined_css(&doc, "github/assets/repository-0005.css");
    let import = repo.split("@import \"").nth(1).expect("@import rewritten");
    let import = &import[..import.find('"').unwrap()];
    assert!(import.starts_with("data:text/css;base64,"));
    let imported = String::from_utf8(decode_data_uri(import)).unwrap();
    let nested = css_urls(&imported);
    assert_eq!(nested.len(), 1);
    assert!(
        nested[0].starts_with("data:image/svg+xml;base64,") && nested[0].ends_with("#frag"),
        "{}",
        nested[0]
    );

    // Content: no data-sourcepos, custom emoji inlined, the author's links untouched.
    assert!(!html.contains("data-sourcepos"));
    let emoji = attr(&doc, "img.emoji", "src").unwrap();
    assert!(emoji.starts_with("data:image/png;base64,"));
    assert_eq!(decode_data_uri(&emoji), PNG);
    assert_eq!(
        attr(&doc, "#md-content a[href=\"docs/x.md\"]", "href").as_deref(),
        Some("docs/x.md")
    );

    // Client JS inlined; no live client; libraries as inert holders referenced from mdr-config.
    assert_eq!(
        doc.select("script[data-mdr-static=\"github.js\"]").length(),
        1
    );
    assert!(!html.contains("data-mdr-static=\"live.js\""));
    let cfg = config(&doc);
    assert!(cfg["live"].is_null());
    assert_eq!(
        cfg["scripts"],
        serde_json::json!({ "mathjax": "#mdr-lib-mathjax", "mathjax-safe": "#mdr-lib-mathjax-safe", "mermaid": "#mdr-lib-mermaid" })
    );
    for key in ["mathjax", "mathjax-safe", "mermaid"] {
        assert_eq!(
            attr(&doc, &format!("script#mdr-lib-{key}"), "type").as_deref(),
            Some("text/plain"),
            "{key}"
        );
    }
    // Inlined script text cannot end its <script> element early.
    let mathjax = doc.select("script#mdr-lib-mathjax").text().to_string();
    assert!(
        mathjax.contains("window.MJ = '\\x3C/script>\\x3C!--';"),
        "{mathjax}"
    );
    assert_eq!(
        attr(&doc, "link[rel=icon]", "href").map(|h| h.starts_with("data:image/svg+xml;base64,")),
        Some(true)
    );
}

#[test]
fn github_inline_without_features_has_no_libraries() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Github);
    page.assets = AssetMode::Inline;
    let html = page.render(&bundle);
    assert_self_contained(&html);
    let doc = Document::from(html.as_str());
    assert!(!doc.select("script[type=\"text/plain\"]").exists());
    assert_eq!(config(&doc)["scripts"], serde_json::json!({}));
    assert!(
        !html.contains("window.MJ") && !html.contains("window.mermaid ="),
        "library code not inlined"
    );
}

#[test]
fn gitlab_inline_is_self_contained() {
    let (_dir, bundle) = bundle();
    let mut page = Page::new(Flavor::Gitlab);
    page.assets = AssetMode::Inline;
    page.content.features = Features {
        math: true,
        mermaid: false,
    };
    let html = page.render(&bundle);
    assert_self_contained(&html);
    let doc = Document::from(html.as_str());

    // Theme media survive inlining (auto mode, GL §11.3).
    assert_eq!(
        attr(
            &doc,
            "style[data-mdr-path=\"gitlab/assets/application_dark-2222.css\"]",
            "media"
        )
        .as_deref(),
        Some("(prefers-color-scheme: dark)")
    );
    let fonts = inlined_css(&doc, "gitlab/assets/fonts-4444.css");
    let urls = css_urls(&fonts);
    assert_eq!(urls.len(), 1);
    assert!(
        urls[0].starts_with("data:font/woff2;base64,"),
        "{}",
        urls[0]
    );
    assert_eq!(decode_data_uri(&urls[0]), WOFF2);
    let app = inlined_css(&doc, "gitlab/assets/application-1111.css");
    assert!(css_urls(&app)[0].starts_with("data:image/svg+xml;base64,"));

    // KaTeX CSS inlined, woff2 kept, legacy twins dropped.
    let katex = doc.select("style#mdr-lib-katex-css").text().to_string();
    let urls = css_urls(&katex);
    assert_eq!(urls.len(), 3, "{katex}");
    assert!(urls[0].starts_with("data:font/woff2;base64,"));
    assert_eq!(&urls[1..], ["data:,", "data:,"]);
    assert_eq!(
        config(&doc)["scripts"],
        serde_json::json!({ "katex": "#mdr-lib-katex", "katex-css": "#mdr-lib-katex-css" })
    );
    // The icon sprite cannot be used from a data: URI; gitlab.js draws its own icon.
    assert!(attr(&doc, "html", "data-icon-sprite").is_none());
    assert_eq!(
        doc.select("script[data-mdr-static=\"gitlab.js\"]").length(),
        1
    );
    // The system-mode bootstrap stays inline in <head>.
    assert!(
        doc.select("head")
            .inner_html()
            .contains("classList.add('gl-dark')")
    );
}

#[test]
fn inline_fallback_styles() {
    for flavor in [Flavor::Github, Flavor::Gitlab] {
        let mut page = Page::new(flavor);
        page.assets = AssetMode::Inline;
        page.theme = theme(Theme::Dark, "light", "dark");
        let html = page.render(&Bundle::fallback());
        assert_self_contained(&html);
        let doc = Document::from(html.as_str());
        let css = inlined_css(&doc, &format!("{flavor}/fallback.css"));
        assert!(css.contains("prefers-color-scheme: dark"), "{flavor}");
    }
}

/// Every `href`/`src` in `html` that is relative resolves to an existing file below `base`.
fn assert_relative_refs_exist(html: &str, base: &Path, prefix: &str) -> usize {
    let doc = Document::from(html);
    let mut n = 0;
    for (sel, name) in [
        ("link[href]", "href"),
        ("script[src]", "src"),
        ("img[src]", "src"),
    ] {
        for node in doc.select(sel).nodes() {
            let value = node.attr(name).unwrap_or_default().to_string();
            if !value.starts_with(prefix) {
                continue;
            }
            let decoded = percent_encoding::percent_decode_str(&value)
                .decode_utf8_lossy()
                .into_owned();
            assert!(
                base.join(&decoded).is_file(),
                "{value} -> {}",
                base.join(&decoded).display()
            );
            n += 1;
        }
    }
    n
}

#[test]
fn directory_mode_copies_and_links_relatively() {
    let (_dir, bundle) = bundle();
    let out = tempfile::tempdir().unwrap();
    let files = out.path().join("my doc_files");
    let mut page = Page::new(Flavor::Github);
    page.assets = AssetMode::Directory {
        href: "my%20doc_files".into(),
        dir: files.clone(),
    };
    page.content.features = Features {
        math: true,
        mermaid: true,
    };
    let html = page.render(&bundle);
    let doc = Document::from(html.as_str());

    let hrefs = stylesheet_hrefs(&doc);
    assert_eq!(hrefs.len(), 8);
    assert_eq!(hrefs[0], "my%20doc_files/github/assets/light-1167.css");
    assert_eq!(
        script_srcs(&doc),
        ["my%20doc_files/static/github.js"],
        "libraries load lazily; the live client is never exported"
    );
    let linked = assert_relative_refs_exist(&html, out.path(), "my%20doc_files/");
    assert!(
        linked >= 11,
        "stylesheets, favicon, github.js, emoji: {linked}"
    );

    // Bundle paths are preserved, so the CSS's relative url()s keep working; their targets
    // (including @import chains) are copied as well.
    for rel in [
        "github/images/spinners/octocat-spinner-16px.gif",
        "github/assets/repo-extra.css",
        "github/images/x.svg",
        "github/emoji/octocat.png",
        "github/lib/mathjax/es5/tex-chtml-full.js",
        "github/lib/mathjax/es5/ui/safe.js",
        "github/lib/mermaid/mermaid.min.js",
        "static/favicon.svg",
    ] {
        assert!(files.join(rel).is_file(), "{rel} copied");
    }
    assert!(
        !files.join("github/images/modules/promo.png").exists(),
        "large site images are not copied"
    );
    let global = std::fs::read_to_string(files.join("github/assets/global-0003.css")).unwrap();
    assert!(
        global.contains("url(../images/spinners/octocat-spinner-16px.gif)"),
        "copied verbatim"
    );

    let cfg = config(&doc);
    assert_eq!(
        cfg["scripts"]["mermaid"],
        "my%20doc_files/github/lib/mermaid/mermaid.min.js"
    );
    assert_eq!(
        attr(&doc, "img.emoji", "src").as_deref(),
        Some("my%20doc_files/github/emoji/octocat.png")
    );
    assert!(!html.contains("data-sourcepos"));
    assert!(!html.contains("/__/md-render/"));
}

#[test]
fn directory_mode_gitlab_and_rendered_with_export_bundle_url() {
    let (_dir, bundle) = bundle();
    let out = tempfile::tempdir().unwrap();
    let files = out.path().join("x_files");
    let mut page = Page::new(Flavor::Gitlab);
    page.assets = AssetMode::Directory {
        href: "x_files".into(),
        dir: files.clone(),
    };
    page.content.features = Features {
        math: true,
        mermaid: false,
    };
    // Content rendered with `bundle_url = href` (what export does) points into the directory.
    page.content
        .html
        .push_str("<p><img src=\"x_files/gitlab/assets/icon_anchor-abc.svg\" alt=\"a\"></p>");
    let html = page.render(&bundle);
    let doc = Document::from(html.as_str());
    assert_relative_refs_exist(&html, out.path(), "x_files/");
    assert!(
        files
            .join("gitlab/assets/gitlab-sans/GitLabSans-abc.woff2")
            .is_file()
    );
    assert!(
        files
            .join("gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff2")
            .is_file()
    );
    assert!(files.join("gitlab/assets/icon_anchor-abc.svg").is_file());
    assert_eq!(
        attr(&doc, "link[data-mdr-lib=katex-css]", "href").as_deref(),
        Some("x_files/gitlab/lib/katex/katex.min.css")
    );
    assert_eq!(
        attr(&doc, "html", "data-icon-sprite").as_deref(),
        Some("x_files/gitlab/assets/icons-0123456789abcdef.svg")
    );
    assert!(
        files
            .join("gitlab/assets/icons-0123456789abcdef.svg")
            .is_file()
    );
}

#[test]
fn directory_mode_reports_io_errors() {
    let (_dir, bundle) = bundle();
    let out = tempfile::tempdir().unwrap();
    // A regular file where the assets directory should go.
    let blocker = out.path().join("blocked_files");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let mut page = Page::new(Flavor::Github);
    page.assets = AssetMode::Directory {
        href: "blocked_files".into(),
        dir: blocker,
    };
    let input = md_render::page::PageInput {
        flavor: page.flavor,
        mode: page.mode,
        theme: &page.theme,
        layout: page.layout,
        title: page.title,
        file_name: page.file_name,
        content: &page.content,
        assets: &page.assets,
        live: None,
        banners: &[],
        extra_css: &[],
    };
    let err = md_render::page::render_page(&input, &bundle).expect_err("cannot write below a file");
    assert!(format!("{err:#}").contains("blocked_files"), "{err:#}");
}

/// EXP-2: raster images referenced from site CSS above 16 KiB (GitHub's dashboard promos and
/// banners, GitLab's blooms: UI chrome that markdown never shows) stay out of exports, however
/// often they are referenced; small ones (spinners, icons) and document images are kept.
#[test]
fn large_site_images_stay_out_of_exports() {
    let dir = tempfile::tempdir().unwrap();
    let promo = vec![7u8; 60 * 1024];
    let icon = vec![9u8; 4 * 1024];
    let emoji = vec![5u8; 40 * 1024]; // like :fishsticks: (41 KB)
    let bundle = BundleBuilder::new()
        .stylesheet(
            Flavor::Github,
            "github/assets/github-1.css",
            "github",
            None,
            ".a{background:url(../images/promo.png)}.b{background:url(\"../images/promo.png\")}\
             .c{background-image:url('../images/promo.png')}.d{background:url(../images/icon.png)}",
        )
        .asset(Flavor::Github, "github/images/promo.png", &promo)
        .asset(Flavor::Github, "github/images/icon.png", &icon)
        .asset(Flavor::Github, "github/emoji/octocat.png", &emoji)
        .open(dir.path());
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);

    let mut page = Page::new(Flavor::Github);
    page.assets = AssetMode::Inline;
    let html = page.render(&bundle);
    assert_self_contained(&html);
    assert!(
        !html.contains(&b64(&promo)[..64]),
        "the promo is not inlined"
    );
    let doc = Document::from(html.as_str());
    let css = inlined_css(&doc, "github/assets/github-1.css");
    let urls = css_urls(&css);
    assert_eq!(urls[..3], ["data:,", "data:,", "data:,"], "{css}");
    assert_eq!(decode_data_uri(&urls[3]), icon, "small images are inlined");
    let emoji_src = attr(&doc, "img.emoji", "src").unwrap();
    assert_eq!(
        decode_data_uri(&emoji_src),
        emoji,
        "document images are never capped"
    );
    assert!(html.len() < 100 * 1024, "{} bytes", html.len());

    let out = tempfile::tempdir().unwrap();
    page.assets = AssetMode::Directory {
        href: "x_files".into(),
        dir: out.path().join("x_files"),
    };
    page.render(&bundle);
    let files = out.path().join("x_files/github");
    assert!(
        !files.join("images/promo.png").exists(),
        "the promo is not copied"
    );
    assert!(files.join("images/icon.png").is_file());
    assert!(files.join("emoji/octocat.png").is_file());
}

/// EXP-3 / Q-DUP-CSS: exports find references with the vendoring tokenizer, so `url(…)` text
/// inside strings and comments is left alone (rewriting it closed the string early and broke
/// the rule) and escaped characters stay part of the URL, in both export modes.
#[test]
fn css_strings_comments_and_escapes_survive_exports() {
    let dir = tempfile::tempdir().unwrap();
    let css = r#"a::after{content:"url(x.png)"}/* old: url(y.png) */b{background:url(a\)b.png)}c{background:url("q\"r.gif")}"#;
    let bundle = BundleBuilder::new()
        .stylesheet(
            Flavor::Github,
            "github/assets/github-1.css",
            "github",
            None,
            css,
        )
        .asset(Flavor::Github, "github/assets/x.png", GIF)
        .asset(Flavor::Github, "github/assets/y.png", GIF)
        .asset(Flavor::Github, "github/assets/a)b.png", PNG)
        .asset(Flavor::Github, "github/assets/q\"r.gif", GIF)
        .open(dir.path());
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);

    let mut page = Page::new(Flavor::Github);
    page.assets = AssetMode::Inline;
    let html = page.render(&bundle);
    assert_self_contained(&html);
    let doc = Document::from(html.as_str());
    let inlined = inlined_css(&doc, "github/assets/github-1.css");
    let expected = css
        .replace(
            r"url(a\)b.png)",
            &format!("url(data:image/png;base64,{})", b64(PNG)),
        )
        .replace(
            r#"url("q\"r.gif")"#,
            &format!("url(\"data:image/gif;base64,{}\")", b64(GIF)),
        );
    assert_eq!(
        inlined.trim(),
        expected,
        "only the two real references change"
    );

    let out = tempfile::tempdir().unwrap();
    page.assets = AssetMode::Directory {
        href: "x_files".into(),
        dir: out.path().join("x_files"),
    };
    page.render(&bundle);
    let files = out.path().join("x_files/github/assets");
    assert!(files.join("a)b.png").is_file(), "escaped URL resolved");
    assert!(files.join("q\"r.gif").is_file());
    assert!(!files.join("x.png").exists(), "strings are not references");
    assert!(!files.join("y.png").exists(), "comments are not references");
    assert_eq!(
        std::fs::read_to_string(files.join("github-1.css")).unwrap(),
        css,
        "copied stylesheets are verbatim"
    );
}
