//! Live preview server (owner: server). Contract: docs/DESIGN.md §3.8, §6, §7.
//!
//! * [`start`] canonicalises the served root, loads the style bundle (D11), binds the
//!   listener(s), starts the file watcher and returns a [`RunningServer`] at once.
//! * Routes (`routes`): documents and directories (README lookup, listings, `/dir` -> `/dir/`),
//!   raw files, `/__/md-render/{static,bundle,css,events,render}`; path safety in `paths`.
//! * Live updates (`hub`, `events`, `watcher`, `deps`): one watcher, per-document workers
//!   that render once per real change, SSE fan-out to every subscriber (DESIGN §7); the
//!   browser side is `assets/client/live.js`.
//! * Shutdown: every SSE stream receives `shutdown` and ends, then axum's graceful shutdown
//!   completes (CR §3).

use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle};

use crate::flavor::{Flavor, Mode, ThemeSel};
use crate::page::Layout;
use crate::render::{RenderOptions, Rendered, Renderer};

mod deps;
mod events;
mod files;
mod hub;
mod paths;
mod routes;
mod styles;
mod watcher;

pub use paths::{GITLAB_EXTRA_EXTENSIONS, MARKDOWN_EXTENSIONS, is_markdown};

use events::WireEvent;
use hub::{Hub, RenderCtx};
use styles::{ExtraCss, Styles};
use watcher::FsWatcher;

/// How long graceful shutdown may take before remaining connections are dropped.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Default bind host (grip's default, GRIP C7).
pub const DEFAULT_HOST: &str = "localhost";

/// Default port (grip's default, GRIP C7).
pub const DEFAULT_PORT: u16 = 6419;

/// What `/` shows. Paths are relative to `ServeConfig::root`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A file (rendered at `/` when it is directly in the root, otherwise `/` redirects to it).
    File(PathBuf),
    /// A directory (`""` or `"."` for the root itself).
    Dir(PathBuf),
    /// Markdown read from stdin, rendered once (no live updates).
    Stdin { name: String, text: String },
}

/// Server configuration.
#[derive(Clone)]
pub struct ServeConfig {
    /// Served root directory.
    pub root: PathBuf,
    /// What `/` shows.
    pub target: Target,
    /// Host name or IP address to bind (`localhost`, `127.0.0.1`, `::1`, `0.0.0.0`, ...).
    pub host: String,
    /// Port (0 = pick a free one).
    pub port: u16,
    /// Site whose rendering and styles are reproduced.
    pub flavor: Flavor,
    /// README/file view or comment rendering.
    pub mode: Mode,
    /// Color theme selection.
    pub theme: ThemeSel,
    /// Page chrome.
    pub layout: Layout,
    /// Page `<title>` override (default: the file name).
    pub title: Option<String>,
    /// `owner/repo` | `group/project`.
    pub context: Option<String>,
    /// Live updates (false with `--norefresh`).
    pub live: bool,
    /// Open the browser once the listener is bound.
    pub open_browser: bool,
    /// No startup banner, change log lines or hints (warnings about broken input still
    /// appear).
    pub quiet: bool,
    /// Style bundle file (watched while serving).
    pub bundle_path: PathBuf,
    /// Vendor the flavor's styles at startup when the bundle lacks them.
    pub auto_vendor: bool,
    /// `--css` values: URLs, or local files (served and watched by the server).
    pub extra_css: Vec<String>,
    /// Offline or API renderer.
    pub renderer: Arc<dyn Renderer>,
    /// Use a polling watcher with this interval (`--poll[=MS]`).
    pub poll: Option<Duration>,
    /// Allow symlinks that point outside the root.
    pub follow_symlinks: bool,
}

