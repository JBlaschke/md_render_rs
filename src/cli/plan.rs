//! From parsed arguments to a fully resolved [`Invocation`] (DESIGN §8, GRIP §2), then to the
//! frozen [`ServeConfig`] / [`ExportConfig`] / [`VendorOptions`] (DESIGN §3.5, §3.8, §3.9).
//!
//! Planning touches the outside world only through [`Host`] (working directory, environment,
//! stdin, cache directory), so argv → configuration is unit-testable.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::error::ErrorKind;
use clap::{CommandFactory as _, ValueEnum as _};

use crate::export::{ExportConfig, ExportInput, ExportOutput};
use crate::flavor::{Flavor, Mode, Theme, ThemeSel};
use crate::page::Layout;
use crate::render::api::{ApiConfig, ApiRenderer};
use crate::render::{OfflineRenderer, Renderer};
use crate::server::{ServeConfig, Target};
use crate::styles::vendor::{GITHUB_SOURCE, GITLAB_SOURCE, VendorOptions};

use super::address::{self, DEFAULT_HOST, DEFAULT_PORT};
use super::args::{
    CacheAction, Cli, Command, ExportArgs, FlavorArg, GITHUB_THEMES, GlobalArgs, ServeArgs,
    ThemeArg, ThemesArg, VendorArgs,
};
use super::paths::{absolutize, decode_text, find_readme};

/// File name of the default style bundle inside the cache directory (D9).
pub const BUNDLE_FILE_NAME: &str = "styles.tar.gz";
/// Name shown for Markdown read from stdin (GRIP C4).
pub const STDIN_NAME: &str = "stdin";

/// The outside world as seen by the planner.
pub trait Host {
    /// Current working directory.
    fn cwd(&self) -> io::Result<PathBuf>;
    /// Environment variable (`None` when unset).
    fn var(&self, name: &str) -> Option<OsString>;
    /// Read all of stdin.
    fn read_stdin(&mut self) -> io::Result<Vec<u8>>;
    /// Whether stdin is an interactive terminal.
    fn stdin_is_terminal(&self) -> bool;
    /// `<cache dir>/md-render` (D9), if it can be determined.
    fn cache_dir(&self) -> Option<PathBuf>;
    /// Show a message to the user right away (stderr).
    fn notice(&mut self, message: &str);
}

/// The real process environment.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemHost;

impl Host for SystemHost {
    fn cwd(&self) -> io::Result<PathBuf> {
        std::env::current_dir()
    }

    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }

    fn read_stdin(&mut self) -> io::Result<Vec<u8>> {
        use io::Read as _;
        let mut buf = Vec::new();
        io::stdin().lock().read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn stdin_is_terminal(&self) -> bool {
        use io::IsTerminal as _;
        io::stdin().is_terminal()
    }

    fn cache_dir(&self) -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "md-render").map(|d| d.cache_dir().to_path_buf())
    }

    fn notice(&mut self, message: &str) {
        eprintln!("{message}");
    }
}

/// Why the command line could not be turned into an [`Invocation`].
#[derive(Debug)]
pub enum CliError {
    /// Usage error (exit 2), or `--help` / `--version` output (exit 0), printed clap-style.
    Clap(clap::Error),
    /// grip's `-a` / `-p` hint (exit 2).
    Hint(String),
    /// Runtime problem such as a missing file (exit 1).
    Runtime(anyhow::Error),
}

