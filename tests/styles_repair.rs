//! Damaged style bundles: what [`load_or_fallback`] tells the user to do about them, and whether
//! doing exactly that works (no network: vendoring runs against canned responses).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use md_render::Flavor;
use md_render::styles::manifest::MANIFEST_PATH;
use md_render::styles::vendor::{Fetch, Response, VendorOptions, vendor_with};
use md_render::styles::{Bundle, load_or_fallback, vendor_command};

/// Canned responses keyed by URL (unknown URLs are 404), with a log of every request.
#[derive(Default)]
struct Canned {
    map: HashMap<String, Response>,
    log: Mutex<Vec<String>>,
}

impl Canned {
    fn add(&mut self, url: &str, content_type: &str, body: &str) {
        self.map.insert(
            url.to_owned(),
            Response::ok(url, content_type, body.as_bytes().to_vec()),
        );
    }

    fn requests(&self) -> usize {
        self.log.lock().map(|l| l.len()).unwrap_or_default()
    }
}

impl Fetch for Canned {
    fn get(&self, url: &str) -> anyhow::Result<Response> {
        if let Ok(mut l) = self.log.lock() {
            l.push(url.to_owned());
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

/// A GitHub-like repository page linking the stylesheets md-render requires.
fn github_site() -> Canned {
    let mut c = Canned::default();
    let names = [
        "light-0123456789abcdef.css",
        "dark-0123456789abcdef.css",
        "primer-primitives-0123456789abcdef.css",
        "primer-0123456789abcdef.css",
        "global-0123456789abcdef.css",
    ];
    let links: String = names
        .iter()
        .map(|n| format!(r#"<link media="all" rel="stylesheet" href="{GH}{n}">"#))
        .collect();
    c.add(
        "https://github.com/o/r",
        "text/html",
        &format!("<head>{links}</head>"),
    );
    for n in names {
        c.add(&format!("{GH}{n}"), "text/css", &format!("/*{n}*/"));
    }
    c
}

fn options(out: &Path) -> VendorOptions {
    VendorOptions {
        flavors: vec![Flavor::Github],
        output: out.to_owned(),
        github_source: "https://github.com/o/r".into(),
        gitlab_source: "https://gitlab.example/g/p".into(),
        all_github_themes: false,
        libs: false,
        quiet: true,
    }
}

/// The first `` `command` `` quoted in `warning`.
fn quoted_command(warning: &str) -> &str {
    warning
        .split('`')
        .nth(1)
        .unwrap_or_else(|| panic!("no command in {warning:?}"))
}

/// A good GitHub bundle at `path`, vendored from the canned site.
fn vendored_bundle(path: &Path) -> Vec<u8> {
    vendor_with(&options(path), &github_site()).expect("vendor");
    std::fs::read(path).expect("read bundle")
}

#[test]
fn corrupt_bundle_at_a_custom_path_is_refused_before_downloading() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    let good = vendored_bundle(&out);
    // Truncated, e.g. by an interrupted copy: no longer a readable archive.
    std::fs::write(&out, &good[..good.len() / 2]).expect("truncate");

    let site = github_site();
    let e = vendor_with(&options(&out), &site).expect_err("refused");
    assert!(format!("{e:#}").contains("refusing to overwrite"), "{e:#}");
    assert_eq!(site.requests(), 0, "refused before any download");
    assert_eq!(
        std::fs::read(&out).expect("read"),
        &good[..good.len() / 2],
        "untouched"
    );
}

#[test]
fn the_fix_suggested_for_an_unreadable_bundle_works() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    let good = vendored_bundle(&out);
    std::fs::write(&out, &good[..good.len() / 2]).expect("truncate");

    let (bundle, warning) = load_or_fallback(&out);
    assert!(bundle.is_fallback(Flavor::Github));
    let warning = warning.expect("warning");
    assert!(
        warning.contains("delete it, then run"),
        "vendor refuses this file, so the hint must say to delete it first: {warning}"
    );
    assert_eq!(quoted_command(&warning), vendor_command(None, &out));

    // Following the advice: delete, then vendor into the same path.
    std::fs::remove_file(&out).expect("delete");
    vendor_with(&options(&out), &github_site()).expect("the suggested command works");
    let (bundle, warning) = load_or_fallback(&out);
    assert!(warning.is_none(), "{warning:?}");
    assert!(!bundle.is_fallback(Flavor::Github));
}

#[test]
fn a_readable_but_invalid_bundle_is_repaired_by_the_suggested_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    vendored_bundle(&out);
    // A readable archive whose manifest no longer parses: vendor replaces it in place.
    let mut fs = tarfs::InMemoryFileSystem::open(&out).expect("open");
    fs.write_file(MANIFEST_PATH, "{ not json").expect("write");
    fs.write_tarball_gz(&out).expect("flush");

    let warning = load_or_fallback(&out).1.expect("warning");
    assert!(!warning.contains("delete"), "{warning}");
    assert_eq!(quoted_command(&warning), vendor_command(None, &out));
    vendor_with(&options(&out), &github_site()).expect("the suggested command works");
    assert!(Bundle::open(&out).is_ok());
}

#[test]
fn a_bundle_from_a_newer_md_render_is_refused_before_downloading() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    let mut fs = tarfs::InMemoryFileSystem::new();
    fs.write_file(
        MANIFEST_PATH,
        r#"{"schema": 99, "tool": "md-render 9", "flavors": {}, "future": true}"#,
    )
    .expect("write");
    fs.write_tarball_gz(&out).expect("flush");
    let before = std::fs::read(&out).expect("read");

    let site = github_site();
    let e = vendor_with(&options(&out), &site).expect_err("refused");
    assert!(format!("{e:#}").contains("newer md-render"), "{e:#}");
    assert_eq!(site.requests(), 0, "refused before any download");
    assert_eq!(std::fs::read(&out).expect("read"), before);

    let warning = load_or_fallback(&out).1.expect("warning");
    assert!(warning.contains("delete it, then run"), "{warning}");
}

#[test]
fn a_directory_is_not_suggested_for_deletion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let warning = load_or_fallback(dir.path()).1.expect("warning");
    assert!(!warning.contains("delete"), "{warning}");
    assert!(warning.contains("md-render vendor -o FILE"), "{warning}");
}
