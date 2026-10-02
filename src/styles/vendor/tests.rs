//! Unit tests for the vendoring pipeline (canned HTTP responses, no network). End-to-end
//! scenarios through the public API live in `tests/styles_vendor.rs`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use super::*;
use crate::styles::{Bundle, sha256_hex};

/// Canned responses keyed by URL; unknown URLs are 404; `errors` simulate transport failures.
#[derive(Default)]
struct Canned {
    map: HashMap<String, Response>,
    errors: HashMap<String, String>,
    log: Mutex<Vec<String>>,
}

impl Canned {
    fn add(&mut self, url: &str, ct: &str, body: &str) -> &mut Self {
        self.map.insert(
            url.to_owned(),
            Response::ok(url, ct, body.as_bytes().to_vec()),
        );
        self
    }
    fn status(&mut self, url: &str, status: u16) -> &mut Self {
        self.map
            .insert(url.to_owned(), Response::status(url, status));
        self
    }
    fn requests(&self) -> Vec<String> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

impl Fetch for Canned {
    fn get(&self, url: &str) -> anyhow::Result<Response> {
        if let Ok(mut l) = self.log.lock() {
            l.push(url.to_owned());
        }
        if let Some(e) = self.errors.get(url) {
            anyhow::bail!("{e}");
        }
        Ok(self
            .map
            .get(url)
            .cloned()
            .unwrap_or_else(|| Response::status(url, 404)))
    }
    fn user_agent(&self) -> &str {
        "canned/1"
    }
}

const GH: &str = "https://github.githubassets.com/assets/";

fn github_site() -> Canned {
    let mut c = Canned::default();
    let links: String = [
        "light-0123456789abcdef.css",
        "dark-0123456789abcdef.css",
        "primer-primitives-0123456789abcdef.css",
        "primer-0123456789abcdef.css",
        "global-0123456789abcdef.css",
        "x.0123456789abcdef.module.css",
    ]
    .iter()
    .map(|n| format!(r#"<link media="all" rel="stylesheet" href="{GH}{n}">"#))
    .collect();
    c.add(
        "https://github.com/o/r",
        "text/html; charset=utf-8",
        &format!("<head>{links}</head>"),
    );
    for n in ["light", "dark", "primer-primitives", "primer"] {
        c.add(
            &format!("{GH}{n}-0123456789abcdef.css"),
            "text/css",
            &format!("/*{n}*/"),
        );
    }
    c.add(
        &format!("{GH}global-0123456789abcdef.css"),
        "text/css",
        "a{background:url(/images/spinners/x.gif)}b{background:url(\"images/modules/missing.svg\")}\
         c{background:url(data:image/png;base64,AAAA)}d{filter:url(#f)}e{background:url('/images/spinners/x.gif#frag')}",
    );
    c.add(
        "https://github.githubassets.com/images/spinners/x.gif",
        "image/gif",
        "GIF89a",
    );
    c
}

fn opts(dir: &Path, flavors: &[Flavor]) -> VendorOptions {
    VendorOptions {
        flavors: flavors.to_vec(),
        output: dir.join("out").join("styles.tar.gz"),
        github_source: "https://github.com/o/r".into(),
        gitlab_source: "https://gitlab.example/g/p".into(),
        all_github_themes: false,
        libs: false,
        quiet: true,
    }
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn github_flavor_pipeline() {
    let dir = tmp();
    let site = github_site();
    let o = opts(dir.path(), &[Flavor::Github]);
    let progress = Progress { quiet: true };
    let out = vendor_flavor(Flavor::Github, &o, &site, &progress).expect("vendor");
    let m = &out.manifest;
    let roles: Vec<(&str, Option<&str>)> = m
        .stylesheets
        .iter()
        .map(|s| (s.role.as_str(), s.theme.as_deref()))
        .collect();
    assert_eq!(
        roles,
        vec![
            ("theme", Some("light")),
            ("theme", Some("dark")),
            ("primitives", None),
            ("primer", None),
            ("global", None)
        ]
    );
    assert_eq!(m.themes, vec!["light", "dark"]);
    assert_eq!(m.user_agent, "canned/1");
    assert_eq!(m.source_page, "https://github.com/o/r");
    let global = &out.files["github/assets/global-0123456789abcdef.css"];
    let global = std::str::from_utf8(global).expect("utf8");
    assert_eq!(
        global,
        "a{background:url(../images/spinners/x.gif)}b{background:url(\"images/modules/missing.svg\")}\
         c{background:url(data:image/png;base64,AAAA)}d{filter:url(#f)}e{background:url('../images/spinners/x.gif#frag')}"
    );
    // sha256/size describe the stored (rewritten) bytes.
    let s = &m.stylesheets[4];
    assert_eq!(s.sha256, sha256_hex(global.as_bytes()));
    assert_eq!(s.size, global.len() as u64);
    assert_eq!(s.url, format!("{GH}global-0123456789abcdef.css"));
    // One asset (downloaded once despite two references), one recorded 404.
    assert_eq!(m.assets.len(), 1);
    assert_eq!(m.assets[0].path, "github/images/spinners/x.gif");
    assert_eq!(&out.files["github/images/spinners/x.gif"][..], b"GIF89a");
    assert_eq!(m.failures.len(), 1);
    let f = &m.failures[0];
    assert_eq!(
        f.url,
        "https://github.githubassets.com/assets/images/modules/missing.svg"
    );
    assert_eq!(f.status, Some(404));
    assert_eq!(
        f.referenced_by.as_deref(),
        Some("github/assets/global-0123456789abcdef.css")
    );
    assert!(chrono_like(&m.fetched_at), "{}", m.fetched_at);
    let requests = site.requests();
    assert_eq!(requests.iter().filter(|r| r.ends_with("x.gif")).count(), 1);
    assert!(!requests.iter().any(|r| r.contains(".module.css")));
}

fn chrono_like(s: &str) -> bool {
    s.len() == 20 && s.ends_with('Z') && s.as_bytes()[10] == b'T'
}

#[test]
fn image_set_strings_are_vendored() {
    let dir = tmp();
    let mut site = github_site();
    site.add(
        &format!("{GH}global-0123456789abcdef.css"),
        "text/css",
        r#"e{background-image:image-set("img/hi.png" 1x, "/images/hi@2x.png" 2x)}f{background:-webkit-image-set('img/w.png' 1x)}g{content:"img/not-a-url.png"}"#,
    );
    for u in [
        format!("{GH}img/hi.png"),
        "https://github.githubassets.com/images/hi@2x.png".to_owned(),
        format!("{GH}img/w.png"),
    ] {
        site.add(&u, "image/png", "PNG");
    }
    let o = opts(dir.path(), &[Flavor::Github]);
    let out = vendor_flavor(Flavor::Github, &o, &site, &Progress { quiet: true }).expect("vendor");
    let global = std::str::from_utf8(&out.files["github/assets/global-0123456789abcdef.css"])
        .expect("utf8")
        .to_owned();
    assert_eq!(
        global,
        r#"e{background-image:image-set("img/hi.png" 1x, "../images/hi@2x.png" 2x)}f{background:-webkit-image-set('img/w.png' 1x)}g{content:"img/not-a-url.png"}"#
    );
    for p in [
        "github/assets/img/hi.png",
        "github/images/hi@2x.png",
        "github/assets/img/w.png",
    ] {
        assert_eq!(out.files.get(p).map(|b| &b[..]), Some(&b"PNG"[..]), "{p}");
    }
    assert!(
        out.manifest.failures.is_empty(),
        "{:?}",
        out.manifest.failures
    );
    assert!(!site.requests().iter().any(|r| r.contains("not-a-url")));
}

#[test]
fn stylesheet_failures_are_fatal() {
    let dir = tmp();
    let progress = Progress { quiet: true };
    let o = opts(dir.path(), &[Flavor::Github]);

    let mut site = github_site();
    site.status(&format!("{GH}primer-0123456789abcdef.css"), 500);
    let e = vendor_flavor(Flavor::Github, &o, &site, &progress).expect_err("500");
    assert!(format!("{e:#}").contains("HTTP 500"), "{e:#}");

    let mut site = github_site();
    site.add(
        &format!("{GH}primer-0123456789abcdef.css"),
        "text/html",
        "<html>challenge</html>",
    );
    let e = vendor_flavor(Flavor::Github, &o, &site, &progress).expect_err("html");
    assert!(format!("{e:#}").contains("HTML page"), "{e:#}");

    let mut site = github_site();
    site.errors.insert(
        format!("{GH}light-0123456789abcdef.css"),
        "connection reset".into(),
    );
    let e = vendor_flavor(Flavor::Github, &o, &site, &progress).expect_err("transport");
    assert!(format!("{e:#}").contains("connection reset"), "{e:#}");

    let mut site = github_site();
    site.status("https://github.com/o/r", 403);
    let e = vendor_flavor(Flavor::Github, &o, &site, &progress).expect_err("page");
    assert!(format!("{e:#}").contains("HTTP 403"), "{e:#}");

    // Fatal failures happen before any non-stylesheet download.
    let mut site = github_site();
    site.status(&format!("{GH}global-0123456789abcdef.css"), 404);
    assert!(vendor_flavor(Flavor::Github, &o, &site, &progress).is_err());
    assert!(!site.requests().iter().any(|r| r.ends_with(".gif")));
}

#[test]
fn missing_required_roles_are_explained() {
    let dir = tmp();
    let mut site = Canned::default();
    site.add(
        "https://github.com/o/r",
        "text/html",
        &format!(r#"<link rel="stylesheet" href="{GH}light-0123456789abcdef.css">"#),
    );
    let o = opts(dir.path(), &[Flavor::Github]);
    let e = vendor_flavor(Flavor::Github, &o, &site, &Progress { quiet: true }).expect_err("roles");
    let msg = format!("{e:#}");
    assert!(
        msg.contains("theme (dark)") && msg.contains("primer") && msg.contains("global"),
        "{msg}"
    );
    assert!(msg.contains("1 stylesheet links"), "{msg}");
    let e = vendor_flavor(Flavor::Gitlab, &o, &site, &Progress { quiet: true }).expect_err("roles");
    assert!(format!("{e:#}").contains("HTTP 404"));
}

fn gitlab_site() -> Canned {
    let mut c = Canned::default();
    let page = r#"<html><head>
<link rel="stylesheet" href="/assets/application-aaaaaaaaaaaaaaaa.css" media="(prefers-color-scheme: light)">
<link rel="stylesheet" href="/assets/application_dark-bbbbbbbbbbbbbbbb.css" media="(prefers-color-scheme: dark)">
<link rel="stylesheet" href="/assets/fonts-cccccccccccccccc.css">
<link rel="stylesheet" href="/assets/highlight/themes/white-dddddddddddddddd.css" media="(prefers-color-scheme: light)">
</head><body><svg><use href="/assets/icons-eeeeeeeeeeeeeeee.svg#copy"></use></svg></body></html>"#;
    c.add("https://gitlab.example/g/p", "text/html", page);
    let a = "https://gitlab.example/assets/";
    c.add(
        &format!("{a}application-aaaaaaaaaaaaaaaa.css"),
        "text/css",
        "@import \"nested/one.css\";.md{background:url(./illustrations/x.svg)}",
    );
    // Imports: one.css -> two.css -> one.css (cycle), two.css also references an image.
    c.add(
        &format!("{a}nested/one.css"),
        "text/css",
        "@import url(two.css);",
    );
    c.add(
        &format!("{a}nested/two.css"),
        "text/css",
        "@import 'one.css';x{y:url(../deep.png)}",
    );
    c.add(&format!("{a}deep.png"), "image/png", "PNG");
    c.add(
        &format!("{a}application_dark-bbbbbbbbbbbbbbbb.css"),
        "text/css",
        "\u{feff}.md{}",
    );
    c.add(
        &format!("{a}fonts-cccccccccccccccc.css"),
        "text/css",
        "@font-face{src:url(./gitlab-sans/GitLabSans-1.woff2) format(\"woff2\")}",
    );
    c.add(
        &format!("{a}gitlab-sans/GitLabSans-1.woff2"),
        "application/octet-stream",
        "wOF2",
    );
    c.add(
        &format!("{a}highlight/themes/white-dddddddddddddddd.css"),
        "text/css",
        ".x{background:url(/assets/icons-stacked-1.svg#git)}",
    );
    c.add(
        &format!("{a}icons-stacked-1.svg"),
        "image/svg+xml",
        "<svg/>",
    );
    c.add(
        &format!("{a}icons-eeeeeeeeeeeeeeee.svg"),
        "image/svg+xml",
        "<svg id=copy/>",
    );
    c.add(
        &format!("{a}illustrations/x.svg"),
        "image/svg+xml",
        "<svg/>",
    );
    let k = "https://cdn.jsdelivr.net/npm/katex@0.16.47/dist/";
    c.add(
        &format!("{k}katex.min.js"),
        "application/javascript",
        "katex",
    );
    c.add(
        &format!("{k}katex.min.css"),
        "text/css",
        "@font-face{src:url(fonts/KaTeX_Main-Regular.woff2) format(\"woff2\"),url(fonts/KaTeX_Main-Regular.woff) format(\"woff\"),url(fonts/KaTeX_Main-Regular.ttf) format(\"truetype\")}",
    );
    c.add(
        &format!("{k}fonts/KaTeX_Main-Regular.woff2"),
        "font/woff2",
        "wOF2k",
    );
    c.add(
        "https://cdn.jsdelivr.net/npm/mermaid@11.16.1/dist/mermaid.min.js",
        "application/javascript",
        "mermaid",
    );
    c
}

#[test]
fn gitlab_flavor_pipeline() {
    let dir = tmp();
    let site = gitlab_site();
    let mut o = opts(dir.path(), &[Flavor::Gitlab]);
    o.libs = true;
    let out = vendor_flavor(Flavor::Gitlab, &o, &site, &Progress { quiet: true }).expect("vendor");
    let m = &out.manifest;
    let sheets: Vec<(&str, &str, Option<&str>, Option<&str>)> = m
        .stylesheets
        .iter()
        .map(|s| {
            (
                s.path.as_str(),
                s.role.as_str(),
                s.theme.as_deref(),
                s.media.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        sheets,
        vec![
            (
                "gitlab/assets/application-aaaaaaaaaaaaaaaa.css",
                "app",
                Some("light"),
                Some("(prefers-color-scheme: light)")
            ),
            (
                "gitlab/assets/application_dark-bbbbbbbbbbbbbbbb.css",
                "app",
                Some("dark"),
                Some("(prefers-color-scheme: dark)")
            ),
            (
                "gitlab/assets/fonts-cccccccccccccccc.css",
                "fonts",
                None,
                None
            ),
            (
                "gitlab/assets/highlight/themes/white-dddddddddddddddd.css",
                "highlight",
                Some("light"),
                Some("(prefers-color-scheme: light)")
            ),
        ]
    );
    assert_eq!(m.themes, vec!["light", "dark"]);
    let text = |p: &str| String::from_utf8(out.files[p].to_vec()).expect("utf8");
    assert_eq!(
        text("gitlab/assets/highlight/themes/white-dddddddddddddddd.css"),
        ".x{background:url(../../icons-stacked-1.svg#git)}"
    );
    assert_eq!(
        text("gitlab/assets/application_dark-bbbbbbbbbbbbbbbb.css"),
        ".md{}",
        "BOM dropped"
    );
    assert_eq!(
        text("gitlab/assets/application-aaaaaaaaaaaaaaaa.css"),
        "@import \"nested/one.css\";.md{background:url(illustrations/x.svg)}"
    );
    assert_eq!(
        text("gitlab/assets/nested/two.css"),
        "@import 'one.css';x{y:url(../deep.png)}"
    );
    assert!(out.files.contains_key("gitlab/assets/deep.png"));
    // KaTeX: woff2 vendored, woff/ttf references kept (relative) but not downloaded.
    assert_eq!(
        text("gitlab/lib/katex/katex.min.css"),
        "@font-face{src:url(fonts/KaTeX_Main-Regular.woff2) format(\"woff2\"),url(fonts/KaTeX_Main-Regular.woff) format(\"woff\"),url(fonts/KaTeX_Main-Regular.ttf) format(\"truetype\")}"
    );
    assert!(
        out.files
            .contains_key("gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff2")
    );
    assert!(
        !site
            .requests()
            .iter()
            .any(|r| r.ends_with(".woff") || r.ends_with(".ttf"))
    );
    let scripts: Vec<(&str, &str)> = m
        .scripts
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        scripts,
        vec![
            ("icons", "gitlab/assets/icons-eeeeeeeeeeeeeeee.svg"),
            ("katex", "gitlab/lib/katex/katex.min.js"),
            ("katex-css", "gitlab/lib/katex/katex.min.css"),
            ("mermaid", "gitlab/lib/mermaid/mermaid.min.js"),
        ]
    );
    assert!(m.failures.is_empty(), "{:?}", m.failures);
    // Assets are sorted and each one is stored.
    let paths: Vec<&str> = m.assets.iter().map(|a| a.path.as_str()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
    for a in &m.assets {
        let b = &out.files[&a.path];
        assert_eq!(a.size, b.len() as u64);
        assert_eq!(a.sha256, sha256_hex(b));
    }
    // Every file is listed exactly once (stylesheets + assets).
    assert_eq!(out.files.len(), m.stylesheets.len() + m.assets.len());
    // Each import was fetched once despite the cycle.
    assert_eq!(
        site.requests()
            .iter()
            .filter(|r| r.ends_with("one.css"))
            .count(),
        1
    );
}

#[test]
fn import_depth_is_limited() {
    let dir = tmp();
    let mut site = gitlab_site();
    let a = "https://gitlab.example/assets/";
    // application -> c0 -> c1 -> ... (an endless chain).
    site.add(
        &format!("{a}application-aaaaaaaaaaaaaaaa.css"),
        "text/css",
        "@import 'c0.css';",
    );
    for i in 0..10 {
        site.add(
            &format!("{a}c{i}.css"),
            "text/css",
            &format!("@import 'c{}.css';", i + 1),
        );
    }
    let out = vendor_flavor(
        Flavor::Gitlab,
        &opts(dir.path(), &[Flavor::Gitlab]),
        &site,
        &Progress { quiet: true },
    )
    .expect("vendor");
    let deep: Vec<&Failure> = out
        .manifest
        .failures
        .iter()
        .filter(|f| f.error.contains("nested"))
        .collect();
    assert_eq!(deep.len(), 1, "{:?}", out.manifest.failures);
    assert_eq!(deep[0].url, format!("{a}c4.css"));
    assert!(out.files.contains_key("gitlab/assets/c3.css"));
    assert!(!out.files.contains_key("gitlab/assets/c4.css"));
}

#[test]
fn library_and_asset_failures_are_recorded() {
    let dir = tmp();
    let mut site = gitlab_site();
    site.status(
        "https://cdn.jsdelivr.net/npm/mermaid@11.16.1/dist/mermaid.min.js",
        503,
    );
    site.errors.insert(
        "https://gitlab.example/assets/illustrations/x.svg".into(),
        "timed out".into(),
    );
    let mut o = opts(dir.path(), &[Flavor::Gitlab]);
    o.libs = true;
    let out = vendor_flavor(Flavor::Gitlab, &o, &site, &Progress { quiet: true }).expect("vendor");
    let m = &out.manifest;
    assert!(
        !m.scripts.contains_key("mermaid"),
        "failed library is not advertised"
    );
    let f: Vec<(&str, Option<u16>)> = m
        .failures
        .iter()
        .map(|f| (f.url.as_str(), f.status))
        .collect();
    assert_eq!(
        f,
        vec![
            (
                "https://cdn.jsdelivr.net/npm/mermaid@11.16.1/dist/mermaid.min.js",
                Some(503)
            ),
            ("https://gitlab.example/assets/illustrations/x.svg", None),
        ]
    );
    assert!(m.failures[1].error.contains("timed out"));
    // The CSS still points at the would-be local path (never back at the network).
    let app = std::str::from_utf8(&out.files["gitlab/assets/application-aaaaaaaaaaaaaaaa.css"])
        .expect("utf8")
        .to_owned();
    assert!(app.contains("url(illustrations/x.svg)"));
}

#[test]
fn bundle_write_read_modify_write() {
    let dir = tmp();
    let gh = github_site();
    let gl = gitlab_site();
    let o = opts(dir.path(), &[Flavor::Github]);
    let progress = Progress { quiet: true };
    let gh_out = vendor_flavor(Flavor::Github, &o, &gh, &progress).expect("github");
    write_bundle(&o.output, &[gh_out], &progress).expect("write");
    let first = Bundle::open(&o.output).expect("open");
    assert!(first.flavor(Flavor::Github).is_some() && first.flavor(Flavor::Gitlab).is_none());

    let gl_out = vendor_flavor(Flavor::Gitlab, &o, &gl, &progress).expect("gitlab");
    write_bundle(&o.output, &[gl_out], &progress).expect("write");
    let both = Bundle::open(&o.output).expect("open");
    assert_eq!(both.flavor(Flavor::Github), first.flavor(Flavor::Github));
    for p in first.paths().filter(|p| p.starts_with("github/")) {
        assert_eq!(both.read(p), first.read(p), "{p}");
    }
    assert!(both.flavor(Flavor::Gitlab).is_some());

    // A damaged other flavor is dropped instead of poisoning the bundle.
    let mut fs = tarfs::InMemoryFileSystem::open(&o.output).expect("open fs");
    fs.delete_file("github/images/spinners/x.gif");
    fs.write_tarball_gz(&o.output).expect("flush");
    let damaged = Bundle::open(&o.output).expect("GitLab is intact");
    assert!(
        damaged.flavor(Flavor::Github).is_none(),
        "damaged flavor dropped"
    );
    assert_eq!(damaged.flavor(Flavor::Gitlab), both.flavor(Flavor::Gitlab));
    let gl_out = vendor_flavor(Flavor::Gitlab, &o, &gl, &progress).expect("gitlab");
    write_bundle(&o.output, &[gl_out], &progress).expect("write");
    let repaired = Bundle::open(&o.output).expect("open");
    assert!(repaired.flavor(Flavor::Github).is_none());
    assert!(!repaired.paths().any(|p| p.starts_with("github/")));
}

#[test]
fn refuses_to_clobber_foreign_files() {
    let dir = tmp();
    let progress = Progress { quiet: true };
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# my notes").expect("write");
    let e = existing_source(&path, &progress).expect_err("refuse");
    assert!(format!("{e:#}").contains("refusing to overwrite"));

    let tar = dir.path().join("other.tar.gz");
    let mut fs = tarfs::InMemoryFileSystem::new();
    fs.write_file("src/main.rs", b"fn main(){}".to_vec())
        .expect("write");
    fs.write_tarball_gz(&tar).expect("flush");
    let e = existing_source(&tar, &progress).expect_err("refuse");
    assert!(format!("{e:#}").contains("not an md-render style bundle"));

    assert!(
        existing_source(dir.path(), &progress).is_err(),
        "directories are refused"
    );
    assert_eq!(
        existing_source(&dir.path().join("new.tar.gz"), &progress).expect("absent"),
        None
    );
}

#[test]
fn existing_manifest_policies() {
    let progress = Progress { quiet: true };
    let out = Path::new("x.tar.gz");
    let mut fs = tarfs::InMemoryFileSystem::new();
    assert_eq!(
        existing_manifest(&mut fs, out, &progress).expect("empty"),
        Manifest::default()
    );

    let newer = Manifest {
        schema: SCHEMA + 1,
        ..Manifest::default()
    };
    fs.write_file(MANIFEST_PATH, serde_json::to_vec(&newer).expect("json"))
        .expect("write");
    let e = existing_manifest(&mut fs, out, &progress).expect_err("newer");
    assert!(format!("{e:#}").contains("newer md-render"));

    let mut fs = tarfs::InMemoryFileSystem::new();
    fs.write_file(MANIFEST_PATH, b"{broken".to_vec())
        .expect("write");
    fs.write_file("github/a.css", b"a".to_vec()).expect("write");
    assert_eq!(
        existing_manifest(&mut fs, out, &progress).expect("fresh"),
        Manifest::default()
    );
    assert!(fs.is_empty(), "a broken bundle is replaced wholesale");
}

#[test]
fn vendor_with_rejects_bad_options() {
    let dir = tmp();
    let site = github_site();
    let mut o = opts(dir.path(), &[]);
    assert!(vendor_with(&o, &site).is_err());
    o.flavors = vec![Flavor::Github];
    o.github_source = "ftp://github.com/x".into();
    let e = vendor_with(&o, &site).expect_err("scheme");
    assert!(format!("{e:#}").contains("only http and https"));
    assert!(!o.output.exists());
    o.github_source = "not a url".into();
    assert!(vendor_with(&o, &site).is_err());
}

#[test]
fn fetch_all_keeps_order() {
    let mut site = Canned::default();
    let urls: Vec<String> = (0..40).map(|i| format!("https://h/{i}")).collect();
    for (i, u) in urls.iter().enumerate() {
        site.add(u, "text/plain", &i.to_string());
    }
    let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    let results = fetch_all(&site, &refs);
    assert_eq!(results.len(), 40);
    for (i, r) in results.into_iter().enumerate() {
        assert_eq!(r.expect("ok").body, i.to_string().into_bytes());
    }
    assert!(fetch_all(&site, &[]).is_empty());
}

#[test]
fn timestamps() {
    let at = |s: u64| rfc3339_utc(UNIX_EPOCH + Duration::from_secs(s));
    assert_eq!(at(0), "1970-01-01T00:00:00Z");
    assert_eq!(at(951_868_799), "2000-02-29T23:59:59Z");
    assert_eq!(at(1_790_000_000), "2026-09-21T14:13:20Z");
    assert_eq!(at(4_102_444_800), "2100-01-01T00:00:00Z");
}

#[test]
fn sizes_and_decoding() {
    assert_eq!(human_bytes(0), "0 B");
    assert_eq!(human_bytes(999), "999 B");
    assert_eq!(human_bytes(87_305), "87.3 KB");
    assert_eq!(human_bytes(3_566_058), "3.6 MB");
    assert_eq!(decode_css(b"\xef\xbb\xbfa{}".to_vec()).expect("bom"), "a{}");
    assert!(decode_css(vec![0xff, 0xfe]).is_err());
}