impl CliError {
    /// Process exit code: 0 (help/version), 1 (runtime) or 2 (usage).
    pub fn exit_code(&self) -> u8 {
        match self {
            CliError::Clap(e) => u8::try_from(e.exit_code()).unwrap_or(2),
            CliError::Hint(_) => 2,
            CliError::Runtime(_) => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Clap(e) => write!(f, "{e}"),
            CliError::Hint(h) => f.write_str(h),
            CliError::Runtime(e) => write!(f, "error: {e:#}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<clap::Error> for CliError {
    fn from(e: clap::Error) -> Self {
        CliError::Clap(e)
    }
}

/// A clap-formatted usage error (exit 2).
pub(crate) fn usage(kind: ErrorKind, message: impl fmt::Display) -> CliError {
    CliError::Clap(Cli::command().error(kind, message))
}

fn runtime(message: impl fmt::Display) -> CliError {
    CliError::Runtime(anyhow::anyhow!("{message}"))
}

/// A resolved command line: what to do plus messages for the user.
#[derive(Debug)]
pub struct Invocation {
    pub plan: Plan,
    /// Warnings to log before running (e.g. `--wide` has no effect).
    pub warnings: Vec<String>,
    pub quiet: bool,
    pub verbose: u8,
}

/// What to do.
#[derive(Debug)]
pub enum Plan {
    Serve(ServePlan),
    Export(ExportPlan),
    Vendor(VendorPlan),
    CacheClear(CacheClearPlan),
    /// Print this bundle path.
    CachePath(PathBuf),
}

/// Rendering options shared by serve and export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderPlan {
    pub flavor: Flavor,
    pub mode: Mode,
    pub theme: ThemeSel,
    pub layout: Layout,
    pub title: Option<String>,
    pub context: Option<String>,
    pub bundle_path: PathBuf,
    /// URLs (`http(s)://`, `//`, `data:`) or absolute local file paths.
    pub extra_css: Vec<String>,
    /// `Some` with `--api`.
    pub api: Option<ApiConfig>,
    pub quiet: bool,
}

impl RenderPlan {
    /// The renderer: offline, or [`ApiRenderer`] (resolves the token) with `--api`.
    pub fn renderer(&self) -> anyhow::Result<Arc<dyn Renderer>> {
        Ok(match &self.api {
            Some(cfg) => Arc::new(ApiRenderer::new(cfg.clone())?),
            None => Arc::new(OfflineRenderer),
        })
    }
}

/// `md-render [PATH] [ADDRESS]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServePlan {
    /// Canonical served root.
    pub root: PathBuf,
    /// Relative to `root` (`Dir("")` is the root itself).
    pub target: Target,
    /// Bare host (IPv6 literals without brackets).
    pub host: String,
    pub port: u16,
    pub render: RenderPlan,
    pub live: bool,
    pub open_browser: bool,
    pub poll: Option<Duration>,
    pub follow_symlinks: bool,
    pub auto_vendor: bool,
}

impl ServePlan {
    /// The frozen server configuration.
    pub fn into_config(self, renderer: Arc<dyn Renderer>) -> ServeConfig {
        let r = self.render;
        ServeConfig {
            root: self.root,
            target: self.target,
            host: self.host,
            port: self.port,
            flavor: r.flavor,
            mode: r.mode,
            theme: r.theme,
            layout: r.layout,
            title: r.title,
            context: r.context,
            live: self.live,
            open_browser: self.open_browser,
            quiet: r.quiet,
            bundle_path: r.bundle_path,
            auto_vendor: self.auto_vendor,
            extra_css: r.extra_css,
            renderer,
            poll: self.poll,
            follow_symlinks: self.follow_symlinks,
        }
    }
}

/// `md-render export` / grip's `--export`.
#[derive(Clone, Debug)]
pub struct ExportPlan {
    /// `File` paths are absolute.
    pub input: ExportInput,
    /// `File` paths are absolute.
    pub output: ExportOutput,
    pub inline: bool,
    pub render: RenderPlan,
}

impl ExportPlan {
    /// The frozen export configuration.
    pub fn into_config(self, renderer: Arc<dyn Renderer>) -> ExportConfig {
        let r = self.render;
        ExportConfig {
            input: self.input,
            output: self.output,
            inline: self.inline,
            flavor: r.flavor,
            mode: r.mode,
            theme: r.theme,
            layout: r.layout,
            title: r.title,
            context: r.context,
            bundle_path: r.bundle_path,
            renderer,
            quiet: r.quiet,
        }
    }
}

/// `md-render vendor`.
#[derive(Clone, Debug)]
pub struct VendorPlan {
    pub options: VendorOptions,
    /// Create the output's parent directory first (true for the default cache location).
    pub create_parent: bool,
}

/// `md-render cache clear` / `--clear`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheClearPlan {
    /// `<cache dir>/md-render`; removed entirely.
    pub cache_dir: PathBuf,
    /// A `--styles` / `MD_RENDER_STYLES` bundle outside the cache directory: never deleted,
    /// only mentioned.
    pub kept_bundle: Option<PathBuf>,
}