impl ServeConfig {
    /// A configuration with grip's defaults (`localhost:6419`, GitHub flavor, live updates,
    /// default style bundle) for `root` and `target`; adjust the public fields as needed.
    pub fn new(root: PathBuf, target: Target, renderer: Arc<dyn Renderer>) -> ServeConfig {
        ServeConfig {
            root,
            target,
            host: DEFAULT_HOST.to_owned(),
            port: DEFAULT_PORT,
            flavor: Flavor::Github,
            mode: Mode::Readme,
            theme: ThemeSel::default(),
            layout: Layout::default(),
            title: None,
            context: None,
            live: true,
            open_browser: false,
            quiet: false,
            bundle_path: crate::styles::default_bundle_path(),
            auto_vendor: false,
            extra_css: Vec::new(),
            renderer,
            poll: None,
            follow_symlinks: false,
        }
    }
}

impl std::fmt::Debug for ServeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServeConfig")
            .field("root", &self.root)
            .field("target", &self.target)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("flavor", &self.flavor)
            .field("live", &self.live)
            .field("renderer", &self.renderer.name())
            .finish_non_exhaustive()
    }
}

/// A started server.
pub struct RunningServer {
    /// The bound address (the first one when `localhost` was bound for IPv4 and IPv6).
    pub addr: SocketAddr,
    url: String,
    shutdown: Arc<watch::Sender<bool>>,
    tasks: Vec<JoinHandle<std::io::Result<()>>>,
}

impl std::fmt::Debug for RunningServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningServer")
            .field("addr", &self.addr)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl RunningServer {
    /// The URL of `/`, as printed on startup (`http://localhost:6419/`).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A handle that stops the server when called (for signal handlers or tests).
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle(self.shutdown.clone())
    }

    /// Stop the server gracefully (SSE clients receive `shutdown`).
    pub async fn shutdown(self) {
        self.shutdown.send_replace(true);
        if let Err(e) = self.wait().await {
            tracing::warn!("server stopped with an error: {e:#}");
        }
    }

    /// Wait until the server stops (after [`ShutdownHandle::shutdown`] or an I/O error).
    pub async fn wait(self) -> anyhow::Result<()> {
        let RunningServer {
            shutdown,
            mut tasks,
            ..
        } = self;
        let aborts: Vec<AbortHandle> = tasks.iter().map(JoinHandle::abort_handle).collect();
        let mut stop_rx = shutdown.subscribe();
        let mut results = Vec::new();
        let joined = async {
            for t in &mut tasks {
                results.push(t.await);
            }
        };
        tokio::pin!(joined);
        tokio::select! {
            _ = &mut joined => {}
            _ = async {
                let _ = stop_rx.wait_for(|stop| *stop).await;
                tokio::time::sleep(SHUTDOWN_GRACE).await;
            } => {
                // Graceful shutdown is stuck (e.g. a slow download): drop the connections.
                for a in &aborts {
                    a.abort();
                }
                (&mut joined).await;
            }
        }
        // Stop workers, the watcher dispatcher and SSE tasks even if serving failed.
        shutdown.send_replace(true);
        for r in results {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(anyhow::Error::new(e).context("HTTP server failed")),
                Err(e) if e.is_cancelled() => {}
                Err(e) => bail!("HTTP server task failed: {e}"),
            }
        }
        Ok(())
    }
}

/// Requests a graceful shutdown of a [`RunningServer`].
#[derive(Clone, Debug)]
pub struct ShutdownHandle(Arc<watch::Sender<bool>>);

impl ShutdownHandle {
    /// Ask the server to stop (idempotent).
    pub fn shutdown(&self) {
        self.0.send_replace(true);
    }
}

/// The resolved `/` target.
pub(crate) enum RootTarget {
    File(String),
    Dir(String),
    Stdin {
        name: String,
        content: Arc<Rendered>,
    },
}

/// Allowed `Host` header values when bound to loopback (DNS-rebinding protection).
#[derive(Clone, Debug)]
pub(crate) struct HostAllow {
    names: Vec<String>,
    port: u16,
}

