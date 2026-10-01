//! Full-page HTML per flavor / mode / theme (owner: page). Contract: docs/DESIGN.md §3.6, §5, §7.
//!
//! SKELETON PLACEHOLDER: emits a minimal but valid page so other owners can integrate.

use std::path::PathBuf;

use crate::flavor::{Flavor, Mode, ThemeSel};
use crate::render::Rendered;
use crate::styles::Bundle;

/// Page chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Layout {
    /// Site-like box with the file name header.
    #[default]
    Readme,
    /// Content only.
    Bare,
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
    pub events_url: String,
    pub render_url: String,
    /// Document sub-path relative to the served root.
    pub doc: String,
    pub version: u64,
    pub server_id: String,
}

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
    pub message: String,
}

/// Everything needed to build one page.
pub struct PageInput<'a> {
    pub flavor: Flavor,
    pub mode: Mode,
    pub theme: &'a ThemeSel,
    pub layout: Layout,
    pub title: &'a str,
    pub file_name: Option<&'a str>,
    pub content: &'a Rendered,
    pub assets: &'a AssetMode,
    pub live: Option<&'a LiveInfo>,
    pub banners: &'a [Banner],
    pub extra_css: &'a [String],
}

/// Build a complete HTML document.
pub fn render_page(input: &PageInput, bundle: &Bundle) -> anyhow::Result<String> {
    let mut links = String::new();
    if let (AssetMode::Server { prefix }, Some(m)) = (input.assets, bundle.flavor(input.flavor)) {
        for s in &m.stylesheets {
            links.push_str(&format!("<link rel=\"stylesheet\" href=\"{prefix}/bundle/{}\">\n", s.path));
        }
    }
    let live = match (input.live, input.assets) {
        (Some(_), AssetMode::Server { prefix }) => format!("<script src=\"{prefix}/static/live.js\"></script>"),
        _ => String::new(),
    };
    Ok(format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n{links}</head>\n<body>\n<article id=\"md-content\" class=\"markdown-body md\">\n{}\n</article>\n{live}\n</body>\n</html>\n",
        crate::render::highlight::escape(input.title),
        input.content.html
    ))
}

/// One row of a directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListingEntry {
    pub name: String,
    pub href: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// Content fragment for a directory listing.
pub fn listing_html(flavor: Flavor, dir: &str, entries: &[ListingEntry]) -> String {
    let _ = flavor;
    let esc = crate::render::highlight::escape;
    let mut s = format!("<h1>{}</h1>\n<ul>\n", esc(dir));
    for e in entries {
        s.push_str(&format!("<li><a href=\"{}\">{}{}</a></li>\n", esc(&e.href), esc(&e.name), if e.is_dir { "/" } else { "" }));
    }
    s.push_str("</ul>\n");
    s
}

/// Content fragment for an error page.
pub fn error_html(flavor: Flavor, status: u16, message: &str) -> String {
    let _ = flavor;
    format!("<h1>{status}</h1>\n<p>{}</p>\n", crate::render::highlight::escape(message))
}
