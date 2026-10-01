//! Live-update wire format (DESIGN §7, GRIP §11.1) and the SSE endpoint.
//!
//! Every event is a named SSE event with a JSON `data:` payload and
//! `id: <server_id>:<version>`; the stream starts with `retry: 1000` and `hello`, and a
//! `: ping` comment is sent every 15 s. Each connection is served by one small task that
//! forwards the document's broadcast events (plus global `styles` events) into the response
//! body and ends the stream with `shutdown` when the server stops, which is what lets
//! axum's graceful shutdown complete (CR §3).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::ReceiverStream;

use super::App;
use super::hub::DocSnapshot;
use crate::render::Rendered;

/// Reconnect delay suggested to `EventSource` clients.
pub const RETRY: Duration = Duration::from_millis(1000);

/// Interval of `: ping` keepalive comments.
pub const KEEPALIVE: Duration = Duration::from_secs(15);

/// `kind` of an `error` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Read,
    /// Part of the wire protocol (DESIGN §7). This server decodes lossily and reports
    /// invalid UTF-8 as a content warning instead (GRIP C38), so it never sends it.
    #[allow(dead_code)]
    Decode,
    Render,
    Api,
    Bundle,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Read => "read",
            ErrorKind::Decode => "decode",
            ErrorKind::Render => "render",
            ErrorKind::Api => "api",
            ErrorKind::Bundle => "bundle",
        }
    }
}

/// One server-sent event, serialized once and shared by all subscribers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireEvent {
    /// SSE event name (`hello`, `content`, ...).
    pub name: &'static str,
    /// Published document version this event carries; `None` for events that do not publish
    /// a new document state (`hello`, `rendering`, `assets`, `styles`, `shutdown`): their
    /// `id` repeats the last version delivered to that subscriber, so a `Last-Event-ID` sent
    /// on reconnect never claims a state the client has not received (e.g. `rendering`
    /// announces version N before `content` N exists).
    pub version: Option<u64>,
    /// State events (`content`, `error`, `deleted`) are skipped by a subscriber that was
    /// already brought up to date past their version.
    pub state: bool,
    /// JSON payload.
    pub data: String,
}

impl WireEvent {
    fn new(name: &'static str, version: Option<u64>, state: bool, data: serde_json::Value) -> WireEvent {
        WireEvent { name, version, state, data: data.to_string() }
    }

    /// `hello {server_id, version, styles_version}`.
    pub fn hello(server_id: &str, version: u64, styles_version: u64) -> WireEvent {
        let data = json!({ "server_id": server_id, "version": version, "styles_version": styles_version });
        WireEvent::new("hello", None, false, data)
    }

    /// `rendering {version}`.
    pub fn rendering(version: u64) -> WireEvent {
        WireEvent::new("rendering", None, false, json!({ "version": version }))
    }

    /// `content {version, html, title, warnings}`.
    pub fn content(version: u64, r: &Rendered) -> WireEvent {
        WireEvent::new("content", Some(version), true, content_json(version, r))
    }

    /// `error {version, kind, message}`.
    pub fn error(version: u64, kind: ErrorKind, message: &str) -> WireEvent {
        let data = json!({ "version": version, "kind": kind.as_str(), "message": message });
        WireEvent::new("error", Some(version), true, data)
    }

    /// `deleted {path}` (plus `version`).
    pub fn deleted(version: u64, path: &str) -> WireEvent {
        WireEvent::new("deleted", Some(version), true, json!({ "version": version, "path": path }))
    }

    /// `assets {urls, v}`: local files referenced by the document changed.
    pub fn assets(urls: &[String], v: &str) -> WireEvent {
        WireEvent::new("assets", None, false, json!({ "urls": urls, "v": v }))
    }

    /// `styles {styles_version, hrefs}`: the style bundle or a `--css` file changed. `hrefs`
    /// lists the stylesheet URLs the page should now use, so a client whose `<link>`s point
    /// at files that no longer exist (re-vendored hashed names) can reload instead.
    pub fn styles(styles_version: u64, hrefs: &[String]) -> WireEvent {
        WireEvent::new("styles", None, false, json!({ "styles_version": styles_version, "hrefs": hrefs }))
    }

    /// A global `error` (e.g. kind `bundle`): not tied to a document version.
    pub fn global_error(kind: ErrorKind, message: &str) -> WireEvent {
        WireEvent::new("error", None, false, json!({ "kind": kind.as_str(), "message": message }))
    }

