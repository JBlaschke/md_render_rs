//! Style bundle loading through the public API, the way the server, page and export modules
//! use it (DESIGN §3.5, §4): open in place, validate, fall back, hot-swap.

use std::path::Path;
use std::sync::Arc;

use md_render::Flavor;
use md_render::styles::manifest::{MANIFEST_PATH, SCHEMA, role, script};
use md_render::styles::{
    Asset, Bundle, BundleOrigin, FlavorManifest, Manifest, SharedBundle, Stylesheet, content_type,
    default_bundle_path, flavor_warning, load_or_fallback, resolve_bundle_ref, vendor_command,
};

/// A GitLab-only bundle as `md-render vendor --flavor gitlab` would write it.
fn gitlab_bundle(path: &Path, css: &str) -> Manifest {
    let files: [(&str, &[u8]); 4] = [
        ("gitlab/assets/application-1.css", css.as_bytes()),
        ("gitlab/assets/icon_anchor-2.svg", b"<svg/>"),
        ("gitlab/lib/katex/katex.min.js", b"katex"),
        ("gitlab/lib/katex/katex.min.css", b".katex{}"),
    ];
    let mut manifest = Manifest::default();
    manifest.flavors.insert(
        Flavor::Gitlab,
        FlavorManifest {
            fetched_at: "2026-10-01T00:00:00Z".into(),
            source_page: "https://gitlab.com/gitlab-org/gitlab-foss".into(),
            user_agent: "test".into(),
            stylesheets: vec![Stylesheet {
                path: files[0].0.into(),
                url: "https://gitlab.com/assets/application-1.css".into(),
                role: role::APP.into(),
                theme: Some("light".into()),
                media: Some("(prefers-color-scheme: light)".into()),
                sha256: String::new(),
                size: css.len() as u64,
            }],
            assets: files[1..]
                .iter()
                .map(|(p, b)| Asset {
                    path: (*p).into(),
                    size: b.len() as u64,
                    ..Default::default()
                })
                .collect(),
            scripts: [
                (script::KATEX.to_owned(), files[2].0.to_owned()),
                (script::KATEX_CSS.to_owned(), files[3].0.to_owned()),
            ]
            .into_iter()
            .collect(),
            themes: vec!["light".into()],
            ..Default::default()
        },
    );
    let mut fs = tarfs::InMemoryFileSystem::new();
    for (p, b) in files {
        fs.write_file(p, b.to_vec()).expect("write");
    }
    fs.write_file(
        MANIFEST_PATH,
        serde_json::to_vec_pretty(&manifest).expect("json"),
    )
    .expect("manifest");
    fs.write_tarball_gz(path).expect("flush");
    manifest
}

#[test]
fn open_read_and_fill_missing_flavor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("styles.tar.gz");
    let css = ".md h1 a.anchor::after{background:url(icon_anchor-2.svg)}";
    let manifest = gitlab_bundle(&path, css);

    let bundle = Bundle::open(&path).expect("open");
    assert_eq!(bundle.manifest(), &manifest);
    assert_eq!(bundle.origin(), &BundleOrigin::File(path.clone()));
    assert_eq!(bundle.manifest().schema, SCHEMA);
    let gl = bundle.flavor(Flavor::Gitlab).expect("gitlab");
    assert_eq!(
        gl.script(script::KATEX),
        Some("gitlab/lib/katex/katex.min.js")
    );
    let app = gl.themed_stylesheet(role::APP, "light").expect("app light");
    let bytes = bundle.read(&app.path).expect("css");
    assert_eq!(&bytes[..], css.as_bytes());
    // Relative url() inside the CSS resolves to a bundle file.
    let target = resolve_bundle_ref(&app.path, "icon_anchor-2.svg").expect("relative");
    assert_eq!(bundle.read(&target).as_deref(), Some(&b"<svg/>"[..]));
    assert!(bundle.flavor(Flavor::Github).is_none());

    // The server's view: missing GitHub styles come from the fallback, with a warning.
    let (served, warning) = load_or_fallback(&path);
    assert!(warning.is_none());
    let gh = served.flavor(Flavor::Github).expect("github fallback");
    assert!(gh.fallback);
    assert_eq!(gh.stylesheets[0].role, role::FALLBACK);
    assert!(
        served
            .read(&gh.stylesheets[0].path)
            .is_some_and(|b| !b.is_empty())
    );
    assert_eq!(served.flavor(Flavor::Gitlab), bundle.flavor(Flavor::Gitlab));
    assert_ne!(
        served.etag(),
        bundle.etag(),
        "filled bundle has its own etag"
    );
    let w = flavor_warning(&served, Flavor::Github).expect("warning for github");
    assert!(
        w.contains(&vendor_command(Some(Flavor::Github), &path)),
        "{w}"
    );
    assert!(flavor_warning(&served, Flavor::Gitlab).is_none());
}

