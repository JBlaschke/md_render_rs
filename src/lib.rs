//! md-render: a grip-like live Markdown previewer with GitHub and GitLab flavors.
//!
//! See `docs/DESIGN.md` for the architecture and the frozen inter-module interfaces.

pub mod assets;
pub mod cli;
pub mod export;
pub mod flavor;
pub(crate) mod hash;
pub(crate) mod html;
pub mod page;
pub mod render;
pub mod server;
pub mod styles;

pub use flavor::{Flavor, Mode, Theme, ThemeSel};

/// Tool name + version, e.g. `md-render 0.1.0`.
pub const TOOL: &str = concat!("md-render ", env!("CARGO_PKG_VERSION"));

/// URL prefix of all internal routes.
pub const PREFIX: &str = "/__/md-render";
