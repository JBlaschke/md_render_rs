//! Command-line interface (owner: cli). Contract: docs/DESIGN.md §8.
//!
//! The pipeline is split so that every step is testable without a process:
//!
//! 1. [`parse_args`]: argv → [`args::Cli`] (clap derive, "design B" of CR §8, plus grip's `-a`/`-p`
//!    hints, GRIP §2.2 step 2);
//! 2. [`plan::plan`]: [`args::Cli`] + [`plan::Host`] (cwd, environment, stdin, cache dir) →
//!    [`plan::Invocation`], applying grip's path-and-address rules (GRIP §2.3), the served-root rules
//!    (D14), the export naming rules (GRIP §2.6) and every post-parse check;
//! 3. [`run::execute`]: [`plan::Invocation`] → the frozen entry points
//!    [`crate::server::serve`], [`crate::export::export`], [`crate::styles::vendor::vendor`].
//!
//! Exit codes: 0 success (also `--help` / `--version`), 1 runtime error, 2 usage error.

use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser as _;
use clap::error::{ContextKind, ContextValue, ErrorKind};

pub mod address;
pub mod args;
pub mod paths;
pub mod plan;
pub mod run;

pub use args::Cli;
pub use plan::{CliError, Host, Invocation, Plan, SystemHost};

/// Entry point used by `src/main.rs`: parse `std::env::args_os()`, plan, run.
pub fn main() -> ExitCode {
    run::run(std::env::args_os(), &mut SystemHost)
}

/// Parse `argv` (including the program name) into a [`Cli`].
///
/// Besides clap's own errors this reports grip's removed `-a`/`--address` and `-p`/`--port`
/// options with grip's hint (exit 2, GRIP §2.2 step 2 / §10 C11). Unlike grip, which scans the raw
/// argv, the hint is only given when clap actually finds the option, so `--title -p` style values
/// and arguments after `--` are not misreported.
pub fn parse_args<I, T>(argv: I) -> Result<Cli, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(argv).map_err(|e| match grip_hint(&e) {
        Some(hint) => CliError::Hint(hint),
        None => CliError::Clap(e),
    })
}

/// [`parse_args`] followed by [`plan::plan`].
pub fn invocation<I, T>(argv: I, host: &mut dyn Host) -> Result<Invocation, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    plan::plan(parse_args(argv)?, host)
}

/// The hint for an unknown `-a`/`--address`/`--host` or `-p`/`--port` option, if `e` is one.
fn grip_hint(e: &clap::Error) -> Option<String> {
    if e.kind() != ErrorKind::UnknownArgument {
        return None;
    }
    let Some(ContextValue::String(arg)) = e.get(ContextKind::InvalidArg) else {
        return None;
    };
    // `--port=80` is reported as `--port=80`, `-p80` as `-p`.
    let name = arg.split_once('=').map_or(arg.as_str(), |(n, _)| n);
    let (option, usage) = match name {
        "-a" | "--address" | "--host" => (name, "md-render [OPTIONS] <PATH> <ADDRESS>"),
        "-p" | "--port" => (name, "md-render [OPTIONS] [PATH] [<HOST>:]<PORT>"),
        _ => return None,
    };
    Some(format!(
        "error: unexpected argument '{option}'\n\n\
         Use {usage} instead of {option}, for example `md-render README.md 8080` or \
         `md-render . 0.0.0.0:6419`.\n\
         See md-render -h for details."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(argv: &[&str]) -> Option<String> {
        match parse_args(argv) {
            Err(CliError::Hint(h)) => Some(h),
            _ => None,
        }
    }

    #[test]
    fn grip_address_and_port_hints() {
        for argv in [
            &["md-render", "-p", "80"][..],
            &["md-render", "-p80"],
            &["md-render", "--port", "80"],
            &["md-render", "--port=80"],
            &["md-render", "README.md", "-p", "80"],
            &["md-render", "-bp", "80"],
        ] {
            let h = hint(argv).unwrap_or_else(|| panic!("no hint for {argv:?}"));
            assert!(h.contains("[<HOST>:]<PORT>"), "{h}");
        }
        for argv in [
            &["md-render", "-a", "x"][..],
            &["md-render", "--address=x"],
            &["md-render", "--host", "x"],
        ] {
            let h = hint(argv).unwrap_or_else(|| panic!("no hint for {argv:?}"));
            assert!(h.contains("<PATH> <ADDRESS>"), "{h}");
        }
        // Other unknown options keep clap's error (with its suggestions); `--` ends options.
        assert_eq!(hint(&["md-render", "--bogus"]), None);
        assert!(parse_args(["md-render", "--", "-p"]).is_ok());
        let code = |argv: &[&str]| parse_args(argv).err().map(|e| e.exit_code());
        assert_eq!(code(&["md-render", "-p", "80"]), Some(2));
        assert_eq!(code(&["md-render", "--bogus"]), Some(2));
        assert_eq!(code(&["md-render", "--version"]), Some(0));
        assert_eq!(code(&["md-render", "-h"]), Some(0));
    }

    #[test]
    fn version_string() {
        let Err(CliError::Clap(e)) = parse_args(["md-render", "-V"]) else {
            panic!("expected version output")
        };
        assert_eq!(e.kind(), ErrorKind::DisplayVersion);
        assert_eq!(e.to_string().trim_end(), crate::TOOL);
    }
}
