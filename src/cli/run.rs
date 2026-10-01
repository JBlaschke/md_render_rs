//! Running a planned [`Invocation`]: logging, the tokio runtime for the server, and the calls
//! into the frozen server / export / vendor entry points (DESIGN §3.5, §3.8, §3.9).

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::process::ExitCode;

use anyhow::Context as _;

use super::plan::{CacheClearPlan, CliError, Host, Invocation, Plan, VendorPlan};

/// Parse, plan and execute `argv`; print errors; return the process exit code (0/1/2).
pub fn run<I, T>(argv: I, host: &mut dyn Host) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let inv = match super::invocation(argv, host) {
        Ok(inv) => inv,
        Err(e) => return report(&e),
    };
    init_logging(inv.quiet, inv.verbose);
    for w in &inv.warnings {
        tracing::warn!("{w}");
    }
    match execute(inv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => report(&CliError::Runtime(e)),
    }
}

/// Print `e` the way its kind expects and return its exit code.
fn report(e: &CliError) -> ExitCode {
    match e {
        // clap prints help/version to stdout and errors (with colors, if a TTY) to stderr.
        CliError::Clap(err) => {
            let _ = err.print();
        }
        other => {
            let _ = writeln!(io::stderr(), "{other}");
        }
    }
    ExitCode::from(e.exit_code())
}

/// The log filter for `--quiet` / `-v` / `-vv`, unless `RUST_LOG` is set.
pub fn log_filter(quiet: bool, verbose: u8) -> &'static str {
    match (quiet, verbose) {
        (true, _) => "error",
        (false, 0) => "warn,md_render=info,tarfs=info",
        (false, 1) => "info,md_render=debug,tarfs=debug",
        (false, _) => "trace",
    }
}

/// Crates whose TRACE output contains request headers verbatim (ureq: "wire level debug. NOT
/// REDACTED"), i.e. API tokens. Their TRACE events are always dropped (DEBUG redacts headers),
/// so neither `-vv` nor `RUST_LOG=trace` can leak a token.
pub const SECRET_BEARING_CRATES: [&str; 2] = ["ureq", "ureq_proto"];

/// Whether an event of `level` from `target` must never be logged (see [`SECRET_BEARING_CRATES`]).
pub fn is_secret_bearing(target: &str, level: tracing::Level) -> bool {
    level == tracing::Level::TRACE
        && SECRET_BEARING_CRATES.iter().any(|krate| {
            target
                .strip_prefix(krate)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
        })
}

/// The subscriber md-render logs through: filter `rust_log` (a `RUST_LOG` value) if given and
/// valid, else [`log_filter`]; never events for which [`is_secret_bearing`] holds; plain
/// messages by default, with time and target from `-v` on.
pub fn log_subscriber<W>(
    rust_log: Option<&str>,
    quiet: bool,
    verbose: u8,
    writer: W,
    ansi: bool,
) -> impl tracing::Subscriber + Send + Sync + 'static
where
    W: for<'w> tracing_subscriber::fmt::MakeWriter<'w> + Send + Sync + 'static,
{
    use tracing_subscriber::filter::{EnvFilter, filter_fn};
    use tracing_subscriber::layer::{Layer as _, SubscriberExt as _};

    let env = rust_log
        .filter(|spec| !spec.trim().is_empty())
        .and_then(|spec| EnvFilter::try_new(spec).ok())
        .unwrap_or_else(|| EnvFilter::new(log_filter(quiet, verbose)));
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(ansi);
    let fmt = if verbose == 0 {
        fmt.without_time().with_target(false).boxed()
    } else {
        fmt.with_target(true).boxed()
    };
    let no_secrets = filter_fn(|meta| !is_secret_bearing(meta.target(), *meta.level()));
    tracing_subscriber::registry().with(fmt.with_filter(env).with_filter(no_secrets))
}

/// Install [`log_subscriber`] for stderr as the global subscriber (also bridging `log` records).
/// An already installed subscriber is kept.
fn init_logging(quiet: bool, verbose: u8) {
    use std::io::IsTerminal as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    let rust_log = std::env::var("RUST_LOG").ok();
    let ansi =
        io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    let _ = log_subscriber(rust_log.as_deref(), quiet, verbose, io::stderr, ansi).try_init();
}

/// Execute a planned invocation.
///
/// # Errors
/// Whatever the server, export, vendor or cache operation reports (exit code 1).
pub fn execute(inv: Invocation) -> anyhow::Result<()> {
    let quiet = inv.quiet;
    match inv.plan {
        Plan::Serve(plan) => {
            let renderer = plan.render.renderer()?;
            let cfg = plan.into_config(renderer);
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("md-render")
                .build()
                .context("starting the async runtime")?;
            runtime.block_on(crate::server::serve(cfg))
        }
        Plan::Export(plan) => {
            let renderer = plan.render.renderer()?;
            crate::export::export(&plan.into_config(renderer))
        }
        Plan::Vendor(plan) => vendor(&plan, quiet),
        Plan::CacheClear(plan) => {
            let message = cache_clear(&plan)?;
            if !quiet {
                println!("{message}");
            }
            Ok(())
        }
        Plan::CachePath(path) => {
            println!("{}", path.display());
            Ok(())
        }
    }
}

