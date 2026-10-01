//! `md-render export` (owner: page). Contract: docs/DESIGN.md §3.9.
//!
//! Renders one document with the configured renderer and writes a standalone HTML page:
//! self-contained by default (all CSS, fonts, images and needed client libraries inlined), or
//! with a `<stem>_files/` directory next to the output (`--no-inline`). Diagnostics go to
//! stderr; with [`ExportOutput::Stdout`] the page is the only thing written to stdout.

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, bail};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use crate::flavor::{Flavor, Mode, ThemeSel};
use crate::page::{AssetMode, Layout, PageInput, render_page};
use crate::render::{RenderOptions, Renderer};

/// Export source.
#[derive(Clone, Debug)]
pub enum ExportInput {
    /// A Markdown file on disk.
    File(PathBuf),
    /// Text read from stdin; `name` is shown in the page header.
    Stdin { name: String, text: String },
}

/// Export destination.
#[derive(Clone, Debug)]
pub enum ExportOutput {
    /// Write this file (atomically: temp file + rename).
    File(PathBuf),
    /// Write the page to stdout.
    Stdout,
}

/// Options of one export.
#[derive(Clone)]
pub struct ExportConfig {
    /// Markdown source.
    pub input: ExportInput,
    /// Destination (the CLI decides the default file name).
    pub output: ExportOutput,
    /// Self-contained single file (default) vs. linked assets directory (`--no-inline`).
    pub inline: bool,
    /// Site to reproduce.
    pub flavor: Flavor,
    /// README/file view or comment rendering and chrome.
    pub mode: Mode,
    /// Colour mode and theme ids.
    pub theme: ThemeSel,
    /// Site-like box or content only.
    pub layout: Layout,
    /// `<title>`; defaults to the first `h1` of the document, then the file name.
    pub title: Option<String>,
    /// `owner/repo` or `group/project`, passed to the renderer.
    pub context: Option<String>,
    /// Style bundle; a missing or broken bundle falls back to the embedded CSS (with a warning).
    pub bundle_path: PathBuf,
    /// Offline or API renderer.
    pub renderer: Arc<dyn Renderer>,
    /// Suppress the progress message and style warnings on stderr.
    pub quiet: bool,
}

/// Characters escaped in the assets-directory href.
const HREF_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'\'');

