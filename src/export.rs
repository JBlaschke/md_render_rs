//! `md-render export` (owner: page). Contract: docs/DESIGN.md §3.9.
//!
//! Renders one document with the configured renderer and writes a standalone HTML page: by
//! default the site's CSS (with its fonts and images), md-render's scripts and the client
//! libraries the document needs are inlined; with `--no-inline` they are copied into a
//! `<stem>_files/` directory next to the output. Images and links of the document itself are
//! not embedded: they stay relative references, rebased when the output is written to another
//! directory than the source (like grip, GRIP §2.6). Diagnostics go to stderr; with
//! [`ExportOutput::Stdout`] the page is the only thing written to stdout.

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};
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
    /// `<title>` and box header text (like grip's `--title`); defaults to the file name (as when
    /// serving), and the title to `md-render` without one.
    pub title: Option<String>,
    /// `owner/repo` or `group/project`, passed to the renderer.
    pub context: Option<String>,
    /// Style bundle; a missing or broken bundle falls back to the embedded CSS (with a warning).
    pub bundle_path: PathBuf,
    /// Extra stylesheets after the site CSS (`--css`, in order): URLs (`http(s)://`, `//`,
    /// `data:`) are linked; local CSS files are embedded in the page, with the files their
    /// relative `url()`s / `@import`s name as `data:` URIs, in both asset modes.
    pub extra_css: Vec<String>,
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

/// Path segments in a rebased document URL: [`HREF_SET`] plus `\\` (a separator for browsers).
const SEGMENT_SET: &AsciiSet = &HREF_SET.add(b'\\');

/// Render `cfg.input` with `cfg.renderer` and write a standalone HTML page.
///
/// The renderer gets `sourcepos: false` and the default `bundle_url` (`/__/md-render/bundle`),
/// which the page resolves inside the export (data URIs, or the `<stem>_files/` directory with
/// `inline: false`). Document-relative URLs (`img/a.png`) are rebased onto the source
/// directory when the output file is written elsewhere. Styles come from
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
        // The default bundle route: the page turns these references into data URIs or into
        // copies in the assets directory (and never mistakes a document URL for one).
        bundle_url: format!("{}/bundle", crate::PREFIX),
    };
    let mut rendered = cfg.renderer.render(&markdown, &opts).with_context(|| {
        format!(
            "rendering {display_name} with the {} renderer",
            cfg.renderer.name()
        )
    })?;
    if let Some(prefix) = source_prefix(opts.source_path.as_deref(), &cfg.output) {
        rendered.html = crate::page::content::rebase_relative_urls(&rendered.html, &prefix);
    }

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
        if let Some(w) = crate::page::theme_fallback_warning(cfg.flavor, &cfg.theme, &bundle) {
            eprintln!("md-render: warning: {w}");
        }
    }

    // The file name, as when serving and as `--help` says (grip: the file name in both).
    let title = cfg.title.clone().unwrap_or_else(|| {
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
        // grip's --title replaces the file name in the box header too (GRIP §2.4, C21).
        file_name: cfg
            .title
            .as_deref()
            .or((!display_name.is_empty()).then_some(display_name.as_str())),
        content: &rendered,
        assets: &assets,
        live: None,
        banners: &[],
        extra_css: &cfg.extra_css,
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

/// The directory of `file`, absolute and lexically normalized (`.`/`..` resolved without
/// following symlinks, as a browser resolves relative `file:` URLs).
fn lexical_dir(file: &Path) -> Option<PathBuf> {
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(file)
    };
    let mut out = PathBuf::new();
    for c in abs.parent()?.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// URL path from directory `from` to directory `to` (`../docs`; `""` when they are the same),
/// percent-encoded; `None` when there is no relative path (different drives on Windows).
fn relative_url_path(from: &Path, to: &Path) -> Option<String> {
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = to.components().collect();
    if from.first() != to.first() {
        return None;
    }
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut segs: Vec<String> = Vec::new();
    for c in &from[common..] {
        match c {
            Component::Normal(_) => segs.push("..".to_owned()),
            _ => return None,
        }
    }
    for c in &to[common..] {
        match c {
            Component::Normal(s) => {
                segs.push(utf8_percent_encode(s.to_str()?, SEGMENT_SET).to_string())
            }
            _ => return None,
        }
    }
    Some(segs.join("/"))
}

/// Prefix for the document-relative URLs of an export of `source` to `output`: the URL path
/// from the output directory to the source directory, when they differ. `None` for stdin and
/// stdout (relative to the current directory either way) and for the same directory.
fn source_prefix(source: Option<&Path>, output: &ExportOutput) -> Option<String> {
    let ExportOutput::File(out) = output else {
        return None;
    };
    let prefix = relative_url_path(&lexical_dir(out)?, &lexical_dir(source?)?)?;
    (!prefix.is_empty()).then_some(prefix)
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

/// The file that writing to `path` must replace: `path`, or the final target of a symlink
/// at `path` (relative links resolved against the link's directory), so an export through a
/// link updates the linked file and keeps the link, as writing to it in place would.
fn write_target(path: &Path) -> io::Result<PathBuf> {
    let mut target = path.to_path_buf();
    // Like the OS's own limit on link chains (ELOOP).
    for _ in 0..40 {
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = fs::read_link(&target)?;
                target = match target.parent() {
                    Some(dir) => dir.join(link),
                    None => link,
                };
            }
            _ => return Ok(target),
        }
    }
    Err(io::Error::other("too many levels of symbolic links"))
}

/// Write `data` to `path` via a temporary file in the same directory and a rename, so readers
/// never see a partial file. A symlink at `path` is followed (its target is replaced, the link
/// kept); an existing file keeps its permissions, and one we may not write is not replaced
/// (as with writing to it in place). An existing non-regular file (`/dev/stdout`, a FIFO) is
/// written to directly, never replaced.
fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let target =
        write_target(path).with_context(|| format!("resolving the output {}", path.display()))?;
    let existing = fs::metadata(&target).ok();
    if let Some(meta) = &existing {
        if meta.is_dir() {
            bail!("output {} is a directory", path.display());
        }
        if !meta.is_file() {
            return OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&target)
                .and_then(|mut f| f.write_all(data))
                .with_context(|| format!("writing {}", path.display()));
        }
        // Opening for writing (without truncating) checks the access the user has.
        if let Err(e) = OpenOptions::new().write(true).open(&target)
            && e.kind() == io::ErrorKind::PermissionDenied
        {
            bail!(
                "output {} is not writable; not replacing it",
                path.display()
            );
        }
    }
    let path = target.as_path();
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
            if let Some(meta) = &existing {
                file.set_permissions(kept_permissions(meta))?;
            }
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

