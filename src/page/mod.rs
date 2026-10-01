//! Full-page HTML per flavor / mode / theme / layout (owner: page). Contract: docs/DESIGN.md
//! §3.6, §5, §7.
//!
//! A page is the site's chrome (GitHub: GH §3, §9.A/§9.B; GitLab: GL §11.3, §12) around the
//! rendered content (`#md-content`), the vendored stylesheets in cascade order (computed from
//! the bundle manifest roles, never from hashed file names), a JSON configuration block for the
//! client scripts (`<script id="mdr-config" type="application/json">`), the flavor client
//! script (`github.js` / `gitlab.js`) and, when live, `live.js`.
//!
//! Asset references depend on [`AssetMode`]: server URLs, a copied assets directory, or a fully
//! self-contained document (CSS inlined with `data:` URIs, libraries inlined when needed).

use std::path::PathBuf;
use std::sync::OnceLock;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use regex::Regex;

use crate::flavor::{Flavor, Mode, ThemeSel};
use crate::render::Rendered;
use crate::styles::Bundle;

mod assets;
mod content;
mod css;
mod github;
mod gitlab;
mod html;
mod octicons;
mod sheets;

use assets::Assets;

/// Page chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Layout {
    /// Site-like box with the file name header.
    #[default]
    Readme,
    /// Content only.
    Bare,
}

impl Layout {
    /// `"readme"` | `"bare"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Readme => "readme",
            Layout::Bare => "bare",
        }
    }
}

/// How bundle files and client scripts are referenced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetMode {
    /// Live server: `<prefix>/bundle/...`, `<prefix>/static/...` (prefix is normally `/__/md-render`).
    Server { prefix: String },
    /// Self-contained export: CSS inlined, `url()` -> data URIs, libraries inlined when needed.
    Inline,
    /// Export with `--no-inline`: files copied into `dir`, referenced through `href`.
    Directory { href: String, dir: PathBuf },
}

/// Live-update wiring for a served page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveInfo {
    /// SSE endpoint, e.g. `/__/md-render/events?path=README.md`.
    pub events_url: String,
    /// Resync endpoint, e.g. `/__/md-render/render?path=README.md`.
    pub render_url: String,
    /// Document sub-path relative to the served root.
    pub doc: String,
    /// Version of the content this page was rendered with.
    pub version: u64,
    /// Random id of the server process that rendered the page.
    pub server_id: String,
}

/// Severity of a [`Banner`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerKind {
    Error,
    Warning,
    Info,
}

/// A message shown above the content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub kind: BannerKind,
    /// Plain text (escaped when rendered).
    pub message: String,
}

/// Everything needed to build one page.
pub struct PageInput<'a> {
    pub flavor: Flavor,
    pub mode: Mode,
    pub theme: &'a ThemeSel,
    pub layout: Layout,
    /// `<title>` text (plain text).
    pub title: &'a str,
    /// Box header text (readme layout); `None` omits the header.
    pub file_name: Option<&'a str>,
    /// Rendered document; `content.html` is trusted HTML placed inside `#md-content`, and
    /// `content.warnings` are shown as warning banners (after `banners`, deduplicated).
    pub content: &'a Rendered,
    pub assets: &'a AssetMode,
    /// Live-update wiring; only used with [`AssetMode::Server`].
    pub live: Option<&'a LiveInfo>,
    pub banners: &'a [Banner],
    /// Extra stylesheet URLs (after the bundle CSS).
    pub extra_css: &'a [String],
}

/// Client library keys (manifest `scripts`) a flavor uses for math and for mermaid.
fn lib_keys(flavor: Flavor) -> (&'static [&'static str], &'static [&'static str]) {
    match flavor {
        // MathJax 3.2.0 tex-chtml-full + ui/safe (CQ §1.4); mermaid 11.17.2 (GH §8.7).
        Flavor::Github => (&["mathjax", "mathjax-safe"], &["mermaid"]),
        // KaTeX 0.16.47 + its CSS (GL §10.2); mermaid 11.16.1 (GL §10.3).
        Flavor::Gitlab => (&["katex", "katex-css"], &["mermaid"]),
    }
}

/// GitLab's copy-button icon sprite (`/assets/icons-<hash>.svg`, GL §10.4), if vendored.
fn gitlab_icon_sprite(bundle: &Bundle) -> Option<&str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^gitlab/(?:.*/)?icons-[0-9a-f]+\.svg$").expect("valid regex"));
    bundle.flavor(Flavor::Gitlab)?.assets.iter().map(|a| a.path.as_str()).find(|p| re.is_match(p))
}

