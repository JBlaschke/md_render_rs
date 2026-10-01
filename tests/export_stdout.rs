//! `md-render export … -`: the page is the only thing written to stdout; diagnostics go to
//! stderr. The export runs in a child process (this test binary re-invoked with a filter), so
//! the real stdout / stderr streams can be inspected.

mod page_support;

use std::process::Command;
use std::sync::Arc;

use dom_query::Document;
use md_render::export::{ExportConfig, ExportInput, ExportOutput, export};
use md_render::page::Layout;
use md_render::render::Features;
use md_render::{Flavor, Mode, ThemeSel};
use page_support::*;

const CHILD_ENV: &str = "MD_RENDER_TEST_EXPORT_STDOUT_DIR";

/// Child side: does nothing in a normal test run.
#[test]
fn child_export_to_stdout() {
    let Some(dir) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let cfg = ExportConfig {
        input: ExportInput::Stdin {
            name: "stdin.md".into(),
            text: "# Title\n".into(),
        },
        output: ExportOutput::Stdout,
        inline: true,
        flavor: Flavor::Github,
        mode: Mode::Readme,
        theme: ThemeSel::default(),
        layout: Layout::Readme,
        title: None,
        context: None,
        // Missing on purpose: the fallback warning must go to stderr, not into the page.
        bundle_path: dir.join("missing-styles.tar.gz"),
        renderer: Arc::new(FixedRenderer::new(content(
            Flavor::Github,
            Features::default(),
        ))),
        quiet: false,
    };
    export(&cfg).expect("export to stdout");
}

#[test]
fn stdout_carries_only_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().expect("test binary path");
    let output = Command::new(exe)
        .args([
            "child_export_to_stdout",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, dir.path())
        .output()
        .expect("child test runs");
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "child failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let start = stdout.find("<!DOCTYPE html>").expect("page on stdout");
    let end = stdout.find("</html>\n").expect("complete page") + "</html>\n".len();
    let page = &stdout[start..end];
    assert_eq!(stdout.matches("<!DOCTYPE html>").count(), 1);
    assert_self_contained(page);
    let doc = Document::from(page);
    assert_eq!(doc.select(".mdr-box-title").text().trim(), "stdin.md");
    assert!(
        doc.select("style[data-mdr-path=\"github/fallback.css\"]")
            .exists()
    );

    // Everything around the page is the test harness's own output.
    let rest = format!("{}{}", &stdout[..start], &stdout[end..]);
    assert!(
        !rest.contains("md-render") && !rest.contains("warning") && !rest.contains('<'),
        "stray stdout: {rest:?}"
    );
    // Diagnostics went to stderr.
    assert!(stderr.contains("md-render: warning"), "stderr: {stderr}");
    assert!(
        stderr.contains("md-render vendor --flavor github"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("<!DOCTYPE"), "page only on stdout");
}
