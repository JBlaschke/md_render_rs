//! `md-render export` to files: self-contained (default) and `--no-inline` directory exports,
//! render options handed to the renderer, title / header naming, atomic writes, fallback
//! styles and error reporting (DESIGN §3.9).

mod page_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dom_query::Document;
use md_render::export::{ExportConfig, ExportInput, ExportOutput, export};
use md_render::page::Layout;
use md_render::render::{Features, OfflineRenderer, RenderOptions, Renderer};
use md_render::{Flavor, Mode, ThemeSel};
use page_support::*;

struct Setup {
    dir: tempfile::TempDir,
    bundle: PathBuf,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle_dir = dir.path().join("cache");
        std::fs::create_dir(&bundle_dir).unwrap();
        let bundle = gitlab(github(BundleBuilder::new())).write(&bundle_dir);
        Setup { dir, bundle }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn markdown(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.path(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, text).unwrap();
        p
    }

    fn config(
        &self,
        input: ExportInput,
        output: ExportOutput,
        renderer: Arc<dyn Renderer>,
    ) -> ExportConfig {
        ExportConfig {
            input,
            output,
            inline: true,
            flavor: Flavor::Github,
            mode: Mode::Readme,
            theme: ThemeSel::default(),
            layout: Layout::Readme,
            title: None,
            context: Some("owner/repo".into()),
            bundle_path: self.bundle.clone(),
            renderer,
            quiet: true,
        }
    }
}

fn fixed(flavor: Flavor, features: Features) -> Arc<FixedRenderer> {
    Arc::new(FixedRenderer::new(content(flavor, features)))
}

fn seen(r: &FixedRenderer) -> Vec<(String, RenderOptions)> {
    r.seen.lock().expect("lock").clone()
}

fn dir_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn inline_export_of_a_file() {
    let s = Setup::new();
    let md = s.markdown("docs/README.md", "\u{feff}# Title\n\nHi :octocat:\n");
    let out = s.path("out/readme.html");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let renderer = fixed(
        Flavor::Github,
        Features {
            math: true,
            mermaid: false,
        },
    );
    let cfg = s.config(
        ExportInput::File(md.clone()),
        ExportOutput::File(out.clone()),
        renderer.clone(),
    );
    export(&cfg).expect("export succeeds");

    // What the renderer was asked to do.
    let calls = seen(&renderer);
    assert_eq!(calls.len(), 1);
    let (text, opts) = &calls[0];
    assert_eq!(text, "# Title\n\nHi :octocat:\n", "BOM stripped");
    assert_eq!(opts.flavor, Flavor::Github);
    assert_eq!(opts.mode, Mode::Readme);
    assert!(!opts.sourcepos, "exports never carry data-sourcepos");
    assert_eq!(opts.bundle_url, "/__/md-render/bundle");
    assert_eq!(opts.source_path.as_deref(), Some(md.as_path()));
    assert_eq!(opts.root.as_deref(), md.parent());
    assert_eq!(opts.context.as_deref(), Some("owner/repo"));

    let html = std::fs::read_to_string(&out).unwrap();
    assert_self_contained(&html);
    let doc = Document::from(html.as_str());
    assert_eq!(
        doc.select("title").text().as_ref(),
        "Title",
        "first h1 is the default title"
    );
    assert_eq!(doc.select(".mdr-box-title").text().trim(), "README.md");
    assert!(!html.contains("data-sourcepos"));
    assert!(
        attr(&doc, "img.emoji", "src").is_some_and(|s| s.starts_with("data:image/png;base64,"))
    );
    assert!(doc.select("script#mdr-lib-mathjax").exists());
    assert!(config(&doc)["live"].is_null());
    assert_eq!(
        dir_entries(out.parent().unwrap()),
        ["readme.html"],
        "no temporary files left"
    );
}