/// Build a complete HTML document.
///
/// Fails only on I/O errors while copying files in [`AssetMode::Directory`].
pub fn render_page(input: &PageInput, bundle: &Bundle) -> anyhow::Result<String> {
    let mut assets = Assets::new(input.assets, bundle);
    let manifest = bundle.flavor(input.flavor);
    let selection = manifest.map(|m| match input.flavor {
        Flavor::Github => sheets::github(m, input.theme),
        Flavor::Gitlab => sheets::gitlab(m, input.theme),
    });
    let (light, dark) = match &selection {
        Some(s) => (s.light.clone(), s.dark.clone()),
        None => (input.theme.light.clone(), input.theme.dark.clone()),
    };
    let live = match input.assets {
        AssetMode::Server { .. } => input.live,
        _ => None,
    };

    // Content: exports drop data-sourcepos and point bundle references into the export.
    let content_html = match input.assets {
        AssetMode::Server { .. } => input.content.html.clone(),
        AssetMode::Inline => content::for_export(&input.content.html, &mut assets, None)?,
        AssetMode::Directory { href, .. } => content::for_export(&input.content.html, &mut assets, Some(href))?,
    };

    // Banners: explicit ones first, then renderer warnings not already shown.
    let mut banners: Vec<Banner> = input.banners.to_vec();
    for w in &input.content.warnings {
        if !banners.iter().any(|b| &b.message == w) {
            banners.push(Banner { kind: BannerKind::Warning, message: w.clone() });
        }
    }
    let banners_html: String = banners
        .iter()
        .map(|b| match input.flavor {
            Flavor::Github => github::banner(b),
            Flavor::Gitlab => gitlab::banner(b),
        })
        .collect();

    // ---- <head>
    let mut head = String::with_capacity(4096);
    head.push_str("<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    if input.flavor == Flavor::Gitlab {
        head.push_str(&gitlab::head_pre(input.theme.theme));
    }
    head.push_str(&format!("<title>{}</title>\n", html::text(input.title)));
    assets.favicon(&mut head)?;
    // `data-mdr-style="bundle"` marks the links live.js re-requests on a `styles` event.
    const BUNDLE_ATTR: &str = " data-mdr-style=\"bundle\"";
    let mut linked = 0usize;
    for s in selection.iter().flat_map(|sel| &sel.sheets) {
        if assets.stylesheet(&mut head, s.path, s.media.as_deref(), BUNDLE_ATTR)? {
            linked += 1;
        }
    }
    if linked == 0 {
        // The flavor is missing from the bundle, or its manifest lists no readable stylesheet:
        // the embedded fallback keeps the page readable (DESIGN D11).
        assets.embedded_stylesheet(&mut head, &format!("fallback-{}.css", input.flavor), BUNDLE_ATTR)?;
    }

    // Client libraries: only what the content needs; a live page may gain math or diagrams
    // later, so it lists everything vendored (the flavor JS loads libraries lazily).
    let (math_keys, mermaid_keys) = lib_keys(input.flavor);
    let mut wanted: Vec<&str> = Vec::new();
    if input.content.features.math || live.is_some() {
        wanted.extend(math_keys);
    }
    if input.content.features.mermaid || live.is_some() {
        wanted.extend(mermaid_keys);
    }
    let mut scripts = serde_json::Map::new();
    let mut holders = String::new();
    if let Some(m) = manifest {
        for key in wanted {
            let Some(path) = m.scripts.get(key) else { continue };
            let reference = if key.ends_with("-css") {
                assets.lib_css(&mut head, key, path, input.content.features.math)?
            } else {
                assets.lib_script(&mut holders, key, path)?
            };
            if let Some(r) = reference {
                scripts.insert(key.to_owned(), serde_json::Value::String(r));
            }
        }
    }

    let chrome_css = match input.flavor {
        Flavor::Github => github::CHROME_CSS,
        Flavor::Gitlab => gitlab::CHROME_CSS,
    };
    head.push_str(&format!("<style data-mdr-style=\"chrome\">\n{chrome_css}</style>\n"));
    for url in input.extra_css {
        head.push_str(&format!("<link rel=\"stylesheet\" href=\"{}\" data-mdr-style=\"extra\">\n", html::attr(url)));
    }

    // ---- <html>
    let html_open = match input.flavor {
        Flavor::Github => github::html_open(input.theme.theme, &light, &dark),
        Flavor::Gitlab => {
            let sprite = match gitlab_icon_sprite(bundle) {
                Some(path) if !assets.is_inline() => assets.bundle_url(path)?,
                _ => None,
            };
            gitlab::html_open(input.theme.theme, sprite.as_deref())
        }
    };

    // ---- <body>
    let root_attrs = live.map(|l| format!(" data-version=\"{}\"", l.version)).unwrap_or_default();
    let chrome = match input.flavor {
        Flavor::Github => github::body(input.mode, input.layout, input.file_name, &content_html, &banners_html, &root_attrs),
        Flavor::Gitlab => gitlab::body(input.mode, input.layout, input.file_name, &content_html, &banners_html, &root_attrs),
    };

    let config = serde_json::json!({
        "flavor": input.flavor.as_str(),
        "mode": input.mode.as_str(),
        "layout": input.layout.as_str(),
        "theme": input.theme.theme.as_str(),
        "themes": { "light": light, "dark": dark },
        "scripts": scripts,
        "live": live.map(|l| serde_json::json!({
            "events_url": l.events_url,
            "render_url": l.render_url,
            "doc": l.doc,
            "version": l.version,
            "server_id": l.server_id,
        })),
    });

    let mut out = String::with_capacity(head.len() + chrome.len() + holders.len() + 2048);
    out.push_str("<!DOCTYPE html>\n");
    out.push_str(&html_open);
    out.push_str("<head>\n");
    out.push_str(&head);
    out.push_str("</head>\n");
    out.push_str(match input.flavor {
        Flavor::Github => "<body>\n",
        Flavor::Gitlab => gitlab::BODY_OPEN,
    });
    out.push_str(&chrome);
    if input.flavor == Flavor::Github {
        out.push_str(&octicons::clipboard_template());
        out.push('\n');
    }
    out.push_str(&format!(
        "<script id=\"mdr-config\" type=\"application/json\">{}</script>\n",
        html::json_for_script(&config)
    ));
    out.push_str(&holders);
    // live.js first: it records the pristine top-level nodes before the flavor script mutates
    // them (math, diagrams, copy buttons), DESIGN §7.
    if live.is_some() {
        assets.static_script(&mut out, "live.js")?;
    }
    assets.static_script(&mut out, &format!("{}.js", input.flavor))?;
    out.push_str("</body>\n</html>\n");
    Ok(out)
}

