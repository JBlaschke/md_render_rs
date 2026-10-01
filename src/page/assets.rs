//! Resolving bundle files and embedded static files for one page, per [`AssetMode`]:
//!
//! * `Server`: URLs under `<prefix>/bundle/…` and `<prefix>/static/…`;
//! * `Directory`: files are copied into `dir` (bundle paths preserved, so the relative `url()`s
//!   inside vendored CSS keep working, DESIGN D10) and linked through `href`;
//! * `Inline`: CSS is inlined with every `url()` turned into a `data:` URI, scripts are inlined.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, bail};
use base64::Engine as _;
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};

use super::AssetMode;
use super::css::{self, BUNDLE_ROUTE, RefKind, Target};
use super::html;
use crate::styles::Bundle;

/// Characters escaped when a bundle path is turned into a URL path (`/` is kept).
const PATH_SET: &AsciiSet =
    &CONTROLS.add(b' ').add(b'"').add(b'#').add(b'<').add(b'>').add(b'?').add(b'`').add(b'{').add(b'}').add(b'%').add(b'\'');

/// Images referenced from site CSS that are larger than this are not inlined. On both sites
/// these are UI-chrome banners and illustrations that markdown never uses (GH §4.3: dashboard
/// promos 53-303 KB; GL §11.2: blooms, icon sheets), and inlining them would triple the size of
/// an export. Fonts are always inlined.
pub(crate) const MAX_INLINE_IMAGE_BYTES: usize = 64 * 1024;

/// Maximum `@import` nesting followed when inlining or copying CSS.
const MAX_CSS_DEPTH: usize = 4;

/// Empty data URI substituted for references that cannot be inlined.
const EMPTY_DATA_URI: &str = "data:,";

/// Per-page asset resolver.
pub(crate) struct Assets<'a> {
    mode: &'a AssetMode,
    bundle: &'a Bundle,
    /// Directory mode: destination-relative paths already written.
    written: HashSet<String>,
    /// Inline mode: data URIs by bundle path (`None`: not inlinable).
    data_uris: HashMap<String, Option<Rc<str>>>,
}

/// Percent-encode a bundle path for use in a URL.
pub(crate) fn encode_path(path: &str) -> String {
    utf8_percent_encode(path, PATH_SET).to_string()
}

fn join_href(base: &str, rel: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.is_empty() { rel.to_owned() } else { format!("{base}/{rel}") }
}

/// A relative path from the bundle (or a static asset name) as a safe relative file path:
/// plain segments only, so writes can never leave the export directory.
fn safe_relative(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for seg in path.split('/') {
        if seg.is_empty()
            || seg == "."
            || seg == ".."
            || seg.contains(['\\', ':', '\0'])
        {
            return None;
        }
        out.push(seg);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn is_css(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("css"))
}

fn mime_of(path: &str) -> String {
    mime_guess::from_path(path).first_or_octet_stream().essence_str().to_owned()
}

fn is_font(mime: &str, path: &str) -> bool {
    mime.starts_with("font/")
        || mime.contains("font")
        || path.rsplit_once('.').is_some_and(|(_, e)| ["woff", "woff2", "ttf", "otf", "eot"].contains(&e.to_ascii_lowercase().as_str()))
}

/// For a legacy web-font file (`.woff`, `.ttf`, `.otf`, `.eot`), the bundle path of the `.woff2`
/// file with the same stem; `None` for anything else.
fn woff2_sibling(path: &str) -> Option<String> {
    let (stem, ext) = path.rsplit_once('.')?;
    let legacy = ["woff", "ttf", "otf", "eot"].iter().any(|e| ext.eq_ignore_ascii_case(e));
    (legacy && !stem.is_empty()).then(|| format!("{stem}.woff2"))
}

fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn write_file(dir: &Path, rel: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let Some(relpath) = safe_relative(rel) else {
        bail!("refusing to write unsafe asset path {rel:?}");
    };
    let dest = dir.join(relpath);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating directory {}", parent.display()))?;
    }
    fs::write(&dest, bytes).with_context(|| format!("writing {}", dest.display()))
}

impl<'a> Assets<'a> {
    pub(crate) fn new(mode: &'a AssetMode, bundle: &'a Bundle) -> Self {
        Assets { mode, bundle, written: HashSet::new(), data_uris: HashMap::new() }
    }

    pub(crate) fn is_inline(&self) -> bool {
        matches!(self.mode, AssetMode::Inline)
    }

    /// Does the bundle contain `path`?
    pub(crate) fn has(&self, path: &str) -> bool {
        self.bundle.read(path).is_some()
    }