#[test]
fn explicit_title_replaces_existing_output() {
    let s = Setup::new();
    let md = s.markdown("a.md", "x");
    let out = s.path("a.html");
    std::fs::write(&out, "old content").unwrap();
    let mut cfg = s.config(
        ExportInput::File(md),
        ExportOutput::File(out.clone()),
        fixed(Flavor::Github, Features::default()),
    );
    cfg.title = Some("My <Title>".into());
    cfg.theme = ThemeSel {
        theme: md_render::Theme::Dark,
        light: "light".into(),
        dark: "dark_dimmed".into(),
    };
    export(&cfg).unwrap();
    let html = std::fs::read_to_string(&out).unwrap();
    let doc = Document::from(html.as_str());
    assert_eq!(doc.select("title").text().as_ref(), "My <Title>");
    assert_eq!(
        attr(&doc, "html", "data-color-mode").as_deref(),
        Some("dark")
    );
    assert_eq!(
        attr(&doc, "html", "data-dark-theme").as_deref(),
        Some("dark_dimmed")
    );
    assert!(
        doc.select("style[data-mdr-path=\"github/assets/dark_dimmed-d1d1.css\"]")
            .exists()
    );
}

#[test]
fn stdin_input_uses_its_name() {
    let s = Setup::new();
    let out = s.path("stdin.html");
    let renderer = fixed(Flavor::Gitlab, Features::default());
    let mut cfg = s.config(
        ExportInput::Stdin {
            name: "notes.md".into(),
            text: "# Notes\n".into(),
        },
        ExportOutput::File(out.clone()),
        renderer.clone(),
    );
    cfg.flavor = Flavor::Gitlab;
    cfg.mode = Mode::Comment;
    export(&cfg).unwrap();
    let (text, opts) = seen(&renderer).remove(0);
    assert_eq!(text, "# Notes\n");
    assert!(opts.source_path.is_none() && opts.root.is_none());
    assert_eq!(opts.flavor, Flavor::Gitlab);
    assert_eq!(opts.mode, Mode::Comment);
    let html = std::fs::read_to_string(&out).unwrap();
    assert_self_contained(&html);
    let doc = Document::from(html.as_str());
    assert_eq!(
        doc.select(".mdr-note-header strong").text().as_ref(),
        "notes.md"
    );
    assert!(doc.select("#md-content.note-text.md").exists());
}

#[test]
fn directory_export() {
    let s = Setup::new();
    let md = s.markdown("My Doc.md", "# Title\n");
    let out = s.path("site/My Doc.html");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let renderer = fixed(
        Flavor::Github,
        Features {
            math: false,
            mermaid: true,
        },
    );
    let mut cfg = s.config(
        ExportInput::File(md),
        ExportOutput::File(out.clone()),
        renderer.clone(),
    );
    cfg.inline = false;
    export(&cfg).unwrap();

    let (_, opts) = seen(&renderer).remove(0);
    assert_eq!(
        opts.bundle_url, "My%20Doc_files",
        "bundle references point into the assets directory"
    );
    let files = s.path("site/My Doc_files");
    assert_eq!(
        dir_entries(out.parent().unwrap()),
        ["My Doc.html", "My Doc_files"]
    );
    let html = std::fs::read_to_string(&out).unwrap();
    let doc = Document::from(html.as_str());
    let hrefs = stylesheet_hrefs(&doc);
    assert!(
        hrefs
            .iter()
            .all(|h| h.starts_with("My%20Doc_files/github/assets/")),
        "{hrefs:?}"
    );
    for h in hrefs.iter().chain(script_srcs(&doc).iter()) {
        let rel = percent_encoding::percent_decode_str(h)
            .decode_utf8_lossy()
            .into_owned();
        assert!(out.parent().unwrap().join(&rel).is_file(), "{h}");
    }
    assert!(files.join("github/emoji/octocat.png").is_file());
    assert!(files.join("github/lib/mermaid/mermaid.min.js").is_file());
    assert!(
        !files.join("github/lib/mathjax").exists(),
        "only the libraries the document needs"
    );
    assert_eq!(
        attr(&doc, "img.emoji", "src").as_deref(),
        Some("My%20Doc_files/github/emoji/octocat.png")
    );
    assert!(!html.contains("/__/md-render/"));
    assert!(!html.contains("data-sourcepos"));
}

