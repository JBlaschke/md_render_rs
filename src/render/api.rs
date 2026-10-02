//! `--api` renderers (owner: cli). Contract: docs/DESIGN.md §2 D15, §3.10.
//!
//! * **GitHub**: `POST {api}/markdown` with `{"text", "mode": "gfm", "context"?}` (GH §2.1, §2.5),
//!   headers `Accept: application/vnd.github+json` and `X-GitHub-Api-Version: 2022-11-28`, optional
//!   `Authorization: Bearer`. The HTML is post-processed towards the README/file view (GH §9.A):
//!   heading wrappers + anchors in readme mode, inline octicon SVGs, canonical (non-camo) image
//!   URLs, clipboard wrappers, mermaid enrichment sections and deterministic run ids.
//!   Soft line breaks stay `<br>` in readme mode: a documented difference (CQ §1.5).
//! * **GitLab**: `POST {instance}/api/v4/markdown` with `{"text", "gfm": true, "project"?}` and
//!   `PRIVATE-TOKEN` (GL §9); lazy images are un-lazied and highlighted `<pre>`s get
//!   `code-syntax-highlight-theme` (GL §6.8, §6.12).
//!
//! API responses are cached by `sha256(flavor, mode, context, text)`. When the API is unusable
//! (rate limit, bad credentials, network) and [`ApiConfig::fallback_offline`] is set, the offline
//! renderer is used and a warning explains why. Tokens are never logged or printed.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use dom_query::{Document, NodeRef};
use regex::Regex;
use sha2::{Digest as _, Sha256};

use crate::flavor::{Flavor, Mode};
use crate::html;

use super::{Features, RenderOptions, Rendered, Renderer};

/// Default GitHub API base URL.
pub const GITHUB_API_URL: &str = "https://api.github.com";
/// Default GitLab instance URL.
pub const GITLAB_URL: &str = "https://gitlab.com";
/// `User-Agent` sent with every API request.
pub const USER_AGENT: &str = concat!("md-render/", env!("CARGO_PKG_VERSION"));
/// GitHub rejects Markdown documents larger than this (grip #258).
pub const GITHUB_MAX_TEXT_BYTES: usize = 400 * 1024;

/// Number of API responses kept in memory.
const CACHE_CAPACITY: usize = 128;
/// Total size of the API responses kept in memory, like the highlight memo's budget: every edit
/// of a large document adds a new response, and only reverts ever hit the old ones.
const CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;
/// Largest API response body accepted.
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
/// After a network failure, skip the API for this long (keeps live preview snappy offline).
const NETWORK_BACKOFF: Duration = Duration::from_secs(15);
/// Rate limited without a reset time: retry after this long.
const DEFAULT_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(60);
/// Credential helpers (`gh`, `glab`) must answer within this time.
const HELPER_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration of the API renderer.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiConfig {
    pub flavor: Flavor,
    /// GitHub API base (default `https://api.github.com`) or GitLab instance (default `https://gitlab.com`).
    pub api_url: Option<String>,
    /// Explicit token (else resolved from the environment / `gh`).
    pub token: Option<String>,
    /// Fall back to the offline renderer (with a warning) on auth/rate-limit/network errors.
    pub fallback_offline: bool,
}

impl fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiConfig")
            .field("flavor", &self.flavor)
            .field("api_url", &self.api_url.as_deref().map(redact_url))
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("fallback_offline", &self.fallback_offline)
            .finish()
    }
}

// ------------------------------------------------------------------------------------------------
// HTTP transport (injectable for tests)
// ------------------------------------------------------------------------------------------------

/// One JSON `POST` to a render API.
#[derive(Clone, PartialEq)]
pub struct ApiRequest {
    pub url: String,
    /// Header name/value pairs. Credentials are redacted by the `Debug` impl.
    pub headers: Vec<(String, String)>,
    pub body: serde_json::Value,
}

impl ApiRequest {
    /// Value of the first header called `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

impl fmt::Debug for ApiRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                let secret = k.eq_ignore_ascii_case("authorization")
                    || k.eq_ignore_ascii_case("private-token");
                (k.as_str(), if secret { "<redacted>" } else { v.as_str() })
            })
            .collect();
        f.debug_struct("ApiRequest")
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body", &self.body)
            .finish()
    }
}

/// Value of the first header called `name` in `headers` (case-insensitive).
fn find_header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// An HTTP response (any status).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    /// Header name/value pairs; names are matched case-insensitively.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl ApiResponse {
    /// Value of the first header called `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

/// The request never produced a response (DNS, connect, TLS, timeout, …).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TransportError(pub String);

/// Sends API requests. The default is [`UreqTransport`]; tests inject canned responses.
pub trait HttpTransport: Send + Sync {
    /// `POST` `request.body` as JSON to `request.url` with `request.headers`.
    ///
    /// # Errors
    /// Only when no response arrived (DNS, connect, TLS, timeout, unreadable body); every HTTP
    /// status, including 4xx/5xx, is an `Ok` [`ApiResponse`].
    fn post_json(&self, request: &ApiRequest) -> Result<ApiResponse, TransportError>;
}

/// [`HttpTransport`] backed by a blocking `ureq` agent (rustls, OS trust store).
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    /// An agent that never follows redirects: a 3xx is returned as an [`ApiResponse`].
    ///
    /// The render endpoints never redirect on a working instance, and following one is never
    /// right: a 301/302/303 turns the `POST` into a `GET` (whose answer, e.g. a login page,
    /// would be taken for the rendered document), and ureq strips only `Authorization` on the
    /// hop, so GitLab's `PRIVATE-TOKEN` would reach whatever host the `Location` names.
    pub fn new() -> Self {
        use ureq::tls::{RootCerts, TlsConfig};
        let config = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();
        UreqTransport {
            agent: config.into(),
        }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for UreqTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UreqTransport")
    }
}

impl HttpTransport for UreqTransport {
    fn post_json(&self, request: &ApiRequest) -> Result<ApiResponse, TransportError> {
        let mut builder = self.agent.post(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let mut response = builder
            .send_json(&request.body)
            .map_err(|e| TransportError(e.to_string()))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|e| TransportError(format!("reading the response body: {e}")))?;
        Ok(ApiResponse {
            status,
            headers,
            body,
        })
    }
}

// ------------------------------------------------------------------------------------------------
// Errors
// ------------------------------------------------------------------------------------------------

/// Why an API render failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// No response (DNS, connect, TLS, timeout).
    Network {
        site: Flavor,
        url: String,
        message: String,
    },
    /// HTTP 401: missing or invalid token.
    Unauthorized {
        site: Flavor,
        message: Option<String>,
        had_token: bool,
        /// Host the token is for (`github.com`, a GitHub Enterprise or a GitLab host); it
        /// selects the variables named in the hint.
        host: String,
    },
    /// HTTP 403 that is not a rate limit (e.g. no access to the `--context` project).
    Forbidden {
        site: Flavor,
        message: Option<String>,
    },
    /// HTTP 429, or a 403 rate-limit response.
    RateLimited {
        site: Flavor,
        message: Option<String>,
        limit: Option<u64>,
        /// Unix time (seconds) when the limit resets.
        reset: Option<i64>,
        /// `Retry-After` seconds.
        retry_after: Option<u64>,
        had_token: bool,
        /// Host the token is for (see [`ApiError::Unauthorized`]).
        host: String,
    },
    /// The document exceeds the API's size limit.
    TooLarge {
        site: Flavor,
        size: usize,
        limit: usize,
    },
    /// Any other non-success status.
    Status {
        site: Flavor,
        status: u16,
        message: Option<String>,
    },
    /// A success status with an unusable body.
    BadResponse { site: Flavor, message: String },
}

impl ApiError {
    /// The site this error came from.
    pub fn site(&self) -> Flavor {
        match self {
            ApiError::Network { site, .. }
            | ApiError::Unauthorized { site, .. }
            | ApiError::Forbidden { site, .. }
            | ApiError::RateLimited { site, .. }
            | ApiError::TooLarge { site, .. }
            | ApiError::Status { site, .. }
            | ApiError::BadResponse { site, .. } => *site,
        }
    }

    /// Like `Display`, with rate-limit reset times shown in `tz` relative to `now` (unix seconds).
    pub fn describe(&self, tz: &jiff::tz::TimeZone, now: i64) -> String {
        let site = site_name(self.site());
        let detail =
            |m: &Option<String>| m.as_deref().map(|m| format!(" ({m})")).unwrap_or_default();
        match self {
            ApiError::Network { url, message, .. } => {
                format!("could not reach the {site} API at {url}: {message}")
            }
            ApiError::Unauthorized {
                message,
                had_token,
                host,
                ..
            } => {
                let hint = if *had_token {
                    "check that the token is valid and has not expired".to_owned()
                } else {
                    format!("{} or pass --token", token_env_hint(self.site(), host))
                };
                format!(
                    "the {site} API rejected the request as unauthorized{}; {hint}",
                    detail(message)
                )
            }
            ApiError::Forbidden { message, .. } => {
                format!(
                    "the {site} API refused the request (HTTP 403){}",
                    detail(message)
                )
            }
            ApiError::RateLimited {
                message,
                limit,
                reset,
                retry_after,
                had_token,
                host,
                ..
            } => {
                let mut s = format!("{site} API rate limit exceeded");
                if let Some(limit) = limit {
                    s.push_str(&format!(" ({limit} requests per window)"));
                }
                if let Some(reset) = reset {
                    s.push_str(&format!("; it resets {}", describe_reset(*reset, now, tz)));
                } else if let Some(secs) = retry_after {
                    s.push_str(&format!("; retry in {}", describe_duration(*secs)));
                } else if let Some(m) = message {
                    s.push_str(&format!(" ({m})"));
                }
                if !had_token {
                    s.push_str(&format!(
                        "; authenticated requests get a higher limit: {} or pass --token",
                        token_env_hint(self.site(), host)
                    ));
                }
                s
            }
            ApiError::TooLarge { size, limit, .. } => format!(
                "the document ({} KiB) exceeds the {site} API limit of {} KiB",
                size.div_ceil(1024),
                limit / 1024
            ),
            ApiError::Status {
                status, message, ..
            } => {
                format!("the {site} API returned HTTP {status}{}", detail(message))
            }
            ApiError::BadResponse { message, .. } => {
                format!("unexpected response from the {site} API: {message}")
            }
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe(&jiff::tz::TimeZone::system(), unix_now()))
    }
}

impl std::error::Error for ApiError {}

fn site_name(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Github => "GitHub",
        Flavor::Gitlab => "GitLab",
    }
}

/// Whether the GitHub token host `host` (see [`token_host`]) takes github.com credentials:
/// github.com itself and GHE.com data-residency tenants (`TENANT.ghe.com`). Every other host is
/// a GitHub Enterprise Server, which gets `GH_ENTERPRISE_TOKEN` / `GITHUB_ENTERPRISE_TOKEN`
/// (gh's convention), never the github.com token.
fn takes_github_com_tokens(host: &str) -> bool {
    host == "github.com" || host.ends_with(".ghe.com")
}

/// "set GITHUB_TOKEN (...)": how to provide a token for `host` (see [`resolve_token`]).
fn token_env_hint(flavor: Flavor, host: &str) -> String {
    match flavor {
        Flavor::Github if host == "github.com" => {
            "set GITHUB_TOKEN (or GH_TOKEN, or log in with `gh auth login`)".to_owned()
        }
        Flavor::Github if takes_github_com_tokens(host) => format!(
            "set GITHUB_TOKEN (or GH_TOKEN, or log in with `gh auth login --hostname {host}`)"
        ),
        Flavor::Github => format!(
            "set GH_ENTERPRISE_TOKEN (or GITHUB_ENTERPRISE_TOKEN, or log in with \
             `gh auth login --hostname {host}`)"
        ),
        Flavor::Gitlab => "set GITLAB_TOKEN".to_owned(),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// "at 14:05:12 CEST (in 23 min)"; the date is included when the reset is not today.
fn describe_reset(reset: i64, now: i64, tz: &jiff::tz::TimeZone) -> String {
    let Ok(ts) = jiff::Timestamp::from_second(reset) else {
        return format!("at unix time {reset}");
    };
    let zoned = ts.to_zoned(tz.clone());
    let today = jiff::Timestamp::from_second(now)
        .ok()
        .map(|n| n.to_zoned(tz.clone()).date());
    let fmt = if today == Some(zoned.date()) {
        "%H:%M:%S %Z"
    } else {
        "%Y-%m-%d %H:%M:%S %Z"
    };
    let when = zoned.strftime(fmt).to_string();
    let delta = reset - now;
    if delta > 0 {
        format!("at {when} (in {})", describe_duration(delta.unsigned_abs()))
    } else {
        format!("at {when}")
    }
}

fn describe_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs.div_ceil(60)),
        _ => format!("{} h {} min", secs / 3600, (secs % 3600) / 60),
    }
}

// ------------------------------------------------------------------------------------------------
// Token resolution
// ------------------------------------------------------------------------------------------------

/// Where the API token came from (for log messages; the value itself is never shown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenOrigin {
    /// `--token` / [`ApiConfig::token`].
    Explicit,
    /// An environment variable.
    Env(&'static str),
    /// A credential helper command (`gh auth token`, `glab config get token`).
    Helper(&'static str),
}

impl fmt::Display for TokenOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenOrigin::Explicit => f.write_str("--token"),
            TokenOrigin::Env(name) => write!(f, "${name}"),
            TokenOrigin::Helper(cmd) => write!(f, "`{cmd}`"),
        }
    }
}