impl HostAllow {
    fn new(configured: &str, port: u16) -> HostAllow {
        let mut names: Vec<String> = ["localhost", "127.0.0.1", "[::1]"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let host = configured
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        let host = if host.parse::<std::net::Ipv6Addr>().is_ok() {
            format!("[{host}]")
        } else {
            host
        };
        if !names.contains(&host) {
            names.push(host);
        }
        HostAllow { names, port }
    }

    /// Does a `Host` header value (`name[:port]`) name this server?
    pub(crate) fn allows(&self, value: &str) -> bool {
        let value = value.trim().to_ascii_lowercase();
        let (name, port) = if value.starts_with('[') {
            match value.find(']') {
                Some(end) => (&value[..=end], &value[end + 1..]),
                None => return false,
            }
        } else {
            match value.rfind(':') {
                Some(i) => (&value[..i], &value[i..]),
                None => (value.as_str(), ""),
            }
        };
        let port_ok = match port {
            "" => true,
            p => p.strip_prefix(':').and_then(|p| p.parse::<u16>().ok()) == Some(self.port),
        };
        let name = name.trim_end_matches('.');
        // `*.localhost` always resolves to loopback in browsers (RFC 6761), so it is safe.
        port_ok && (self.names.iter().any(|n| n == name) || name.ends_with(".localhost"))
    }
}

/// Shared state of a running server.
pub(crate) struct App {
    pub flavor: Flavor,
    pub mode: Mode,
    pub theme: ThemeSel,
    pub layout: Layout,
    pub title: Option<String>,
    pub live: bool,
    pub follow_symlinks: bool,
    /// Canonical served root.
    pub root: PathBuf,
    pub target: RootTarget,
    /// Random per process; clients reload when it changes (server restart).
    pub server_id: String,
    pub hub: Hub,
    pub styles: Arc<Styles>,
    /// Global events (`styles`, bundle errors) for every subscriber.
    pub global: broadcast::Sender<Arc<WireEvent>>,
    pub shutdown: watch::Receiver<bool>,
    pub hosts: Option<HostAllow>,
}

impl App {
    /// May `rel` resolve outside the root (`--follow-symlinks`, or the target file the user
    /// named; see [`RenderCtx::follows`])?
    pub(crate) fn follows(&self, rel: &str) -> bool {
        self.follow_symlinks || matches!(&self.target, RootTarget::File(t) if t == rel)
    }

    /// Is `rel` rendered as a document (Markdown extension, or the configured target file)?
    pub(crate) fn is_document(&self, rel: &str) -> bool {
        paths::is_markdown(rel, self.flavor)
            || matches!(&self.target, RootTarget::File(t) if t == rel)
    }

    /// [`paths::resolve`] for request path `rel`, off the reactor: canonicalisation and
    /// metadata calls block on a slow or hung (network) file system (Q-ASYNC-1).
    pub(crate) async fn resolve(&self, rel: &str) -> Result<paths::Resolved, paths::ResolveError> {
        let (root, rel, follow) = (self.root.clone(), rel.to_owned(), self.follows(rel));
        blocking(move || paths::resolve(&root, &rel, follow))
            .await
            .unwrap_or_else(|e| Err(paths::ResolveError::Io(e)))
    }

    /// Validate a `?path=` parameter of the events / render endpoints: a Markdown document,
    /// or a directory URL (trailing `/`, `/` for the root) whose listing is live (LIVE-9).
    /// Returns the hub key.
    pub(crate) async fn validate_doc_param(
        &self,
        path: Option<&str>,
    ) -> Result<String, (axum::http::StatusCode, String)> {
        use axum::http::StatusCode;
        let path = path.filter(|p| !p.is_empty()).ok_or((
            StatusCode::BAD_REQUEST,
            "missing ?path=<document>".to_owned(),
        ))?;
        let rp = paths::RequestPath::from_decoded(path).map_err(|e| {
            (
                StatusCode::from_u16(e.status()).unwrap_or(StatusCode::BAD_REQUEST),
                e.to_string(),
            )
        })?;
        let rel = rp.rel();
        let listing = rp.is_root() || rp.trailing_slash;
        if !listing && !self.is_document(&rel) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("{path:?} is not a Markdown document path"),
            ));
        }
        match self.resolve(&rel).await {
            Ok(paths::Resolved::Dir(_) | paths::Resolved::Missing) if listing => {
                Ok(hub::listing_key(&rel))
            }
            Ok(paths::Resolved::File(_)) if listing => Err((
                StatusCode::BAD_REQUEST,
                format!("{path:?} is not a directory"),
            )),
            Ok(paths::Resolved::File(_) | paths::Resolved::Missing) => Ok(rel),
            Ok(paths::Resolved::Dir(_)) => {
                Err((StatusCode::BAD_REQUEST, format!("{path:?} is a directory")))
            }
            Err(paths::ResolveError::Forbidden(m)) => Err((StatusCode::FORBIDDEN, m)),
            Err(paths::ResolveError::Io(e)) => {
                Err((StatusCode::INTERNAL_SERVER_ERROR, format!("{path:?}: {e}")))
            }
        }
    }
}