    /// `shutdown {}`.
    pub fn shutdown() -> WireEvent {
        WireEvent::new("shutdown", None, false, json!({}))
    }

    /// The axum SSE event, with `id: <server_id>:<version>`; `delivered` is the last document
    /// version this subscriber has received (used for events without their own version).
    pub fn to_sse(&self, server_id: &str, delivered: u64) -> Event {
        let version = self.version.unwrap_or(delivered);
        // JSON never contains raw newlines or CRs, so `data` cannot panic here.
        Event::default().event(self.name).id(format!("{server_id}:{version}")).data(&self.data)
    }
}

/// The JSON object shared by `content` events and the `/render` endpoint.
pub fn content_json(version: u64, r: &Rendered) -> serde_json::Value {
    json!({ "version": version, "html": r.html, "title": r.title, "warnings": r.warnings })
}

/// Query of `/__/md-render/events` and `/__/md-render/render`.
#[derive(Debug, Deserialize)]
pub struct DocQuery {
    /// Document path relative to the served root.
    pub path: Option<String>,
    /// Version the client already has (alternative to `Last-Event-ID`).
    pub v: Option<u64>,
    /// Server id the `v` was issued by; `v` is ignored when it names another process
    /// (versions restart at 1 with every server start).
    pub sid: Option<String>,
}

/// The version a (re)connecting client already has: from `Last-Event-ID`
/// (`<server_id>:<v>`) or the `v` query parameter, each only when issued by this process;
/// the larger wins.
pub fn client_version(headers: &HeaderMap, q: &DocQuery, server_id: &str) -> u64 {
    let from_header = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.rsplit_once(':'))
        .filter(|(sid, _)| *sid == server_id)
        .and_then(|(_, v)| v.parse::<u64>().ok());
    let from_query = q.v.filter(|_| q.sid.as_deref().is_none_or(|sid| sid == server_id));
    from_header.unwrap_or(0).max(from_query.unwrap_or(0))
}

/// `GET /__/md-render/events?path=<doc>[&v=<version>]`.
pub async fn events(State(app): State<Arc<App>>, Query(q): Query<DocQuery>, headers: HeaderMap) -> Response {
    if !app.live {
        return (StatusCode::NOT_FOUND, "live updates are disabled (--norefresh)").into_response();
    }
    let key = match app.validate_doc_param(q.path.as_deref()) {
        Ok(k) => k,
        Err((status, msg)) => return (status, msg).into_response(),
    };
    let since = client_version(&headers, &q, &app.server_id);
    let sub = app.hub.subscribe(&key);
    let (tx, rx) = mpsc::channel::<Event>(32);
    tokio::spawn(client_task(app.clone(), sub, since, tx));
    let stream = ReceiverStream::new(rx).map(Ok::<Event, Infallible>);
    let mut resp = Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEPALIVE).text("ping")).into_response();
    // Proxies (and some dev tools) must not buffer the stream.
    resp.headers_mut().insert("x-accel-buffering", axum::http::HeaderValue::from_static("no"));
    resp
}