/// A token that never shows up in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Resolve the token for `host` (see [`token_host`]): explicit > environment > credential helper.
///
/// GitHub, for github.com and `*.ghe.com`: `GITHUB_TOKEN` > `GH_TOKEN` >
/// `gh auth token --hostname HOST`. GitHub Enterprise Server (any other host):
/// `GH_ENTERPRISE_TOKEN` > `GITHUB_ENTERPRISE_TOKEN` > `gh auth token --hostname HOST`; a
/// github.com token is never sent there (gh's convention). GitLab: `GITLAB_TOKEN` >
/// `glab config get token --host HOST`. `env` and `helper` are injected so tests never see the
/// developer's real credentials.
fn resolve_token(
    flavor: Flavor,
    explicit: Option<&str>,
    host: &str,
    env: &dyn Fn(&str) -> Option<String>,
    helper: &dyn Fn(&str, &[&str]) -> Option<String>,
) -> Option<(Secret, TokenOrigin)> {
    let clean = |s: String| {
        let t = s.trim().to_owned();
        (!t.is_empty()).then_some(t)
    };
    if let Some(t) = explicit.map(str::to_owned).and_then(clean) {
        return Some((Secret(t), TokenOrigin::Explicit));
    }
    let vars: &[&'static str] = match flavor {
        Flavor::Github if takes_github_com_tokens(host) => &["GITHUB_TOKEN", "GH_TOKEN"],
        Flavor::Github => &["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"],
        Flavor::Gitlab => &["GITLAB_TOKEN"],
    };
    for &var in vars {
        if let Some(t) = env(var).and_then(clean) {
            return Some((Secret(t), TokenOrigin::Env(var)));
        }
    }
    let (program, args, label): (&str, Vec<&str>, &'static str) = match flavor {
        Flavor::Github => (
            "gh",
            vec!["auth", "token", "--hostname", host],
            "gh auth token",
        ),
        Flavor::Gitlab => (
            "glab",
            vec!["config", "get", "token", "--host", host],
            "glab config get token",
        ),
    };
    let token = helper(program, &args).and_then(clean)?;
    // A helper that prints anything but a single token (e.g. an error text) is ignored.
    let plausible =
        token.len() <= 4096 && !token.chars().any(|c| c.is_whitespace() || c.is_control());
    plausible.then_some((Secret(token), TokenOrigin::Helper(label)))
}

/// Run a credential helper and return its stdout. `None` if it is not installed, fails, or
/// does not finish within [`HELPER_TIMEOUT`]. Its stderr is discarded.
fn run_helper(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if started.elapsed() < HELPER_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

// ------------------------------------------------------------------------------------------------
// The renderer
// ------------------------------------------------------------------------------------------------

/// FIFO cache of API response HTML, bounded by [`CACHE_CAPACITY`] entries and
/// [`CACHE_MAX_BYTES`] bytes (Q-MEM-APICACHE: by entries alone it could hold 128 responses of up
/// to [`MAX_RESPONSE_BYTES`] each).
#[derive(Default)]
struct ResponseCache {
    map: HashMap<[u8; 32], Arc<str>>,
    order: VecDeque<[u8; 32]>,
    /// Total length of the cached HTML.
    bytes: usize,
}

impl ResponseCache {
    fn get(&self, key: &[u8; 32]) -> Option<Arc<str>> {
        self.map.get(key).cloned()
    }

    fn insert(&mut self, key: [u8; 32], html: Arc<str>) {
        // A response that would take more than a quarter of the budget is not worth evicting
        // everything else for (it is also the least likely to be requested unchanged again).
        if html.len() > CACHE_MAX_BYTES / 4 {
            return;
        }
        self.bytes += html.len();
        match self.map.insert(key, html) {
            Some(old) => self.bytes -= old.len(),
            None => self.order.push_back(key),
        }
        while self.order.len() > CACHE_CAPACITY || self.bytes > CACHE_MAX_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.map.remove(&oldest) {
                self.bytes -= old.len();
            }
        }
    }
}

/// The API is skipped until this point (or forever) after certain failures.
#[derive(Clone, Debug)]
struct Block {
    until: Option<SystemTime>,
    error: ApiError,
}

/// Renders through the GitHub or GitLab markdown API.
pub struct ApiRenderer {
    cfg: ApiConfig,
    endpoint: String,
    /// GitLab: origin + path prefix used to absolutize `/-/…` URLs in API output.
    instance_origin: String,
    instance_prefix: String,
    /// Host the token is for (see [`token_host`]).
    token_host: String,
    token: Option<Secret>,
    token_origin: Option<TokenOrigin>,
    transport: Arc<dyn HttpTransport>,
    cache: Mutex<ResponseCache>,
    block: Mutex<Option<Block>>,
}

impl fmt::Debug for ApiRenderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiRenderer")
            .field("flavor", &self.cfg.flavor)
            .field("endpoint", &self.endpoint)
            .field("token_origin", &self.token_origin)
            .field("fallback_offline", &self.cfg.fallback_offline)
            .finish_non_exhaustive()
    }
}

impl ApiRenderer {
    /// Create a renderer using the real network. The token is resolved from `cfg.token`, then
    /// the environment, then `gh auth token` / `glab` (only if installed).
    ///
    /// # Errors
    /// An `api_url` that is not an `http(s)://` URL without query or fragment.
    pub fn new(cfg: ApiConfig) -> anyhow::Result<Self> {
        let base = parse_base_url(&cfg)?;
        let host = token_host(cfg.flavor, &base);
        let env = |name: &str| std::env::var(name).ok();
        let resolved = resolve_token(cfg.flavor, cfg.token.as_deref(), &host, &env, &run_helper);
        let renderer = Self::build(cfg, resolved, Arc::new(UreqTransport::new()))?;
        match &renderer.token_origin {
            Some(origin) => tracing::info!(
                "rendering through the {} API at {} (token from {origin})",
                site_name(renderer.cfg.flavor),
                renderer.endpoint
            ),
            None => tracing::info!(
                "rendering through the {} API at {} (no token: {} for higher limits)",
                site_name(renderer.cfg.flavor),
                renderer.endpoint,
                token_env_hint(renderer.cfg.flavor, &renderer.token_host)
            ),
        }
        if renderer.token.is_some() && is_cleartext_remote(&base) {
            tracing::warn!(
                "the {} API URL is http:// on a remote host: the token is sent unencrypted; \
                 use https://",
                site_name(renderer.cfg.flavor)
            );
        }
        Ok(renderer)
    }

    /// Create a renderer with an injected transport. Only `cfg.token` is used (no environment
    /// or credential-helper lookup), which keeps tests hermetic.
    ///
    /// # Errors
    /// Same as [`ApiRenderer::new`].
    pub fn with_transport(
        cfg: ApiConfig,
        transport: Arc<dyn HttpTransport>,
    ) -> anyhow::Result<Self> {
        let resolved = cfg
            .token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| (Secret(t.to_owned()), TokenOrigin::Explicit));
        Self::build(cfg, resolved, transport)
    }

    fn build(
        cfg: ApiConfig,
        token: Option<(Secret, TokenOrigin)>,
        transport: Arc<dyn HttpTransport>,
    ) -> anyhow::Result<Self> {
        let base = parse_base_url(&cfg)?;
        let token_host = token_host(cfg.flavor, &base);
        let trimmed = base.as_str().trim_end_matches('/').to_owned();
        let endpoint = match cfg.flavor {
            Flavor::Github => format!("{trimmed}/markdown"),
            Flavor::Gitlab if trimmed.ends_with("/api/v4") => format!("{trimmed}/markdown"),
            Flavor::Gitlab => format!("{trimmed}/api/v4/markdown"),
        };
        let instance_origin = base.origin().ascii_serialization();
        let mut instance_prefix = base.path().trim_end_matches('/').to_owned();
        if let Some(p) = instance_prefix.strip_suffix("/api/v4") {
            instance_prefix = p.to_owned();
        }
        let (token, token_origin) = match token {
            Some((t, o)) => (Some(t), Some(o)),
            None => (None, None),
        };
        Ok(ApiRenderer {
            cfg,
            endpoint,
            instance_origin,
            instance_prefix,
            token_host,
            token,
            token_origin,
            transport,
            cache: Mutex::new(ResponseCache::default()),
            block: Mutex::new(None),
        })
    }

    /// The URL requests are sent to.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Where the token came from (`None`: unauthenticated).
    pub fn token_origin(&self) -> Option<&TokenOrigin> {
        self.token_origin.as_ref()
    }

    /// Build the request for `text` (already stripped of front matter where applicable).
    fn build_request(&self, text: &str, opts: &RenderOptions) -> ApiRequest {
        let mut headers = vec![("User-Agent".to_owned(), USER_AGENT.to_owned())];
        let mut body = serde_json::Map::new();
        body.insert("text".into(), text.into());
        match self.cfg.flavor {
            Flavor::Github => {
                headers.push(("Accept".into(), "application/vnd.github+json".into()));
                headers.push(("X-GitHub-Api-Version".into(), "2022-11-28".into()));
                if let Some(t) = &self.token {
                    headers.push(("Authorization".into(), format!("Bearer {}", t.0)));
                }
                body.insert("mode".into(), "gfm".into());
                if let Some(ctx) = opts.context.as_deref().filter(|c| !c.is_empty()) {
                    body.insert("context".into(), ctx.into());
                }
            }
            Flavor::Gitlab => {
                headers.push(("Accept".into(), "application/json".into()));
                if let Some(t) = &self.token {
                    headers.push(("PRIVATE-TOKEN".into(), t.0.clone()));
                }
                body.insert("gfm".into(), true.into());
                if let Some(ctx) = opts.context.as_deref().filter(|c| !c.is_empty()) {
                    body.insert("project".into(), ctx.into());
                }
            }
        }
        ApiRequest {
            url: self.endpoint.clone(),
            headers,
            body: serde_json::Value::Object(body),
        }
    }

    fn lock_block(&self) -> MutexGuard<'_, Option<Block>> {
        self.block.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_cache(&self) -> MutexGuard<'_, ResponseCache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The API HTML for `text`, from the cache or the network.
    fn fetch(&self, text: &str, opts: &RenderOptions) -> Result<Arc<str>, ApiError> {
        let key = cache_key(self.cfg.flavor, opts.mode, opts.context.as_deref(), text);
        if let Some(hit) = self.lock_cache().get(&key) {
            return Ok(hit);
        }
        {
            let mut block = self.lock_block();
            if let Some(b) = block.as_ref() {
                match b.until {
                    Some(until) if SystemTime::now() >= until => *block = None,
                    _ => {
                        tracing::debug!(
                            "{} API skipped after an earlier error",
                            site_name(self.cfg.flavor)
                        );
                        return Err(b.error.clone());
                    }
                }
            }
        }
        let site = self.cfg.flavor;
        if site == Flavor::Github && text.len() > GITHUB_MAX_TEXT_BYTES {
            return Err(ApiError::TooLarge {
                site,
                size: text.len(),
                limit: GITHUB_MAX_TEXT_BYTES,
            });
        }
        let request = self.build_request(text, opts);
        tracing::debug!(url = %request.url, bytes = text.len(), "API render request");
        let result = match self.transport.post_json(&request) {
            Err(e) => Err(ApiError::Network {
                site,
                url: request.url.clone(),
                message: e.0,
            }),
            Ok(resp) => {
                tracing::debug!(status = resp.status, "API render response");
                let exchange = Exchange {
                    site,
                    url: &request.url,
                    had_token: self.token.is_some(),
                    token_host: &self.token_host,
                };
                interpret_response(&exchange, &resp)
            }
        };
        match result {
            Ok(html) => {
                let html: Arc<str> = Arc::from(html);
                self.lock_cache().insert(key, html.clone());
                Ok(html)
            }
            Err(err) => {
                // Logged once here, not on every render while the API stays blocked. Without
                // the fallback the caller reports the error itself.
                if self.cfg.fallback_offline {
                    tracing::warn!("{err}; rendering offline instead");
                }
                if let Some(block) = block_for(&err) {
                    *self.lock_block() = Some(block);
                }
                Err(err)
            }
        }
    }

    fn render_api(&self, markdown: &str, opts: &RenderOptions) -> Result<Rendered, ApiError> {
        match self.cfg.flavor {
            Flavor::Github => {
                // GitHub's README view renders YAML front matter as a table, the API as
                // `<hr>` + setext heading (GH §8.12): strip it and render it offline.
                let (front, body) = match opts.mode {
                    Mode::Readme => split_front_matter(markdown),
                    Mode::Comment => (None, markdown),
                };
                let html = self.fetch(body, opts)?;
                let seed = rid_seed(opts.source_path.as_deref());
                let hints = SourceHints::from_markdown(body);
                let mut rendered = postprocess_github(&html, opts.mode, &seed, &hints);
                if let Some(front) = front {
                    let mut fm_opts = opts.clone();
                    fm_opts.sourcepos = false;
                    let fm = super::github::render(front, &fm_opts);
                    // The table is the first element, then "\n\n" and the rest (GH §8.12).
                    rendered.html = format!("{}\n\n{}", fm.html.trim_end(), rendered.html);
                    rendered.warnings.extend(fm.warnings);
                }
                Ok(rendered)
            }
            Flavor::Gitlab => {
                let html = self.fetch(markdown, opts)?;
                Ok(postprocess_gitlab(
                    &html,
                    &self.instance_origin,
                    &self.instance_prefix,
                ))
            }
        }
    }
}

impl Renderer for ApiRenderer {
    fn render(&self, markdown: &str, opts: &RenderOptions) -> anyhow::Result<Rendered> {
        anyhow::ensure!(
            opts.flavor == self.cfg.flavor,
            "the {} API renderer cannot render {} flavor",
            site_name(self.cfg.flavor),
            site_name(opts.flavor)
        );
        match self.render_api(markdown, opts) {
            Ok(rendered) => Ok(rendered),
            Err(err) if self.cfg.fallback_offline => {
                let mut rendered = super::render(markdown, opts);
                rendered.warnings.insert(
                    0,
                    format!(
                        "{}. Showing the offline rendering instead.",
                        capitalize(&err.to_string())
                    ),
                );
                Ok(rendered)
            }
            // Every ApiError names the site's API already; a "rendering through the GitHub
            // API" context only repeated it (CLI-23).
            Err(err) => Err(anyhow::Error::new(err)),
        }
    }