#[test]
fn missing_and_invalid_bundles_fall_back_with_the_fix_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let absent = dir.path().join("absent.tar.gz");
    let (b, w) = load_or_fallback(&absent);
    assert_eq!(b.origin(), &BundleOrigin::Fallback);
    for f in Flavor::ALL {
        assert!(b.flavor(f).is_some_and(|m| m.fallback), "{f}");
    }
    let w = w.expect("warning");
    assert!(w.contains(&vendor_command(None, &absent)), "{w}");

    // A manifest listing a file that is not in the archive is rejected.
    let broken = dir.path().join("broken.tar.gz");
    gitlab_bundle(&broken, "a{}");
    let mut fs = tarfs::InMemoryFileSystem::open(&broken).expect("open fs");
    assert!(fs.delete_file("gitlab/lib/katex/katex.min.js"));
    fs.write_tarball_gz(&broken).expect("flush");
    let e = Bundle::open(&broken).expect_err("invalid");
    assert!(
        format!("{e:#}").contains("gitlab/lib/katex/katex.min.js"),
        "{e:#}"
    );
    let (b, w) = load_or_fallback(&broken);
    assert_eq!(b.origin(), &BundleOrigin::Fallback);
    assert!(w.expect("warning").contains("katex.min.js"));

    // Not a tarball at all.
    let junk = dir.path().join("junk.tar.gz");
    std::fs::write(&junk, b"<html>proxy error</html>").expect("write");
    assert!(Bundle::open(&junk).is_err());
    assert!(load_or_fallback(&junk).1.is_some());
}

#[test]
fn shared_bundle_hot_swap_keeps_old_bundle_on_corruption() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("styles.tar.gz");
    gitlab_bundle(&path, "a{color:red}");
    let (shared, warning) = SharedBundle::load(&path);
    assert!(warning.is_none());
    let v1 = shared.get();
    let v1_etag = shared.etag();
    assert!(!shared.reload().expect("unchanged"));

    // Re-vendored: swapped, readers holding the old Arc keep a consistent view.
    gitlab_bundle(&path, "a{color:blue}");
    assert!(shared.reload().expect("changed"));
    let v2 = shared.get();
    assert!(!Arc::ptr_eq(&v1, &v2));
    assert_ne!(shared.etag(), v1_etag);
    assert_eq!(
        v1.read("gitlab/assets/application-1.css").as_deref(),
        Some(&b"a{color:red}"[..])
    );
    assert_eq!(
        v2.read("gitlab/assets/application-1.css").as_deref(),
        Some(&b"a{color:blue}"[..])
    );

    // Corrupt (e.g. truncated by a full disk): error, old bundle stays.
    let good = std::fs::read(&path).expect("read");
    std::fs::write(&path, &good[..good.len() / 3]).expect("truncate");
    let e = shared.reload().expect_err("corrupt");
    assert!(
        format!("{e:#}").contains("keeping the previously loaded styles"),
        "{e:#}"
    );
    assert!(Arc::ptr_eq(&v2, &shared.get()));

    // Fixed again.
    std::fs::write(&path, &good).expect("restore");
    assert!(
        !shared.reload().expect("same as current"),
        "identical to the current bundle"
    );

    // Deleted: back to the fallback, like a fresh start.
    std::fs::remove_file(&path).expect("remove");
    assert!(shared.reload().expect("fallback"));
    assert_eq!(shared.get().origin(), &BundleOrigin::Fallback);
}

#[test]
fn fallback_and_helpers() {
    let fb = Bundle::fallback();
    assert_eq!(fb.origin(), &BundleOrigin::Fallback);
    assert_eq!(fb.etag(), Bundle::fallback().etag(), "deterministic");
    assert!(
        fb.etag()
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    );
    let same = fb.clone().with_fallbacks();
    assert_eq!(same.etag(), fb.etag());
    assert_eq!(same.manifest(), fb.manifest());

    assert_eq!(
        content_type("github/assets/light-1.css"),
        "text/css; charset=utf-8"
    );
    assert_eq!(
        content_type("github/lib/mathjax/es5/tex-chtml-full.js"),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        content_type("gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff2"),
        "font/woff2"
    );
    assert_eq!(
        vendor_command(None, &default_bundle_path()),
        "md-render vendor"
    );
}
