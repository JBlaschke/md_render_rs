//! Bundle `manifest.json`, schema 1 (frozen, docs/DESIGN.md §3.4).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::flavor::Flavor;

/// Current manifest schema version.
pub const SCHEMA: u32 = 1;

/// Path of the manifest inside the bundle.
pub const MANIFEST_PATH: &str = "manifest.json";

/// Root manifest of a style bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    /// Tool that wrote the bundle, e.g. `md-render 0.1.0`.
    pub tool: String,
    pub flavors: BTreeMap<Flavor, FlavorManifest>,
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest { schema: SCHEMA, tool: crate::TOOL.to_owned(), flavors: BTreeMap::new() }
    }
}

/// Everything vendored for one flavor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlavorManifest {
    /// RFC 3339 UTC timestamp of the vendoring run.
    pub fetched_at: String,
    /// Page scraped for `<link rel=stylesheet>`.
    pub source_page: String,
    pub user_agent: String,
    /// True for the embedded fallback (nothing vendored).
    #[serde(default)]
    pub fallback: bool,
    /// Stylesheets in cascade order.
    pub stylesheets: Vec<Stylesheet>,
    /// `url()` targets, emoji images, fonts, ...
    #[serde(default)]
    pub assets: Vec<Asset>,
    /// Client libraries: `mathjax`, `mathjax-safe`, `mermaid`, `katex`, `katex-css` -> bundle path.
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
    /// Theme ids available (GitHub: `light`, `dark`, ...; GitLab: `light`, `dark`).
    #[serde(default)]
    pub themes: Vec<String>,
    /// Non-fatal download failures.
    #[serde(default)]
    pub failures: Vec<Failure>,
}

/// One vendored stylesheet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stylesheet {
    /// Bundle path, e.g. `github/assets/light-99f877e9ddfc0e51.css`.
    pub path: String,
    /// Original URL.
    pub url: String,
    /// GitHub: `theme` | `primitives` | `primer` | `global` | `github` | `repository` | `code`;
    /// GitLab: `app` | `tailwind` | `fonts` | `highlight`; embedded: `fallback`.
    pub role: String,
    /// Theme id for theme-specific files (`light`, `dark`, `dark_dimmed`, ...).
    #[serde(default)]
    pub theme: Option<String>,
    /// `media` attribute the site uses for this link, if any.
    #[serde(default)]
    pub media: Option<String>,
    pub sha256: String,
    pub size: u64,
}

/// One vendored non-CSS file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub path: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// A non-fatal download failure.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub url: String,
    #[serde(default)]
    pub status: Option<u16>,
    pub error: String,
    #[serde(default)]
    pub referenced_by: Option<String>,
}
