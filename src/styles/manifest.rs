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
        Manifest {
            schema: SCHEMA,
            tool: crate::TOOL.to_owned(),
            flavors: BTreeMap::new(),
        }
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

// ------------------------------------------------------------------------------------------------
// Helpers (additive; the serialized format above is frozen).

/// Stylesheet role names used in [`Stylesheet::role`] (DESIGN §3.4).
pub mod role {
    /// GitHub theme variables (`light`, `dark`, ... in [`super::Stylesheet::theme`]).
    pub const THEME: &str = "theme";
    /// GitHub `primer-primitives-*.css` (layer order, size/typography tokens).
    pub const PRIMITIVES: &str = "primitives";
    /// GitHub `primer-*.css` (base `.markdown-body` typography, utilities).
    pub const PRIMER: &str = "primer";
    /// GitHub `global-*.css` (`.pl-*`, alerts, task lists, math, ...).
    pub const GLOBAL: &str = "global";
    /// GitHub `github-*.css` (site chrome, clipboard button position).
    pub const GITHUB: &str = "github";
    /// GitHub `repository-*.css`.
    pub const REPOSITORY: &str = "repository";
    /// GitHub `code-*.css` (clipboard layout).
    pub const CODE: &str = "code";
    /// GitLab `application-*.css` (theme `light`) / `application_dark-*.css` (theme `dark`).
    pub const APP: &str = "app";
    /// GitLab `tailwind-*.css`.
    pub const TAILWIND: &str = "tailwind";
    /// GitLab `fonts-*.css` (GitLab Sans / GitLab Mono `@font-face`).
    pub const FONTS: &str = "fonts";
    /// GitLab `highlight/themes/{white,dark}-*.css` (themes `light` / `dark`).
    pub const HIGHLIGHT: &str = "highlight";
    /// Embedded fallback CSS.
    pub const FALLBACK: &str = "fallback";
}

/// Keys of [`FlavorManifest::scripts`].
pub mod script {
    /// GitHub: MathJax 3.2.0 `es5/tex-chtml-full.js`.
    pub const MATHJAX: &str = "mathjax";
    /// GitHub: MathJax 3.2.0 `es5/ui/safe.js` (same relative layout as `mathjax`, CQ §1.4).
    pub const MATHJAX_SAFE: &str = "mathjax-safe";
    /// Both flavors: `mermaid.min.js` (GitHub 11.17.2, GitLab 11.16.1).
    pub const MERMAID: &str = "mermaid";
    /// GitLab: KaTeX 0.16.47 `katex.min.js`.
    pub const KATEX: &str = "katex";
    /// GitLab: KaTeX 0.16.47 `katex.min.css` (its `fonts/` live next to it).
    pub const KATEX_CSS: &str = "katex-css";
    /// GitLab: the `icons-<hash>.svg` sprite used by the copy-code button (GL §10.4). Not a
    /// script, but recorded here so pages can find it without knowing the hashed name.
    pub const ICONS: &str = "icons";
}

impl Manifest {
    /// Every bundle path the manifest references (stylesheets, assets and scripts of all
    /// flavors), possibly with duplicates.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.flavors.values().flat_map(FlavorManifest::paths)
    }
}

impl FlavorManifest {
    /// Stylesheets with the given role (see [`role`]), in cascade order.
    pub fn stylesheets_with_role<'a>(
        &'a self,
        role: &'a str,
    ) -> impl Iterator<Item = &'a Stylesheet> + 'a {
        self.stylesheets.iter().filter(move |s| s.role == role)
    }

    /// The stylesheet of `role` for theme id `theme` (e.g. GitHub `theme`/`dark_dimmed`,
    /// GitLab `app`/`dark`).
    pub fn themed_stylesheet(&self, role: &str, theme: &str) -> Option<&Stylesheet> {
        self.stylesheets
            .iter()
            .find(|s| s.role == role && s.theme.as_deref() == Some(theme))
    }

    /// Whether theme id `theme` was vendored.
    pub fn has_theme(&self, theme: &str) -> bool {
        self.themes.iter().any(|t| t == theme)
    }

    /// Bundle path of a client library (see [`script`]).
    pub fn script(&self, key: &str) -> Option<&str> {
        self.scripts.get(key).map(String::as_str)
    }

    /// Every bundle path this flavor references (stylesheets, assets, scripts).
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.stylesheets
            .iter()
            .map(|s| s.path.as_str())
            .chain(self.assets.iter().map(|a| a.path.as_str()))
            .chain(self.scripts.values().map(String::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        let mut m = Manifest::default();
        m.flavors.insert(
            Flavor::Gitlab,
            FlavorManifest {
                stylesheets: vec![
                    Stylesheet {
                        path: "gitlab/assets/application-1.css".into(),
                        role: role::APP.into(),
                        theme: Some("light".into()),
                        media: Some("(prefers-color-scheme: light)".into()),
                        ..Default::default()
                    },
                    Stylesheet {
                        path: "gitlab/assets/application_dark-2.css".into(),
                        role: role::APP.into(),
                        theme: Some("dark".into()),
                        ..Default::default()
                    },
                    Stylesheet {
                        path: "gitlab/assets/tailwind-3.css".into(),
                        role: role::TAILWIND.into(),
                        ..Default::default()
                    },
                ],
                assets: vec![Asset {
                    path: "gitlab/lib/katex/katex.min.js".into(),
                    ..Default::default()
                }],
                scripts: [(
                    script::KATEX.to_owned(),
                    "gitlab/lib/katex/katex.min.js".to_owned(),
                )]
                .into_iter()
                .collect(),
                themes: vec!["light".into(), "dark".into()],
                ..Default::default()
            },
        );
        m
    }

    #[test]
    fn helpers() {
        let m = sample();
        let gl = &m.flavors[&Flavor::Gitlab];
        assert_eq!(gl.stylesheets_with_role(role::APP).count(), 2);
        assert_eq!(
            gl.themed_stylesheet(role::APP, "dark")
                .map(|s| s.path.as_str()),
            Some("gitlab/assets/application_dark-2.css")
        );
        assert!(gl.themed_stylesheet(role::APP, "dark_dimmed").is_none());
        assert!(gl.has_theme("light") && !gl.has_theme("sepia"));
        assert_eq!(
            gl.script(script::KATEX),
            Some("gitlab/lib/katex/katex.min.js")
        );
        assert_eq!(m.paths().count(), 5);
    }

    #[test]
    fn serialized_format_is_stable() {
        // The on-disk format is frozen: field names, `flavors` keyed by lowercase flavor names,
        // optional fields defaulting when absent.
        let json = r#"{"schema":1,"tool":"md-render 0.0.0","flavors":{"github":{
            "fetched_at":"2026-10-01T00:00:00Z","source_page":"https://github.com/x/y",
            "user_agent":"ua","stylesheets":[{"path":"github/assets/light-1.css","url":"u",
            "role":"theme","theme":"light","sha256":"00","size":1}]}}}"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        let gh = &m.flavors[&Flavor::Github];
        assert!(!gh.fallback && gh.assets.is_empty() && gh.scripts.is_empty());
        assert_eq!(gh.stylesheets[0].media, None);
        let back = serde_json::to_value(&m).expect("serialize");
        assert_eq!(
            back["flavors"]["github"]["stylesheets"][0]["theme"],
            "light"
        );
        assert_eq!(back["schema"], SCHEMA);
    }
}