/// Where the bundle path came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BundleSource {
    Flag,
    Env,
    Default,
}

/// Turn parsed arguments into an [`Invocation`].
pub fn plan(cli: Cli, host: &mut dyn Host) -> Result<Invocation, CliError> {
    let g = &cli.global;
    let mut warnings = Vec::new();

    // grip compatibility flags (GRIP §10 C14, C20).
    if g.user.is_some() || g.pass.is_some() {
        return Err(usage(
            ErrorKind::ArgumentConflict,
            "--user/--pass are not supported: GitHub no longer accepts password authentication; \
             use --api with a token (--token TOKEN, or set GITHUB_TOKEN)",
        ));
    }
    if g.wide {
        warnings.push("--wide has no effect (it was a no-op in grip too); see --layout".to_owned());
    }

    let quiet = g.quiet;
    let cwd = host.cwd().map_err(|e| runtime(format!("cannot determine the current directory: {e}")))?;
    let flavor_arg = match g.flavor {
        Some(f) => Some(f),
        None => env_flavor(host)?,
    };
    let bundle = bundle_path(g, host, &cwd)?;

    let plan = match &cli.command {
        Some(cmd) => {
            reject_serve_args(&cli.serve, cmd)?;
            match cmd {
                Command::Export(a) => {
                    let render = render_plan(g, flavor_arg, &bundle.0, &cwd, true, &mut warnings)?;
                    let inline = !(a.no_inline || cli.serve.no_inline);
                    Plan::Export(export_plan(a, inline, render, host, &cwd, &mut warnings)?)
                }
                Command::Vendor(a) => Plan::Vendor(vendor_plan(a, flavor_arg, bundle, &cwd, quiet)?),
                Command::Cache(c) => match c.action {
                    CacheAction::Clear => Plan::CacheClear(cache_clear_plan(host, bundle)?),
                    CacheAction::Path => Plan::CachePath(bundle.0),
                },
            }
        }
        // grip runs `--clear` before any path resolution (GRIP §2.2 step 5).
        None if cli.serve.clear => Plan::CacheClear(cache_clear_plan(host, bundle)?),
        None if cli.serve.export => {
            reject_with_export(&cli.serve)?;
            let render = render_plan(g, flavor_arg, &bundle.0, &cwd, true, &mut warnings)?;
            // grip: no address resolution in export mode; the 2nd positional is the output.
            let args = ExportArgs {
                path: cli.serve.path.clone(),
                out: cli.serve.address.clone(),
                no_inline: cli.serve.no_inline,
            };
            let inline = !cli.serve.no_inline;
            Plan::Export(export_plan(&args, inline, render, host, &cwd, &mut warnings)?)
        }
        None => {
            if cli.serve.no_inline {
                return Err(usage(
                    ErrorKind::ArgumentConflict,
                    "'--no-inline' only applies to exports (md-render export --no-inline, or --export)",
                ));
            }
            let render = render_plan(g, flavor_arg, &bundle.0, &cwd, false, &mut warnings)?;
            Plan::Serve(serve_plan(&cli.serve, render, host, &cwd, &mut warnings)?)
        }
    };
    Ok(Invocation { plan, warnings, quiet, verbose: g.verbose })
}

/// Non-empty UTF-8 environment variable.
fn env_string(host: &dyn Host, name: &str) -> Result<Option<String>, CliError> {
    match host.var(name) {
        None => Ok(None),
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => v.into_string().map(Some).map_err(|_| {
            usage(ErrorKind::InvalidValue, format!("environment variable {name} is not valid UTF-8"))
        }),
    }
}

fn env_flavor(host: &dyn Host) -> Result<Option<FlavorArg>, CliError> {
    let Some(v) = env_string(host, "MD_RENDER_FLAVOR")? else { return Ok(None) };
    FlavorArg::from_str(v.trim(), true).map(Some).map_err(|_| {
        usage(
            ErrorKind::InvalidValue,
            format!("invalid value '{v}' in MD_RENDER_FLAVOR (expected github or gitlab)"),
        )
    })
}