fn vendor(plan: &VendorPlan, quiet: bool) -> anyhow::Result<()> {
    let output = &plan.options.output;
    if plan.create_parent {
        if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating the cache directory {}", parent.display()))?;
        }
    }
    let report = crate::styles::vendor::vendor(&plan.options)?;
    for f in &report.failures {
        let status = f.status.map(|s| format!("HTTP {s}: ")).unwrap_or_default();
        let by = f
            .referenced_by
            .as_deref()
            .map(|r| format!(" (referenced by {r})"))
            .unwrap_or_default();
        tracing::warn!("could not download {}{by}: {status}{}", f.url, f.error);
    }
    if !quiet {
        eprintln!(
            "Wrote {} files ({}) to {}{}",
            report.files,
            human_bytes(report.bytes),
            report.output.display(),
            match report.failures.len() {
                0 => String::new(),
                1 => " (1 asset could not be downloaded, see above)".to_owned(),
                n => format!(" ({n} assets could not be downloaded, see above)"),
            }
        );
    }
    Ok(())
}

/// `1234567` → `1.2 MiB`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    // Precision loss is irrelevant for a one-decimal display value.
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `md-render cache clear` / grip's `--clear` (GRIP §10 C30): delete md-render's own cache
/// directory (the default bundle and anything else md-render stored there, including a
/// `--styles` bundle that lives inside it). A bundle elsewhere is never deleted. Returns the
/// message for the user.
///
/// # Errors
/// The cache directory exists but cannot be inspected or removed.
pub fn cache_clear(plan: &CacheClearPlan) -> anyhow::Result<String> {
    let dir = &plan.cache_dir;
    let mut message = match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            format!("Cache is already empty ({} does not exist).", dir.display())
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("inspecting the cache directory {}", dir.display()));
        }
        Ok(meta) => {
            // `symlink_metadata`: a symlinked cache directory is unlinked, its target is kept.
            if meta.is_dir() {
                fs::remove_dir_all(dir)
            } else {
                fs::remove_file(dir)
            }
            .with_context(|| format!("removing the cache directory {}", dir.display()))?;
            format!("Cache cleared (removed {}).", dir.display())
        }
    };
    if let Some(kept) = &plan.kept_bundle {
        message.push_str(&format!(
            "\nKept the style bundle {} (it was given with --styles or MD_RENDER_STYLES and is not \
             in the cache directory; delete it yourself if it is no longer needed).",
            kept.display()
        ));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        assert_eq!(log_filter(true, 3), "error");
        assert_eq!(log_filter(false, 0), "warn,md_render=info,tarfs=info");
        assert_eq!(log_filter(false, 1), "info,md_render=debug,tarfs=debug");
        assert_eq!(log_filter(false, 2), "trace");
    }

    #[test]
    fn wire_traces_of_the_http_client_are_never_logged() {
        use tracing::Level;
        assert!(is_secret_bearing("ureq", Level::TRACE));
        assert!(is_secret_bearing("ureq::run", Level::TRACE));
        assert!(is_secret_bearing("ureq_proto::client::flow", Level::TRACE));
        assert!(!is_secret_bearing("ureq::run", Level::DEBUG));
        assert!(!is_secret_bearing("ureqx", Level::TRACE));
        assert!(!is_secret_bearing("md_render::render::api", Level::TRACE));
    }

    #[test]
    fn byte_sizes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MiB");
    }

    #[test]
    fn clearing_the_cache() {
        let t = tempfile::tempdir().unwrap();
        let cache = t.path().join("md-render");
        let plan = CacheClearPlan {
            cache_dir: cache.clone(),
            kept_bundle: None,
        };
        assert!(
            cache_clear(&plan)
                .unwrap()
                .starts_with("Cache is already empty")
        );

        fs::create_dir_all(cache.join("sub")).unwrap();
        fs::write(cache.join("styles.tar.gz"), b"x").unwrap();
        fs::write(cache.join("sub/other.tar.gz"), b"x").unwrap();
        let outside = t.path().join("mine.tar.gz");
        fs::write(&outside, b"keep").unwrap();
        let plan = CacheClearPlan {
            cache_dir: cache.clone(),
            kept_bundle: Some(outside.clone()),
        };
        let msg = cache_clear(&plan).unwrap();
        assert!(msg.starts_with("Cache cleared"), "{msg}");
        assert!(msg.contains("Kept the style bundle"), "{msg}");
        assert!(!cache.exists());
        assert_eq!(fs::read(&outside).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn clearing_a_symlinked_cache_keeps_the_target() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("styles.tar.gz"), b"x").unwrap();
        let link = t.path().join("md-render");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        cache_clear(&CacheClearPlan {
            cache_dir: link.clone(),
            kept_bundle: None,
        })
        .unwrap();
        assert!(!link.exists() && real.join("styles.tar.gz").exists());
    }
}