/// Bind and start serving in the background; returns once the listener is bound.
pub async fn start(cfg: ServeConfig) -> anyhow::Result<RunningServer> {
    let root = std::fs::canonicalize(&cfg.root)
        .with_context(|| format!("cannot serve {}: directory not found", cfg.root.display()))?;
    if !root.is_dir() {
        bail!("cannot serve {}: not a directory", cfg.root.display());
    }
    let quiet = cfg.quiet;
    let log = move |msg: &str| {
        if !quiet {
            eprintln!(" * {msg}");
        }
    };

    let bundle =
        styles::load_initial(&cfg.bundle_path, cfg.flavor, cfg.auto_vendor, cfg.quiet).await;
    let (css, css_warnings) = ExtraCss::from_config(&cfg.extra_css);
    for w in css_warnings {
        eprintln!("md-render: warning: {w}");
    }

    let listeners = bind(&cfg.host, cfg.port).await?;
    let addrs: Vec<SocketAddr> = listeners
        .iter()
        .filter_map(|l| l.local_addr().ok())
        .collect();
    let addr = *addrs.first().context("no listening address")?;
    let loopback = addrs.iter().all(|a| a.ip().is_loopback());

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let shutdown_tx = Arc::new(shutdown_tx);
    let (global, _) = broadcast::channel(16);

    let (path_tx, path_rx) = mpsc::unbounded_channel();
    let watcher = if cfg.live {
        match FsWatcher::new(cfg.poll, path_tx) {
            Ok((w, warning)) => {
                if let Some(w) = warning {
                    eprintln!("md-render: warning: {w}");
                }
                Some(Arc::new(w))
            }
            Err(e) => {
                eprintln!("md-render: warning: live updates are disabled: {e:#}");
                None
            }
        }
    } else {
        None
    };

    let options = RenderOptions {
        flavor: cfg.flavor,
        mode: cfg.mode,
        context: cfg.context.clone(),
        source_path: None,
        root: Some(root.clone()),
        sourcepos: true,
        bundle_url: format!("{}/bundle", crate::PREFIX),
    };
    let target = match &cfg.target {
        Target::Stdin { name, text } => {
            let renderer = cfg.renderer.clone();
            let (text, flavor, options) = (text.clone(), cfg.flavor, options.clone());
            let rendered =
                tokio::task::spawn_blocking(move || renderer.render(&text, &options)).await;
            let content = match rendered {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => error_content(flavor, &format!("rendering {name} failed: {e:#}")),
                Err(e) => error_content(flavor, &format!("rendering {name} failed: {e}")),
            };
            RootTarget::Stdin {
                name: name.clone(),
                content: Arc::new(content),
            }
        }
        Target::File(p) => {
            let rel = target_rel(&root, p)?;
            if paths::join_rel(&root, &rel).is_dir() {
                RootTarget::Dir(rel)
            } else {
                RootTarget::File(rel)
            }
        }
        Target::Dir(p) => RootTarget::Dir(target_rel(&root, p)?),
    };
    let ctx = Arc::new(RenderCtx {
        renderer: cfg.renderer.clone(),
        options,
        root: root.clone(),
        follow_symlinks: cfg.follow_symlinks,
        target: match &target {
            RootTarget::File(rel) => Some(rel.clone()),
            _ => None,
        },
        quiet: cfg.quiet,
        watcher: watcher.clone(),
    });

    let styles = Arc::new(Styles::new(
        cfg.flavor,
        cfg.bundle_path.clone(),
        bundle,
        css,
        global.clone(),
        cfg.quiet,
    ));
    if let Some(w) = &watcher {
        styles.watch(w.clone());
    }

    let app = Arc::new(App {
        flavor: cfg.flavor,
        mode: cfg.mode,
        theme: cfg.theme.clone(),
        layout: cfg.layout,
        title: cfg.title.clone(),
        live: cfg.live && watcher.is_some(),
        follow_symlinks: cfg.follow_symlinks,
        root,
        target,
        server_id: new_server_id(),
        hub: Hub::new(ctx, shutdown_rx.clone()),
        styles: styles.clone(),
        global,
        shutdown: shutdown_rx.clone(),
        hosts: loopback.then(|| HostAllow::new(&cfg.host, addr.port())),
    });

    if let Some(w) = &watcher {
        tokio::spawn(dispatch(path_rx, app.clone(), shutdown_rx.clone()));
        tokio::spawn(styles.clone().run(shutdown_rx.clone()));
        if !w.is_polling() {
            // FSEvents may drop changes made right after `watch()` (CR §5): re-check once.
            let s = styles.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(750)).await;
                s.recheck();
            });
        }
    }

    let router = routes::router(app.clone());
    let mut tasks = Vec::new();
    for listener in listeners {
        let router = router.clone();
        let mut stop = shutdown_rx.clone();
        tasks.push(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stop.wait_for(|s| *s).await;
                })
                .await
        }));
    }

    let url = display_url(&cfg.host, addr);
    for line in banner(&url, &addrs, network_address) {
        log(&line);
    }
    if addrs.len() > 1 {
        let all: Vec<String> = addrs.iter().map(ToString::to_string).collect();
        tracing::debug!("listening on {}", all.join(", "));
    }
    if cfg.open_browser
        && let Err(e) = open::that_detached(&url)
    {
        eprintln!("md-render: warning: could not open a browser for {url}: {e}");
    }
    Ok(RunningServer {
        addr,
        url,
        shutdown: shutdown_tx,
        tasks,
    })
}