    /// URL of a bundle file (Server / Directory; copies the file and, for CSS, everything it
    /// references in Directory mode). `None` in Inline mode or when the file is missing.
    pub(crate) fn bundle_url(&mut self, path: &str) -> anyhow::Result<Option<String>> {
        match self.mode {
            AssetMode::Server { prefix } => {
                Ok(self.has(path).then(|| format!("{}/bundle/{}", prefix.trim_end_matches('/'), encode_path(path))))
            }
            AssetMode::Directory { href, .. } => {
                if self.copy_bundle_file(path, 0)? {
                    Ok(Some(join_href(href, &encode_path(path))))
                } else {
                    Ok(None)
                }
            }
            AssetMode::Inline => Ok(None),
        }
    }

    /// URL of an embedded static file (`live.js`, `github.js`, `favicon.svg`, …);
    /// `None` in Inline mode or for unknown names.
    pub(crate) fn static_url(&mut self, name: &str) -> anyhow::Result<Option<String>> {
        let Some((bytes, _)) = crate::assets::get(name) else { return Ok(None) };
        match self.mode {
            AssetMode::Server { prefix } => Ok(Some(format!("{}/static/{}", prefix.trim_end_matches('/'), encode_path(name)))),
            AssetMode::Directory { href, dir } => {
                let rel = format!("static/{name}");
                if self.written.insert(rel.clone()) {
                    write_file(dir, &rel, bytes)?;
                }
                Ok(Some(join_href(href, &encode_path(&rel))))
            }
            AssetMode::Inline => Ok(None),
        }
    }

    /// Directory mode: copy `path` (and, for CSS, the files it references) into the export
    /// directory. Returns false when the bundle does not have the file.
    fn copy_bundle_file(&mut self, path: &str, depth: usize) -> anyhow::Result<bool> {
        let AssetMode::Directory { dir, .. } = self.mode else { return Ok(false) };
        if self.written.contains(path) {
            return Ok(true);
        }
        let Some(bytes) = self.bundle.read(path) else {
            tracing::warn!(path, "bundle file missing; not copied");
            return Ok(false);
        };
        write_file(dir, path, &bytes)?;
        self.written.insert(path.to_owned());
        if is_css(path) && depth < MAX_CSS_DEPTH {
            let text = String::from_utf8_lossy(&bytes);
            for (_, reference) in css::references(&text) {
                if let Target::Bundle { path: dep, .. } = css::resolve(path, &reference) {
                    self.copy_bundle_file(&dep, depth + 1)?;
                }
            }
        }
        Ok(true)
    }

    /// Inline mode: `data:` URI of a bundle file. CSS files are inlined recursively, images
    /// above [`MAX_INLINE_IMAGE_BYTES`] and missing files give `None`.
    pub(crate) fn data_uri(&mut self, path: &str) -> Option<Rc<str>> {
        self.data_uri_at(path, 0)
    }

    fn data_uri_at(&mut self, path: &str, depth: usize) -> Option<Rc<str>> {
        if let Some(cached) = self.data_uris.get(path) {
            return cached.clone();
        }
        let uri = self.compute_data_uri(path, depth).map(Rc::from);
        self.data_uris.insert(path.to_owned(), uri.clone());
        uri
    }

