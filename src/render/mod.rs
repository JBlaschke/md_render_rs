//! Markdown rendering (frozen interface, docs/DESIGN.md §3.2).

use std::path::PathBuf;

use crate::flavor::{Flavor, Mode};

pub mod api;
pub mod github;
pub mod gitlab;
pub mod highlight;

/// Options for one render.
#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub flavor: Flavor,
    pub mode: Mode,
    /// `owner/repo` (GitHub) or `group/project` (GitLab).
    pub context: Option<String>,
    /// The Markdown file on disk (None for stdin).
    pub source_path: Option<PathBuf>,
    /// Served root; GitLab `::include` must stay inside it.
    pub root: Option<PathBuf>,
    /// Emit `data-sourcepos` on block elements (live server: true, export: false).
    pub sourcepos: bool,
    /// URL prefix under which bundle files are served (no trailing slash).
    pub bundle_url: String,
}

impl RenderOptions {
    pub fn new(flavor: Flavor) -> Self {
        RenderOptions {
            flavor,
            mode: Mode::Readme,
            context: None,
            source_path: None,
            root: None,
            sourcepos: true,
            bundle_url: format!("{}/bundle", crate::PREFIX),
        }
    }
}

/// Client-side libraries a rendered document needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    pub math: bool,
    pub mermaid: bool,
}

/// Output of one render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    /// Inner HTML of the content root (`#md-content`).
    pub html: String,
    /// Plain text of the first `h1`, if any.
    pub title: Option<String>,
    pub features: Features,
    /// Non-fatal problems to show as an in-page banner.
    pub warnings: Vec<String>,
}

/// A Markdown renderer (offline or API-backed).
pub trait Renderer: Send + Sync {
    fn render(&self, markdown: &str, opts: &RenderOptions) -> anyhow::Result<Rendered>;
    fn name(&self) -> &'static str;
}

/// The default, offline renderer.
#[derive(Clone, Copy, Debug, Default)]
pub struct OfflineRenderer;

impl Renderer for OfflineRenderer {
    fn render(&self, markdown: &str, opts: &RenderOptions) -> anyhow::Result<Rendered> {
        Ok(render(markdown, opts))
    }

    fn name(&self) -> &'static str {
        "offline"
    }
}

/// Render `markdown` offline for `opts.flavor`.
pub fn render(markdown: &str, opts: &RenderOptions) -> Rendered {
    match opts.flavor {
        Flavor::Github => github::render(markdown, opts),
        Flavor::Gitlab => gitlab::render(markdown, opts),
    }
}