/// One row of a directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListingEntry {
    /// Display name (plain text).
    pub name: String,
    /// Link target (already URL-encoded by the caller).
    pub href: String,
    pub is_dir: bool,
    /// File size in bytes (shown for files).
    pub size: Option<u64>,
}

/// Content fragment for a directory listing, styled with the flavor's site classes
/// (GitHub `.Box` / `.Box-row`; GitLab tree rows). `dir` is the directory path relative to
/// the served root (`""` or `"/"` for the root); it is shown as linked breadcrumbs.
pub fn listing_html(flavor: Flavor, dir: &str, entries: &[ListingEntry]) -> String {
    match flavor {
        Flavor::Github => github::listing(dir, entries),
        Flavor::Gitlab => gitlab::listing(dir, entries),
    }
}

/// Content fragment for an error page (GitHub `.flash-error`, GitLab `.gl-alert-danger`).
pub fn error_html(flavor: Flavor, status: u16, message: &str) -> String {
    match flavor {
        Flavor::Github => github::error(status, message),
        Flavor::Gitlab => gitlab::error(status, message),
    }
}

/// URL path segment escape set for breadcrumbs.
const SEGMENT_SET: &AsciiSet =
    &CONTROLS.add(b' ').add(b'"').add(b'#').add(b'%').add(b'<').add(b'>').add(b'?').add(b'`').add(b'{').add(b'}').add(b'\'');

/// `/` + one link per path segment of `dir`, each pointing at that directory.
fn crumbs_html(dir: &str) -> String {
    let mut out = String::from("<a href=\"/\">/</a>");
    let mut href = String::from("/");
    for seg in dir.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..") {
        href.push_str(&utf8_percent_encode(seg, SEGMENT_SET).to_string());
        href.push('/');
        out.push_str(&format!("<a href=\"{}\">{}</a>/", html::attr(&href), html::text(seg)));
    }
    out
}

/// Human-readable byte size (`512 B`, `1.5 KB`, `3.2 MB`).
fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

/// HTTP reason phrase for the status codes the server produces.
fn reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        410 => "Gone",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn crumbs() {
        assert_eq!(crumbs_html(""), "<a href=\"/\">/</a>");
        assert_eq!(
            crumbs_html("/docs/a b/"),
            "<a href=\"/\">/</a><a href=\"/docs/\">docs</a>/<a href=\"/docs/a%20b/\">a b</a>/"
        );
        assert!(crumbs_html("../<x>").contains("&lt;x&gt;"));
    }

    #[test]
    fn reasons() {
        assert_eq!(reason(404), "Not Found");
        assert_eq!(reason(599), "Error");
    }
}