/// Start, then run until Ctrl-C (or SIGTERM).
pub async fn serve(cfg: ServeConfig) -> anyhow::Result<()> {
    let quiet = cfg.quiet;
    let server = start(cfg).await?;
    let handle = server.shutdown_handle();
    tokio::spawn(async move {
        wait_for_signal().await;
        if !quiet {
            eprintln!(" * Shutting down...");
        }
        handle.shutdown();
        // The handler stays installed while connections drain (up to SHUTDOWN_GRACE): a
        // second Ctrl-C must not be swallowed (LIVE-11).
        wait_for_signal().await;
        eprintln!(" * Forced exit");
        std::process::exit(130);
    });
    server.wait().await
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    r = tokio::signal::ctrl_c() => {
                        if r.is_err() {
                            std::future::pending::<()>().await;
                        }
                    }
                    _ = term.recv() => {}
                }
            }
            Err(_) => ctrl_c_or_pending().await,
        }
    }
    #[cfg(not(unix))]
    ctrl_c_or_pending().await;
}

async fn ctrl_c_or_pending() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::warn!("cannot listen for Ctrl-C: {e}");
        std::future::pending::<()>().await;
    }
}

/// Forward watcher batches to the style state and the document hub until shutdown.
async fn dispatch(
    mut rx: mpsc::UnboundedReceiver<Vec<PathBuf>>,
    app: Arc<App>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let batch = tokio::select! {
            biased;
            _ = stop.wait_for(|s| *s) => break,
            b = rx.recv() => match b {
                Some(b) => b,
                None => break,
            },
        };
        let mut paths = batch;
        while let Ok(more) = rx.try_recv() {
            paths.extend(more);
        }
        paths.sort();
        paths.dedup();
        app.styles.on_paths_changed(&paths);
        app.hub.on_paths_changed(&paths);
    }
}