/// `--styles` > `MD_RENDER_STYLES` > `<cache dir>/md-render/styles.tar.gz` (D9).
fn bundle_path(g: &GlobalArgs, host: &dyn Host, cwd: &Path) -> Result<(PathBuf, BundleSource), CliError> {
    if let Some(p) = &g.styles {
        return Ok((absolutize(cwd, p), BundleSource::Flag));
    }
    if let Some(v) = host.var("MD_RENDER_STYLES").filter(|v| !v.is_empty()) {
        return Ok((absolutize(cwd, Path::new(&v)), BundleSource::Env));
    }
    let path = match host.cache_dir() {
        Some(dir) => dir.join(BUNDLE_FILE_NAME),
        // Same last resort as `styles::default_bundle_path`.
        None => cwd.join("md-render-styles.tar.gz"),
    };
    Ok((path, BundleSource::Default))
}

fn subcommand_name(cmd: &Command) -> &'static str {
    match cmd {
        Command::Export(_) => "export",
        Command::Vendor(_) => "vendor",
        Command::Cache(_) => "cache",
    }
}

/// Serve-only flags that are set, by their option name.
fn serve_flags(s: &ServeArgs) -> Vec<&'static str> {
    let mut v = Vec::new();
    let mut flag = |set: bool, name: &'static str| {
        if set {
            v.push(name);
        }
    };
    flag(s.browser, "--browser");
    flag(s.norefresh, "--norefresh");
    flag(s.poll.is_some(), "--poll");
    flag(s.root.is_some(), "--root");
    flag(s.follow_symlinks, "--follow-symlinks");
    flag(s.auto_vendor, "--auto-vendor");
    flag(s.export, "--export");
    flag(s.clear, "--clear");
    flag(s.no_inline, "--no-inline");
    v
}

/// Design B post-parse check (CR §8): a subcommand plus serve-only arguments is a usage error.
fn reject_serve_args(s: &ServeArgs, cmd: &Command) -> Result<(), CliError> {
    let name = subcommand_name(cmd);
    let hint = format!("(to render a file named '{name}' use ./{name})");
    if let Some(p) = s.path.as_ref().or(s.address.as_ref()) {
        return Err(usage(
            ErrorKind::ArgumentConflict,
            format!("unexpected argument '{}' before subcommand '{name}' {hint}", p.to_string_lossy()),
        ));
    }
    let flags: Vec<_> = serve_flags(s)
        .into_iter()
        // `--no-inline export …` means the same as `export --no-inline …`.
        .filter(|f| !(*f == "--no-inline" && matches!(cmd, Command::Export(_))))
        .collect();
    if let Some(flag) = flags.first() {
        return Err(usage(
            ErrorKind::ArgumentConflict,
            format!("'{flag}' cannot be used with the '{name}' subcommand {hint}"),
        ));
    }
    Ok(())
}

/// grip's `--export` ignores serve flags silently; reject the ones that cannot apply.
fn reject_with_export(s: &ServeArgs) -> Result<(), CliError> {
    let bad = [
        (s.browser, "--browser"),
        (s.poll.is_some(), "--poll"),
        (s.root.is_some(), "--root"),
        (s.follow_symlinks, "--follow-symlinks"),
        (s.auto_vendor, "--auto-vendor"),
    ];
    match bad.iter().find(|(set, _)| *set) {
        Some((_, flag)) => {
            Err(usage(ErrorKind::ArgumentConflict, format!("'{flag}' cannot be used with '--export'")))
        }
        None => Ok(()),
    }
}

