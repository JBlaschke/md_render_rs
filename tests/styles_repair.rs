//! Damaged style bundles: what [`load_or_fallback`] tells the user to do about them, and whether
//! doing exactly that works (no network: vendoring runs against canned responses).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use md_render::Flavor;
use md_render::styles::manifest::{MANIFEST_PATH, role};
use md_render::styles::vendor::{Fetch, Response, VendorOptions, vendor_with};
use md_render::styles::{
    Asset, Bundle, FlavorManifest, Manifest, Stylesheet, flavor_warning, load_or_fallback,
    vendor_command,
};

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

/// Add a small GitLab flavor to the bundle at `path` (as `md-render vendor --flavor gitlab`
/// would) and return the paths it lists.
fn add_gitlab(path: &Path) -> Vec<String> {
    let files = [
        ("gitlab/assets/application-1.css", "a{b:url(x.woff2)}"),
        ("gitlab/assets/x.woff2", "font"),
    ];
    let mut fs = tarfs::InMemoryFileSystem::open(path).expect("open");
    let mut manifest: Manifest =
        serde_json::from_slice(fs.get(MANIFEST_PATH).expect("manifest")).expect("json");
    manifest.flavors.insert(
        Flavor::Gitlab,
        FlavorManifest {
            stylesheets: vec![Stylesheet {
                path: files[0].0.into(),
                role: role::APP.into(),
                theme: Some("light".into()),
                ..Default::default()
            }],
            assets: vec![Asset {
                path: files[1].0.into(),
                ..Default::default()
            }],
            themes: vec!["light".into()],
            ..Default::default()
        },
    );
    for (p, body) in files {
        fs.write_file(p, body).expect("write");
    }
    fs.write_file(MANIFEST_PATH, serde_json::to_vec(&manifest).expect("json"))
        .expect("write");
    fs.write_tarball_gz(path).expect("flush");
    files.iter().map(|(p, _)| (*p).to_owned()).collect()
}

/// Remove `victim` from the archive at `path`, leaving its manifest entry dangling.
fn damage(path: &Path, victim: &str) {
    let mut fs = tarfs::InMemoryFileSystem::open(path).expect("open");
    assert!(fs.delete_file(victim), "{victim} was in the bundle");
    fs.write_tarball_gz(path).expect("flush");
}

/// A vendored GitHub stylesheet in the bundle at `path`.
fn github_css(path: &Path) -> String {
    let fs = tarfs::InMemoryFileSystem::open(path).expect("open");
    fs.paths()
        .find(|p| p.starts_with("github/assets/global-"))
        .expect("github css")
        .to_owned()
}

#[test]
fn one_damaged_flavor_leaves_the_other_usable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    vendored_bundle(&out);
    let gitlab = add_gitlab(&out);
    damage(&out, &gitlab[1]); // e.g. one KaTeX font lost

    let bundle = Bundle::open(&out).expect("the GitHub styles are intact");
    assert!(!bundle.is_fallback(Flavor::Github));
    assert!(
        bundle.flavor(Flavor::Gitlab).is_none(),
        "damaged flavor dropped"
    );
    assert!(
        !bundle.contains(&gitlab[0]),
        "its files are dropped with it"
    );

    let (bundle, warning) = load_or_fallback(&out);
    assert!(
        !bundle.is_fallback(Flavor::Github),
        "GitHub keeps its styles"
    );
    assert!(
        bundle.is_fallback(Flavor::Gitlab),
        "GitLab gets the fallback"
    );
    let warning = warning.expect("the damage is reported");
    assert!(
        warning.contains("damaged gitlab styles") && warning.contains(&gitlab[1]),
        "names the broken flavor and file: {warning}"
    );
    assert_eq!(
        quoted_command(&warning),
        vendor_command(Some(Flavor::Gitlab), &out)
    );
    assert_eq!(flavor_warning(&bundle, Flavor::Github), None);
    let gl = flavor_warning(&bundle, Flavor::Gitlab).expect("gitlab warning");
    assert!(gl.contains("damaged gitlab styles"), "{gl}");
}

#[test]
fn the_fix_suggested_for_a_damaged_flavor_works_and_keeps_the_other() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    vendored_bundle(&out);
    let gitlab = add_gitlab(&out);
    damage(&out, &github_css(&out));

    let (bundle, warning) = load_or_fallback(&out);
    assert!(bundle.is_fallback(Flavor::Github));
    assert!(
        !bundle.is_fallback(Flavor::Gitlab),
        "GitLab keeps its styles"
    );
    let warning = warning.expect("warning");
    let cmd = quoted_command(&warning);
    assert_eq!(cmd, vendor_command(Some(Flavor::Github), &out));
    assert!(
        cmd.contains("--flavor github") && cmd.contains(" -o "),
        "{cmd}"
    );

    // Running it: re-vendor GitHub into the same file.
    vendor_with(&options(&out), &github_site()).expect("the suggested command works");
    let (bundle, warning) = load_or_fallback(&out);
    assert!(warning.is_none(), "{warning:?}");
    assert!(!bundle.is_fallback(Flavor::Github));
    assert!(
        !bundle.is_fallback(Flavor::Gitlab),
        "GitLab survived the repair"
    );
    for p in &gitlab {
        assert!(bundle.contains(p), "{p}");
    }
}

#[test]
fn a_bundle_without_any_intact_flavor_is_still_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    vendored_bundle(&out);
    let gitlab = add_gitlab(&out);
    damage(&out, &gitlab[0]);
    damage(&out, &github_css(&out));

    let e = Bundle::open(&out).expect_err("nothing usable");
    assert!(
        format!("{e:#}").contains("missing from the archive"),
        "{e:#}"
    );
    let (bundle, warning) = load_or_fallback(&out);
    assert!(bundle.is_fallback(Flavor::Github) && bundle.is_fallback(Flavor::Gitlab));
    let warning = warning.expect("warning");
    assert!(warning.contains("missing from the archive"), "{warning}");
}