/// Feed one SSE connection until the client goes away or the server shuts down.
async fn client_task(app: Arc<App>, sub: super::hub::Subscription, since: u64, tx: mpsc::Sender<Event>) {
    // Subscribe to the broadcasts *before* reading the snapshot so nothing published in
    // between is lost; duplicates are filtered by version below.
    let mut doc_rx = sub.entry.subscribe_events();
    let mut global_rx = app.global.subscribe();
    let mut shutdown = app.shutdown.clone();
    let server_id = app.server_id.clone();
    let snap = sub.entry.snapshot();
    let mut covered = snap.version.max(since);

    let send = |ev: Event| {
        let tx = tx.clone();
        async move { tx.send(ev).await.is_ok() }
    };
    if !send(Event::default().retry(RETRY)).await {
        return;
    }
    // `hello` goes first with the version the client is at *before* the catch-up, so a
    // client that reconnects mid-catch-up asks for it again.
    let hello = WireEvent::hello(&server_id, snap.version, app.styles.version());
    if !send(hello.to_sse(&server_id, since.min(snap.version))).await {
        return;
    }
    for ev in snap.catch_up(&sub.entry.key, since) {
        if !send(ev.to_sse(&server_id, covered)).await {
            return;
        }
    }

    loop {
        tokio::select! {
            biased;
            // The `wait_for` guard must not live across the `send` below (it is not `Send`).
            _ = async { drop(shutdown.wait_for(|stop| *stop).await) } => {
                let _ = send(WireEvent::shutdown().to_sse(&server_id, covered)).await;
                break;
            }
            _ = tx.closed() => break,
            msg = doc_rx.recv() => match msg {
                Ok(ev) => {
                    if ev.state && ev.version.is_some_and(|v| v <= covered) {
                        continue; // already delivered through the catch-up
                    }
                    if let Some(v) = ev.version {
                        covered = covered.max(v);
                    }
                    if !send(ev.to_sse(&server_id, covered)).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Too slow to keep up: resend the current state instead of the backlog.
                    let snap: DocSnapshot = sub.entry.snapshot();
                    let since = covered;
                    covered = covered.max(snap.version);
                    for ev in snap.catch_up(&sub.entry.key, since) {
                        if !send(ev.to_sse(&server_id, covered)).await {
                            return;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = global_rx.recv() => match msg {
                Ok(ev) => {
                    if !send(ev.to_sse(&server_id, covered)).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Only `styles` and bundle errors are global; the latest styles state
                    // supersedes whatever was missed.
                    let ev = WireEvent::styles(app.styles.version(), &app.styles.hrefs());
                    if !send(ev.to_sse(&server_id, covered)).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    drop(sub);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_payloads() {
        let r = Rendered { html: "<p>x</p>\n".into(), title: Some("T".into()), ..Default::default() };
        let c = WireEvent::content(3, &r);
        assert_eq!(c.name, "content");
        assert!(c.state);
        let v: serde_json::Value = serde_json::from_str(&c.data).unwrap();
        assert_eq!(v["version"], 3);
        assert_eq!(v["html"], "<p>x</p>\n");
        assert_eq!(v["title"], "T");
        assert_eq!(v["warnings"], json!([]));
        assert!(!c.data.contains('\n'), "JSON payloads are single-line");

        let h = WireEvent::hello("abc", 2, 5);
        let v: serde_json::Value = serde_json::from_str(&h.data).unwrap();
        assert_eq!(v, json!({"server_id": "abc", "version": 2, "styles_version": 5}));

        let e = WireEvent::error(4, ErrorKind::Render, "boom");
        let v: serde_json::Value = serde_json::from_str(&e.data).unwrap();
        assert_eq!(v, json!({"version": 4, "kind": "render", "message": "boom"}));

        let d = WireEvent::deleted(5, "docs/a.md");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&d.data).unwrap()["path"], "docs/a.md");
        let a = WireEvent::assets(&["/img.png".into()], "17");
        assert_eq!(a.version, None);
        let s = WireEvent::styles(2, &["/__/md-render/bundle/github/a.css".into()]);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&s.data).unwrap()["styles_version"], 2);
        assert_eq!(WireEvent::shutdown().data, "{}");
        assert_eq!(ErrorKind::Bundle.as_str(), "bundle");
        assert_eq!(ErrorKind::Decode.as_str(), "decode");
        assert_eq!(ErrorKind::Api.as_str(), "api");
        assert_eq!(ErrorKind::Read.as_str(), "read");
    }

    fn q(v: Option<u64>, sid: Option<&str>) -> DocQuery {
        DocQuery { path: None, v, sid: sid.map(str::to_owned) }
    }

    #[test]
    fn last_event_id_parsing() {
        let mut h = HeaderMap::new();
        assert_eq!(client_version(&h, &q(None, None), "sid"), 0);
        assert_eq!(client_version(&h, &q(Some(4), None), "sid"), 4);
        assert_eq!(client_version(&h, &q(Some(4), Some("sid")), "sid"), 4);
        // A `v` issued by another server process is meaningless here.
        assert_eq!(client_version(&h, &q(Some(4), Some("old")), "sid"), 0);
        h.insert("last-event-id", "sid:7".parse().unwrap());
        assert_eq!(client_version(&h, &q(None, None), "sid"), 7);
        assert_eq!(client_version(&h, &q(Some(9), None), "sid"), 9);
        // Ids from another server process are ignored.
        assert_eq!(client_version(&h, &q(None, None), "other"), 0);
        h.insert("last-event-id", "garbage".parse().unwrap());
        assert_eq!(client_version(&h, &q(None, None), "sid"), 0);
    }

    #[test]
    fn ids_of_unversioned_events_use_the_delivered_version() {
        assert_eq!(WireEvent::rendering(9).version, None);
        assert_eq!(WireEvent::hello("s", 3, 0).version, None);
        assert_eq!(WireEvent::content(4, &Rendered::default()).version, Some(4));
        assert!(!WireEvent::rendering(9).state);
        assert!(WireEvent::deleted(2, "a.md").state);
    }
}