fn render_plan(
    g: &GlobalArgs,
    flavor_arg: Option<FlavorArg>,
    bundle_path: &Path,
    cwd: &Path,
    for_export: bool,
    warnings: &mut Vec<String>,
) -> Result<RenderPlan, CliError> {
    let flavor = match flavor_arg {
        None | Some(FlavorArg::Github) => Flavor::Github,
        Some(FlavorArg::Gitlab) => Flavor::Gitlab,
        Some(FlavorArg::All) => {
            return Err(usage(
                ErrorKind::InvalidValue,
                "'--flavor all' is only valid for 'md-render vendor'; choose github or gitlab",
            ));
        }
    };
    let mode = match (g.mode, g.user_content) {
        (Some(Mode::Readme), true) => {
            return Err(usage(
                ErrorKind::ArgumentConflict,
                "'--user-content' (comment mode) conflicts with '--mode readme'",
            ));
        }
        (_, true) => Mode::Comment,
        (m, false) => m.unwrap_or_default(),
    };
    let api = if g.api {
        if let Some(url) = &g.api_url {
            validate_http_url("--api-url", url)?;
        }
        Some(ApiConfig {
            flavor,
            api_url: g.api_url.clone(),
            token: g.token.clone(),
            // Serving shows a banner and renders offline; exports must fail loudly (GRIP C27).
            fallback_offline: !for_export,
        })
    } else {
        if g.api_url.is_some() || g.token.is_some() {
            warnings.push("--api-url and --token have no effect without --api".to_owned());
        }
        None
    };
    Ok(RenderPlan {
        flavor,
        mode,
        theme: theme_sel(g, flavor)?,
        layout: g.layout.unwrap_or_default(),
        title: g.title.clone(),
        context: g.context.clone().filter(|c| !c.trim().is_empty()),
        bundle_path: bundle_path.to_path_buf(),
        extra_css: extra_css(&g.css, cwd)?,
        api,
        quiet: g.quiet,
    })
}

fn validate_http_url(option: &str, value: &str) -> Result<(), CliError> {
    match url::Url::parse(value) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.host_str().is_some() => Ok(()),
        _ => Err(usage(
            ErrorKind::ValueValidation,
            format!("invalid value '{value}' for '{option} <URL>': expected an http:// or https:// URL"),
        )),
    }
}

/// `--theme`, `--light-theme`, `--dark-theme` → [`ThemeSel`] (GRIP C25).
fn theme_sel(g: &GlobalArgs, flavor: Flavor) -> Result<ThemeSel, CliError> {
    let check = |option: &str, id: &str| -> Result<String, CliError> {
        if GITHUB_THEMES.contains(&id) {
            Ok(id.to_owned())
        } else {
            Err(usage(
                ErrorKind::InvalidValue,
                format!(
                    "invalid value '{id}' for '--{option} <ID>': expected one of {}",
                    GITHUB_THEMES.join(", ")
                ),
            ))
        }
    };
    let mut sel = ThemeSel::default();
    if let Some(id) = &g.light_theme {
        sel.light = check("light-theme", id)?;
    }
    if let Some(id) = &g.dark_theme {
        sel.dark = check("dark-theme", id)?;
    }
    match &g.theme {
        None => {}
        Some(ThemeArg::Mode(t)) => sel.theme = *t,
        Some(ThemeArg::Id(id)) => {
            let light = id.starts_with("light");
            let (explicit, option) =
                if light { (&g.light_theme, "--light-theme") } else { (&g.dark_theme, "--dark-theme") };
            if let Some(other) = explicit.as_ref().filter(|o| *o != id) {
                return Err(usage(
                    ErrorKind::ArgumentConflict,
                    format!("'--theme {id}' conflicts with '{option} {other}'"),
                ));
            }
            if light {
                sel.light = id.clone();
                sel.theme = Theme::Light;
            } else {
                sel.dark = id.clone();
                sel.theme = Theme::Dark;
            }
        }
    }
    if flavor == Flavor::Gitlab && (sel.light != "light" || sel.dark != "dark") {
        return Err(usage(
            ErrorKind::InvalidValue,
            "the GitLab flavor has only the 'light' and 'dark' themes",
        ));
    }
    Ok(sel)
}

fn is_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    ["http://", "https://", "//", "data:"].iter().any(|p| lower.starts_with(p))
}

