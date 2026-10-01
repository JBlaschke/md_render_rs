//! The HTTP layer of `md-render vendor`, behind a small trait so tests can inject canned
//! responses. The real implementation uses ureq 3 (CR §4): rustls with the platform verifier,
//! redirects followed, status codes returned as values, bounded bodies, timeouts, and a couple
//! of retries for transient failures.

use std::time::Duration;

use anyhow::Context as _;

/// User-Agent sent by `md-render vendor` (GL §11.2: send a descriptive UA).
pub const USER_AGENT: &str = concat!(
    "md-render/",
    env!("CARGO_PKG_VERSION"),
    " (grip-like Markdown previewer; vendoring site styles for offline preview)"
);

/// Largest body accepted for one download (mermaid.min.js is ~3.6 MB, GitLab's application CSS
/// ~1 MB).
pub const MAX_BODY_BYTES: u64 = 64 << 20;

/// One HTTP response, fully read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// HTTP status code (non-2xx statuses are values, not errors).
    pub status: u16,
    /// URL after redirects.
    pub final_url: String,
    /// `Content-Type` header, if any.
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl Response {
    /// A `200 OK` response (handy for test doubles).
    pub fn ok(url: &str, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status: 200,
            final_url: url.to_owned(),
            content_type: Some(content_type.to_owned()),
            body: body.into(),
        }
    }

    /// A body-less response with `status` (handy for test doubles).
    pub fn status(url: &str, status: u16) -> Response {
        Response {
            status,
            final_url: url.to_owned(),
            content_type: None,
            body: Vec::new(),
        }
    }

    /// Whether the status is 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The media type without parameters, lowercased (`text/css; charset=utf-8` → `text/css`).
    pub fn mime(&self) -> Option<String> {
        let ct = self.content_type.as_deref()?;
        let mime = ct
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        (!mime.is_empty()).then_some(mime)
    }
}

/// Minimal HTTP GET used by the vendoring code.
///
/// `Err` means the request could not be made or the body could not be read (DNS, TLS, timeout,
/// body too large); an HTTP error status is an `Ok` response with that status.
pub trait Fetch: Send + Sync {
    /// GET `url`, following redirects.
    fn get(&self, url: &str) -> anyhow::Result<Response>;

    /// User-Agent recorded in the manifest.
    fn user_agent(&self) -> &str {
        USER_AGENT
    }
}

/// [`Fetch`] over a ureq 3 agent.
#[derive(Debug)]
pub struct UreqFetcher {
    agent: ureq::Agent,
    user_agent: String,
    retries: u32,
    backoff: Duration,
}

impl Default for UreqFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqFetcher {
    /// Agent with the [`USER_AGENT`], 15 s connect / 180 s overall timeouts, up to 10 redirects
    /// and 2 retries on transient failures.
    pub fn new() -> Self {
        Self::with_user_agent(USER_AGENT)
    }

    /// Like [`UreqFetcher::new`] with a custom User-Agent.
    pub fn with_user_agent(user_agent: &str) -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            // OS trust store, so corporate TLS-inspecting proxies work (CR §4).
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(user_agent)
            .max_redirects(10)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(180)))
            .http_status_as_error(false)
            .tls_config(tls)
            .build()
            .into();
        UreqFetcher {
            agent,
            user_agent: user_agent.to_owned(),
            retries: 2,
            backoff: Duration::from_millis(500),
        }
    }

    fn get_once(&self, url: &str) -> Result<Response, ureq::Error> {
        use ureq::ResponseExt as _;
        let mut r = self.agent.get(url).call()?;
        let status = r.status().as_u16();
        let final_url = r.get_uri().to_string();
        let content_type = r
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = r
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()?;
        Ok(Response {
            status,
            final_url,
            content_type,
            body,
        })
    }
}

/// Errors worth retrying (the network hiccupped; the request itself is fine).
fn is_transient(e: &ureq::Error) -> bool {
    matches!(
        e,
        ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::ConnectionFailed
            | ureq::Error::BodyStalled
            | ureq::Error::Protocol(_)
    )
}

impl Fetch for UreqFetcher {
    fn get(&self, url: &str) -> anyhow::Result<Response> {
        let mut attempt = 0;
        loop {
            let result = self.get_once(url);
            let retry = match &result {
                Ok(r) => matches!(r.status, 429 | 502 | 503 | 504),
                Err(e) => is_transient(e),
            };
            if !retry || attempt >= self.retries {
                return result.with_context(|| format!("GET {url}"));
            }
            attempt += 1;
            std::thread::sleep(self.backoff * attempt);
        }
    }

    fn user_agent(&self) -> &str {
        &self.user_agent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_helpers() {
        let r = Response::ok("u", "Text/CSS; charset=utf-8", "a{}");
        assert!(r.is_success());
        assert_eq!(r.mime().as_deref(), Some("text/css"));
        let r = Response::status("u", 404);
        assert!(!r.is_success());
        assert_eq!(r.mime(), None);
        assert!(Response::status("u", 204).is_success());
    }

    #[test]
    fn user_agent_names_the_tool() {
        assert!(USER_AGENT.starts_with(concat!("md-render/", env!("CARGO_PKG_VERSION"))));
        let f = UreqFetcher::with_user_agent("x/1");
        assert_eq!(f.user_agent(), "x/1");
    }
}
