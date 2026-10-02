//! Unit tests for `Bundle`, `SharedBundle` and the loading helpers.

use std::path::Path;

use super::*;
use crate::styles::manifest::script;

/// A small valid GitHub-only bundle: (manifest, files).
fn github_manifest() -> (Manifest, Vec<(&'static str, &'static [u8])>) {
    let files: Vec<(&'static str, &'static [u8])> = vec![
        ("github/assets/light-0123456789abcdef.css", b":root{--x:1}"),
        (
            "github/assets/global-0123456789abcdef.css",
            b"a{background:url(../images/a.gif)}",
        ),
        ("github/images/a.gif", b"GIF89a"),
        ("github/lib/mermaid/mermaid.min.js", b"mermaid"),
    ];
    let mut m = Manifest::default();
    m.flavors.insert(
        Flavor::Github,
        FlavorManifest {
            fetched_at: "2026-10-01T00:00:00Z".into(),
            source_page: "https://github.com/joeyespo/grip".into(),
            user_agent: "test".into(),
            fallback: false,
            stylesheets: vec![
                Stylesheet {
                    path: files[0].0.into(),
                    role: role::THEME.into(),
                    theme: Some("light".into()),
                    ..Default::default()
                },
                Stylesheet {
                    path: files[1].0.into(),
                    role: role::GLOBAL.into(),
                    ..Default::default()
                },
            ],
            assets: vec![
                Asset {
                    path: files[2].0.into(),
                    ..Default::default()
                },
                Asset {
                    path: files[3].0.into(),
                    ..Default::default()
                },
            ],
            scripts: [(script::MERMAID.to_owned(), files[3].0.to_owned())]
                .into_iter()
                .collect(),
            themes: vec!["light".into()],
            failures: vec![],
        },
    );
    (m, files)
}