/// Render `cfg.input` with `cfg.renderer` and write a standalone HTML page.
///
/// The renderer gets `sourcepos: false` and a `bundle_url` that the page resolves inside the
/// export (data URIs, or the `<stem>_files/` directory with `inline: false`). Styles come from
/// [`crate::styles::load_or_fallback`]; falling back is reported on stderr unless `quiet`.
/// A file is written atomically (temporary file + rename); with [`ExportOutput::Stdout`] the
/// page is the only output on stdout.
///
/// # Errors
/// Unreadable input, a renderer error, `inline: false` with stdout, or an I/O error while
/// writing the page or its assets directory.
pub fn export(cfg: &ExportConfig) -> anyhow::Result<()> {
    let (markdown, source_path, display_name) = read_input(cfg)?;

    let assets = match (&cfg.output, cfg.inline) {
        (_, true) => AssetMode::Inline,
        (ExportOutput::File(out), false) => assets_directory(out)?,
        (ExportOutput::Stdout, false) => bail!(
            "`export --no-inline` writes an assets directory next to the output file, so it needs an output \
             file instead of stdout"
        ),
    };

    let opts = RenderOptions {
        flavor: cfg.flavor,
        mode: cfg.mode,
        context: cfg.context.clone(),
        root: source_path.as_deref().map(|p| {
            p.parent()
                .filter(|d| !d.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf()
        }),
        source_path,
        sourcepos: false,
        // Inline: the default bundle route, rewritten to data URIs by the page. Directory:
        // references point straight into the copied assets directory.
        bundle_url: match &assets {
            AssetMode::Directory { href, .. } => href.clone(),
            _ => format!("{}/bundle", crate::PREFIX),
        },
    };
    let rendered = cfg.renderer.render(&markdown, &opts).with_context(|| {
        format!(
            "rendering {display_name} with the {} renderer",
            cfg.renderer.name()
        )
    })?;

    let (bundle, warning) = crate::styles::load_or_fallback(&cfg.bundle_path);
    if !cfg.quiet {
        let fallback = bundle.flavor(cfg.flavor).is_none_or(|m| m.fallback);
        match &warning {
            Some(w) => eprintln!("md-render: warning: {w}"),
            None if fallback => eprintln!(
                "md-render: warning: {} has no vendored {} styles; using minimal fallback styles",
                cfg.bundle_path.display(),
                cfg.flavor
            ),
            None => {}
        }
        if fallback {
            // DESIGN D11: always name the exact command.
            eprintln!(
                "md-render: hint: run `md-render vendor --flavor {}` to export with the site's own styles",
                cfg.flavor
            );
        }
    }

    let title = cfg
        .title
        .clone()
        .or_else(|| rendered.title.clone().filter(|t| !t.trim().is_empty()))
        .unwrap_or_else(|| {
            if display_name.is_empty() {
                "md-render".to_owned()
            } else {
                display_name.clone()
            }
        });
    let input = PageInput {
        flavor: cfg.flavor,
        mode: cfg.mode,
        theme: &cfg.theme,
        layout: cfg.layout,
        title: &title,
        file_name: (!display_name.is_empty()).then_some(display_name.as_str()),
        content: &rendered,
        assets: &assets,
        live: None,
        banners: &[],
        extra_css: &[],
    };
    let page = render_page(&input, &bundle)?;

    match &cfg.output {
        ExportOutput::File(path) => {
            if !cfg.quiet {
                eprintln!("Exporting to {}", path.display());
            }
            write_atomic(path, page.as_bytes())
        }
        ExportOutput::Stdout => write_stdout(page.as_bytes()),
    }
}

/// Markdown text, source path and display name of the input.
fn read_input(cfg: &ExportConfig) -> anyhow::Result<(String, Option<PathBuf>, String)> {
    match &cfg.input {
        ExportInput::Stdin { name, text } => Ok((strip_bom(text).to_owned(), None, name.clone())),
        ExportInput::File(path) => {
            let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            let text = match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(e) => {
                    if !cfg.quiet {
                        eprintln!(
                            "md-render: warning: {} is not valid UTF-8 (at byte {}); invalid bytes were replaced",
                            path.display(),
                            e.utf8_error().valid_up_to()
                        );
                    }
                    String::from_utf8_lossy(e.as_bytes()).into_owned()
                }
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((strip_bom(&text).to_owned(), Some(path.clone()), name))
        }
    }
}

fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// `--no-inline`: assets go to `<dir of out>/<stem>_files/`, linked relatively.
fn assets_directory(out: &Path) -> anyhow::Result<AssetMode> {
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("output path {} has no file name", out.display()))?;
    let name = format!("{stem}_files");
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(AssetMode::Directory {
        href: utf8_percent_encode(&name, HREF_SET).to_string(),
        dir: parent.join(name),
    })
}

/// Write `data` to `path` via a temporary file in the same directory and a rename, so readers
/// never see a partial file.
fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .with_context(|| format!("output path {} has no file name", path.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut last_err = None;
    for attempt in 0..16u32 {
        let tmp = dir.join(format!(
            ".{}.{}.{nanos:x}{attempt}.tmp",
            name.to_string_lossy(),
            std::process::id()
        ));
        let file = match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                last_err = Some(e);
                continue;
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("creating a temporary file in {}", dir.display()));
            }
        };
        let result = (|| -> io::Result<()> {
            let mut file = file;
            file.write_all(data)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, path)
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("writing {}", path.display()));
        }
        return Ok(());
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("no free temporary file name")))
        .with_context(|| format!("creating a temporary file in {}", dir.display()))
}

/// Write the page to stdout; a closed pipe (`md-render export - | head`) is not an error.
fn write_stdout(data: &[u8]) -> anyhow::Result<()> {
    let mut out = io::stdout().lock();
    match out.write_all(data).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e).context("writing to stdout"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_dir_naming() {
        let AssetMode::Directory { href, dir } =
            assets_directory(Path::new("out/My Doc.html")).unwrap()
        else {
            panic!("expected directory mode")
        };
        assert_eq!(href, "My%20Doc_files");
        assert_eq!(dir, Path::new("out/My Doc_files"));
        let AssetMode::Directory { dir, .. } = assets_directory(Path::new("x.html")).unwrap()
        else {
            panic!()
        };
        assert_eq!(dir, Path::new("./x_files"));
    }

    #[test]
    fn atomic_write_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.html");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(leftovers.len(), 1, "temporary files must not remain");
    }

    #[test]
    fn atomic_write_reports_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let err = write_atomic(&tmp.path().join("missing/a.html"), b"x").unwrap_err();
        assert!(format!("{err:#}").contains("temporary file"));
    }

    #[test]
    fn bom_is_stripped() {
        assert_eq!(strip_bom("\u{feff}# x"), "# x");
        assert_eq!(strip_bom("# x"), "# x");
    }
}
