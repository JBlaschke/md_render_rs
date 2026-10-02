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

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{CommandFactory as _, Parser as _};

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
///
/// `--poll 500` (with a space) is a usage error: `--poll` takes its value only as `--poll=MS`,
/// so the number would silently become the PATH or ADDRESS (CLI-13).
///
/// # Errors
/// [`CliError::Clap`] for usage errors and for `--help` / `--version` (exit code 0), or
/// [`CliError::Hint`] for `-a` / `-p`.
pub fn parse_args<I, T>(argv: I) -> Result<Cli, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let argv: Vec<OsString> = argv.into_iter().map(Into::into).collect();
    let cli = Cli::try_parse_from(&argv).map_err(|e| {
        if let Some(hint) = grip_hint(&e) {
            CliError::Hint(hint)
        } else if let Some(conflict) = serve_flag_after_subcommand(&e, &argv) {
            conflict
        } else {
            CliError::Clap(e)
        }
    })?;
    if cli.command.is_none() && cli.serve.poll.is_some() {
        if let Some(ms) = detached_poll_value(&argv) {
            return Err(plan::usage(
                ErrorKind::ValueValidation,
                format!(
                    "'--poll {ms}' does not set the interval (it would be taken as the PATH or \
                     ADDRESS): write --poll={ms}, or put the address before the option \
                     (`md-render README.md {ms} --poll`)"
                ),
            ));
        }
    }
    Ok(cli)
}

/// The number in `--poll NUMBER` (separate arguments, before any `--`), if there is one.
fn detached_poll_value(argv: &[OsString]) -> Option<&str> {
    let args = argv.get(1..)?;
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    args[..end].windows(2).find_map(|pair| {
        let value = pair[1].to_str()?;
        (pair[0] == "--poll" && !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            .then_some(value)
    })
}

/// [`parse_args`] followed by [`plan::plan`].
///
/// # Errors
/// Everything [`parse_args`] and [`plan::plan`] report.
pub fn invocation<I, T>(argv: I, host: &mut dyn Host) -> Result<Invocation, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    plan::plan(parse_args(argv)?, host)
}

/// A serve-only flag after a subcommand (`md-render export --browser`) is unknown to clap,
/// which would suggest a lookalike option and print it in the usage line as if it were
/// required (`Usage: md-render export --verbose... [PATH] [OUT]`). Report it like the same flag
/// before the subcommand instead (CLI-14).
fn serve_flag_after_subcommand(e: &clap::Error, argv: &[OsString]) -> Option<CliError> {
    if e.kind() != ErrorKind::UnknownArgument {
        return None;
    }
    let Some(ContextValue::String(arg)) = e.get(ContextKind::InvalidArg) else {
        return None;
    };
    let flag = serve_flag(arg.split_once('=').map_or(arg.as_str(), |(n, _)| n))?;
    // The subcommand the flag was given to (clap stops at the first error otherwise).
    let matches = Cli::command()
        .ignore_errors(true)
        .try_get_matches_from(argv)
        .ok()?;
    let mut path = Vec::new();
    let mut m = &matches;
    while let Some((name, sub)) = m.subcommand() {
        path.push(name);
        m = sub;
    }
    (!path.is_empty()).then(|| plan::serve_flag_conflict(&flag, &path))
}

/// `--browser` for `--browser`, `-b` or the alias `--no-reload` → `--norefresh`: the canonical
/// name of a serve-only option, if `name` is one.
fn serve_flag(name: &str) -> Option<String> {
    let cmd = Cli::command();
    let found = cmd
        .get_arguments()
        .filter(|a| !a.is_global_set() && !a.is_positional())
        .find(|a| {
            if let Some(long) = name.strip_prefix("--") {
                a.get_long() == Some(long)
                    || a.get_all_aliases()
                        .is_some_and(|aliases| aliases.contains(&long))
            } else {
                let mut short = name.strip_prefix('-').unwrap_or_default().chars();
                short.next().is_some_and(|c| a.get_short() == Some(c)) && short.next().is_none()
            }
        })?;
    found.get_long().map(|long| format!("--{long}"))
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

    /// CLI-13: `--poll 500` must not quietly use 500 as the PATH or ADDRESS.
    #[test]
    fn detached_poll_values_are_usage_errors() {
        for argv in [
            &["md-render", "README.md", "--poll", "500"][..],
            &["md-render", "--poll", "500", "README.md"],
            &["md-render", "--poll", "500"],
        ] {
            let err = parse_args(argv).expect_err("a detached --poll value");
            assert_eq!(err.exit_code(), 2, "{argv:?}");
            let msg = err.to_string();
            assert!(msg.contains("write --poll=500"), "{msg}");
        }
        for argv in [
            &["md-render", "README.md", "--poll=500"][..],
            &["md-render", "--poll", "README.md"],
            &["md-render", "README.md", "8080", "--poll"],
            &["md-render", "--poll", "--", "500"],
            &["md-render", "--title=--poll", "500", "--poll"],
        ] {
            assert!(parse_args(argv).is_ok(), "{argv:?}");
        }
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
