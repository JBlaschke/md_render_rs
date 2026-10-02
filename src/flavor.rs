//! Shared enums (frozen interface, docs/DESIGN.md §3.1).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Which site's rendering and styles to reproduce.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Flavor {
    Github,
    Gitlab,
}

impl Flavor {
    pub const ALL: [Flavor; 2] = [Flavor::Github, Flavor::Gitlab];

    pub fn as_str(self) -> &'static str {
        match self {
            Flavor::Github => "github",
            Flavor::Gitlab => "gitlab",
        }
    }
}

impl fmt::Display for Flavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Rendering context: a repository README/file view, or a comment/issue body.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Readme,
    Comment,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Readme => "readme",
            Mode::Comment => "comment",
        }
    }
}

/// Color theme selection.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the OS (prefers-color-scheme).
    #[default]
    Auto,
    Light,
    Dark,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Auto => "auto",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

/// Theme mode plus the concrete light/dark theme ids (GitHub has 14 theme ids; GitLab only
/// "light"/"dark").
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThemeSel {
    pub theme: Theme,
    pub light: String,
    pub dark: String,
}

impl Default for ThemeSel {
    fn default() -> Self {
        ThemeSel {
            theme: Theme::Auto,
            light: "light".into(),
            dark: "dark".into(),
        }
    }
}
