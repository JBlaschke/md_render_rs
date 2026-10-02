//! md-render: a grip-like live Markdown previewer with GitHub and GitLab flavors.
//!
//! See `docs/DESIGN.md` for the architecture and the frozen inter-module interfaces.

#![forbid(unsafe_code)]
#![warn(unreachable_pub)]

pub mod assets;
pub mod cli;
pub mod export;
pub mod flavor;
pub(crate) mod hash;
pub(crate) mod html;
pub(crate) mod markdown_files;
pub mod page;
pub mod render;
pub mod server;
pub mod styles;

pub use flavor::{Flavor, Mode, Theme, ThemeSel};

/// Tool name + version, e.g. `md-render 0.1.0`.
pub const TOOL: &str = concat!("md-render ", env!("CARGO_PKG_VERSION"));

/// URL prefix of all internal routes.
pub const PREFIX: &str = "/__/md-render";

/// `tracing` target of the status lines meant to be read as they are: the server's grip-style
/// ` * Running on …` lines and `md-render vendor`'s progress. The CLI prints their message
/// without its `md-render: ` prefix. Diagnostics use the default targets (`warn!`, `info!`), which
/// the CLI prints as `md-render: warning: …` / `md-render: …`.
pub(crate) const STATUS: &str = "md_render::status";

/// A byte count for people, in IEC units with one decimal: `512 B`, `85.3 KiB`, `3.4 MiB`
/// (directory listings, `md-render vendor` progress and summary).
pub(crate) fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
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

#[cfg(test)]
mod tests {
    #[test]
    fn human_bytes() {
        use super::human_bytes;
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(87_305), "85.3 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
        assert_eq!(human_bytes(u64::MAX), "16384.0 PiB");
    }
}
