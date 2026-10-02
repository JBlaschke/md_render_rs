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
        // `md-render export README.md - | head`: the reader went away, which is not an error
        // (grip ignores EPIPE too, GRIP §2.6).
        Err(e) if is_broken_pipe(&e) => ExitCode::SUCCESS,
        Err(e) => report(&CliError::Runtime(e)),
    }
}

/// Whether `e` was caused by writing to a closed pipe.
pub fn is_broken_pipe(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|cause| cause.downcast_ref::<io::Error>())
        .any(|io| io.kind() == io::ErrorKind::BrokenPipe)
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

/// The log filter for `--quiet` / `-v` / `-vv` / `-vvv`, unless `RUST_LOG` is set.
///
/// `-vv` traces md-render and tarfs only: the HTML parser and selector engine log a line per
/// character at TRACE/DEBUG, which would bury md-render's own messages (CLI-5). `-vvv` traces
/// everything (still without the secret-bearing crates, see [`SECRET_BEARING_CRATES`]).
pub fn log_filter(quiet: bool, verbose: u8) -> &'static str {
    match (quiet, verbose) {
        (true, _) => "error",
        (false, 0) => "warn,md_render=info,tarfs=info",
        (false, 1) => "info,md_render=debug,tarfs=debug",
        (false, 2) => "info,md_render=trace,tarfs=trace",
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
/// valid, else [`log_filter`]; never events for which [`is_secret_bearing`] holds; md-render's
/// own `md-render: warning: …` lines by default ([`PlainFormat`]), tracing's format with time
/// and target from `-v` on.
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
        fmt.event_format(PlainFormat).boxed()
    } else {
        fmt.with_target(true).boxed()
    };
    let no_secrets = filter_fn(|meta| !is_secret_bearing(meta.target(), *meta.level()));
    tracing_subscriber::registry().with(fmt.with_filter(env).with_filter(no_secrets))
}

/// The log format without `-v`: the `md-render: warning: …` prefix that the export, server and
/// bundle diagnostics print, instead of the padded ` WARN` / ` INFO` tags, with the event's
/// fields folded into the message (`… not copied (path: x.svg)` rather than `path="x.svg"`)
/// (CLI-23). `-v` keeps tracing's full format with time and target.
struct PlainFormat;

impl<S, N> tracing_subscriber::fmt::FormatEvent<S, N> for PlainFormat
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        use tracing::Level;
        let (label, color) = match *event.metadata().level() {
            Level::ERROR => (Some("error"), "\x1b[1;31m"),
            Level::WARN => (Some("warning"), "\x1b[1;33m"),
            _ => (None, ""),
        };
        write!(writer, "md-render: ")?;
        if let Some(label) = label {
            if writer.has_ansi_escapes() {
                write!(writer, "{color}{label}\x1b[0m: ")?;
            } else {
                write!(writer, "{label}: ")?;
            }
        }
        let mut fields = PlainFields::default();
        event.record(&mut fields);
        write!(writer, "{}", fields.message)?;
        if !fields.rest.is_empty() {
            write!(writer, " ({})", fields.rest.join(", "))?;
        }
        writeln!(writer)
    }
}

/// An event's message and its other fields as `name: value`.
#[derive(Default)]
struct PlainFields {
    message: String,
    rest: Vec<String>,
}

impl PlainFields {
    fn push(&mut self, field: &tracing::field::Field, value: String) {
        match field.name() {
            "message" => self.message = value,
            // Metadata of records bridged from the `log` crate.
            name if name.starts_with("log.") => {}
            name => self.rest.push(format!("{name}: {value}")),
        }
    }
}

impl tracing::field::Visit for PlainFields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.push(field, value.to_owned());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.push(field, format!("{value:?}"));
    }
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
        Plan::Vendor(plan) => vendor(&plan),
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

fn vendor(plan: &VendorPlan) -> anyhow::Result<()> {
    let output = &plan.options.output;
    if plan.create_parent {
        if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating the cache directory {}", parent.display()))?;
        }
    }
    // The vendor module reports progress, every failed download and the final summary itself
    // (unless quiet); repeating the summary here printed it twice, in other units (CLI-15).
    crate::styles::vendor::vendor(&plan.options)?;
    Ok(())
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
        assert_eq!(log_filter(false, 2), "info,md_render=trace,tarfs=trace");
        assert_eq!(log_filter(false, 3), "trace");
    }

    /// Collects what a subscriber writes.
    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'w> tracing_subscriber::fmt::MakeWriter<'w> for Capture {
        type Writer = Capture;
        fn make_writer(&'w self) -> Capture {
            self.clone()
        }
    }

    fn logged(verbose: u8) -> String {
        let capture = Capture::default();
        let subscriber = log_subscriber(None, false, verbose, capture.clone(), false);
        tracing::subscriber::with_default(subscriber, || {
            tracing::trace!(target: "html5ever::tokenizer", "got character x");
            tracing::debug!(target: "selectors::matching", "matching");
            tracing::trace!(target: "md_render::render::api", "own trace");
            tracing::trace!(target: "tarfs::fs", "tarfs trace");
            tracing::info!(target: "html5ever::tree_builder", "parser info");
        });
        String::from_utf8_lossy(&capture.0.lock().unwrap()).into_owned()
    }

    /// CLI-23: without `-v`, log lines look like the other diagnostics (`md-render: warning: …`),
    /// with the fields folded into the message; `-v` keeps tracing's format.
    #[test]
    fn default_log_lines_match_the_other_diagnostics() {
        let log = |verbose: u8| {
            let capture = Capture::default();
            let subscriber = log_subscriber(None, false, verbose, capture.clone(), false);
            tracing::subscriber::with_default(subscriber, || {
                tracing::warn!("--wide has no effect");
                let path = "github/assets/x.svg";
                tracing::warn!(path, count = 2, "bundle file missing; not copied");
                tracing::info!(target: "md_render::render::api", "rendering through the API");
                tracing::error!("boom");
            });
            String::from_utf8_lossy(&capture.0.lock().unwrap()).into_owned()
        };
        assert_eq!(
            log(0),
            "md-render: warning: --wide has no effect\n\
             md-render: warning: bundle file missing; not copied (path: github/assets/x.svg, \
             count: 2)\n\
             md-render: rendering through the API\n\
             md-render: error: boom\n"
        );
        let verbose = log(1);
        assert!(
            verbose.contains(" WARN ") && verbose.contains("path=\"github/assets/x.svg\""),
            "{verbose}"
        );
    }

    /// CLI-5: `-vv` traces md-render, not every dependency.
    #[test]
    fn very_verbose_traces_md_render_but_not_the_html_parser() {
        let log = logged(2);
        assert!(
            log.contains("own trace") && log.contains("tarfs trace"),
            "{log}"
        );
        assert!(log.contains("parser info"), "{log}");
        assert!(
            !log.contains("got character") && !log.contains("matching"),
            "{log}"
        );
        let all = logged(3);
        assert!(
            all.contains("got character") && all.contains("own trace"),
            "{all}"
        );
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
    fn broken_pipes_are_recognized_through_context() {
        let pipe = anyhow::Error::new(io::Error::from(io::ErrorKind::BrokenPipe))
            .context("writing the HTML to stdout");
        assert!(is_broken_pipe(&pipe));
        let other =
            anyhow::Error::new(io::Error::from(io::ErrorKind::PermissionDenied)).context("writing");
        assert!(!is_broken_pipe(&other));
        assert!(!is_broken_pipe(&anyhow::anyhow!("plain")));
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
