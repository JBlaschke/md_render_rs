//! Live preview server (owner: server). Contract: docs/DESIGN.md §3.8, §6, §7.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::flavor::{Flavor, Mode, ThemeSel};
use crate::page::Layout;
use crate::render::Renderer;

/// What `/` shows. Paths are relative to `ServeConfig::root`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    File(PathBuf),
    Dir(PathBuf),
    Stdin { name: String, text: String },
}

/// Server configuration.
#[derive(Clone)]
pub struct ServeConfig {
    pub root: PathBuf,
    pub target: Target,
    pub host: String,
    pub port: u16,
    pub flavor: Flavor,
    pub mode: Mode,
    pub theme: ThemeSel,
    pub layout: Layout,
    pub title: Option<String>,
    pub context: Option<String>,
    pub live: bool,
    pub open_browser: bool,
    pub quiet: bool,
    pub bundle_path: PathBuf,
    pub auto_vendor: bool,
    pub extra_css: Vec<String>,
    pub renderer: Arc<dyn Renderer>,
    pub poll: Option<Duration>,
    pub follow_symlinks: bool,
}

/// A started server.
pub struct RunningServer {
    pub addr: SocketAddr,
}

impl RunningServer {
    /// Stop the server gracefully (SSE clients receive `shutdown`).
    pub async fn shutdown(self) {}

    /// Wait until the server stops.
    pub async fn wait(self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Bind and start serving in the background; returns once the listener is bound.
pub async fn start(cfg: ServeConfig) -> anyhow::Result<RunningServer> {
    let _ = cfg;
    anyhow::bail!("server is not implemented yet")
}

/// Start, then run until Ctrl-C.
pub async fn serve(cfg: ServeConfig) -> anyhow::Result<()> {
    start(cfg).await?.wait().await
}
