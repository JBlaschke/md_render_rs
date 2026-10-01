//! `--api` renderers (owner: cli). Contract: docs/DESIGN.md §2 D15, §3.10.

use crate::flavor::Flavor;

use super::{RenderOptions, Rendered, Renderer};

/// Configuration of the API renderer.
#[derive(Clone, Debug)]
pub struct ApiConfig {
    pub flavor: Flavor,
    /// GitHub API base (default `https://api.github.com`) or GitLab instance (default `https://gitlab.com`).
    pub api_url: Option<String>,
    /// Explicit token (else resolved from the environment / `gh`).
    pub token: Option<String>,
    /// Fall back to the offline renderer (with a warning) on auth/rate-limit/network errors.
    pub fallback_offline: bool,
}

/// Renders through the GitHub or GitLab markdown API.
pub struct ApiRenderer {
    cfg: ApiConfig,
}

impl ApiRenderer {
    pub fn new(cfg: ApiConfig) -> anyhow::Result<Self> {
        Ok(ApiRenderer { cfg })
    }
}

impl Renderer for ApiRenderer {
    fn render(&self, markdown: &str, opts: &RenderOptions) -> anyhow::Result<Rendered> {
        let _ = (&self.cfg, markdown, opts);
        anyhow::bail!("API rendering is not implemented yet")
    }

    fn name(&self) -> &'static str {
        "api"
    }
}