/// `--css`: URLs are kept, local files must exist and become absolute paths.
fn extra_css(values: &[String], cwd: &Path) -> Result<Vec<String>, CliError> {
    values
        .iter()
        .map(|v| {
            if is_url(v) {
                return Ok(v.clone());
            }
            let path = absolutize(cwd, Path::new(v));
            if !path.is_file() {
                return Err(runtime(format!("--css {v}: no such file")));
            }
            path.into_os_string()
                .into_string()
                .map_err(|_| runtime(format!("--css {v}: the path is not valid UTF-8")))
        })
        .collect()
}

fn invalid_address(addr: &str) -> CliError {
    let mut msg = format!(
        "invalid address '{addr}': expected <host>[:<port>], <port>, or [<ipv6>][:<port>]"
    );
    if addr.matches(':').count() > 1 && !addr.starts_with('[') {
        msg.push_str(&format!(" (put IPv6 addresses in brackets, e.g. [{addr}]:{DEFAULT_PORT})"));
    }
    usage(ErrorKind::ValueValidation, msg)
}

/// GRIP §2.3 + C10: positionals → (path, host, port); invalid addresses exit 2.
fn serve_plan(
    s: &ServeArgs,
    render: RenderPlan,
    host: &mut dyn Host,
    cwd: &Path,
    warnings: &mut Vec<String>,
) -> Result<ServePlan, CliError> {
    let (path, address): (Option<OsString>, Option<String>) = match (&s.path, &s.address) {
        (None, None) => (None, None),
        (Some(single), None) => match single.to_str() {
            Some(text) if address::resolve(Some(text), None).1.is_some() => {
                if cwd.join(text).exists() {
                    warnings.push(format!(
                        "treating '{text}' as the address to listen on; to serve the file '{text}' use ./{text}"
                    ));
                }
                (None, Some(text.to_owned()))
            }
            _ => (Some(single.clone()), None),
        },
        (p, Some(a)) => {
            let a = a.to_str().ok_or_else(|| invalid_address(&a.to_string_lossy()))?;
            (p.clone(), Some(a.to_owned()))
        }
    };

    let (host_name, port) = match &address {
        None => (DEFAULT_HOST.to_owned(), DEFAULT_PORT),
        Some(a) => {
            let hp = address::split_address(a).ok_or_else(|| invalid_address(a))?;
            // An empty host (`:80`) means the default host (grip passed '' to Flask, C26).
            let h = hp.host.filter(|h| !h.is_empty()).unwrap_or_else(|| DEFAULT_HOST.to_owned());
            (h, hp.port.unwrap_or(DEFAULT_PORT))
        }
    };

    let root_arg = s.root.as_deref();
    let (root, target) = match &path {
        Some(p) if p == OsStr::new("-") => {
            let root = match root_arg {
                Some(r) => canonical_root(cwd, r)?,
                None => fs::canonicalize(cwd)
                    .map_err(|e| runtime(format!("cannot resolve {}: {e}", cwd.display())))?,
            };
            let text = read_stdin(host, render.quiet, warnings)?;
            (root, Target::Stdin { name: STDIN_NAME.to_owned(), text })
        }
        Some(p) => fs_target(cwd, Path::new(p), root_arg, s.follow_symlinks, warnings)?,
        None => fs_target(cwd, Path::new("."), root_arg, s.follow_symlinks, warnings)?,
    };

    Ok(ServePlan {
        root,
        target,
        host: host_name,
        port,
        render,
        live: !s.norefresh,
        open_browser: s.browser,
        poll: s.poll.map(Duration::from_millis),
        follow_symlinks: s.follow_symlinks,
        auto_vendor: s.auto_vendor,
    })
}

fn canonical_root(cwd: &Path, root: &Path) -> Result<PathBuf, CliError> {
    let abs = absolutize(cwd, root);
    match fs::canonicalize(&abs) {
        Ok(c) if c.is_dir() => Ok(c),
        Ok(_) => Err(usage(
            ErrorKind::ValueValidation,
            format!("invalid value '{}' for '--root <DIR>': not a directory", root.display()),
        )),
        Err(e) => Err(usage(
            ErrorKind::ValueValidation,
            format!("invalid value '{}' for '--root <DIR>': {e}", root.display()),
        )),
    }
}