/// Run blocking file-system work on tokio's blocking pool (Q-ASYNC-1).
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(std::io::Error::other)
}

/// A content fragment showing an error (stdin render failures).
fn error_content(flavor: Flavor, message: &str) -> Rendered {
    eprintln!("md-render: {message}");
    Rendered {
        html: crate::page::error_html(flavor, 500, message),
        ..Default::default()
    }
}

/// Normalise a target path (relative to the root, or absolute inside it) to a validated
/// root-relative `/`-separated path.
fn target_rel(root: &Path, p: &Path) -> anyhow::Result<String> {
    let rel_path: PathBuf = if p.is_absolute() {
        let canon = std::fs::canonicalize(p).or_else(|_| {
            // A target file that does not exist yet: canonicalise its directory.
            let parent = p.parent().context("target has no parent directory")?;
            let name = p.file_name().context("target has no file name")?;
            Ok::<_, anyhow::Error>(std::fs::canonicalize(parent)?.join(name))
        })?;
        canon
            .strip_prefix(root)
            .with_context(|| {
                format!(
                    "{} is outside the served root {}",
                    p.display(),
                    root.display()
                )
            })?
            .to_path_buf()
    } else {
        p.to_path_buf()
    };
    let mut segs = Vec::new();
    for c in rel_path.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(s) => segs.push(
                s.to_str()
                    .with_context(|| format!("{} is not valid UTF-8", p.display()))?
                    .to_owned(),
            ),
            _ => bail!(
                "target {} must be inside the served root {}",
                p.display(),
                root.display()
            ),
        }
    }
    let rel = segs.join("/");
    paths::RequestPath::from_decoded(&rel)
        .map_err(|e| anyhow::anyhow!("cannot serve {}: {e}", p.display()))?;
    Ok(rel)
}

/// How often binding `localhost:0` picks a new port when the one chosen for the first
/// loopback family is taken on the other.
const PICK_PORT_ATTEMPTS: usize = 8;

/// Resolve and bind `host:port`. For `localhost` both loopback families are bound on the
/// same port when possible (GRIP C7); the first address decides the port.
///
/// When another program already listens on the other family's loopback address, the bind
/// fails like a port in use on the first one (CLI-4): `http://localhost:PORT/` would reach
/// that program from every client that resolves `localhost` to it. Other errors there (no
/// IPv6 on this host, ...) only mean serving one family.
async fn bind(host: &str, port: u16) -> anyhow::Result<Vec<TcpListener>> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host((bare, port))
        .await
        .with_context(|| format!("cannot resolve host {host:?}"))?
        .collect();
    addrs.dedup();
    let first = *addrs
        .first()
        .with_context(|| format!("host {host:?} has no addresses"))?;
    let in_use = |e: std::io::Error, ip: IpAddr| {
        let what = if e.kind() == std::io::ErrorKind::AddrInUse {
            format!("port {port} is already in use on {ip}")
        } else {
            format!("cannot listen on {}", SocketAddr::new(ip, port))
        };
        anyhow::Error::new(e).context(what)
    };
    let other = addrs
        .iter()
        .find(|a| a.ip().is_loopback() && a.is_ipv4() != first.is_ipv4())
        .filter(|_| bare.eq_ignore_ascii_case("localhost"))
        .map(SocketAddr::ip);
    let mut attempt = 0;
    loop {
        attempt += 1;
        let primary = TcpListener::bind(first)
            .await
            .map_err(|e| in_use(e, first.ip()))?;
        let bound_port = primary
            .local_addr()
            .context("reading the bound address")?
            .port();
        let mut out = vec![primary];
        let Some(other) = other else {
            return Ok(out);
        };
        match TcpListener::bind(SocketAddr::new(other, bound_port)).await {
            Ok(l) => out.push(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if port != 0 {
                    return Err(in_use(e, other));
                }
                if attempt < PICK_PORT_ATTEMPTS {
                    continue; // the free port picked on `first` is taken on `other`
                }
                tracing::warn!(
                    "serving {} only: no free port on both loopback addresses",
                    first.ip()
                );
            }
            Err(e) => tracing::debug!("not listening on {other}: {e}"),
        }
        return Ok(out);
    }
}

