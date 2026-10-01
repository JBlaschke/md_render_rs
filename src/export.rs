//! `md-render export` (owner: page). Contract: docs/DESIGN.md §3.9.

use std::path::PathBuf;
use std::sync::Arc;

use crate::flavor::{Flavor, Mode, ThemeSel};
use crate::page::Layout;
use crate::render::Renderer;

/// Export source.
#[derive(Clone, Debug)]
pub enum ExportInput {
    File(PathBuf),
    Stdin { name: String, text: String },
}

/// Export destination.
#[derive(Clone, Debug)]
pub enum ExportOutput {
    File(PathBuf),
    Stdout,
}

/// Options of one export.
#[derive(Clone)]
pub struct ExportConfig {
    pub input: ExportInput,
    pub output: ExportOutput,
    /// Self-contained single file (default) vs. linked assets directory (`--no-inline`).
    pub inline: bool,
    pub flavor: Flavor,
    pub mode: Mode,
    pub theme: ThemeSel,
    pub layout: Layout,
    pub title: Option<String>,
    pub context: Option<String>,
    pub bundle_path: PathBuf,
    pub renderer: Arc<dyn Renderer>,
    pub quiet: bool,
}

/// Render and write a standalone HTML file.
pub fn export(cfg: &ExportConfig) -> anyhow::Result<()> {
    let _ = cfg;
    anyhow::bail!("export is not implemented yet")
}