    fn name(&self) -> &'static str {
        "api"
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Validated base URL (`api_url` or the site default).
fn parse_base_url(cfg: &ApiConfig) -> anyhow::Result<url::Url> {
    let default = match cfg.flavor {
        Flavor::Github => GITHUB_API_URL,
        Flavor::Gitlab => GITLAB_URL,
    };
    let raw = cfg
        .api_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or(default);
    // Messages show the URL without credentials, query or fragment (CLI-6).
    let shown = redact_url(raw);
    let url = url::Url::parse(raw).with_context(|| format!("invalid API URL {shown:?}"))?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "invalid API URL {shown:?}: expected an http:// or https:// URL"
    );
    // ureq would turn `user:password@` into Basic authentication, and the URL is shown in logs,
    // errors and the page banner: tokens go through `--token` and the environment only.
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "invalid API URL {shown:?}: credentials in the URL are not supported; {}",
        credential_hint(cfg.flavor)
    );
    anyhow::ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "invalid API URL {shown:?}: it must not have a query or fragment"
    );
    Ok(url)
}

/// Where an API token goes instead of the URL.
pub(crate) fn credential_hint(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Github => {
            "pass --token, or set GITHUB_TOKEN (GH_ENTERPRISE_TOKEN for GitHub Enterprise Server)"
        }
        Flavor::Gitlab => "pass --token, or set GITLAB_TOKEN",
    }
}

/// The host a token for the API at `base` belongs to, as `gh` / `glab` name it: `github.com`
/// for `api.github.com`, `TENANT.ghe.com` for a GHE.com tenant's `api.TENANT.ghe.com`, else the
/// URL's host (a GitHub Enterprise Server or GitLab instance).
fn token_host(flavor: Flavor, base: &url::Url) -> String {
    let host = base.host_str().unwrap_or_default().to_ascii_lowercase();
    match flavor {
        Flavor::Github if host == "api.github.com" => "github.com".to_owned(),
        Flavor::Github => match host.strip_prefix("api.") {
            Some(tenant) if tenant.ends_with(".ghe.com") => tenant.to_owned(),
            _ => host,
        },
        Flavor::Gitlab => host,
    }
}

/// Whether requests to `base` travel unencrypted beyond this machine (`http://`, not loopback).
fn is_cleartext_remote(base: &url::Url) -> bool {
    let loopback = match base.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    base.scheme() == "http" && !loopback
}

/// `sha256(flavor, mode, context, text)` with length-prefixed fields.
fn cache_key(flavor: Flavor, mode: Mode, context: Option<&str>, text: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    let mut field = |bytes: &[u8]| {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    field(flavor.as_str().as_bytes());
    field(mode.as_str().as_bytes());
    match context {
        Some(c) => {
            field(b"1");
            field(c.as_bytes());
        }
        None => field(b"0"),
    }
    field(text.as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Seed for the deterministic run id: stable across edits of the same file (GH §8.4).
fn rid_seed(source: Option<&Path>) -> String {
    match source {
        Some(p) => format!("path:{}", p.display()),
        None => "stdin".to_owned(),
    }
}

/// What [`interpret_response`] needs to know about the request a response answers.
struct Exchange<'a> {
    site: Flavor,
    /// Where the request went (a redirect's relative `Location` is resolved against it).
    url: &'a str,
    had_token: bool,
    /// See [`token_host`].
    token_host: &'a str,
}

/// Turn a response into HTML or an [`ApiError`].
fn interpret_response(exchange: &Exchange<'_>, resp: &ApiResponse) -> Result<String, ApiError> {
    let site = exchange.site;
    if (200..300).contains(&resp.status) {
        return match site {
            Flavor::Github => Ok(resp.body.clone()),
            Flavor::Gitlab => {
                let value: serde_json::Value =
                    serde_json::from_str(&resp.body).map_err(|e| ApiError::BadResponse {
                        site,
                        message: format!("invalid JSON: {e}"),
                    })?;
                value
                    .get("html")
                    .and_then(|h| h.as_str())
                    .map(str::to_owned)
                    .ok_or_else(|| ApiError::BadResponse {
                        site,
                        message: "the JSON has no \"html\" string".into(),
                    })
            }
        };
    }
    Err(classify_error(exchange, resp))
}

fn classify_error(exchange: &Exchange<'_>, resp: &ApiResponse) -> ApiError {
    let Exchange {
        site,
        had_token,
        token_host,
        ..
    } = *exchange;
    if (300..400).contains(&resp.status) {
        // Redirects are never followed (see `UreqTransport::new`); say where it pointed.
        return ApiError::Status {
            site,
            status: resp.status,
            message: Some(redirect_note(site, resp, exchange.url)),
        };
    }
    let message = error_message(&resp.body);
    let num = |names: &[&str]| -> Option<u64> {
        names
            .iter()
            .find_map(|n| resp.header(n))
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    let (limit_h, remaining_h, reset_h): (&[&str], &[&str], &[&str]) = match site {
        Flavor::Github => (
            &["x-ratelimit-limit"],
            &["x-ratelimit-remaining"],
            &["x-ratelimit-reset"],
        ),
        Flavor::Gitlab => (
            &["ratelimit-limit", "x-ratelimit-limit"],
            &["ratelimit-remaining", "x-ratelimit-remaining"],
            &["ratelimit-reset", "x-ratelimit-reset"],
        ),
    };
    let remaining = num(remaining_h);
    let retry_after = num(&["retry-after"]);
    let mentions_limit = message
        .as_deref()
        .is_some_and(|m| m.to_ascii_lowercase().contains("rate limit"));
    let rate_limited = resp.status == 429
        || (resp.status == 403
            && (remaining == Some(0) || retry_after.is_some() || mentions_limit));
    if rate_limited {
        return ApiError::RateLimited {
            site,
            message,
            limit: num(limit_h),
            reset: num(reset_h).and_then(|r| i64::try_from(r).ok()),
            retry_after,
            had_token,
            host: token_host.to_owned(),
        };
    }
    match resp.status {
        401 => ApiError::Unauthorized {
            site,
            message,
            had_token,
            host: token_host.to_owned(),
        },
        403 => ApiError::Forbidden { site, message },
        status => ApiError::Status {
            site,
            status,
            message,
        },
    }
}

/// "redirected to https://host/login; …" for a 3xx answer to a request sent to `request_url`.
fn redirect_note(site: Flavor, resp: &ApiResponse, request_url: &str) -> String {
    let target = resp.header("location").and_then(|location| {
        let base = url::Url::parse(request_url).ok()?;
        Some(redact_url(base.join(location.trim()).ok()?.as_str()))
    });
    let example = match site {
        Flavor::Github => "https://HOST/api/v3 for GitHub Enterprise",
        Flavor::Gitlab => "https://HOST for a self-hosted GitLab",
    };
    let target = target.map_or_else(|| "a redirect".to_owned(), |t| format!("redirected to {t}"));
    format!("{target}, which is not followed; check --api-url, e.g. {example}")
}

/// `raw` (a URL, or what was meant as one) for messages and logs: credentials are replaced by
/// `***`, the query and fragment by `…` (either may carry a token).
pub(crate) fn redact_url(raw: &str) -> String {
    let (main, tail) = match raw.find(['?', '#']) {
        Some(i) if raw[i..].starts_with('?') => (&raw[..i], "?…"),
        Some(i) => (&raw[..i], "#…"),
        None => (raw, ""),
    };
    let (scheme, rest) = match main.find("://") {
        Some(i) => main.split_at(i + 3),
        None => ("", main),
    };
    let authority_end = rest.find(['/', '\\']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}***@{}{tail}", &rest[at + 1..]),
        None => format!("{scheme}{rest}{tail}"),
    }
}

/// `{"message": …}` / `{"error": …}` from a JSON error body, else a short plain-text excerpt.
fn error_message(body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let text = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => ["message", "error", "error_description"]
            .iter()
            .find_map(|k| v.get(k).and_then(|m| m.as_str()).map(str::to_owned))
            .unwrap_or_else(|| body.to_owned()),
        Err(_) if body.starts_with('<') => return None, // an HTML error page
        Err(_) => body.to_owned(),
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(if text.chars().count() > 200 {
        let mut t: String = text.chars().take(200).collect();
        t.push('…');
        t
    } else {
        text
    })
}

fn block_for(err: &ApiError) -> Option<Block> {
    let now = SystemTime::now();
    let until = match err {
        ApiError::Unauthorized { .. } => None,
        ApiError::RateLimited {
            reset, retry_after, ..
        } => {
            let from_reset = reset
                .and_then(|r| u64::try_from(r).ok())
                .map(|r| UNIX_EPOCH + Duration::from_secs(r))
                .filter(|t| *t > now);
            let from_retry = retry_after.map(|s| now + Duration::from_secs(s));
            Some(
                from_reset
                    .or(from_retry)
                    .unwrap_or(now + DEFAULT_RATE_LIMIT_BACKOFF),
            )
        }
        ApiError::Network { .. } => Some(now + NETWORK_BACKOFF),
        _ => return None,
    };
    Some(Block {
        until,
        error: err.clone(),
    })
}

/// Split YAML front matter from the body (GH §8.12), with the delimiter rules of the offline
/// renderer (`github::frontmatter::extract`, after its line-ending normalization), so that
/// `--api` and offline pages agree on what is front matter: a `---` line at byte 0, then a
/// closing `---` or `...` line; trailing blanks are allowed on both, and `\r\n` line ends.
/// The `front_matter_split_matches_the_offline_renderer` test pins the two together.
fn split_front_matter(text: &str) -> (Option<&str>, &str) {
    fn bare(line: &str) -> &str {
        line.trim_end_matches(['\n', '\r', ' ', '\t'])
    }
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, text);
    };
    if !first.ends_with('\n') || bare(first) != "---" {
        return (None, text);
    }
    let mut end = first.len();
    for line in lines {
        end += line.len();
        if matches!(bare(line), "---" | "...") {
            return (Some(&text[..end]), &text[end..]);
        }
    }
    (None, text)
}

// ------------------------------------------------------------------------------------------------
// Source hints: what the API HTML lost but the Markdown still has
// ------------------------------------------------------------------------------------------------

/// Facts taken from the Markdown source that `POST /markdown` output no longer carries.
///
/// Each hint is matched to its DOM counterpart individually (by comparing contents), so a hint
/// that cannot be placed is ignored and the DOM-derived value is used instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SourceHints {
    /// Heading texts in document order as GitHub's slugger sees them (GH §6.2 "heading text
    /// source"): emoji shortcodes literal (`:rocket:`), image alt text skipped, footnote numbers
    /// and `$math$` included; raw HTML headings with their exact text.
    headings: Vec<String>,
    /// Code block contents in document order (the API drops leading blank lines, the clipboard
    /// text of the README view keeps them, GH §2.4).
    code: Vec<String>,
}

impl SourceHints {
    /// Collect the hints of `markdown` (front matter already removed).
    pub(crate) fn from_markdown(markdown: &str) -> SourceHints {
        use comrak::nodes::NodeValue;
        let arena = comrak::Arena::new();
        let mut options = comrak::Options::default();
        let ext = &mut options.extension;
        ext.strikethrough = true;
        ext.table = true;
        ext.autolink = true;
        ext.tasklist = true;
        ext.footnotes = true;
        ext.math_dollars = true;
        ext.math_code = true;
        ext.shortcodes = true;
        let root = comrak::parse_document(&arena, markdown, &options);
        let mut hints = SourceHints::default();
        for node in root.descendants() {
            match &node.data().value {
                NodeValue::Heading(_) => {
                    let mut text = String::new();
                    heading_source_text(node, &mut text);
                    hints.headings.push(text);
                }
                NodeValue::HtmlBlock(block) => {
                    let fragment = Document::fragment(block.literal.as_str());
                    for h in fragment.select("h1, h2, h3, h4, h5, h6").nodes().iter() {
                        hints.headings.push(h.text().to_string());
                    }
                }
                NodeValue::CodeBlock(block) => hints.code.push(block.literal.clone()),
                _ => {}
            }
        }
        hints
    }
}

/// Append the slug source text of a heading's inlines (GH §6.2).
fn heading_source_text<'a>(node: &'a comrak::nodes::AstNode<'a>, out: &mut String) {
    use comrak::nodes::NodeValue;
    for child in node.children() {
        match &child.data().value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(c) => out.push_str(&c.literal),
            NodeValue::ShortCode(sc) => {
                out.push(':');
                out.push_str(&sc.code);
                out.push(':');
            }
            NodeValue::FootnoteReference(r) => out.push_str(&r.ix.to_string()),
            NodeValue::Math(m) => {
                let fence = if m.display_math { "$$" } else { "$" };
                out.push_str(fence);
                out.push_str(&m.literal);
                out.push_str(fence);
            }
            NodeValue::Image(_)
            | NodeValue::HtmlInline(_)
            | NodeValue::SoftBreak
            | NodeValue::LineBreak => {}
            _ => heading_source_text(child, out),
        }
    }
}

/// Comparison key that is equal for a heading's source text and its rendered DOM text: emoji
/// shortcodes and every non-alphanumeric character (emoji, punctuation, spaces) removed.
fn heading_key(text: &str) -> String {
    static SHORTCODE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re =
        SHORTCODE.get_or_init(|| Regex::new(r":[a-z0-9_+-]+:").expect("valid shortcode regex"));
    re.replace_all(text, "")
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Hands out hints in document order to the DOM items that match them.
struct HintCursor<'h> {
    hints: &'h [String],
    next: usize,
}

impl<'h> HintCursor<'h> {
    fn new(hints: &'h [String]) -> Self {
        HintCursor { hints, next: 0 }
    }

    /// The first unused hint (from the current position) for which `matches` holds; hints that
    /// are skipped over stay unused. With `adjacent_only`, only the hint at the current position
    /// is considered.
    fn take(&mut self, adjacent_only: bool, matches: impl Fn(&str) -> bool) -> Option<&'h str> {
        let end = if adjacent_only {
            (self.next + 1).min(self.hints.len())
        } else {
            self.hints.len()
        };
        let found = (self.next..end).find(|&i| matches(&self.hints[i]))?;
        self.next = found + 1;
        Some(&self.hints[found])
    }

    /// The source text of a DOM heading whose text is `dom_text`.
    fn heading(&mut self, dom_text: &str) -> Option<&'h str> {
        let key = heading_key(dom_text);
        // An empty key (`# ???`, emoji only) would match any such heading: only accept the
        // next hint in order then.
        self.take(key.is_empty(), |h| heading_key(h) == key)
    }

    /// The source literal of a code block whose DOM text is `dom_text`.
    fn code(&mut self, dom_text: &str) -> Option<&'h str> {
        let want = dom_text.trim_matches(['\n', '\r']);
        self.take(false, |literal| literal.trim_matches(['\n', '\r']) == want)
    }
}