fn write_bundle(path: &Path, manifest: &Manifest, files: &[(&str, &[u8])]) {
    let mut fs = tarfs::InMemoryFileSystem::new();
    for (p, b) in files {
        fs.write_file(p, b.to_vec()).expect("write file");
    }
    fs.write_file(MANIFEST_PATH, serde_json::to_vec(manifest).expect("json"))
        .expect("manifest");
    fs.write_tarball_gz(path).expect("flush");
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn open_round_trip() {
    let dir = tmp();
    let path = dir.path().join("styles.tar.gz");
    let (m, files) = github_manifest();
    write_bundle(&path, &m, &files);
    let b = Bundle::open(&path).expect("open");
    assert_eq!(b.manifest(), &m);
    assert_eq!(b.origin(), &BundleOrigin::File(path.clone()));
    for (p, content) in &files {
        assert_eq!(b.read(p).as_deref(), Some(*content), "{p}");
        assert!(b.contains(p));
        assert_eq!(b.file_size(p), Some(content.len() as u64));
    }
    assert!(b.read("github/nope.css").is_none());
    assert!(
        b.read("/github/images/a.gif").is_none(),
        "paths are exact bundle paths"
    );
    let disk = std::fs::read(&path).expect("read");
    assert_eq!(b.etag(), sha256_hex(&disk));
    assert_eq!(b.etag().len(), 64);
    assert!(!b.is_fallback(Flavor::Github));
    assert!(
        b.is_fallback(Flavor::Gitlab),
        "absent flavor counts as fallback"
    );
    assert_eq!(b.paths().count(), files.len() + 1);
    assert!(format!("{b:?}").contains("etag"));
}

#[test]
fn etag_changes_with_content() {
    let dir = tmp();
    let path = dir.path().join("s.tar.gz");
    let (m, mut files) = github_manifest();
    write_bundle(&path, &m, &files);
    let a = Bundle::open(&path).expect("open").etag().to_owned();
    write_bundle(&path, &m, &files);
    assert_eq!(
        Bundle::open(&path).expect("open").etag(),
        a,
        "same content, same etag"
    );
    files[2].1 = b"GIF89b";
    write_bundle(&path, &m, &files);
    assert_ne!(Bundle::open(&path).expect("open").etag(), a);
}

#[test]
fn open_rejects_invalid_bundles() {
    let dir = tmp();
    let (m, files) = github_manifest();
    let msg = |path: &Path| format!("{:#}", Bundle::open(path).expect_err("must fail"));

    // A listed file is missing.
    let p = dir.path().join("missing.tar.gz");
    write_bundle(&p, &m, &files[..2]);
    let e = msg(&p);
    assert!(
        e.contains("github/images/a.gif") && e.contains("missing"),
        "{e}"
    );
    assert!(e.contains("missing.tar.gz"), "error names the file: {e}");

    // A script path missing.
    let mut m2 = m.clone();
    if let Some(gh) = m2.flavors.get_mut(&Flavor::Github) {
        gh.scripts
            .insert(script::MATHJAX.into(), "github/lib/mathjax/es5/x.js".into());
    }
    let p = dir.path().join("script.tar.gz");
    write_bundle(&p, &m2, &files);
    assert!(msg(&p).contains("github/lib/mathjax/es5/x.js"));

    // Path outside the flavor directory.
    let mut m3 = m.clone();
    if let Some(gh) = m3.flavors.get_mut(&Flavor::Github) {
        gh.assets.push(Asset {
            path: "gitlab/x.png".into(),
            ..Default::default()
        });
    }
    let p = dir.path().join("outside.tar.gz");
    let mut files3 = files.clone();
    files3.push(("gitlab/x.png", b"x"));
    write_bundle(&p, &m3, &files3);
    assert!(msg(&p).contains("outside github/"));

    // Unsupported schema.
    let mut m4 = m.clone();
    m4.schema = 99;
    let p = dir.path().join("schema.tar.gz");
    write_bundle(&p, &m4, &files);
    assert!(msg(&p).contains("schema 99"));

    // No manifest.
    let p = dir.path().join("nomanifest.tar.gz");
    let mut fs = tarfs::InMemoryFileSystem::new();
    fs.write_file("github/a.css", b"a".to_vec()).expect("write");
    fs.write_tarball_gz(&p).expect("flush");
    assert!(msg(&p).contains("no manifest.json"));

    // Broken manifest JSON.
    let p = dir.path().join("badjson.tar.gz");
    let mut fs = tarfs::InMemoryFileSystem::new();
    fs.write_file(MANIFEST_PATH, b"{not json".to_vec())
        .expect("write");
    fs.write_tarball_gz(&p).expect("flush");
    assert!(msg(&p).contains("parsing manifest.json"));

    // Not a gzip at all / truncated.
    let p = dir.path().join("garbage.tar.gz");
    std::fs::write(&p, b"this is not a tarball").expect("write");
    assert!(Bundle::open(&p).is_err());
    let good = dir.path().join("good.tar.gz");
    write_bundle(&good, &m, &files);
    let bytes = std::fs::read(&good).expect("read");
    let p = dir.path().join("truncated.tar.gz");
    std::fs::write(&p, &bytes[..bytes.len() / 2]).expect("write");
    assert!(Bundle::open(&p).is_err());

    // Missing file: NotFound is detectable.
    let e = Bundle::open(&dir.path().join("absent.tar.gz")).expect_err("absent");
    assert!(is_not_found(&e));
}

#[test]
fn fallback_bundle() {
    let b = Bundle::fallback();
    assert_eq!(b.origin(), &BundleOrigin::Fallback);
    assert!(b.etag().starts_with("fallback-"));
    for f in Flavor::ALL {
        let fm = b.flavor(f).expect("flavor present");
        assert!(fm.fallback && b.is_fallback(f));
        assert_eq!(fm.stylesheets.len(), 1);
        let s = &fm.stylesheets[0];
        assert_eq!(s.role, role::FALLBACK);
        assert_eq!(s.path, format!("{f}/fallback.css"));
        let css = b.read(&s.path).expect("css");
        let (embedded, _) = crate::assets::get(&format!("fallback-{f}.css")).expect("embedded");
        assert_eq!(&css[..], embedded);
        assert_eq!(s.size, embedded.len() as u64);
        assert_eq!(s.sha256, sha256_hex(embedded));
        assert_eq!(fm.themes, vec!["light", "dark"]);
    }
    // Deterministic.
    assert_eq!(Bundle::fallback().etag(), b.etag());
    // The fallback validates like any bundle.
    validate_manifest(b.manifest(), |p| b.contains(p)).expect("valid");
}

#[test]
fn with_fallbacks_fills_only_missing_flavors() {
    let dir = tmp();
    let path = dir.path().join("s.tar.gz");
    let (m, files) = github_manifest();
    write_bundle(&path, &m, &files);
    let b = Bundle::open(&path).expect("open");
    let etag = b.etag().to_owned();
    let filled = b.clone().with_fallbacks();
    assert_eq!(
        filled.flavor(Flavor::Github),
        b.flavor(Flavor::Github),
        "vendored flavor untouched"
    );
    assert!(filled.is_fallback(Flavor::Gitlab));
    assert!(filled.read("gitlab/fallback.css").is_some());
    assert_ne!(filled.etag(), etag);
    assert!(filled.etag().starts_with(&etag));
    assert_eq!(filled.origin(), b.origin());
    // Deterministic, and idempotent.
    assert_eq!(b.clone().with_fallbacks().etag(), filled.etag());
    let twice = filled.clone().with_fallbacks();
    assert_eq!(twice.etag(), filled.etag());
    assert_eq!(twice.manifest(), filled.manifest());
    // Complete bundles are returned unchanged.
    let fb = Bundle::fallback();
    assert_eq!(fb.clone().with_fallbacks().etag(), fb.etag());
}

#[test]
fn load_or_fallback_warnings() {
    let dir = tmp();
    let absent = dir.path().join("absent.tar.gz");
    let (b, w) = load_or_fallback(&absent);
    assert_eq!(b.origin(), &BundleOrigin::Fallback);
    let w = w.expect("warning");
    assert!(
        w.contains("no style bundle at") && w.contains("md-render vendor"),
        "{w}"
    );
    assert!(
        w.contains("-o "),
        "non-default path is part of the command: {w}"
    );

    let corrupt = dir.path().join("corrupt.tar.gz");
    std::fs::write(&corrupt, b"junk").expect("write");
    let (b, w) = load_or_fallback(&corrupt);
    assert_eq!(b.origin(), &BundleOrigin::Fallback);
    let w = w.expect("warning");
    assert!(
        w.contains("invalid style bundle") && w.contains("md-render vendor"),
        "{w}"
    );

    let good = dir.path().join("good.tar.gz");
    let (m, files) = github_manifest();
    write_bundle(&good, &m, &files);
    let (b, w) = load_or_fallback(&good);
    assert!(w.is_none());
    assert!(!b.is_fallback(Flavor::Github) && b.is_fallback(Flavor::Gitlab));
    assert!(flavor_warning(&b, Flavor::Github).is_none());
    let fw = flavor_warning(&b, Flavor::Gitlab).expect("gitlab warning");
    assert!(
        fw.contains("has no gitlab styles") && fw.contains("--flavor gitlab"),
        "{fw}"
    );
    assert!(flavor_warning(&Bundle::fallback(), Flavor::Gitlab).is_none());
}

#[test]
fn vendor_command_quotes_paths() {
    assert_eq!(
        vendor_command(Some(Flavor::Github), Path::new("/tmp/x.tar.gz")),
        "md-render vendor --flavor github -o /tmp/x.tar.gz"
    );
    assert_eq!(
        vendor_command(None, Path::new("/a b/it's.tar.gz")),
        r"md-render vendor -o '/a b/it'\''s.tar.gz'"
    );
    assert_eq!(
        vendor_command(None, &default_bundle_path()),
        "md-render vendor"
    );
}

#[test]
fn default_path_is_a_tarball() {
    let p = default_bundle_path();
    assert!(!p.as_os_str().is_empty());
    if std::env::var_os("MD_RENDER_STYLES").is_none() {
        assert!(p.ends_with("styles.tar.gz"), "{}", p.display());
    }
}

#[test]
fn content_types() {
    assert_eq!(
        content_type("github/assets/a.css"),
        "text/css; charset=utf-8"
    );
    assert_eq!(
        content_type("github/lib/mermaid/mermaid.min.js"),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        content_type("gitlab/assets/gitlab-sans/G.woff2"),
        "font/woff2"
    );
    assert_eq!(content_type("gitlab/assets/icons-1.svg"), "image/svg+xml");
    assert_eq!(content_type("github/emoji/octocat.png"), "image/png");
    assert_eq!(content_type("manifest.json"), "application/json");
    assert_eq!(content_type("x/unknown.zzz"), "application/octet-stream");
}

#[test]
fn shared_bundle_reload() {
    let dir = tmp();
    let path = dir.path().join("styles.tar.gz");

    // Start without a file: fallback plus a warning.
    let (shared, warning) = SharedBundle::load(&path);
    assert!(warning.is_some());
    assert_eq!(shared.path(), path.as_path());
    assert_eq!(shared.get().origin(), &BundleOrigin::Fallback);
    assert!(
        !shared.reload().expect("still absent"),
        "no change while the file is absent"
    );

    // A valid bundle appears.
    let (m, mut files) = github_manifest();
    write_bundle(&path, &m, &files);
    assert!(shared.reload().expect("reload"));
    let first = shared.get();
    assert_eq!(first.origin(), &BundleOrigin::File(path.clone()));
    assert!(!first.is_fallback(Flavor::Github));
    assert!(
        first.is_fallback(Flavor::Gitlab),
        "missing flavors are filled"
    );
    assert!(!shared.reload().expect("unchanged"), "same bytes: no swap");

    // A corrupt file keeps the old bundle.
    std::fs::write(&path, b"corrupt").expect("write");
    let err = shared.reload().expect_err("corrupt");
    assert!(format!("{err:#}").contains("keeping the previously loaded styles"));
    assert_eq!(shared.etag(), first.etag());
    // A bundle with a missing file keeps the old one too.
    write_bundle(&path, &m, &files[..1]);
    assert!(shared.reload().is_err());
    assert_eq!(shared.etag(), first.etag());

    // A changed valid bundle swaps; old Arcs stay usable.
    files[2].1 = b"GIF89a-new";
    write_bundle(&path, &m, &files);
    assert!(shared.reload().expect("reload"));
    assert_ne!(shared.etag(), first.etag());
    assert_eq!(
        first.read("github/images/a.gif").as_deref(),
        Some(&b"GIF89a"[..])
    );
    assert_eq!(
        shared.get().read("github/images/a.gif").as_deref(),
        Some(&b"GIF89a-new"[..])
    );

    // Deleting the file falls back.
    std::fs::remove_file(&path).expect("remove");
    assert!(shared.reload().expect("fallback"));
    assert_eq!(shared.get().origin(), &BundleOrigin::Fallback);

    // replace() reports whether anything changed.
    assert!(!shared.replace(Bundle::fallback()));
    assert!(format!("{shared:?}").contains("SharedBundle"));
}

#[test]
fn shared_bundle_concurrent_readers() {
    let dir = tmp();
    let path = dir.path().join("styles.tar.gz");
    let (m, files) = github_manifest();
    write_bundle(&path, &m, &files);
    let (shared, _) = SharedBundle::load(&path);
    let shared = std::sync::Arc::new(shared);
    std::thread::scope(|s| {
        for _ in 0..4 {
            let shared = shared.clone();
            s.spawn(move || {
                for _ in 0..200 {
                    let b = shared.get();
                    // Every bundle a reader sees is complete.
                    assert!(b.read("github/images/a.gif").is_some());
                }
            });
        }
        for i in 0..10u8 {
            let content = vec![b'G', i];
            let mut changed: Vec<(&str, &[u8])> = files.clone();
            changed[2].1 = &content;
            write_bundle(&path, &m, &changed);
            shared.reload().expect("reload");
        }
    });
}

#[test]
fn shared_bundle_unchanged_file_keeps_the_same_bundle() {
    let dir = tmp();
    let path = dir.path().join("styles.tar.gz");
    let (m, files) = github_manifest();
    write_bundle(&path, &m, &files);
    let (shared, warning) = SharedBundle::load(&path);
    assert!(warning.is_none());
    let before = shared.get();
    // Rewritten with identical bytes (e.g. a no-op re-vendor): not a change.
    write_bundle(&path, &m, &files);
    assert!(!shared.reload().expect("reload"));
    assert!(Arc::ptr_eq(&before, &shared.get()));
    // An unreadable path (a directory) is an error that keeps the old bundle.
    let as_dir = SharedBundle::new(dir.path(), Bundle::fallback());
    let e = as_dir.reload().expect_err("directory");
    assert!(
        format!("{e:#}").contains("keeping the previously loaded styles"),
        "{e:#}"
    );
    assert_eq!(as_dir.get().origin(), &BundleOrigin::Fallback);
}

#[test]
fn resolve_bundle_refs() {
    let css = "gitlab/assets/highlight/themes/white-1.css";
    let cases: [(&str, Option<&str>); 16] = [
        (
            "../../icons-stacked-2.svg#git",
            Some("gitlab/assets/icons-stacked-2.svg"),
        ),
        ("x.png?v=3", Some("gitlab/assets/highlight/themes/x.png")),
        (
            "./a/./b/../c.svg",
            Some("gitlab/assets/highlight/themes/a/c.svg"),
        ),
        ("a%20b.png", Some("gitlab/assets/highlight/themes/a b.png")),
        ("../../../../manifest.json", Some("manifest.json")),
        ("../../../../../x", None),
        ("/assets/icons.svg", None),
        ("https://gitlab.com/assets/x.svg", None),
        ("//cdn.example/x.svg", None),
        ("data:image/png;base64,AAAA", None),
        ("DATA:x", None),
        ("#frag", None),
        ("", None),
        ("dir/", None),
        ("..", None),
        ("%2e%2e/%2e%2e/%2e%2e/%2e%2e/%2e%2e/x", None),
    ];
    for (r, want) in cases {
        assert_eq!(resolve_bundle_ref(css, r).as_deref(), want, "{r:?}");
    }
    assert_eq!(
        resolve_bundle_ref("top.css", "a/b.png").as_deref(),
        Some("a/b.png")
    );
    assert_eq!(resolve_bundle_ref("top.css", "../b.png"), None);
    // A colon after a slash is a path character, not a scheme.
    assert_eq!(
        resolve_bundle_ref("a/b.css", "c/d:e.png").as_deref(),
        Some("a/c/d:e.png")
    );
}

#[test]
fn css_reference_helpers() {
    let css = r#"/* url(no.png) */a{b:url(../images/a.gif)}@import "x.css";c{d:url("data:,x")}"#;
    assert_eq!(
        css_references(css),
        vec!["../images/a.gif", "x.css", "data:,x"]
    );
    let out = rewrite_css_references(css, |v| {
        (!v.starts_with("data:")).then(|| format!("data:text/plain;base64,{}", v.len()))
    });
    assert_eq!(
        out,
        r#"/* url(no.png) */a{b:url(data:text/plain;base64,15)}@import "data:text/plain;base64,5";c{d:url("data:,x")}"#
    );
    // Values needing escapes stay valid CSS.
    let out = rewrite_css_references("a{b:url(x)}", |_| Some("a b(c)".into()));
    assert_eq!(css_references(&out), vec!["a b(c)"]);
    // image-set() string arguments are image URLs, other strings are not.
    let css = r#"a{b:image-set("i.png" 1x)}c{content:"t.png"}"#;
    assert_eq!(css_references(css), vec!["i.png"]);
    assert_eq!(
        rewrite_css_references(css, |_| Some("data:,\"q\"".into())),
        r#"a{b:image-set("data:,\"q\"" 1x)}c{content:"t.png"}"#
    );
}

#[test]
fn bundle_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Bundle>();
    assert_send_sync::<SharedBundle>();
}