    fn compute_data_uri(&mut self, path: &str, depth: usize) -> Option<String> {
        let Some(bytes) = self.bundle.read(path) else {
            tracing::debug!(path, "referenced bundle file missing; dropped from export");
            return None;
        };
        if is_css(path) {
            if depth >= MAX_CSS_DEPTH {
                return None;
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let inlined = self.inline_css_at(path, &text, depth + 1);
            return Some(data_uri("text/css", inlined.as_bytes()));
        }
        let mime = mime_of(path);
        if mime.starts_with("image/") && bytes.len() > MAX_INLINE_IMAGE_BYTES && !is_font(&mime, path) {
            tracing::debug!(path, size = bytes.len(), "large CSS image not inlined");
            return None;
        }
        Some(data_uri(&mime, &bytes))
    }

    /// Inline mode: `css` (the text of bundle file `path`) with every reference replaced by a
    /// data URI; references that cannot be inlined become `data:,` so the export never points
    /// outside itself.
    pub(crate) fn inline_css(&mut self, path: &str, css: &str) -> String {
        self.inline_css_at(path, css, 0)
    }

    fn inline_css_at(&mut self, path: &str, text: &str, depth: usize) -> String {
        // Bundle files this stylesheet references, to spot `@font-face` source lists that offer
        // a woff2 next to legacy formats (KaTeX: woff2, woff, ttf for every face).
        let referenced: HashSet<String> = css::references(text)
            .into_iter()
            .filter_map(|(_, r)| match css::resolve(path, &r) {
                Target::Bundle { path, .. } => Some(path),
                _ => None,
            })
            .collect();
        css::rewrite(text, |kind, reference| match css::resolve(path, reference) {
            Target::Keep => None,
            Target::Bundle { path: dep, fragment } => {
                // Every browser that runs the client scripts supports woff2 and picks the first
                // usable `src`, so inlining the legacy twins would only bloat the export.
                if let Some(woff2) = woff2_sibling(&dep)
                    && referenced.contains(&woff2)
                    && self.has(&woff2)
                {
                    return Some(EMPTY_DATA_URI.to_owned());
                }
                let uri = self.data_uri_at(&dep, depth);
                Some(match (uri, fragment) {
                    (Some(uri), Some(f)) if kind == RefKind::Url => format!("{uri}#{f}"),
                    (Some(uri), _) => uri.to_string(),
                    (None, _) if kind == RefKind::ImportString => "data:text/css,".to_owned(),
                    (None, _) => EMPTY_DATA_URI.to_owned(),
                })
            }
            Target::Unresolved => Some(match kind {
                RefKind::ImportString => "data:text/css,".to_owned(),
                RefKind::Url => EMPTY_DATA_URI.to_owned(),
            }),
        })
        .into_owned()
    }

    /// Emit a bundle stylesheet: `<link>` (Server / Directory) or `<style>` (Inline).
    /// Missing files are skipped; returns whether anything was written. `attrs` are extra
    /// attributes (already escaped, leading space).
    pub(crate) fn stylesheet(&mut self, out: &mut String, path: &str, media: Option<&str>, attrs: &str) -> anyhow::Result<bool> {
        let media_attr = media.filter(|m| !m.is_empty()).map(|m| format!(" media=\"{}\"", html::attr(m))).unwrap_or_default();
        if self.is_inline() {
            let Some(bytes) = self.bundle.read(path) else {
                tracing::warn!(path, "stylesheet missing from bundle; skipped");
                return Ok(false);
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let inlined = self.inline_css(path, &text);
            out.push_str(&format!(
                "<style data-mdr-path=\"{}\"{media_attr}{attrs}>\n{}\n</style>\n",
                html::attr(path),
                html::inline_style(&inlined)
            ));
            Ok(true)
        } else if let Some(url) = self.bundle_url(path)? {
            out.push_str(&format!("<link rel=\"stylesheet\" href=\"{}\"{media_attr}{attrs}>\n", html::attr(&url)));
            Ok(true)
        } else {
            tracing::warn!(path, "stylesheet missing from bundle; skipped");
            Ok(false)
        }
    }

    /// Emit an embedded stylesheet from `crate::assets` (the fallback CSS when the bundle has
    /// no entry for a flavor at all).
    pub(crate) fn embedded_stylesheet(&mut self, out: &mut String, name: &str, attrs: &str) -> anyhow::Result<()> {
        if self.is_inline() {
            if let Some((bytes, _)) = crate::assets::get(name) {
                out.push_str(&format!(
                    "<style data-mdr-static=\"{}\"{attrs}>\n{}\n</style>\n",
                    html::attr(name),
                    html::inline_style(&String::from_utf8_lossy(bytes))
                ));
            }
        } else if let Some(url) = self.static_url(name)? {
            out.push_str(&format!("<link rel=\"stylesheet\" href=\"{}\"{attrs}>\n", html::attr(&url)));
        }
        Ok(())
    }

    /// Emit an embedded client script (`<script src>` or inline).
    pub(crate) fn static_script(&mut self, out: &mut String, name: &str) -> anyhow::Result<()> {
        if self.is_inline() {
            let Some((bytes, _)) = crate::assets::get(name) else { return Ok(()) };
            out.push_str(&format!(
                "<script data-mdr-static=\"{}\">\n{}\n</script>\n",
                html::attr(name),
                html::inline_script(&String::from_utf8_lossy(bytes))
            ));
        } else if let Some(url) = self.static_url(name)? {
            out.push_str(&format!("<script src=\"{}\"></script>\n", html::attr(&url)));
        }
        Ok(())
    }

    /// Emit the favicon link.
    pub(crate) fn favicon(&mut self, out: &mut String) -> anyhow::Result<()> {
        let href = if self.is_inline() {
            crate::assets::get("favicon.svg").map(|(bytes, mime)| data_uri(mime, bytes))
        } else {
            self.static_url("favicon.svg")?
        };
        if let Some(href) = href {
            out.push_str(&format!("<link rel=\"icon\" type=\"image/svg+xml\" href=\"{}\">\n", html::attr(&href)));
        }
        Ok(())
    }

    /// A client library script. Server / Directory: its URL. Inline: a non-executing holder
    /// `<script type="text/plain" id="mdr-lib-KEY">` is appended to `holders` and the
    /// reference `#mdr-lib-KEY` is returned; the flavor JS executes it on demand.
    pub(crate) fn lib_script(&mut self, holders: &mut String, key: &str, path: &str) -> anyhow::Result<Option<String>> {
        if !self.is_inline() {
            return self.bundle_url(path);
        }
        let Some(bytes) = self.bundle.read(path) else { return Ok(None) };
        holders.push_str(&format!(
            "<script type=\"text/plain\" id=\"mdr-lib-{key}\" data-mdr-path=\"{}\">\n{}\n</script>\n",
            html::attr(path),
            html::inline_script(&String::from_utf8_lossy(&bytes))
        ));
        Ok(Some(format!("#mdr-lib-{key}")))
    }

    /// A client library stylesheet (KaTeX CSS). Writes a `<link>` / `<style>` into `head` when
    /// `emit` is set and returns the reference for `mdr-config` (URL, or `#mdr-lib-KEY` inline).
    pub(crate) fn lib_css(&mut self, head: &mut String, key: &str, path: &str, emit: bool) -> anyhow::Result<Option<String>> {
        if self.is_inline() {
            if !emit {
                return Ok(None);
            }
            let Some(bytes) = self.bundle.read(path) else { return Ok(None) };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let inlined = self.inline_css(path, &text);
            head.push_str(&format!(
                "<style id=\"mdr-lib-{key}\" data-mdr-lib=\"{key}\" data-mdr-path=\"{}\">\n{}\n</style>\n",
                html::attr(path),
                html::inline_style(&inlined)
            ));
            return Ok(Some(format!("#mdr-lib-{key}")));
        }
        let url = self.bundle_url(path)?;
        if let (Some(url), true) = (&url, emit) {
            head.push_str(&format!("<link rel=\"stylesheet\" href=\"{}\" data-mdr-lib=\"{key}\">\n", html::attr(url)));
        }
        Ok(url)
    }

    /// Export modes: map a URL found in the rendered content. Bundle references
    /// (`/__/md-render/bundle/…`, or `<href>/…` in Directory mode) become data URIs (Inline;
    /// empty string when not inlinable) or copied files (Directory). `None`: keep as is.
    pub(crate) fn content_url(&mut self, url: &str) -> anyhow::Result<Option<String>> {
        let bundle_rel = url.strip_prefix(BUNDLE_ROUTE).and_then(|r| r.strip_prefix('/'));
        match self.mode {
            AssetMode::Server { .. } => Ok(None),
            AssetMode::Inline => {
                let Some(rel) = bundle_rel else { return Ok(None) };
                let path = percent_decode_str(rel.split(['?', '#']).next().unwrap_or("")).decode_utf8_lossy().into_owned();
                Ok(Some(css::normalize(&path).and_then(|p| self.data_uri(&p)).map(|u| u.to_string()).unwrap_or_default()))
            }
            AssetMode::Directory { href, .. } => {
                let href = href.trim_end_matches('/');
                let own_rel = if href.is_empty() { None } else { url.strip_prefix(href).and_then(|r| r.strip_prefix('/')) };
                let Some(rel) = bundle_rel.or(own_rel) else { return Ok(None) };
                let path = percent_decode_str(rel.split(['?', '#']).next().unwrap_or("")).decode_utf8_lossy().into_owned();
                let Some(path) = css::normalize(&path) else { return Ok(None) };
                match self.bundle_url(&path)? {
                    Some(new) => Ok(Some(new)),
                    None => Ok(None),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_paths() {
        assert_eq!(safe_relative("github/assets/x.css"), Some(PathBuf::from("github/assets/x.css")));
        for bad in ["", "/abs", "a/../b", "a/./b", "a//b", "c:/x", "a\\b"] {
            assert_eq!(safe_relative(bad), None, "{bad}");
        }
    }

    #[test]
    fn hrefs_and_encoding() {
        assert_eq!(join_href("x_files/", "a/b.css"), "x_files/a/b.css");
        assert_eq!(join_href("", "a/b.css"), "a/b.css");
        assert_eq!(encode_path("github/a b#c.css"), "github/a%20b%23c.css");
    }

    #[test]
    fn font_detection() {
        assert!(is_font(&mime_of("a/x.woff2"), "a/x.woff2"));
        assert!(is_font("application/octet-stream", "a/x.TTF"));
        assert!(!is_font(&mime_of("a/x.png"), "a/x.png"));
        assert_eq!(mime_of("a/x.svg"), "image/svg+xml");
    }
}