/// grip's "File not found", plus a hint when the path looks like a host name (GRIP §2.3: a
/// lone host without a port is a path).
fn not_found(path: &Path) -> CliError {
    let shown = path.display().to_string();
    let mut msg = format!("File not found: {shown}");
    if address::split_address(&shown).is_some_and(|hp| hp.host.is_some() && hp.port.is_none())
        && !shown.contains('/')
        && !shown.contains('.') | shown.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        msg.push_str(&format!(
            " (to listen on host '{shown}' give both a path and an address, e.g. `md-render . {shown}`)"
        ));
    }
    runtime(msg)
}

/// Root and target for a file or directory PATH (D14): the directory itself or the file's
/// parent, unless `--root` (which must contain the target) widens it.
fn fs_target(
    cwd: &Path,
    path: &Path,
    root_arg: Option<&Path>,
    follow_symlinks: bool,
    warnings: &mut Vec<String>,
) -> Result<(PathBuf, Target), CliError> {
    let abs = absolutize(cwd, path);
    let meta = fs::metadata(&abs).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => not_found(path),
        _ => runtime(format!("cannot access {}: {e}", path.display())),
    })?;
    let canon = |p: &Path| {
        fs::canonicalize(p).map_err(|e| runtime(format!("cannot resolve {}: {e}", p.display())))
    };
    let (natural_root, location, is_dir) = if meta.is_dir() {
        let dir = canon(&abs)?;
        (dir.clone(), dir, true)
    } else {
        // Keep the file name as given (a symlinked file keeps its own name); resolve the parent.
        let name = abs.file_name().ok_or_else(|| not_found(path))?.to_owned();
        let parent = canon(abs.parent().unwrap_or(cwd))?;
        (parent.clone(), parent.join(name), false)
    };
    let root = match root_arg {
        None => natural_root,
        Some(r) => canonical_root(cwd, r)?,
    };
    let rel = location.strip_prefix(&root).map(Path::to_path_buf).map_err(|_| {
        usage(
            ErrorKind::ValueValidation,
            format!(
                "--root {} does not contain {} (the served root must contain the file or directory)",
                root.display(),
                path.display()
            ),
        )
    })?;
    if !is_dir && !follow_symlinks {
        let is_link = fs::symlink_metadata(&abs).is_ok_and(|m| m.file_type().is_symlink());
        if is_link {
            if let Ok(real) = fs::canonicalize(&abs) {
                if !real.starts_with(&root) {
                    warnings.push(format!(
                        "{} is a symlink to {}, outside the served root {}; pass --follow-symlinks to serve it",
                        path.display(),
                        real.display(),
                        root.display()
                    ));
                }
            }
        }
    }
    Ok((root, if is_dir { Target::Dir(rel) } else { Target::File(rel) }))
}

/// Read stdin eagerly, before the server binds (GRIP C4).
fn read_stdin(host: &mut dyn Host, quiet: bool, warnings: &mut Vec<String>) -> Result<String, CliError> {
    if host.stdin_is_terminal() && !quiet {
        let eof = if cfg!(windows) { "Ctrl+Z, Enter" } else { "Ctrl+D" };
        host.notice(&format!("Reading Markdown from stdin (press {eof} when done)"));
    }
    let bytes = host
        .read_stdin()
        .map_err(|e| CliError::Runtime(anyhow::Error::new(e).context("reading stdin")))?;
    Ok(decode_text(bytes, STDIN_NAME, warnings))
}