/// The URL to print and open: wildcard addresses map to `localhost`, IPv6 is bracketed.
fn display_url(host: &str, addr: SocketAddr) -> String {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let shown = match bare.parse::<IpAddr>() {
        Ok(ip) if ip.is_unspecified() => "localhost".to_owned(),
        Ok(IpAddr::V6(ip)) => format!("[{ip}]"),
        Ok(IpAddr::V4(ip)) => ip.to_string(),
        Err(_) if bare.is_empty() => "localhost".to_owned(),
        Err(_) => bare.to_owned(),
    };
    format!("http://{shown}:{}/", addr.port())
}

/// The startup lines (printed as ` * {line}`) for a server at `url` listening on `addrs`.
///
/// A wildcard bind is reachable from the network: the banner says so and adds this
/// machine's address on the network (`lan(ipv6)`), like Werkzeug/grip, instead of showing
/// only the local URL (CLI-19).
fn banner(url: &str, addrs: &[SocketAddr], lan: impl Fn(bool) -> Option<IpAddr>) -> Vec<String> {
    let mut lines = Vec::new();
    let wildcard = addrs.iter().find(|a| a.ip().is_unspecified());
    if let Some(w) = wildcard {
        lines.push(format!("Running on all addresses ({})", w.ip()));
    }
    lines.push(format!("Running on {url} (Press CTRL+C to quit)"));
    if let Some(w) = wildcard
        && let Some(ip) = lan(w.is_ipv6())
    {
        lines.push(format!("Running on {}", display_url(&ip.to_string(), *w)));
    }
    lines
}