// ------------------------------------------------------------------------------------------------
// Post-processing (D15)
// ------------------------------------------------------------------------------------------------

/// GitHub's octicon-link SVG, as emitted by the README/file view (GH §5.1).
const OCTICON_LINK: &str = r#"<svg data-component="Octicon" class="octicon octicon-link" viewBox="0 0 16 16" version="1.1" width="16" height="16" aria-hidden="true"><path d="m7.775 3.275 1.25-1.25a3.5 3.5 0 1 1 4.95 4.95l-2.5 2.5a3.5 3.5 0 0 1-4.95 0 .751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018 1.998 1.998 0 0 0 2.83 0l2.5-2.5a2.002 2.002 0 0 0-2.83-2.83l-1.25 1.25a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042Zm-4.69 9.64a1.998 1.998 0 0 0 2.83 0l1.25-1.25a.751.751 0 0 1 1.042.018.751.751 0 0 1 .018 1.042l-1.25 1.25a3.5 3.5 0 1 1-4.95-4.95l2.5-2.5a3.5 3.5 0 0 1 4.95 0 .751.751 0 0 1-.018 1.042.751.751 0 0 1-1.042.018 1.998 1.998 0 0 0-2.83 0l-2.5 2.5a1.998 1.998 0 0 0 0 2.83Z"></path></svg>"#;

/// The loading spinner of a mermaid enrichment section (GH §8.7).
const ENRICHMENT_LOADER: &str = r#"<span class="js-render-enrichment-loader d-flex flex-justify-center flex-items-center width-full" style="min-height:100px" role="presentation">
    <span data-view-component="true">
  <svg style="box-sizing: content-box; color: var(--color-icon-primary);" width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true" data-view-component="true" class="octospinner mx-auto tmp-mx-auto anim-rotate">
    <circle cx="8" cy="8" r="7" stroke="currentColor" stroke-opacity="0.25" stroke-width="2" vector-effect="non-scaling-stroke" fill="none"></circle>
    <path d="M15 8a7.002 7.002 0 00-7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" vector-effect="non-scaling-stroke"></path>
</svg>    <span class="sr-only">Loading</span>
</span>
  </span>"#;

/// Parse an HTML fragment for post-processing.
///
/// The HTML parser drops a newline that directly follows a `<pre>` start tag (HTML §13.2.6.4.7),
/// but GitHub's clipboard text keeps it (a code block that starts with a blank line). Such `pre`s
/// are marked with [`PRE_LF_ATTR`] so the clipboard text can be rebuilt exactly.
fn parse_fragment(html: &str) -> Document {
    static PRE_START: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re =
        PRE_START.get_or_init(|| Regex::new(r"(?i)<pre\b[^>]*>(\r?\n)?").expect("valid pre regex"));
    let leading_lf: Vec<bool> = re.captures_iter(html).map(|c| c.get(1).is_some()).collect();
    let doc = Document::fragment(html);
    if leading_lf.iter().any(|&lf| lf) {
        let pres = doc.select("pre");
        let nodes = pres.nodes();
        // Only trust the mapping when every `<pre` tag became exactly one element.
        if nodes.len() == leading_lf.len() {
            for (pre, &lf) in nodes.iter().zip(&leading_lf) {
                if lf {
                    pre.set_attr(PRE_LF_ATTR, "");
                }
            }
        }
    }
    doc
}

/// Temporary marker: this `pre`'s text started with a newline in the API HTML.
const PRE_LF_ATTR: &str = "data-mdr-pre-lf";

/// Text of a `pre` as the API emitted it (see [`parse_fragment`]).
fn pre_text(pre: &NodeRef) -> String {
    let text = pre.text().to_string();
    if pre.has_attr(PRE_LF_ATTR) {
        format!("\n{text}")
    } else {
        text
    }
}

/// Serialize a fragment parsed with [`parse_fragment`] (dropping its temporary markers).
fn serialize_fragment(doc: &Document) -> String {
    for pre in doc.select("pre[data-mdr-pre-lf]").nodes().iter() {
        pre.remove_attr(PRE_LF_ATTR);
    }
    doc.html_root().inner_html().to_string()
}

