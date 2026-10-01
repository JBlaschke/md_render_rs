//! Command-line interface (owner: cli). Contract: docs/DESIGN.md §8.

use std::process::ExitCode;

/// Entry point used by `src/main.rs`.
pub fn main() -> ExitCode {
    eprintln!("{}: CLI not implemented yet", crate::TOOL);
    ExitCode::from(1)
}