/// This machine's address on its default route (the one other machines reach it by), for
/// the startup banner of a wildcard bind. Connecting a UDP socket sends nothing; it only
/// asks the OS which local address it would use (here towards a documentation address).
fn network_address(v6: bool) -> Option<IpAddr> {
    let (local, remote) = if v6 {
        ("[::]:0", "[2001:db8::1]:80")
    } else {
        ("0.0.0.0:0", "192.0.2.1:80")
    };
    let socket = std::net::UdpSocket::bind(local).ok()?;
    socket.connect(remote).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// A random identifier for this server process.
fn new_server_id() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_allowlist() {
        let allow = HostAllow::new("localhost", 6419);
        for ok in [
            "localhost",
            "localhost:6419",
            "127.0.0.1:6419",
            "[::1]:6419",
            "LOCALHOST:6419",
            "app.localhost:6419",
        ] {
            assert!(allow.allows(ok), "{ok}");
        }
        for bad in [
            "evil.com",
            "evil.com:6419",
            "localhost:80",
            "127.0.0.1:1",
            "[::1",
            "localhost.evil.com:6419",
            "10.0.0.1:6419",
        ] {
            assert!(!allow.allows(bad), "{bad}");
        }
        let v6 = HostAllow::new("::1", 8080);
        assert!(v6.allows("[::1]:8080"));
        let named = HostAllow::new("127.0.0.2", 1);
        assert!(named.allows("127.0.0.2:1"));
    }

    #[test]
    fn urls() {
        let a: SocketAddr = "127.0.0.1:6419".parse().unwrap();
        assert_eq!(display_url("localhost", a), "http://localhost:6419/");
        assert_eq!(display_url("0.0.0.0", a), "http://localhost:6419/");
        assert_eq!(display_url("::", a), "http://localhost:6419/");
        assert_eq!(display_url("::1", a), "http://[::1]:6419/");
        assert_eq!(display_url("[::1]", a), "http://[::1]:6419/");
        assert_eq!(display_url("192.168.1.2", a), "http://192.168.1.2:6419/");
    }

    /// CLI-19: a wildcard bind says that it serves all addresses and where the network
    /// reaches it; other binds print the one URL as before.
    #[test]
    fn startup_banner() {
        let lan = |v6: bool| {
            Some(if v6 {
                "2001:db8::5".parse().unwrap()
            } else {
                "192.168.1.5".parse().unwrap()
            })
        };
        let any: SocketAddr = "0.0.0.0:6419".parse().unwrap();
        assert_eq!(
            banner("http://localhost:6419/", &[any], lan),
            [
                "Running on all addresses (0.0.0.0)",
                "Running on http://localhost:6419/ (Press CTRL+C to quit)",
                "Running on http://192.168.1.5:6419/",
            ]
        );
        let any6: SocketAddr = "[::]:7213".parse().unwrap();
        assert_eq!(
            banner("http://localhost:7213/", &[any6], lan),
            [
                "Running on all addresses (::)",
                "Running on http://localhost:7213/ (Press CTRL+C to quit)",
                "Running on http://[2001:db8::5]:7213/",
            ]
        );
        assert_eq!(banner("http://localhost:1/", &[any], |_| None).len(), 2);
        let local: SocketAddr = "127.0.0.1:6419".parse().unwrap();
        assert_eq!(
            banner("http://localhost:6419/", &[local], lan),
            ["Running on http://localhost:6419/ (Press CTRL+C to quit)"]
        );
        // Whatever the machine's network, it is never a loopback or wildcard address.
        for v6 in [false, true] {
            if let Some(ip) = network_address(v6) {
                assert!(!ip.is_loopback() && !ip.is_unspecified(), "{ip}");
            }
        }
    }

    #[test]
    fn target_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("docs")).unwrap();
        assert_eq!(
            target_rel(&root, Path::new("README.md")).unwrap(),
            "README.md"
        );
        assert_eq!(
            target_rel(&root, Path::new("./docs/a.md")).unwrap(),
            "docs/a.md"
        );
        assert_eq!(target_rel(&root, Path::new("")).unwrap(), "");
        assert_eq!(target_rel(&root, Path::new(".")).unwrap(), "");
        assert_eq!(
            target_rel(&root, &root.join("docs/new.md")).unwrap(),
            "docs/new.md"
        );
        assert!(target_rel(&root, Path::new("../x.md")).is_err());
        assert!(target_rel(&root, Path::new("/")).is_err());
        assert!(target_rel(&root, Path::new(".git/config")).is_err());
    }

    #[test]
    fn config_defaults_follow_grip() {
        let cfg = ServeConfig::new(
            ".".into(),
            Target::Dir(PathBuf::new()),
            Arc::new(crate::render::OfflineRenderer),
        );
        assert_eq!((cfg.host.as_str(), cfg.port), ("localhost", 6419));
        assert!(cfg.live && !cfg.open_browser && !cfg.follow_symlinks && cfg.poll.is_none());
        assert_eq!(cfg.bundle_path, crate::styles::default_bundle_path());
    }

    /// CLI-4: `localhost:PORT` with another program on one loopback address is a port in
    /// use, not a server that only half of the clients reach.
    #[tokio::test]
    async fn localhost_with_the_port_taken_on_one_family_fails() {
        let families: Vec<SocketAddr> = match tokio::net::lookup_host(("localhost", 0)).await {
            Ok(a) => a.collect(),
            Err(_) => return,
        };
        let has = |v4: bool| families.iter().any(|a| a.is_ipv4() == v4);
        if !has(true) || !has(false) || std::net::TcpListener::bind("[::1]:0").is_err() {
            eprintln!("SKIPPED: localhost does not resolve to both loopback families here");
            return;
        }
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = other.local_addr().unwrap().port();
        let err = bind("localhost", port).await.err().expect("bind must fail");
        assert!(
            format!("{err:#}").contains(&format!("port {port} is already in use on 127.0.0.1")),
            "{err:#}"
        );
        // A free port binds both families.
        let both = bind("localhost", 0).await.unwrap();
        assert_eq!(both.len(), 2);
        let ports: Vec<u16> = both
            .iter()
            .map(|l| l.local_addr().unwrap().port())
            .collect();
        assert_eq!(ports[0], ports[1]);
    }

    #[test]
    fn server_ids_differ() {
        assert_ne!(new_server_id(), new_server_id());
        assert_eq!(new_server_id().len(), 16);
    }
}
