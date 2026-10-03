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
            extra_css: Vec::new(),
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
    // CLI-9: the default title is the file name, as when serving (and as `--help` and grip
    // have it), not the document's first h1.
    assert_eq!(
        doc.select("title").text().as_ref(),
        "README.md",
        "the file name is the default title"
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
    // CLI-24: like grip's --title, it replaces the file name in the box header too.
    assert_eq!(doc.select(".mdr-box-title").text().trim(), "My <Title>");
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
        opts.bundle_url, "/__/md-render/bundle",
        "the page maps bundle references into the assets directory"
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

/// EXP-1: an export written to another directory than its source rebases the document's
/// relative image and link URLs onto the source directory; absolute URLs, fragments and
/// exports next to the source keep them as written. (Stdout exports never rebase; see the
/// unit tests in src/export.rs.)
#[test]
fn relative_document_urls_follow_the_output_directory() {
    let s = Setup::new();
    let md = s.markdown(
        "docs/e.md",
        "# E\n\n![local](img/pic.png) ![remote](https://example.com/r.png)\n\n\
         [sibling](other.md#part) [top](#e) [mail](mailto:a@example.com)\n\n\
         <picture><source srcset=\"img/b.png 1x, img/b@2x.png 2x\"><img src=\"img/b.png\" alt=\"b\"></picture>\n",
    );
    std::fs::create_dir_all(s.path("docs/img")).unwrap();
    std::fs::write(s.path("docs/img/pic.png"), PNG).unwrap();
    std::fs::create_dir_all(s.path("out")).unwrap();
    for flavor in [Flavor::Github, Flavor::Gitlab] {
        for (out, prefix) in [
            (s.path(&format!("out/{flavor}.html")), "../docs/"),
            (s.path(&format!("docs/{flavor}.html")), ""),
        ] {
            for inline in [true, false] {
                let mut cfg = s.config(
                    ExportInput::File(md.clone()),
                    ExportOutput::File(out.clone()),
                    Arc::new(OfflineRenderer),
                );
                cfg.flavor = flavor;
                cfg.inline = inline;
                export(&cfg).unwrap();
                let html = std::fs::read_to_string(&out).unwrap();
                let doc = Document::from(html.as_str());
                let ctx = format!("{flavor} -> {} (inline: {inline})", out.display());
                let local = attr(&doc, "#md-content img[alt=local]", "src");
                assert_eq!(local, Some(format!("{prefix}img/pic.png")), "{ctx}");
                assert!(
                    out.parent().unwrap().join(local.unwrap()).is_file(),
                    "{ctx}: the image resolves"
                );
                assert_eq!(
                    attr(&doc, "#md-content img[alt=remote]", "src").as_deref(),
                    Some("https://example.com/r.png"),
                    "{ctx}"
                );
                let hrefs: Vec<String> = doc
                    .select("#md-content a")
                    .nodes()
                    .iter()
                    .filter_map(|n| n.attr("href").map(|h| h.to_string()))
                    .collect();
                assert!(
                    hrefs.contains(&format!("{prefix}other.md#part")),
                    "{ctx}: {hrefs:?}"
                );
                assert!(hrefs.iter().any(|h| h == "mailto:a@example.com"), "{ctx}");
                assert!(hrefs.iter().any(|h| h == "#e"), "{ctx}: {hrefs:?}");
                assert_eq!(
                    attr(&doc, "#md-content img[alt=b]", "src"),
                    Some(format!("{prefix}img/b.png")),
                    "{ctx}"
                );
                // (GitLab's sanitizer drops <source srcset>.)
                if flavor == Flavor::Github {
                    assert_eq!(
                        attr(&doc, "#md-content source", "srcset"),
                        Some(format!("{prefix}img/b.png 1x, {prefix}img/b@2x.png 2x")),
                        "{ctx}"
                    );
                }
            }
        }
    }
}

/// CLI-12: `--css` applies to exports. A local file is embedded after the site CSS (with the
/// files it references as data: URIs, so the export does not depend on the file staying where
/// it is), in both asset modes; a URL is linked.
#[test]
fn extra_css_is_embedded_or_linked() {
    let s = Setup::new();
    let md = s.markdown("doc.md", "# Doc\n\nText.\n");
    std::fs::create_dir_all(s.path("css/img")).unwrap();
    let css_path = s.path("css/extra.css");
    std::fs::write(
        &css_path,
        "@import \"more.css\";\n.markdown-body{color:rgb(1,2,3)}\n\
         h1{background:url(img/bg.gif)} h2{background:url(https://example.com/x.png)}\n\
         h3{background:url(img/missing.png)} a::after{content:\"url(img/bg.gif)\"}",
    )
    .unwrap();
    std::fs::write(s.path("css/more.css"), "p{color:rgb(4,5,6)}").unwrap();
    std::fs::write(s.path("css/img/bg.gif"), GIF).unwrap();
    std::fs::create_dir_all(s.path("out")).unwrap();
    let remote = "https://example.com/theme.css".to_owned();
    for inline in [true, false] {
        let out = s.path(&format!("out/doc-{inline}.html"));
        let mut cfg = s.config(
            ExportInput::File(md.clone()),
            ExportOutput::File(out.clone()),
            Arc::new(OfflineRenderer),
        );
        cfg.inline = inline;
        cfg.extra_css = vec![css_path.to_string_lossy().into_owned(), remote.clone()];
        export(&cfg).unwrap();
        let html = std::fs::read_to_string(&out).unwrap();
        let doc = Document::from(html.as_str());
        let ctx = format!("inline: {inline}");

        let style = doc.select("style[data-mdr-style=extra]");
        assert_eq!(style.length(), 1, "{ctx}");
        assert_eq!(
            style.attr("data-mdr-file").as_deref(),
            Some("extra.css"),
            "{ctx}"
        );
        let css = style.text().to_string();
        assert!(css.contains(".markdown-body{color:rgb(1,2,3)}"), "{ctx}");
        let urls = css_urls(&css);
        assert_eq!(urls.len(), 3, "{ctx}: {css}");
        assert!(urls[0].starts_with("data:image/gif;base64,"), "{ctx}");
        assert_eq!(urls[1], "https://example.com/x.png", "{ctx}");
        assert_eq!(
            urls[2], "img/missing.png",
            "{ctx}: missing files stay as written"
        );
        assert!(
            css.contains("content:\"url(img/bg.gif)\""),
            "{ctx}: strings are not references"
        );
        let import = css
            .split("@import \"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap();
        assert!(
            import.starts_with("data:text/css;base64,"),
            "{ctx}: {import}"
        );
        assert!(
            !html.contains(&*s.path("css").to_string_lossy()),
            "{ctx}: no local path in the export"
        );

        let links: Vec<String> = doc
            .select("link[data-mdr-style=extra]")
            .nodes()
            .iter()
            .filter_map(|n| n.attr("href").map(|h| h.to_string()))
            .collect();
        assert_eq!(links, std::slice::from_ref(&remote), "{ctx}");
        // After the site CSS: the extra stylesheets come last in <head>.
        let head = doc.select("head").html().to_string();
        let site = head.rfind("data-mdr-style=\"bundle\"").expect("site CSS");
        assert!(
            head.find("data-mdr-style=\"extra\"").unwrap() > site,
            "{ctx}"
        );
    }
}

/// PAGE-1: an export whose GitHub theme is not vendored says so on stderr (with the vendor
/// command) instead of silently looking like the default dark theme; `-q` silences it.
#[test]
fn missing_theme_is_reported_on_stderr() {
    let s = Setup::new();
    let md = s.markdown("doc.md", "# Doc\n");
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_md-render"))
            .arg("export")
            .arg("--styles")
            .arg(&s.bundle)
            .args(args)
            .arg(&md)
            .arg(s.path("doc.html"))
            .output()
            .expect("md-render runs");
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let stderr = run(&["--theme", "dark_high_contrast"]);
    // The command names the --styles bundle (`-o FILE`), or it would vendor the default one.
    let cmd = md_render::styles::vendor_command(Some(Flavor::Github), &s.bundle);
    assert!(cmd.contains(" -o "), "{cmd}");
    assert!(
        stderr.contains(
            "warning: the style bundle has no GitHub theme dark_high_contrast; using dark instead"
        ) && stderr.contains(&format!("(run `{cmd} --themes all` to vendor every theme)")),
        "{stderr}"
    );
    let html = std::fs::read_to_string(s.path("doc.html")).unwrap();
    assert!(html.contains("data-dark-theme=\"dark\""));
    // Vendored (the test bundle has dark_dimmed), not needed by the mode, or quiet: silent.
    for args in [
        &["--theme", "dark_dimmed"][..],
        &["--theme", "light", "--dark-theme", "dark_high_contrast"],
        &["-q", "--theme", "dark_high_contrast"],
    ] {
        let stderr = run(args);
        assert!(!stderr.contains("GitHub theme"), "{args:?}: {stderr}");
    }
}