fn class_list(node: &NodeRef) -> Vec<String> {
    node.class()
        .map(|c| c.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Remove `class` from `node`, dropping the attribute when it becomes empty.
fn remove_class(node: &NodeRef, class: &str) {
    if node.has_class(class) {
        let rest: Vec<String> = class_list(node)
            .into_iter()
            .filter(|c| c != class)
            .collect();
        if rest.is_empty() {
            node.remove_attr("class");
        } else {
            node.set_attr("class", &rest.join(" "));
        }
    }
}

fn add_classes(node: &NodeRef, classes: &[&str]) {
    let mut list = class_list(node);
    for c in classes {
        if !list.iter().any(|x| x == c) {
            list.push((*c).to_owned());
        }
    }
    node.set_attr("class", &list.join(" "));
}

fn has_ancestor(node: &NodeRef, pred: impl Fn(&NodeRef) -> bool) -> bool {
    node.ancestors_it(None).any(|a| a.is_element() && pred(&a))
}

/// Short content hash used as `data-mdr-key` (DESIGN §5).
fn mdr_key(html: &str) -> String {
    sha256_hex(html.as_bytes())[..16].to_owned()
}

fn set_mdr_keys(doc: &Document, selector: &str) {
    for node in doc.select(selector).nodes().iter() {
        if !node.has_attr("data-mdr-key") {
            node.set_attr("data-mdr-key", &mdr_key(&node.html()));
        }
    }
}

fn first_h1_title(doc: &Document) -> Option<String> {
    let h1 = doc.select("h1").nodes().first().cloned()?;
    let text = h1.text().split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

/// Replace GitHub's random per-render run id (footnote ids, `math-renderer[data-run-id]`) by a
/// deterministic one, so ids stay stable across live reloads (GH §8.4).
fn stabilize_run_id(html: &str, seed: &str) -> String {
    static RID: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RID.get_or_init(|| {
        Regex::new(r##"(?:data-run-id="|user-content-fn(?:ref)?-[^"#\s]*?-)([0-9a-f]{32})\b"##)
            .expect("valid run-id regex")
    });
    match re.captures(html).and_then(|c| c.get(1)) {
        Some(rid) => {
            let stable = &sha256_hex(seed.as_bytes())[..32];
            html.replace(rid.as_str(), stable)
        }
        None => html.to_owned(),
    }
}

/// UUID-shaped deterministic identity for enrichment sections.
fn stable_identity(code: &str) -> String {
    let h = sha256_hex(code.as_bytes());
    format!(
        "{}-{}-4{}-a{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}

fn mermaid_section(code: &str) -> String {
    let json = serde_json::json!({ "data": code }).to_string();
    format!(
        concat!(
            r#"<section class="js-render-needs-enrichment render-needs-enrichment position-relative" data-identity="{id}" data-host="https://viewscreen.githubusercontent.com" data-src="https://viewscreen.githubusercontent.com/markdown/mermaid?docs_host=https%3A%2F%2Fdocs.github.com" data-type="mermaid" aria-label="mermaid rendered output container">"#,
            "\n  ",
            r#"<div class="js-render-enrichment-target" data-json="{json}" data-plain="{plain}" dir="auto">"#,
            "\n    ",
            r#"<div class="render-plaintext-hidden" dir="auto">"#,
            "\n      ",
            r#"<pre lang="mermaid" aria-label="Raw mermaid code">{text}</pre>"#,
            "\n    </div>\n  </div>\n  {loader}\n</section>"
        ),
        id = stable_identity(code),
        json = html::attr(&json),
        plain = html::attr(code),
        text = html::text(code),
        loader = ENRICHMENT_LOADER,
    )
}

/// GitHub heading slugs (GH §6.2): per-char lowercase, keep word characters, space and `-`,
/// then spaces → `-`; github-slugger de-duplication.
#[derive(Default)]
struct Slugger {
    occurrences: HashMap<String, usize>,
}

impl Slugger {
    fn base(text: &str) -> String {
        static KEEP: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let keep = KEEP.get_or_init(|| {
            Regex::new(r"[\p{Alphabetic}\p{Mn}\p{Mc}\p{Me}\p{Nd}\p{Pc}\x{200C}\x{200D} -]")
                .expect("valid slug regex")
        });
        let mut out = String::new();
        let mut buf = [0u8; 4];
        // Per-character lowercase mapping (not `str::to_lowercase`: no final-sigma rule).
        for c in text.chars().flat_map(char::to_lowercase) {
            if keep.is_match(c.encode_utf8(&mut buf)) {
                out.push(if c == ' ' { '-' } else { c });
            }
        }
        out
    }

    fn slug(&mut self, text: &str) -> String {
        let base = Self::base(text);
        let mut result = base.clone();
        while self.occurrences.contains_key(&result) {
            let n = self.occurrences.get(&base).copied().unwrap_or(0) + 1;
            self.occurrences.insert(base.clone(), n);
            result = format!("{base}-{n}");
        }
        self.occurrences.insert(result.clone(), 0);
        result
    }
}

/// Post-process `POST /markdown` HTML (D15, GH §2.5, §9). `hints` come from the Markdown that
/// was sent ([`SourceHints::default`] when unknown).
pub(crate) fn postprocess_github(
    html: &str,
    mode: Mode,
    rid_seed: &str,
    hints: &SourceHints,
) -> Rendered {
    let html = stabilize_run_id(html, rid_seed);
    let doc = parse_fragment(&html);

    // `mode=gfm` without `context` emits no `dir`; the README and comment views have it on these
    // blocks (task lists excepted). Runs before any wrapper of ours is added.
    set_direction(&doc);

    // Anchor icons: the API's empty `span.octicon-link` renders nothing (GH §5.1).
    for span in doc.select("span.octicon-link").nodes().iter() {
        span.replace_with_html(OCTICON_LINK);
    }

    // Canonical image URLs instead of camo proxies (GH §8.8, grip C51).
    for img in doc.select("img[data-canonical-src]").nodes().iter() {
        let canonical = img
            .attr("data-canonical-src")
            .map(|v| v.to_string())
            .unwrap_or_default();
        let old = img.attr("src").map(|v| v.to_string());
        if !canonical.is_empty() {
            img.set_attr("src", &canonical);
            if let Some(parent) = img.parent().filter(|p| p.has_name("a")) {
                if old.is_some() && parent.attr("href").map(|h| h.to_string()) == old {
                    parent.set_attr("href", &canonical);
                }
            }
        }
        img.remove_attr("data-canonical-src");
    }
    for source in doc.select("source[data-canonical-src]").nodes().iter() {
        if let Some(c) = source.attr("data-canonical-src").filter(|c| !c.is_empty()) {
            source.set_attr("srcset", &c);
        }
        source.remove_attr("data-canonical-src");
    }

    // Mermaid: plain highlighted blocks (no context) become enrichment sections (GH §8.7).
    let mut mermaid_hints = HintCursor::new(&hints.code);
    for div in doc.select("div.highlight-source-mermaid").nodes().iter() {
        let dom = code_text(div);
        let mut code = mermaid_hints.code(&dom).map_or(dom, str::to_owned);
        if !code.ends_with('\n') {
            code.push('\n');
        }
        div.replace_with_html(mermaid_section(&code));
    }
    for pre in doc.select(r#"pre[lang="mermaid"]"#).nodes().iter() {
        if has_ancestor(pre, |a| a.has_class("render-plaintext-hidden")) {
            continue;
        }
        let code = pre_text(pre);
        pre.replace_with_html(mermaid_section(&code));
    }
    for section in doc.select("section[data-identity]").nodes().iter() {
        let plain = section
            .descendants_it()
            .find_map(|d| d.attr("data-plain").map(|v| v.to_string()))
            .unwrap_or_else(|| section.text().to_string());
        section.set_attr("data-identity", &stable_identity(&plain));
    }

    if mode == Mode::Readme {
        readme_headings(&doc, hints);
        // README view: no `notranslate` on code, none on `pre` inside `.highlight` or in an
        // enrichment's plain-text copy, no `role="table"` (GH §2.2, §9.A).
        for code in doc.select("code.notranslate").nodes().iter() {
            remove_class(code, "notranslate");
        }
        for pre in doc.select("pre.notranslate").nodes().iter() {
            // Kept only on plain code blocks (`pre > code`); raw HTML `<pre>`s lose it too.
            let in_code_view = has_ancestor(pre, |a| {
                a.has_class("highlight") || a.has_class("render-plaintext-hidden")
            });
            if in_code_view || !has_code_child(pre) {
                remove_class(pre, "notranslate");
            }
        }
        // Absolute `github.com/O/R/blob/…` image URLs are shown through `/raw/` (GH §2.3).
        for img in doc.select("img[src]").nodes().iter() {
            rewrite_blob_url(img, "src");
        }
        for source in doc.select("source[srcset]").nodes().iter() {
            rewrite_blob_url(source, "srcset");
        }
        for table in doc.select(r#"table[role="table"]"#).nodes().iter() {
            table.remove_attr("role");
        }
    }

    clipboard_wrappers(&doc, hints);

    set_mdr_keys(
        &doc,
        "math-renderer, section.js-render-needs-enrichment, div.highlight, div.snippet-clipboard-content",
    );
    let features = Features {
        math: doc.select("math-renderer").exists(),
        mermaid: doc.select(r#"section[data-type="mermaid"]"#).exists(),
    };
    let title = first_h1_title(&doc);
    Rendered {
        html: serialize_fragment(&doc),
        title,
        features,
        warnings: Vec::new(),
    }
}

/// `dir="auto"` where GitHub's views put it (GH §9 `D`): every `p`, `h1`–`h6`, `div`, `ul` and
/// `ol`, except task lists, empty headings (`<h1></h1>`) and the clipboard wrappers added later
/// (GH §2.4); an existing `dir` is kept.
fn set_direction(doc: &Document) {
    for node in doc
        .select("p, h1, h2, h3, h4, h5, h6, div, ul, ol")
        .nodes()
        .iter()
    {
        let empty_heading =
            node.is("h1, h2, h3, h4, h5, h6") && node.inner_html().trim().is_empty();
        let excluded =
            node.has_class("contains-task-list") || node.has_class("snippet-clipboard-content");
        if !node.has_attr("dir") && !excluded && !empty_heading {
            node.set_attr("dir", "auto");
        }
    }
}

/// Whether `pre` holds a code block (`pre > code`) rather than raw HTML preformatted text.
fn has_code_child(pre: &NodeRef) -> bool {
    pre.element_children().iter().any(|c| c.has_name("code"))
}

/// `https://github.com/O/R/blob/REF/x.png` → `https://github.com/O/R/raw/REF/x.png` in `attr`.
fn rewrite_blob_url(node: &NodeRef, attr: &str) {
    static BLOB: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = BLOB.get_or_init(|| {
        Regex::new(r"(https://github\.com/[^/\s]+/[^/\s]+)/blob/").expect("valid blob-url regex")
    });
    if let Some(value) = node.attr(attr) {
        if let std::borrow::Cow::Owned(rewritten) = re.replace_all(&value, "$1/raw/") {
            node.set_attr(attr, &rewritten);
        }
    }
}

/// Text of a code container (`div.highlight` or `pre`) as the API emitted it.
fn code_text(node: &NodeRef) -> String {
    if node.has_name("pre") {
        return pre_text(node);
    }
    match node.descendants_it().find(|n| n.has_name("pre")) {
        Some(pre) => pre_text(&pre),
        None => node.text().to_string(),
    }
}

/// Wrap every heading as the README view does (GH §6.1). Slug and label come from the source
/// text when a heading hint matches (emoji shortcodes, raw HTML headings), else from the DOM.
fn readme_headings(doc: &Document, hints: &SourceHints) {
    let mut slugger = Slugger::default();
    let mut cursor = HintCursor::new(&hints.headings);
    for h in doc.select("h1, h2, h3, h4, h5, h6").nodes().iter() {
        if h.parent().is_some_and(|p| p.has_class("markdown-heading")) {
            continue;
        }
        if h.id_attr().is_some_and(|id| &*id == "footnote-label") || h.has_class("sr-only") {
            continue;
        }
        if h.inner_html().trim().is_empty() {
            // An empty heading (`#` alone) gets no wrapper; consume its (empty) hint.
            cursor.take(true, |hint| hint.trim().is_empty());
            continue;
        }
        let dom_text = h.text().to_string();
        let text = cursor.heading(&dom_text).map_or(dom_text, str::to_owned);
        let slug = slugger.slug(&text);
        // GitHub's attribute order: existing attributes, then `class`, then `dir` (GH §6.1).
        let dir = h
            .attr("dir")
            .map_or_else(|| "auto".to_owned(), |d| d.to_string());
        h.remove_attr("dir");
        add_classes(h, &["heading-element"]);
        h.set_attr("dir", &dir);
        let (id, href) = if slug.is_empty() {
            (String::new(), "#".to_owned())
        } else {
            (format!("user-content-{slug}"), format!("#{slug}"))
        };
        // The label keeps the heading text as is, surrounding whitespace included (GH §6.3).
        let wrapper = format!(
            r#"<div class="markdown-heading" dir="auto">{}<a id="{}" class="anchor" aria-label="Permalink: {}" href="{}">{OCTICON_LINK}</a></div>"#,
            h.html(),
            html::attr(&id),
            html::attr(&text),
            html::attr(&href),
        );
        h.replace_with_html(wrapper);
    }
}

/// Clipboard text: the source literal when a code hint matches, else the DOM text; without the
/// final newline (GH §9 `RAW`).
fn clipboard_text(cursor: &mut HintCursor<'_>, node: &NodeRef) -> String {
    let dom = code_text(node);
    let mut raw = cursor.code(&dom).map_or(dom, str::to_owned);
    if raw.ends_with('\n') {
        raw.pop();
    }
    raw
}

/// Clipboard wrappers as stored by GitHub (GH §2.4, §5.3).
fn clipboard_wrappers(doc: &Document, hints: &SourceHints) {
    let mut cursor = HintCursor::new(&hints.code);
    for div in doc.select("div.highlight").nodes().iter() {
        if div.has_attr("data-snippet-clipboard-copy-content") {
            continue;
        }
        let raw = clipboard_text(&mut cursor, div);
        add_classes(div, &["notranslate", "position-relative", "overflow-auto"]);
        div.set_attr("data-snippet-clipboard-copy-content", &raw);
    }
    let mut cursor = HintCursor::new(&hints.code);
    for pre in doc.select("pre").nodes().iter() {
        // Only code blocks get a copy button; raw HTML `<pre>` text does not.
        let skip = !has_code_child(pre)
            || has_ancestor(pre, |a| {
                a.has_class("highlight")
                    || a.has_class("snippet-clipboard-content")
                    || a.has_class("render-plaintext-hidden")
                    || a.has_class("js-render-needs-enrichment")
                    || a.has_name("math-renderer")
            });
        if skip {
            continue;
        }
        let raw = clipboard_text(&mut cursor, pre);
        let wrapper = format!(
            r#"<div class="snippet-clipboard-content notranslate position-relative overflow-auto" data-snippet-clipboard-copy-content="{}">{}</div>"#,
            html::attr(&raw),
            pre.html()
        );
        pre.replace_with_html(wrapper);
    }
}

/// Post-process `POST /api/v4/markdown` HTML (D15, GL §6.8, §6.12, §9).
pub(crate) fn postprocess_gitlab(html: &str, origin: &str, prefix: &str) -> Rendered {
    let doc = parse_fragment(html);

    // GitLab-internal routes (`/-/project/<id>/uploads/…`) cannot be served locally.
    let internal = format!("{prefix}/-/");
    let absolutize = |node: &NodeRef, attr: &str| {
        if let Some(v) = node.attr(attr) {
            if v.starts_with(&internal) {
                node.set_attr(attr, &format!("{origin}{v}"));
            }
        }
    };

    // Un-lazy images: what GitLab's lazy loader does client-side (GL §6.12, §9): `src` from
    // `data-src`, class `lazy` → `js-lazy-loaded` (no placeholder size/background), native
    // `loading="lazy"`.
    for img in doc.select("img").nodes().iter() {
        if let Some(src) = img.attr("data-src").filter(|s| !s.is_empty()) {
            img.set_attr("src", &src);
            img.remove_attr("data-src");
            if img.has_class("lazy") {
                remove_class(img, "lazy");
                add_classes(img, &["js-lazy-loaded"]);
            }
            if !img.has_attr("loading") {
                img.set_attr("loading", "lazy");
            }
        }
        absolutize(img, "src");
    }
    for node in doc.select("a[href]").nodes().iter() {
        absolutize(node, "href");
    }
    for node in doc
        .select("video[src], audio[src], source[src]")
        .nodes()
        .iter()
    {
        absolutize(node, "src");
    }

    // The highlight theme CSS only matches this class (GL §6.8, §10.1).
    for pre in doc
        .select("pre.js-syntax-highlight, pre.code.highlight")
        .nodes()
        .iter()
    {
        add_classes(pre, &["code-syntax-highlight-theme"]);
    }

    set_mdr_keys(&doc, ".js-render-math, div.markdown-code-block");
    let features = Features {
        math: doc.select(".js-render-math").exists(),
        mermaid: doc.select(".js-render-mermaid").exists(),
    };
    let title = first_h1_title(&doc);
    Rendered {
        html: serialize_fragment(&doc),
        title,
        features,
        warnings: Vec::new(),
    }
}

// ------------------------------------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Q-MEM-APICACHE: the response cache is bounded by bytes as well as by entries.
    #[test]
    fn the_response_cache_is_bounded_by_entries_and_bytes() {
        let key = |i: u64| {
            let mut k = [0u8; 32];
            k[..8].copy_from_slice(&i.to_le_bytes());
            k
        };
        let capacity = CACHE_CAPACITY as u64;
        let mut cache = ResponseCache::default();
        for i in 0..capacity + 10 {
            cache.insert(key(i), Arc::from("<p>x</p>"));
        }
        assert_eq!(cache.map.len(), CACHE_CAPACITY);
        assert!(cache.get(&key(9)).is_none() && cache.get(&key(10)).is_some());

        // Large responses: the oldest go as soon as the byte budget is exceeded.
        let mut cache = ResponseCache::default();
        let big: Arc<str> = "x".repeat(CACHE_MAX_BYTES / 5).into();
        for i in 0..20 {
            cache.insert(key(i), big.clone());
            assert!(cache.bytes <= CACHE_MAX_BYTES, "{}", cache.bytes);
            let total: usize = cache.map.values().map(|h| h.len()).sum();
            assert_eq!(cache.bytes, total);
        }
        assert_eq!(cache.map.len(), 5);
        assert!(cache.get(&key(19)).is_some() && cache.get(&key(14)).is_none());
        // Replacing an entry accounts for the old size; huge responses are not cached at all.
        cache.insert(key(19), Arc::from("small"));
        assert_eq!(cache.bytes, 4 * big.len() + "small".len());
        let huge: Arc<str> = "x".repeat(CACHE_MAX_BYTES / 4 + 1).into();
        cache.insert(key(100), huge);
        assert!(cache.get(&key(100)).is_none());
        assert_eq!(cache.map.len(), 5);
    }

    /// Records requests and answers from a queue of canned responses.
    #[derive(Default)]
    struct Fake {
        responses: Mutex<VecDeque<Result<ApiResponse, TransportError>>>,
        requests: Mutex<Vec<ApiRequest>>,
    }

    impl Fake {
        fn with(responses: Vec<Result<ApiResponse, TransportError>>) -> Arc<Fake> {
            Arc::new(Fake {
                responses: Mutex::new(responses.into()),
                requests: Mutex::default(),
            })
        }
        fn requests(&self) -> Vec<ApiRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl HttpTransport for Fake {
        fn post_json(&self, request: &ApiRequest) -> Result<ApiResponse, TransportError> {
            self.requests.lock().unwrap().push(request.clone());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(TransportError("no canned response".into())))
        }
    }

    fn ok(body: &str) -> Result<ApiResponse, TransportError> {
        Ok(ApiResponse {
            status: 200,
            headers: vec![],
            body: body.into(),
        })
    }

    fn status(
        code: u16,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Result<ApiResponse, TransportError> {
        Ok(ApiResponse {
            status: code,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            body: body.into(),
        })
    }

    fn cfg(flavor: Flavor, token: Option<&str>, fallback: bool) -> ApiConfig {
        ApiConfig {
            flavor,
            api_url: None,
            token: token.map(Into::into),
            fallback_offline: fallback,
        }
    }

    fn opts(flavor: Flavor, mode: Mode, context: Option<&str>) -> RenderOptions {
        let mut o = RenderOptions::new(flavor);
        o.mode = mode;
        o.context = context.map(Into::into);
        o
    }

    #[test]
    fn github_request_shape() {
        let fake = Fake::with(vec![ok("<p>x</p>")]);
        let r =
            ApiRenderer::with_transport(cfg(Flavor::Github, Some(" tok "), false), fake.clone())
                .unwrap();
        assert_eq!(r.endpoint(), "https://api.github.com/markdown");
        r.render("x", &opts(Flavor::Github, Mode::Comment, Some("o/r")))
            .unwrap();
        let req = &fake.requests()[0];
        assert_eq!(req.url, "https://api.github.com/markdown");
        assert_eq!(req.header("accept"), Some("application/vnd.github+json"));
        assert_eq!(req.header("X-GitHub-Api-Version"), Some("2022-11-28"));
        assert_eq!(req.header("user-agent"), Some(USER_AGENT));
        assert_eq!(req.header("authorization"), Some("Bearer tok"));
        assert_eq!(
            req.body,
            serde_json::json!({"text": "x", "mode": "gfm", "context": "o/r"})
        );
        let dbg = format!(
            "{req:?} {r:?} {:?}",
            cfg(Flavor::Github, Some("tok"), false)
        );
        assert!(!dbg.contains("tok\"") && !dbg.contains("Bearer"), "{dbg}");
    }

    #[test]
    fn github_request_without_token_or_context() {
        let fake = Fake::with(vec![ok("<p>x</p>")]);
        let r =
            ApiRenderer::with_transport(cfg(Flavor::Github, None, false), fake.clone()).unwrap();
        r.render("x", &opts(Flavor::Github, Mode::Readme, None))
            .unwrap();
        let req = &fake.requests()[0];
        assert_eq!(req.header("authorization"), None);
        assert_eq!(req.body, serde_json::json!({"text": "x", "mode": "gfm"}));
    }

    #[test]
    fn gitlab_request_shape_and_urls() {
        let fake = Fake::with(vec![ok(r#"{"html":"<p>x</p>"}"#)]);
        let r =
            ApiRenderer::with_transport(cfg(Flavor::Gitlab, Some("glpat"), false), fake.clone())
                .unwrap();
        assert_eq!(r.endpoint(), "https://gitlab.com/api/v4/markdown");
        let out = r
            .render("x", &opts(Flavor::Gitlab, Mode::Readme, Some("g/p")))
            .unwrap();
        assert_eq!(out.html, "<p>x</p>");
        let req = &fake.requests()[0];
        assert_eq!(req.header("private-token"), Some("glpat"));
        assert_eq!(req.header("authorization"), None);
        assert_eq!(
            req.body,
            serde_json::json!({"text": "x", "gfm": true, "project": "g/p"})
        );
        assert!(!format!("{req:?}").contains("glpat"));

        for (url, want) in [
            (
                "https://gl.example.com/",
                "https://gl.example.com/api/v4/markdown",
            ),
            (
                "https://gl.example.com/api/v4",
                "https://gl.example.com/api/v4/markdown",
            ),
            (
                "https://x.org/gitlab",
                "https://x.org/gitlab/api/v4/markdown",
            ),
        ] {
            let c = ApiConfig {
                api_url: Some(url.into()),
                ..cfg(Flavor::Gitlab, None, false)
            };
            let r = ApiRenderer::with_transport(c, Fake::with(vec![])).unwrap();
            assert_eq!(r.endpoint(), want);
        }
        let c = ApiConfig {
            api_url: Some("https://ghe.example.com/api/v3/".into()),
            ..cfg(Flavor::Github, None, false)
        };
        let r = ApiRenderer::with_transport(c, Fake::with(vec![])).unwrap();
        assert_eq!(r.endpoint(), "https://ghe.example.com/api/v3/markdown");
    }

    /// CLI-6: credentials in `--api-url` are rejected, and no message repeats them.
    #[test]
    fn credentials_in_the_api_url_are_rejected_without_echoing_them() {
        for (flavor, bad) in [
            (Flavor::Github, "https://u:s3cr3t@h.example/api/v3"),
            (Flavor::Gitlab, "http://oauth2:s3cr3t@127.0.0.1:7249"),
            (Flavor::Gitlab, "https://s3cr3t@h.example"),
            (Flavor::Github, "https://u:s3cr3t@h.example/?q=1"),
            (Flavor::Github, "https://u:s3cr3t@h.example:99999/"),
            (Flavor::Github, "ftp://u:s3cr3t@h.example/"),
            (Flavor::Github, "https://h.example/?private_token=s3cr3t"),
        ] {
            let c = ApiConfig {
                api_url: Some(bad.into()),
                ..cfg(flavor, Some("t"), false)
            };
            let shown = format!("{c:?}");
            assert!(!shown.contains("s3cr3t"), "{shown}");
            let err = ApiRenderer::with_transport(c.clone(), Fake::with(vec![])).unwrap_err();
            let msg = format!("{err:#}");
            assert!(!msg.contains("s3cr3t"), "{bad}: {msg}");
            assert!(ApiRenderer::new(c).is_err(), "{bad}");
        }
        let c = ApiConfig {
            api_url: Some("https://oauth2:s3cr3t@gl.example".into()),
            ..cfg(Flavor::Gitlab, None, false)
        };
        let msg = ApiRenderer::with_transport(c, Fake::with(vec![]))
            .unwrap_err()
            .to_string();
        assert_eq!(
            msg,
            "invalid API URL \"https://***@gl.example\": credentials in the URL are not \
             supported; pass --token, or set GITLAB_TOKEN"
        );
    }

    #[test]
    fn invalid_api_urls_are_rejected() {
        for bad in ["ftp://x", "not a url", "https://x/?q=1", "mailto:a@b"] {
            let c = ApiConfig {
                api_url: Some(bad.into()),
                ..cfg(Flavor::Github, None, false)
            };
            assert!(
                ApiRenderer::with_transport(c, Fake::with(vec![])).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn responses_are_cached_by_flavor_mode_context_text() {
        let fake = Fake::with(vec![ok("<p>a</p>"), ok("<p>b</p>"), ok("<p>c</p>")]);
        let r =
            ApiRenderer::with_transport(cfg(Flavor::Github, None, false), fake.clone()).unwrap();
        let o = opts(Flavor::Github, Mode::Comment, None);
        assert_eq!(r.render("t", &o).unwrap().html, "<p dir=\"auto\">a</p>");
        assert_eq!(r.render("t", &o).unwrap().html, "<p dir=\"auto\">a</p>");
        assert_eq!(fake.requests().len(), 1);
        let o2 = opts(Flavor::Github, Mode::Comment, Some("o/r"));
        assert_eq!(r.render("t", &o2).unwrap().html, "<p dir=\"auto\">b</p>");
        assert_eq!(r.render("u", &o).unwrap().html, "<p dir=\"auto\">c</p>");
        assert_eq!(fake.requests().len(), 3);
        assert_ne!(
            cache_key(Flavor::Github, Mode::Readme, None, "t"),
            cache_key(Flavor::Github, Mode::Comment, None, "t")
        );
        assert_ne!(
            cache_key(Flavor::Github, Mode::Readme, Some(""), "t"),
            cache_key(Flavor::Github, Mode::Readme, None, "t")
        );
    }

    #[test]
    fn rate_limit_falls_back_offline_with_warning_and_backs_off() {
        let reset = (unix_now() + 600).to_string();
        let fake = Fake::with(vec![status(
            403,
            &[
                ("x-ratelimit-limit", "60"),
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", &reset),
            ],
            r#"{"message":"API rate limit exceeded for 1.2.3.4."}"#,
        )]);
        let r = ApiRenderer::with_transport(cfg(Flavor::Github, None, true), fake.clone()).unwrap();
        let o = opts(Flavor::Github, Mode::Readme, None);
        let out = r.render("# Hi", &o).unwrap();
        assert!(out.html.contains("Hi"));
        let w = &out.warnings[0];
        assert!(
            w.starts_with("GitHub API rate limit exceeded (60 requests per window); it resets at "),
            "{w}"
        );
        assert!(w.contains("(in 10 min)"), "{w}");
        assert!(
            w.contains("GITHUB_TOKEN") && w.ends_with("Showing the offline rendering instead."),
            "{w}"
        );
        // Still blocked: no second request.
        let again = r.render("# Other", &o).unwrap();
        assert!(again.warnings[0].contains("rate limit"));
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    fn errors_without_fallback() {
        let fake = Fake::with(vec![status(401, &[], r#"{"message":"401 Unauthorized"}"#)]);
        let r = ApiRenderer::with_transport(cfg(Flavor::Gitlab, None, false), fake).unwrap();
        let err = r
            .render("x", &opts(Flavor::Gitlab, Mode::Readme, None))
            .unwrap_err();
        let msg = format!("{err:#}");
        // One message, which names the API itself (no repeated "rendering through" context).
        assert_eq!(msg, err.to_string(), "{msg}");
        assert!(msg.starts_with("the GitLab API rejected"), "{msg}");
        assert!(
            msg.contains("unauthorized (401 Unauthorized); set GITLAB_TOKEN or pass --token"),
            "{msg}"
        );
        assert!(matches!(
            err.downcast_ref::<ApiError>(),
            Some(ApiError::Unauthorized { .. })
        ));
    }

    #[test]
    fn network_errors_fall_back_and_flavor_mismatch_errors() {
        let fake = Fake::with(vec![Err(TransportError("dns error".into()))]);
        let r = ApiRenderer::with_transport(cfg(Flavor::Github, None, true), fake).unwrap();
        let out = r
            .render("x", &opts(Flavor::Github, Mode::Readme, None))
            .unwrap();
        assert!(out.warnings[0].starts_with(
            "Could not reach the GitHub API at https://api.github.com/markdown: dns error"
        ));
        assert!(
            r.render("x", &opts(Flavor::Gitlab, Mode::Readme, None))
                .is_err()
        );
    }

    #[test]
    fn too_large_documents_are_not_sent() {
        let fake = Fake::with(vec![]);
        let r = ApiRenderer::with_transport(cfg(Flavor::Github, None, true), fake.clone()).unwrap();
        let big = "a".repeat(GITHUB_MAX_TEXT_BYTES + 1);
        let out = r
            .render(&big, &opts(Flavor::Github, Mode::Comment, None))
            .unwrap();
        assert!(
            out.warnings[0].contains("exceeds the GitHub API limit of 400 KiB"),
            "{:?}",
            out.warnings
        );
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn classification() {
        let exchange = |site| Exchange {
            site,
            url: "https://api.github.com/markdown",
            had_token: true,
            token_host: "github.com",
        };
        let gh = |code, h: &[(&str, &str)], b| {
            classify_error(&exchange(Flavor::Github), &status(code, h, b).unwrap())
        };
        assert!(matches!(gh(429, &[], ""), ApiError::RateLimited { .. }));
        assert!(matches!(
            gh(403, &[("retry-after", "30")], ""),
            ApiError::RateLimited {
                retry_after: Some(30),
                ..
            }
        ));
        assert!(matches!(
            gh(
                403,
                &[],
                r#"{"message":"You have exceeded a secondary rate limit"}"#
            ),
            ApiError::RateLimited { .. }
        ));
        assert!(matches!(
            gh(
                403,
                &[("x-ratelimit-remaining", "12")],
                r#"{"message":"Resource not accessible"}"#
            ),
            ApiError::Forbidden { .. }
        ));
        assert!(matches!(
            gh(401, &[], ""),
            ApiError::Unauthorized {
                had_token: true,
                ..
            }
        ));
        assert!(matches!(
            gh(500, &[], "<html>oops</html>"),
            ApiError::Status {
                status: 500,
                message: None,
                ..
            }
        ));
        assert!(matches!(
            gh(413, &[], "too big"),
            ApiError::Status { status: 413, .. }
        ));
        let gl = classify_error(
            &Exchange {
                had_token: false,
                ..exchange(Flavor::Gitlab)
            },
            &status(
                429,
                &[("RateLimit-Reset", "100"), ("RateLimit-Limit", "5")],
                "",
            )
            .unwrap(),
        );
        assert_eq!(
            gl,
            ApiError::RateLimited {
                site: Flavor::Gitlab,
                message: None,
                limit: Some(5),
                reset: Some(100),
                retry_after: None,
                had_token: false,
                host: "github.com".into(),
            }
        );
        let bad = interpret_response(&exchange(Flavor::Gitlab), &ok("{}").unwrap()).unwrap_err();
        assert!(matches!(bad, ApiError::BadResponse { .. }));
        assert_eq!(
            error_message(&format!("{{\"message\":\"{}\"}}", "x".repeat(300)))
                .unwrap()
                .chars()
                .count(),
            201
        );
    }

    #[test]
    fn redirects_are_errors_that_name_a_redacted_target() {
        let gitlab = Exchange {
            site: Flavor::Gitlab,
            url: "https://gl.example/api/v4/markdown",
            had_token: true,
            token_host: "gl.example",
        };
        let err = classify_error(
            &gitlab,
            &status(302, &[("Location", "/users/sign_in?token=x")], "<html>").unwrap(),
        );
        assert!(matches!(err, ApiError::Status { status: 302, .. }));
        assert_eq!(
            err.to_string(),
            "the GitLab API returned HTTP 302 (redirected to https://gl.example/users/sign_in?…, \
             which is not followed; check --api-url, e.g. https://HOST for a self-hosted GitLab)"
        );
        let github = Exchange {
            site: Flavor::Github,
            ..gitlab
        };
        let err = classify_error(
            &github,
            &status(301, &[("location", "https://u:pw@sso.example/x#f")], "").unwrap(),
        );
        let msg = err.to_string();
        assert!(
            msg.contains("redirected to https://***@sso.example/x#…")
                && msg.contains("https://HOST/api/v3")
                && !msg.contains("pw"),
            "{msg}"
        );
        let err = classify_error(&github, &status(307, &[], "").unwrap());
        assert!(
            err.to_string()
                .contains("(a redirect, which is not followed")
        );
        // A redirect is not retried later: it is no reason to block the API.
        assert!(block_for(&err).is_none());
    }

    #[test]
    fn urls_are_redacted_for_display() {
        for (raw, shown) in [
            ("https://h.example/api/v3", "https://h.example/api/v3"),
            (
                "https://u:s3cr3t@h.example/api",
                "https://***@h.example/api",
            ),
            ("https://u@h.example", "https://***@h.example"),
            ("https://h.example/?private_token=x", "https://h.example/?…"),
            ("https://h.example/a#tok", "https://h.example/a#…"),
            ("http://u:p@w@h:99999/x?y", "http://***@h:99999/x?…"),
            ("u:s3cr3t@h.example/x", "***@h.example/x"),
            ("https://h.example/a@b", "https://h.example/a@b"),
            ("not a url", "not a url"),
        ] {
            assert_eq!(redact_url(raw), shown, "{raw}");
        }
    }

    #[test]
    fn reset_time_formatting() {
        let utc = jiff::tz::TimeZone::UTC;
        // 2026-10-01T12:00:00Z
        let now = 1_790_856_000;
        assert_eq!(
            describe_reset(now + 300, now, &utc),
            "at 12:05:00 UTC (in 5 min)"
        );
        assert_eq!(
            describe_reset(now + 86_400, now, &utc),
            "at 2026-10-02 12:00:00 UTC (in 24 h 0 min)"
        );
        assert_eq!(describe_reset(now - 5, now, &utc), "at 11:59:55 UTC");
        let fixed = jiff::tz::TimeZone::fixed(jiff::tz::offset(2));
        assert!(describe_reset(now + 30, now, &fixed).starts_with("at 14:00:30"));
        let e = ApiError::RateLimited {
            site: Flavor::Github,
            message: None,
            limit: None,
            reset: None,
            retry_after: Some(90),
            had_token: true,
            host: "github.com".into(),
        };
        assert_eq!(
            e.describe(&utc, now),
            "GitHub API rate limit exceeded; retry in 2 min"
        );
    }

    #[test]
    fn token_resolution_order() {
        let env_all = |n: &str| match n {
            "GITHUB_TOKEN" => Some("gh1".to_owned()),
            "GH_TOKEN" => Some("gh2".to_owned()),
            "GITLAB_TOKEN" => Some(" gl ".to_owned()),
            _ => None,
        };
        let no_env = |_: &str| None;
        let helper_calls = std::cell::RefCell::new(Vec::new());
        let helper = |p: &str, a: &[&str]| {
            helper_calls
                .borrow_mut()
                .push(format!("{p} {}", a.join(" ")));
            Some("helpertok\n".to_owned())
        };
        let no_helper = |_: &str, _: &[&str]| None;
        let t = |r: Option<(Secret, TokenOrigin)>| r.map(|(s, o)| (s.0, o));

        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                Some("x"),
                "github.com",
                &env_all,
                &helper
            )),
            Some(("x".into(), TokenOrigin::Explicit))
        );
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                Some("  "),
                "github.com",
                &env_all,
                &helper
            )),
            Some(("gh1".into(), TokenOrigin::Env("GITHUB_TOKEN")))
        );
        let only_gh = |n: &str| (n == "GH_TOKEN").then(|| "gh2".to_owned());
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                None,
                "github.com",
                &only_gh,
                &helper
            )),
            Some(("gh2".into(), TokenOrigin::Env("GH_TOKEN")))
        );
        assert!(helper_calls.borrow().is_empty());
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                None,
                "github.com",
                &no_env,
                &helper
            )),
            Some(("helpertok".into(), TokenOrigin::Helper("gh auth token")))
        );
        assert_eq!(
            helper_calls.borrow().last().unwrap(),
            "gh auth token --hostname github.com"
        );
        assert_eq!(
            t(resolve_token(
                Flavor::Gitlab,
                None,
                "gitlab.com",
                &env_all,
                &helper
            )),
            Some(("gl".into(), TokenOrigin::Env("GITLAB_TOKEN")))
        );
        resolve_token(Flavor::Gitlab, None, "gl.example.com", &no_env, &helper);
        assert_eq!(
            helper_calls.borrow().last().unwrap(),
            "glab config get token --host gl.example.com"
        );
        assert_eq!(
            t(resolve_token(
                Flavor::Gitlab,
                None,
                "gitlab.com",
                &no_env,
                &no_helper
            )),
            None
        );
        let noisy = |_: &str, _: &[&str]| Some("error: not logged in to any hosts".to_owned());
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                None,
                "github.com",
                &no_env,
                &noisy
            )),
            None
        );
        let ent = |n: &str| (n == "GH_ENTERPRISE_TOKEN").then(|| "ent".to_owned());
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                None,
                "ghe.corp",
                &ent,
                &no_helper
            )),
            Some(("ent".into(), TokenOrigin::Env("GH_ENTERPRISE_TOKEN")))
        );
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                None,
                "github.com",
                &ent,
                &no_helper
            )),
            None
        );

        let host = |url: Option<&str>, f| {
            token_host(
                f,
                &parse_base_url(&ApiConfig {
                    api_url: url.map(Into::into),
                    ..cfg(f, None, true)
                })
                .unwrap(),
            )
        };
        assert_eq!(host(None, Flavor::Github), "github.com");
        assert_eq!(
            host(Some("https://GHE.corp/api/v3"), Flavor::Github),
            "ghe.corp"
        );
        assert_eq!(
            host(Some("https://api.octo.ghe.com"), Flavor::Github),
            "octo.ghe.com"
        );
        assert_eq!(host(None, Flavor::Gitlab), "gitlab.com");
        assert_eq!(
            host(Some("https://api.gl.example"), Flavor::Gitlab),
            "api.gl.example"
        );
    }

    /// CLI-3: a GitHub Enterprise Server host never gets the github.com token (gh's convention:
    /// `GH_TOKEN`/`GITHUB_TOKEN` for github.com and `*.ghe.com`, `GH_ENTERPRISE_TOKEN`/
    /// `GITHUB_ENTERPRISE_TOKEN` for every other host).
    #[test]
    fn github_com_tokens_are_never_sent_to_an_enterprise_host() {
        let env_of = |pairs: &'static [(&'static str, &'static str)]| {
            move |n: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == n)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let helper_calls = std::cell::RefCell::new(Vec::new());
        let helper = |p: &str, a: &[&str]| {
            helper_calls
                .borrow_mut()
                .push(format!("{p} {}", a.join(" ")));
            Some("helpertok".to_owned())
        };
        let no_helper = |_: &str, _: &[&str]| None;
        let t = |r: Option<(Secret, TokenOrigin)>| r.map(|(s, o)| (s.0, o));
        let both = env_of(&[("GITHUB_TOKEN", "a"), ("GH_ENTERPRISE_TOKEN", "b")]);
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                None,
                "ghe.corp",
                &both,
                &helper
            )),
            Some(("b".into(), TokenOrigin::Env("GH_ENTERPRISE_TOKEN")))
        );
        let long = env_of(&[("GITHUB_ENTERPRISE_TOKEN", "c")]);
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                None,
                "ghe.corp",
                &long,
                &no_helper
            )),
            Some(("c".into(), TokenOrigin::Env("GITHUB_ENTERPRISE_TOKEN")))
        );
        let dotcom = env_of(&[("GITHUB_TOKEN", "a"), ("GH_TOKEN", "a2")]);
        assert!(helper_calls.borrow().is_empty());
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                None,
                "ghe.corp",
                &dotcom,
                &helper
            )),
            Some(("helpertok".into(), TokenOrigin::Helper("gh auth token")))
        );
        assert_eq!(
            helper_calls.borrow().last().unwrap(),
            "gh auth token --hostname ghe.corp"
        );
        assert_eq!(
            t(resolve_token(
                Flavor::Github,
                None,
                "ghe.corp",
                &dotcom,
                &no_helper
            )),
            None
        );
        for host in ["github.com", "octo.ghe.com"] {
            assert_eq!(
                t(resolve_token(Flavor::Github, None, host, &both, &no_helper)),
                Some(("a".into(), TokenOrigin::Env("GITHUB_TOKEN"))),
                "{host}"
            );
        }

        // The hints name the variables that apply to the host.
        assert!(token_env_hint(Flavor::Github, "github.com").starts_with("set GITHUB_TOKEN"));
        let ghe = token_env_hint(Flavor::Github, "ghe.corp");
        assert!(
            ghe.starts_with("set GH_ENTERPRISE_TOKEN (or GITHUB_ENTERPRISE_TOKEN")
                && ghe.contains("gh auth login --hostname ghe.corp")
                && !ghe.contains("GITHUB_TOKEN"),
            "{ghe}"
        );
        assert!(token_env_hint(Flavor::Github, "octo.ghe.com").contains(
            "GITHUB_TOKEN (or GH_TOKEN, or log in with `gh auth login --hostname octo.ghe.com`)"
        ));
        let fake = Fake::with(vec![status(401, &[], r#"{"message":"Bad credentials"}"#)]);
        let c = ApiConfig {
            api_url: Some("https://ghe.corp/api/v3".into()),
            ..cfg(Flavor::Github, None, false)
        };
        let r = ApiRenderer::with_transport(c, fake).unwrap();
        let err = r
            .render("x", &opts(Flavor::Github, Mode::Comment, None))
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unauthorized (Bad credentials); set GH_ENTERPRISE_TOKEN"),
            "{msg}"
        );
    }

    #[test]
    fn cleartext_detection() {
        let url = |u: &str| url::Url::parse(u).unwrap();
        assert!(is_cleartext_remote(&url("http://ghe.corp/api/v3")));
        assert!(!is_cleartext_remote(&url("https://ghe.corp/api/v3")));
        for local in [
            "http://localhost:8080",
            "http://LOCALHOST",
            "http://gl.localhost",
            "http://127.0.0.1:1",
            "http://[::1]:1",
        ] {
            assert!(!is_cleartext_remote(&url(local)), "{local}");
        }
    }

    #[test]
    fn missing_helper_is_none() {
        assert_eq!(
            run_helper("md-render-definitely-not-installed-helper", &[]),
            None
        );
    }

    #[test]
    fn front_matter_split() {
        assert_eq!(
            split_front_matter("---\na: 1\n---\n# x\n"),
            (Some("---\na: 1\n---\n"), "# x\n")
        );
        assert_eq!(
            split_front_matter("---\r\na: 1\r\n---  \r\nrest"),
            (Some("---\r\na: 1\r\n---  \r\n"), "rest")
        );
        assert_eq!(split_front_matter("---\na: 1\n"), (None, "---\na: 1\n"));
        assert_eq!(
            split_front_matter(" ---\na\n---\n"),
            (None, " ---\na\n---\n")
        );
        assert_eq!(split_front_matter("---"), (None, "---"));
        assert_eq!(split_front_matter("---\n---\nx"), (Some("---\n---\n"), "x"));
        assert_eq!(
            split_front_matter("---\na: 1\n...\nx"),
            (Some("---\na: 1\n...\n"), "x")
        );
        assert_eq!(
            split_front_matter("--- \t\na: 1\n---"),
            (Some("--- \t\na: 1\n---"), "")
        );
        // `...` only closes a block.
        assert_eq!(split_front_matter("...\na\n...\n"), (None, "...\na\n...\n"));
    }

    /// Q-API-DUP: `--api` treats exactly the blocks as front matter that the offline renderer
    /// shows as a table, and sends GitHub only the rest. Pins `split_front_matter` to
    /// `github::frontmatter::extract` (and its line-ending normalization) until they share code.
    #[test]
    fn front_matter_split_matches_the_offline_renderer() {
        let table = |html: &str| html.contains("<markdown-accessiblity-table");
        for (md, front_matter) in [
            ("---\ntitle: T\n---\n# Doc\n", true),
            ("---\ntitle: T\n...\n# Doc\n", true),
            ("--- \ntitle: T\n---  \n# Doc\n", true),
            ("---\t\ntitle: T\n...\t\n# Doc\n", true),
            ("---\r\ntitle: T\r\n---\r\n# Doc\r\n", true),
            ("---\ntitle: T\n---", true),
            ("---\ntitle: T\n", false),
            (" ---\ntitle: T\n---\n# Doc\n", false),
            ("----\ntitle: T\n----\n# Doc\n", false),
            ("...\ntitle: T\n...\n# Doc\n", false),
            ("---", false),
            ("# Doc\n---\ntitle: T\n---\n", false),
        ] {
            let opts = opts(Flavor::Github, Mode::Readme, None);
            let offline = crate::render::github::render(md, &opts);
            assert_eq!(table(&offline.html), front_matter, "offline: {md:?}");

            let fake = Fake::with(vec![ok("<p>body</p>")]);
            let r = ApiRenderer::with_transport(cfg(Flavor::Github, None, false), fake.clone())
                .unwrap();
            let out = r.render(md, &opts).unwrap();
            let sent = fake.requests()[0].body["text"].as_str().unwrap().to_owned();
            assert_eq!(
                table(&out.html),
                front_matter,
                "--api: {md:?}: {}",
                out.html
            );
            if front_matter {
                assert!(
                    md.ends_with(&sent) && sent.len() < md.len(),
                    "{md:?}: {sent:?}"
                );
                assert!(!sent.contains("title: T"), "{md:?}: {sent:?}");
            } else {
                assert_eq!(sent, md);
            }
        }
    }

    #[test]
    fn readme_mode_strips_front_matter_before_sending() {
        let fake = Fake::with(vec![ok("<h1>Doc</h1>")]);
        let r =
            ApiRenderer::with_transport(cfg(Flavor::Github, None, false), fake.clone()).unwrap();
        let out = r
            .render(
                "---\ntitle: T\n---\n# Doc\n",
                &opts(Flavor::Github, Mode::Readme, None),
            )
            .unwrap();
        assert_eq!(fake.requests()[0].body["text"], "# Doc\n");
        assert_eq!(out.title.as_deref(), Some("Doc"));
        assert!(out.html.find("title").unwrap() < out.html.find("markdown-heading").unwrap());
        // Comment mode sends the document untouched.
        let fake = Fake::with(vec![ok("<hr>")]);
        let r =
            ApiRenderer::with_transport(cfg(Flavor::Github, None, false), fake.clone()).unwrap();
        r.render(
            "---\ntitle: T\n---\n",
            &opts(Flavor::Github, Mode::Comment, None),
        )
        .unwrap();
        assert_eq!(fake.requests()[0].body["text"], "---\ntitle: T\n---\n");
    }

    #[test]
    fn slugs_match_github() {
        let mut s = Slugger::default();
        let got: Vec<String> = ["Duplicate", "Duplicate", "Duplicate", "Duplicate-1"]
            .iter()
            .map(|t| s.slug(t))
            .collect();
        assert_eq!(
            got,
            ["duplicate", "duplicate-1", "duplicate-2", "duplicate-1-1"]
        );
        let mut s = Slugger::default();
        let got: Vec<String> = ["Foo-1", "Foo", "Foo", "Foo"]
            .iter()
            .map(|t| s.slug(t))
            .collect();
        assert_eq!(got, ["foo-1", "foo", "foo-2", "foo-3"]);
        for (text, slug) in [
            ("Hello, World! What's up?", "hello-world-whats-up"),
            (
                "Ünïcödé Hëädïng — Ελληνικά 日本語",
                "ünïcödé-hëädïng--ελληνικά-日本語",
            ),
            (
                r#"C++ & C# / Rust's "quotes" (parens) [brackets] {braces}"#,
                "c--c--rusts-quotes-parens-brackets-braces",
            ),
            ("under_score-dash.dot", "under_score-dashdot"),
            ("Tab\tinside", "tabinside"),
            ("İstanbul ΣΑΣ", "i\u{307}stanbul-σασ"),
            ("x² ½ Ⅻ ① ٣ ๓", "x--ⅻ--٣-๓"),
            ("a‿b ﹏c _d", "a‿b-﹏c-_d"),
            (
                "Zero\u{200C}width\u{200D}joiner\u{200B}",
                "zero\u{200C}width\u{200D}joiner",
            ),
            ("ẞ", "ß"),
            ("e\u{301}", "e\u{301}"),
            ("???", ""),
        ] {
            assert_eq!(Slugger::base(text), slug, "{text}");
        }
    }

    #[test]
    fn github_readme_postprocessing() {
        let api = concat!(
            "<h1>Title <code class=\"notranslate\">x</code></h1>\n",
            "<h2>Title</h2>\n",
            "<h2 id=\"user-content-raw\" dir=\"auto\">Raw</h2>\n",
            "<h3></h3>\n",
            "<h2>???</h2>\n",
            "<p><a target=\"_blank\" rel=\"noopener noreferrer nofollow\" href=\"https://camo.githubusercontent.com/abc/def\"><img src=\"https://camo.githubusercontent.com/abc/def\" alt=\"A\" data-canonical-src=\"https://e.org/a.png\" style=\"max-width: 100%;\"></a></p>\n",
            "<table role=\"table\"><tr><td>c</td></tr></table>\n",
            "<div class=\"highlight highlight-source-rust\"><pre class=\"notranslate\"><span class=\"pl-k\">fn</span> a&lt;b&gt;()\n</pre></div>\n",
            "<pre class=\"notranslate\"><code class=\"notranslate\">plain \"q\"\n</code></pre>\n",
            "<section data-footnotes=\"\" class=\"footnotes\"><h2 id=\"footnote-label\" class=\"sr-only\">Footnotes</h2>\n<ol><li id=\"user-content-fn-1-0123456789abcdef0123456789abcdef\"><p>n</p></li></ol></section>\n",
            "<p><math-renderer class=\"js-inline-math\" style=\"display: inline-block\" data-run-id=\"0123456789abcdef0123456789abcdef\">$x$</math-renderer></p>\n",
        );
        let out = postprocess_github(
            api,
            Mode::Readme,
            "path:/a/README.md",
            &SourceHints::default(),
        );
        let html = &out.html;
        assert_eq!(out.title.as_deref(), Some("Title x"));
        assert!(html.contains(r##"<div class="markdown-heading" dir="auto"><h1 class="heading-element" dir="auto">Title <code>x</code></h1><a id="user-content-title-x" class="anchor" aria-label="Permalink: Title x" href="#title-x"><svg data-component="Octicon" class="octicon octicon-link""##), "{html}");
        assert!(html.contains(r##"<h2 class="heading-element" dir="auto">Title</h2><a id="user-content-title" class="anchor" aria-label="Permalink: Title" href="#title">"##), "{html}");
        assert!(html.contains(r##"<h2 id="user-content-raw" class="heading-element" dir="auto">Raw</h2><a id="user-content-raw" class="anchor""##), "{html}");
        assert!(html.contains("<h3></h3>"), "{html}");
        assert!(
            html.contains(r##"<a id="" class="anchor" aria-label="Permalink: ???" href="#">"##),
            "{html}"
        );
        assert!(
            html.contains(r#"<h2 id="footnote-label" class="sr-only" dir="auto">Footnotes</h2>"#),
            "{html}"
        );
        assert!(html.contains(r#"href="https://e.org/a.png"><img src="https://e.org/a.png" alt="A" style="max-width: 100%;">"#), "{html}");
        assert!(
            !html.contains("camo") && !html.contains("data-canonical-src"),
            "{html}"
        );
        assert!(
            html.contains("<table><tbody><tr><td>c</td></tr></tbody></table>"),
            "{html}"
        );
        assert!(html.contains(r#"<div class="highlight highlight-source-rust notranslate position-relative overflow-auto" dir="auto" data-snippet-clipboard-copy-content="fn a&lt;b&gt;()" data-mdr-key=""#), "{html}");
        assert!(
            html.contains(r#"><pre><span class="pl-k">fn</span>"#),
            "{html}"
        );
        assert!(html.contains(r#"<div class="snippet-clipboard-content notranslate position-relative overflow-auto" data-snippet-clipboard-copy-content="plain &quot;q&quot;" data-mdr-key=""#), "{html}");
        assert!(
            html.contains(r#"<pre class="notranslate"><code>plain "q""#),
            "{html}"
        );
        let stable = &sha256_hex(b"path:/a/README.md")[..32];
        assert!(!html.contains("0123456789abcdef0123456789abcdef"), "{html}");
        assert!(
            html.contains(&format!("user-content-fn-1-{stable}"))
                && html.contains(&format!("data-run-id=\"{stable}\"")),
            "{html}"
        );
        assert!(out.features.math && !out.features.mermaid);
        // Idempotent on its own output.
        let again = postprocess_github(
            html,
            Mode::Readme,
            "path:/a/README.md",
            &SourceHints::default(),
        );
        assert_same(&again.html, html);
    }

    /// `assert_eq!` for long HTML: shows the neighbourhood of the first difference.
    fn assert_same(got: &str, want: &str) {
        if got == want {
            return;
        }
        let at = got
            .bytes()
            .zip(want.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let from = at.saturating_sub(80);
        let window = |s: &str| s.get(from..(at + 120).min(s.len())).unwrap_or(s).to_owned();
        panic!(
            "outputs differ at byte {at}:\n got: {}\nwant: {}",
            window(got),
            window(want)
        );
    }

    #[test]
    fn source_hints_collect_heading_texts_and_code() {
        let md = concat!(
            "# :rocket: Launch ![logo](x.png) `code` $x^2$[^1]\n\n",
            "<h1 align=\"center\">\n  :black_circle: Raw :black_circle:\n</h1>\n\n",
            "Setext\n======\n\n",
            "```md\n\n> quoted\n```\n\n",
            "    indented\n\n",
            "[^1]: note\n",
        );
        let hints = SourceHints::from_markdown(md);
        assert_eq!(
            hints.headings,
            [
                ":rocket: Launch  code $x^2$1",
                "\n  :black_circle: Raw :black_circle:\n",
                "Setext"
            ]
        );
        assert_eq!(hints.code, ["\n> quoted\n", "indented\n"]);
        assert_eq!(
            heading_key(":rocket: Launch  code $x^2$1"),
            heading_key("🚀 Launch  code $x^2$1")
        );
        assert_eq!(heading_key("Ünï :x: 日本"), "ünï日本");
    }

    #[test]
    fn github_headings_and_code_use_source_hints() {
        let md = concat!(
            "## :rocket: Launch\n\n",
            "<h2 align=\"center\">\n  :black_circle: Raw\n</h2>\n\n",
            "#\n\n",
            "## ???\n\n",
            "```md\n\n> [!WARNING]\n```\n\n",
            "```\n\nplain\n```\n",
        );
        let api = concat!(
            "<h2>🚀 Launch</h2>\n",
            "<h2 align=\"center\">\n  ⚫ Raw\n</h2>\n",
            "<h1></h1>\n",
            "<h2>???</h2>\n",
            "<div class=\"highlight highlight-text-md\"><pre class=\"notranslate\"><span class=\"pl-ent\">&gt; [!WARNING]</span></pre></div>\n",
            "<pre class=\"notranslate\"><code class=\"notranslate\">\nplain\n</code></pre>\n",
            "<pre class=\"notranslate\">raw <b>pre</b></pre>\n",
            "<p><img src=\"https://github.com/o/r/blob/main/a.png\"></p>\n",
        );
        let out = postprocess_github(api, Mode::Readme, "stdin", &SourceHints::from_markdown(md));
        let html = &out.html;
        assert!(html.contains(r##"<h2 class="heading-element" dir="auto">🚀 Launch</h2><a id="user-content-rocket-launch" class="anchor" aria-label="Permalink: :rocket: Launch" href="#rocket-launch">"##), "{html}");
        assert!(
            html.contains(r##"<h2 align="center" class="heading-element" dir="auto">"##),
            "{html}"
        );
        assert!(
            html.contains("id=\"user-content---black_circle-raw\""),
            "{html}"
        );
        assert!(
            html.contains("aria-label=\"Permalink: \n  :black_circle: Raw\n\""),
            "{html}"
        );
        assert!(html.contains("<h1></h1>"), "{html}");
        assert!(
            html.contains(r##"<a id="" class="anchor" aria-label="Permalink: ???" href="#">"##),
            "{html}"
        );
        assert!(
            html.contains("data-snippet-clipboard-copy-content=\"\n&gt; [!WARNING]\""),
            "{html}"
        );
        // The leading newline of `<pre>\n` survives HTML parsing for the clipboard text.
        assert!(
            html.contains("data-snippet-clipboard-copy-content=\"\nplain\""),
            "{html}"
        );
        // Raw HTML `<pre>`: no copy button, no `notranslate` in the README view.
        assert!(
            html.contains("<pre>raw <b>pre</b></pre>") && !html.contains("data-mdr-pre-lf"),
            "{html}"
        );
        assert!(
            html.contains(r#"src="https://github.com/o/r/raw/main/a.png""#),
            "{html}"
        );
        // Without hints the DOM text is used.
        let plain = postprocess_github(api, Mode::Readme, "stdin", &SourceHints::default());
        assert!(
            plain.html.contains("id=\"user-content--launch\""),
            "{}",
            plain.html
        );
        assert!(
            plain.html.contains("aria-label=\"Permalink: 🚀 Launch\""),
            "{}",
            plain.html
        );
    }

    #[test]
    fn misaligned_hints_fall_back_to_the_dom() {
        // A hint list that does not correspond to the document must not mislabel headings.
        let hints = SourceHints::from_markdown("# Alpha\n\n# Beta\n");
        let out = postprocess_github(
            "<h1>Gamma</h1><h1>Beta</h1><h1>Alpha</h1>",
            Mode::Readme,
            "s",
            &hints,
        );
        assert!(
            out.html.contains("id=\"user-content-gamma\""),
            "{}",
            out.html
        );
        assert!(
            out.html.contains("id=\"user-content-beta\""),
            "{}",
            out.html
        );
        // `Alpha` was skipped over when `Beta` matched; it is not reused out of order.
        assert!(
            out.html.contains("id=\"user-content-alpha\""),
            "{}",
            out.html
        );
    }

    #[test]
    fn github_comment_mode_keeps_api_markup() {
        let api = "<h2 dir=\"auto\">T</h2>\n<p dir=\"auto\"><code class=\"notranslate\">x</code> <span aria-hidden=\"true\" class=\"octicon octicon-link\"></span></p>\n<table role=\"table\"><tr><td>c</td></tr></table>";
        let out = postprocess_github(api, Mode::Comment, "stdin", &SourceHints::default());
        assert!(
            out.html.starts_with("<h2 dir=\"auto\">T</h2>"),
            "{}",
            out.html
        );
        assert!(out.html.contains("<code class=\"notranslate\">x</code>"));
        assert!(out.html.contains("role=\"table\""));
        assert!(
            out.html
                .contains("<svg data-component=\"Octicon\" class=\"octicon octicon-link\"")
        );
        assert_eq!(out.title, None);
    }

    #[test]
    fn github_mermaid_becomes_enrichment_section() {
        let api = "<div class=\"highlight highlight-source-mermaid\"><pre class=\"notranslate\">graph TD\n  A --&gt; B</pre></div>";
        let out = postprocess_github(api, Mode::Readme, "stdin", &SourceHints::default());
        let html = &out.html;
        assert!(out.features.mermaid);
        assert!(html.contains(r#"data-type="mermaid""#), "{html}");
        assert!(
            html.contains(
                r#"data-plain="graph TD
  A --&gt; B
""#
            ),
            "{html}"
        );
        assert!(
            html.contains(r#"data-json="{&quot;data&quot;:&quot;graph TD\n  A --&gt; B\n&quot;}""#),
            "{html}"
        );
        assert!(html.contains("<pre lang=\"mermaid\" aria-label=\"Raw mermaid code\">graph TD\n  A --&gt; B\n</pre>"), "{html}");
        assert!(!html.contains("snippet-clipboard-content"), "{html}");
        let id = stable_identity("graph TD\n  A --> B\n");
        assert!(html.contains(&format!("data-identity=\"{id}\"")), "{html}");
        // An API section with a random identity gets the same deterministic one.
        let again = postprocess_github(
            &html.replace(&id, "11111111-2222-4333-a444-555555555555"),
            Mode::Readme,
            "stdin",
            &SourceHints::default(),
        );
        assert!(again.html.contains(&id));
    }

    #[test]
    fn gitlab_postprocessing() {
        let api = concat!(
            "<h1 id=\"user-content-t\" dir=\"auto\">T<a href=\"#t\" class=\"anchor\"></a></h1>\n",
            "<p><a class=\"no-attachment-icon gfm\" href=\"/-/project/1/uploads/a/i.png\" target=\"_blank\"><img src=\"data:image/gif;base64,R0lGOD\" alt=\"i\" decoding=\"async\" class=\"lazy gfm\" data-src=\"/-/project/1/uploads/a/i.png\"></a></p>\n",
            "<p><img src=\"data:image/gif;base64,R0lGOD\" class=\"lazy\" data-src=\"https://e.org/x.png\"></p>\n",
            "<div class=\"gl-relative markdown-code-block js-markdown-code\"><pre class=\"code highlight js-syntax-highlight language-rust\" v-pre=\"true\"><code><span id=\"LC1\" class=\"line\" lang=\"rust\">fn</span></code></pre><copy-code></copy-code></div>\n",
            "<p><span data-math-style=\"inline\" class=\"js-render-math\">x</span></p>\n",
        );
        let out = postprocess_gitlab(api, "https://gitlab.com", "");
        let html = &out.html;
        assert_eq!(out.title.as_deref(), Some("T"));
        assert!(
            html.contains(r#"href="https://gitlab.com/-/project/1/uploads/a/i.png""#),
            "{html}"
        );
        assert!(html.contains(r#"<img src="https://gitlab.com/-/project/1/uploads/a/i.png" alt="i" decoding="async" class="gfm js-lazy-loaded" loading="lazy">"#), "{html}");
        assert!(
            html.contains(
                r#"<img src="https://e.org/x.png" class="js-lazy-loaded" loading="lazy">"#
            ),
            "{html}"
        );
        assert!(html.contains(r#"class="code highlight js-syntax-highlight language-rust code-syntax-highlight-theme""#), "{html}");
        assert!(
            html.contains(
                r#"class="gl-relative markdown-code-block js-markdown-code" data-mdr-key=""#
            ),
            "{html}"
        );
        assert!(out.features.math && !out.features.mermaid);
        assert_eq!(
            postprocess_gitlab(html, "https://gitlab.com", "").html,
            *html
        );

        let prefixed = postprocess_gitlab(
            "<a href=\"/gl/-/x\">x</a><a href=\"/-/y\">y</a>",
            "https://x.org",
            "/gl",
        );
        assert!(
            prefixed.html.contains("href=\"https://x.org/gl/-/x\"")
                && prefixed.html.contains("href=\"/-/y\""),
            "{}",
            prefixed.html
        );
    }

    #[test]
    fn postprocessing_never_panics_on_odd_input() {
        for input in [
            "",
            "<",
            "<h1>",
            "</div></div>",
            "<pre>",
            "<img data-src>",
            "<svg><h1>x</h1></svg>",
            "\u{0}",
        ] {
            postprocess_github(input, Mode::Readme, "s", &SourceHints::default());
            postprocess_github(input, Mode::Comment, "s", &SourceHints::default());
            postprocess_gitlab(input, "https://gitlab.com", "");
        }
    }
}