#[test]
fn missing_bundle_uses_fallback_styles() {
    let s = Setup::new();
    let md = s.markdown("a.md", "x");
    let out = s.path("a.html");
    let mut cfg = s.config(
        ExportInput::File(md),
        ExportOutput::File(out.clone()),
        fixed(Flavor::Github, Features::default()),
    );
    cfg.bundle_path = s.path("does-not-exist.tar.gz");
    export(&cfg).unwrap();
    let html = std::fs::read_to_string(&out).unwrap();
    assert_self_contained(&html);
    let doc = Document::from(html.as_str());
    assert!(
        doc.select("style[data-mdr-path=\"github/fallback.css\"]")
            .exists()
    );
}

#[test]
fn errors_have_context() {
    let s = Setup::new();
    let renderer = fixed(Flavor::Github, Features::default());

    let missing = s.path("missing.md");
    let cfg = s.config(
        ExportInput::File(missing.clone()),
        ExportOutput::File(s.path("m.html")),
        renderer.clone(),
    );
    let err = format!("{:#}", export(&cfg).unwrap_err());
    assert!(err.contains("missing.md"), "{err}");

    let md = s.markdown("a.md", "x");
    let mut cfg = s.config(
        ExportInput::File(md.clone()),
        ExportOutput::Stdout,
        renderer.clone(),
    );
    cfg.inline = false;
    let err = format!("{:#}", export(&cfg).unwrap_err());
    assert!(err.contains("--no-inline"), "{err}");

    let cfg = s.config(
        ExportInput::File(md.clone()),
        ExportOutput::File(s.path("no/such/dir/a.html")),
        renderer.clone(),
    );
    let err = format!("{:#}", export(&cfg).unwrap_err());
    assert!(
        err.contains("no/such/dir") || err.contains("no\\such\\dir"),
        "{err}"
    );

    let mut failing = FixedRenderer::new(content(Flavor::Github, Features::default()));
    failing.fail = true;
    let cfg = s.config(
        ExportInput::File(md),
        ExportOutput::File(s.path("f.html")),
        Arc::new(failing),
    );
    let err = format!("{:#}", export(&cfg).unwrap_err());
    assert!(
        err.contains("a.md") && err.contains("synthetic renderer failure"),
        "{err}"
    );
    assert!(!s.path("f.html").exists());
}

#[test]
fn invalid_utf8_is_replaced() {
    let s = Setup::new();
    let md = s.path("bad.md");
    std::fs::write(&md, b"# ok \xff\xfe end\n").unwrap();
    let renderer = fixed(Flavor::Github, Features::default());
    let cfg = s.config(
        ExportInput::File(md),
        ExportOutput::File(s.path("bad.html")),
        renderer.clone(),
    );
    export(&cfg).unwrap();
    assert_eq!(seen(&renderer)[0].0, "# ok \u{fffd}\u{fffd} end\n");
}

#[test]
fn offline_renderer_end_to_end() {
    // Whatever the offline renderers emit, the export stays self-contained.
    let s = Setup::new();
    let md = s.markdown(
        "doc.md",
        "# Heading\n\nSome *text* with `code`.\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
    );
    for flavor in [Flavor::Github, Flavor::Gitlab] {
        let out = s.path(&format!("doc-{flavor}.html"));
        let mut cfg = s.config(
            ExportInput::File(md.clone()),
            ExportOutput::File(out.clone()),
            Arc::new(OfflineRenderer),
        );
        cfg.flavor = flavor;
        export(&cfg).unwrap();
        let html = std::fs::read_to_string(&out).unwrap();
        assert_self_contained(&html);
        assert!(!html.contains("data-sourcepos"), "{flavor}");
        let doc = Document::from(html.as_str());
        assert!(
            doc.select("#md-content")
                .text()
                .contains("Some text with code."),
            "{flavor}"
        );
    }
}