/// The permissions of a replaced output file for its successor: the access bits (a private
/// 0600 export stays private), without set-id or sticky bits.
fn kept_permissions(meta: &fs::Metadata) -> fs::Permissions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::Permissions::from_mode(meta.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        meta.permissions()
    }
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

    /// CLI-20: an output that is a symlink is written through (the link stays a link, its
    /// target gets the page), and a replaced file keeps its permissions.
    #[cfg(unix)]
    #[test]
    fn atomic_write_follows_symlinks_and_keeps_permissions() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let tmp = tempfile::tempdir().unwrap();
        let www = tmp.path().join("www");
        fs::create_dir(&www).unwrap();
        let site = www.join("site.html");
        fs::write(&site, "old").unwrap();
        fs::set_permissions(&site, fs::Permissions::from_mode(0o600)).unwrap();
        // A relative link, and a link to that link.
        let link = tmp.path().join("doc.html");
        symlink("www/site.html", &link).unwrap();
        let link2 = tmp.path().join("again.html");
        symlink("doc.html", &link2).unwrap();

        write_atomic(&link, b"new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&site).unwrap(), b"new");
        let mode = fs::metadata(&site).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a private output stays private");
        write_atomic(&link2, b"newer").unwrap();
        assert!(
            fs::symlink_metadata(&link2)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&site).unwrap(), b"newer");
        let entries = |dir: &Path| fs::read_dir(dir).unwrap().count();
        assert_eq!(entries(&www), 1, "no temporary files left");

        // A dangling link creates its target.
        let dangling = tmp.path().join("new.html");
        symlink("www/created.html", &dangling).unwrap();
        write_atomic(&dangling, b"x").unwrap();
        assert_eq!(fs::read(www.join("created.html")).unwrap(), b"x");
        assert!(
            fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    /// CLI-20: a read-only output is not replaced (writing it in place would fail too).
    #[cfg(unix)]
    #[test]
    fn atomic_write_refuses_read_only_outputs() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let ro = tmp.path().join("ro.html");
        fs::write(&ro, "ro").unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o444)).unwrap();
        if OpenOptions::new().write(true).open(&ro).is_ok() {
            eprintln!("SKIPPED (running with permission to write read-only files)");
            return;
        }
        let err = write_atomic(&ro, b"new").unwrap_err();
        assert!(format!("{err:#}").contains("not writable"), "{err:#}");
        assert_eq!(fs::read(&ro).unwrap(), b"ro");
        let mode = fs::metadata(&ro).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o444);
        let err = write_atomic(tmp.path(), b"x").unwrap_err();
        assert!(format!("{err:#}").contains("is a directory"), "{err:#}");
    }

    #[test]
    fn atomic_write_reports_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let err = write_atomic(&tmp.path().join("missing/a.html"), b"x").unwrap_err();
        assert!(format!("{err:#}").contains("temporary file"));
    }

    #[test]
    fn relative_url_paths() {
        let p = |from: &str, to: &str| relative_url_path(Path::new(from), Path::new(to));
        assert_eq!(p("/a/out", "/a/docs").as_deref(), Some("../docs"));
        assert_eq!(p("/a", "/a/docs/sub").as_deref(), Some("docs/sub"));
        assert_eq!(p("/a/b/c", "/a").as_deref(), Some("../.."));
        assert_eq!(p("/a", "/a").as_deref(), Some(""));
        assert_eq!(p("/a/out", "/a/My Docs").as_deref(), Some("../My%20Docs"));
        assert_eq!(
            lexical_dir(Path::new("/a/./b/../c/x.html")),
            Some(PathBuf::from("/a/c"))
        );
    }

    #[test]
    fn source_prefixes() {
        let out = |p: &str| ExportOutput::File(PathBuf::from(p));
        let src = Path::new("/w/docs/e.md");
        assert_eq!(
            source_prefix(Some(src), &out("/w/out/e.html")).as_deref(),
            Some("../docs")
        );
        assert_eq!(source_prefix(Some(src), &out("/w/docs/e.html")), None);
        assert_eq!(source_prefix(Some(src), &ExportOutput::Stdout), None);
        assert_eq!(source_prefix(None, &out("/w/out/e.html")), None);
    }

    #[test]
    fn bom_is_stripped() {
        assert_eq!(strip_bom("\u{feff}# x"), "# x");
        assert_eq!(strip_bom("# x"), "# x");
    }
}