/// GRIP §2.6 output naming: default `<input without extension>.html` next to the input (a
/// directory's README), `-` = stdout, input `-` = stdin (then stdout by default).
fn export_plan(
    a: &ExportArgs,
    inline: bool,
    render: RenderPlan,
    host: &mut dyn Host,
    cwd: &Path,
    warnings: &mut Vec<String>,
) -> Result<ExportPlan, CliError> {
    let input = match a.path.as_deref() {
        Some(p) if p == OsStr::new("-") => {
            let text = read_stdin(host, render.quiet, warnings)?;
            ExportInput::Stdin { name: STDIN_NAME.to_owned(), text }
        }
        p => {
            let given = Path::new(p.unwrap_or(OsStr::new(".")));
            let abs = absolutize(cwd, given);
            let meta = fs::metadata(&abs).map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => not_found(given),
                _ => runtime(format!("cannot access {}: {e}", given.display())),
            })?;
            if meta.is_dir() {
                let readme = find_readme(&abs, render.flavor, true)
                    .ok_or_else(|| runtime(format!("No README found at {}", given.display())))?;
                ExportInput::File(readme)
            } else {
                ExportInput::File(abs)
            }
        }
    };
    let output = match a.out.as_deref() {
        Some(o) if o == OsStr::new("-") => ExportOutput::Stdout,
        Some(o) => ExportOutput::File(absolutize(cwd, Path::new(o))),
        None => match &input {
            ExportInput::Stdin { .. } => ExportOutput::Stdout,
            ExportInput::File(f) => ExportOutput::File(f.with_extension("html")),
        },
    };
    if let (ExportInput::File(i), ExportOutput::File(o)) = (&input, &output) {
        let same = i == o
            || matches!((fs::canonicalize(i), fs::canonicalize(o)), (Ok(a), Ok(b)) if a == b);
        if same {
            return Err(usage(
                ErrorKind::ValueValidation,
                format!("the export would overwrite its input {}; give an output file", i.display()),
            ));
        }
    }
    Ok(ExportPlan { input, output, inline, render })
}

fn vendor_plan(
    a: &VendorArgs,
    flavor_arg: Option<FlavorArg>,
    bundle: (PathBuf, BundleSource),
    cwd: &Path,
    quiet: bool,
) -> Result<VendorPlan, CliError> {
    let flavors = match flavor_arg {
        Some(FlavorArg::Github) => vec![Flavor::Github],
        Some(FlavorArg::Gitlab) => vec![Flavor::Gitlab],
        Some(FlavorArg::All) | None => Flavor::ALL.to_vec(),
    };
    let github_source = a.github_source.clone().unwrap_or_else(|| GITHUB_SOURCE.to_owned());
    let gitlab_source = a.gitlab_source.clone().unwrap_or_else(|| GITLAB_SOURCE.to_owned());
    validate_http_url("--github-source", &github_source)?;
    validate_http_url("--gitlab-source", &gitlab_source)?;
    let (output, create_parent) = match &a.output {
        Some(o) => (absolutize(cwd, o), false),
        None => (bundle.0, bundle.1 == BundleSource::Default),
    };
    Ok(VendorPlan {
        options: VendorOptions {
            flavors,
            output,
            github_source,
            gitlab_source,
            all_github_themes: a.themes == ThemesArg::All,
            libs: !a.no_libs,
            quiet,
        },
        create_parent,
    })
}

fn cache_clear_plan(host: &dyn Host, bundle: (PathBuf, BundleSource)) -> Result<CacheClearPlan, CliError> {
    let cache_dir = host
        .cache_dir()
        .ok_or_else(|| runtime("cannot determine the cache directory (is HOME set?)"))?;
    let kept_bundle = match bundle.1 {
        BundleSource::Default => None,
        BundleSource::Flag | BundleSource::Env => {
            (!bundle.0.starts_with(&cache_dir)).then_some(bundle.0)
        }
    };
    Ok(CacheClearPlan { cache_dir, kept_bundle })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_name_hint_only_for_host_like_paths() {
        let msg = |p: &str| match not_found(Path::new(p)) {
            CliError::Runtime(e) => e.to_string(),
            other => panic!("{other:?}"),
        };
        assert!(msg("localhost").contains("md-render . localhost"));
        assert!(msg("0.0.0.0").contains("md-render . 0.0.0.0"));
        assert_eq!(msg("README.md"), "File not found: README.md");
        assert_eq!(msg("docs/x"), "File not found: docs/x");
        assert_eq!(msg("my_file"), "File not found: my_file");
    }

    #[test]
    fn url_detection() {
        assert!(is_url("https://x/y.css") && is_url("HTTP://x") && is_url("//cdn/x.css") && is_url("data:text/css,a"));
        assert!(!is_url("style.css") && !is_url("/abs/style.css") && !is_url("C:\\x.css"));
    }
}
