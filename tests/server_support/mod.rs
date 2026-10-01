//! Shared helpers for the `server_*` integration tests.
//!
//! * [`TestServer`]: a temporary served root (`<tmp>/root`, so `<tmp>` can hold files that
//!   must stay unreachable), a counting test renderer and a server on `127.0.0.1:0`.
//! * A raw HTTP/1.1 client over `TcpStream` ([`request`]): exact request targets (no client
//!   side path normalisation, so traversal attempts reach the server as written), custom
//!   `Host` headers, chunked decoding.
//! * An SSE reader ([`SseStream`]) that parses the event stream incrementally.
//!
//! No network access: everything talks to the loopback server.

#![allow(dead_code)] // each test crate uses a different subset

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use md_render::flavor::Flavor;
use md_render::render::{RenderOptions, Rendered, Renderer};
use md_render::server::{RunningServer, ServeConfig, Target};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// Generous upper bound for anything that involves the file watcher.
pub const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Deterministic stand-in for the flavor renderers (whose exact HTML is not this crate
/// area's concern): every non-empty line becomes a block with `data-sourcepos`;
/// `![](url)` lines become images; a line `FAIL` makes the render fail.
pub struct TestRenderer {
    pub renders: AtomicUsize,
}

impl Renderer for TestRenderer {
    fn render(&self, markdown: &str, opts: &RenderOptions) -> anyhow::Result<Rendered> {
        self.renders.fetch_add(1, Ordering::SeqCst);
        let mut html = String::new();
        for (i, line) in markdown.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line == "FAIL" {
                anyhow::bail!("the test renderer refuses line {}", i + 1);
            }
            let n = i + 1;
            if let Some(src) = line.strip_prefix("![](").and_then(|s| s.strip_suffix(')')) {
                html.push_str(&format!(
                    "<p data-sourcepos=\"{n}:1-{n}:{}\"><img src=\"{}\" alt=\"\"></p>\n",
                    line.len(),
                    escape(src)
                ));
            } else {
                html.push_str(&format!(
                    "<p data-sourcepos=\"{n}:1-{n}:{}\">{}</p>\n",
                    line.len(),
                    escape(line)
                ));
            }
        }
        assert!(opts.sourcepos, "the server renders with sourcepos");
        Ok(Rendered {
            html,
            title: None,
            ..Default::default()
        })
    }

    fn name(&self) -> &'static str {
        "offline"
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A running server over a temporary directory.
pub struct TestServer {
    server: Option<RunningServer>,
    pub addr: SocketAddr,
    /// The temporary directory (canonical); the served root is `tmp/root`.
    pub tmp: PathBuf,
    /// Canonical served root.
    pub root: PathBuf,
    pub renderer: Arc<TestRenderer>,
    _dir: tempfile::TempDir,
}

/// A temporary directory with a served root inside it, before the server starts.
pub struct Fixture {
    pub tmp: PathBuf,
    pub root: PathBuf,
    dir: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let tmp = std::fs::canonicalize(dir.path()).expect("canonical tempdir");
        let root = tmp.join("root");
        std::fs::create_dir(&root).expect("create root");
        Fixture { tmp, root, dir }
    }

    /// Write a file below the served root (creating parent directories).
    pub fn file(&self, rel: &str, content: impl AsRef<[u8]>) -> &Fixture {
        write(&self.root.join(rel), content);
        self
    }

    /// Default test configuration: `127.0.0.1:0`, live, quiet, no browser, test renderer,
    /// a (missing) bundle inside the fixture so the user's cache is never touched.
    pub fn config(&self, renderer: Arc<TestRenderer>) -> ServeConfig {
        let mut cfg = ServeConfig::new(self.root.clone(), Target::Dir(PathBuf::new()), renderer);
        cfg.host = "127.0.0.1".into();
        cfg.port = 0;
        cfg.quiet = true;
        cfg.bundle_path = self.tmp.join("styles").join("styles.tar.gz");
        cfg
    }

    /// Start a server with the default configuration adjusted by `tweak`.
    pub async fn start(self, tweak: impl FnOnce(&mut ServeConfig)) -> TestServer {
        let renderer = Arc::new(TestRenderer {
            renders: AtomicUsize::new(0),
        });
        let mut cfg = self.config(renderer.clone());
        tweak(&mut cfg);
        let server = md_render::server::start(cfg).await.expect("server starts");
        TestServer {
            addr: server.addr,
            server: Some(server),
            tmp: self.tmp,
            root: self.root,
            renderer,
            _dir: self.dir,
        }
    }
}

impl TestServer {
    /// Number of renders so far.
    pub fn renders(&self) -> usize {
        self.renderer.renders.load(Ordering::SeqCst)
    }

    /// `GET target` with the right `Host` header.
    pub async fn get(&self, target: &str) -> Response {
        request(self.addr, "GET", target, &[]).await
    }

    /// `GET target` with extra headers.
    pub async fn get_with(&self, target: &str, headers: &[(&str, &str)]) -> Response {
        request(self.addr, "GET", target, headers).await
    }

    /// `/__/md-render/render?path=<doc>` as JSON.
    pub async fn render_json(&self, doc: &str) -> (u16, serde_json::Value) {
        let r = self.get(&format!("/__/md-render/render?path={doc}")).await;
        let v = serde_json::from_slice(&r.body)
            .unwrap_or_else(|e| panic!("render JSON for {doc}: {e}: {}", r.text()));
        (r.status, v)
    }

    /// Open the event stream of `doc` (`query` is appended verbatim, e.g. `&v=3`).
    pub async fn events(&self, doc: &str, query: &str, headers: &[(&str, &str)]) -> SseStream {
        SseStream::connect(
            self.addr,
            &format!("/__/md-render/events?path={doc}{query}"),
            headers,
        )
        .await
    }

    /// Open the event stream of `doc`, already up to date with its current state, and
    /// wait for `hello`; returns the stream, the hello payload and the current version.
    pub async fn subscribe(&self, doc: &str) -> (SseStream, serde_json::Value) {
        let (_, state) = self.render_json(doc).await;
        let v = state["version"].as_u64().expect("version");
        let sid = state["server_id"].as_str().expect("server_id").to_owned();
        let mut sse = self.events(doc, &format!("&v={v}&sid={sid}"), &[]).await;
        assert_eq!(sse.status, 200, "events endpoint for {doc}");
        let hello = sse.wait_for("hello").await;
        // FSEvents starts delivering ~0.5 s after a directory is watched (CR §5); the server
        // re-checks on its own, but changes are more realistic once the watch is live.
        tokio::time::sleep(Duration::from_millis(600)).await;
        (sse, hello.json())
    }

    /// Stop the server gracefully.
    pub async fn shutdown(&mut self) {
        if let Some(s) = self.server.take() {
            s.shutdown().await;
        }
    }

    /// Absolute path of a file below the root.
    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

/// Write a file, creating parent directories.
pub fn write(path: &Path, content: impl AsRef<[u8]>) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).expect("create parent directories");
    }
    std::fs::write(path, content).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

/// Save like an editor with "atomic save": write a temporary sibling, then rename it over
/// the target (a new inode, CR §5).
pub fn write_atomic(path: &Path, content: impl AsRef<[u8]>) {
    let name = path.file_name().expect("file name").to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.tmp{}", std::process::id()));
    write(&tmp, content);
    std::fs::rename(&tmp, path).unwrap_or_else(|e| panic!("renaming over {}: {e}", path.display()));
}

/// Write a GitHub-flavor style bundle whose single stylesheet is `github/a.css` (plus a
/// font, to check content types), atomically.
pub fn write_bundle(path: &Path, css: &str) {
    use md_render::styles::{FlavorManifest, Manifest, Stylesheet};
    use sha2::Digest as _;
    let hex = |b: &[u8]| {
        sha2::Sha256::digest(b)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect::<String>()
    };
    let mut fs = tarfs::InMemoryFileSystem::new();
    let mut manifest = Manifest {
        schema: md_render::styles::manifest::SCHEMA,
        tool: md_render::TOOL.into(),
        ..Default::default()
    };
    manifest.flavors.insert(
        Flavor::Github,
        FlavorManifest {
            fetched_at: "2026-10-01T00:00:00Z".into(),
            source_page: "https://github.com/joeyespo/grip".into(),
            user_agent: md_render::TOOL.into(),
            fallback: false,
            stylesheets: vec![Stylesheet {
                path: "github/a.css".into(),
                url: "https://github.githubassets.com/assets/a.css".into(),
                role: "github".into(),
                theme: None,
                media: None,
                sha256: hex(css.as_bytes()),
                size: css.len() as u64,
            }],
            themes: vec!["light".into(), "dark".into()],
            ..Default::default()
        },
    );
    fs.write_file(
        "manifest.json",
        serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
    )
    .expect("manifest");
    fs.write_file("github/a.css", css.as_bytes().to_vec())
        .expect("css");
    fs.write_file("github/fonts/x.woff2", b"wOF2fake".to_vec())
        .expect("font");
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).expect("bundle directory");
    }
    let tmp = path.with_file_name(".bundle.tmp.tar.gz");
    fs.write_tarball_gz(&tmp).expect("write bundle");
    std::fs::rename(&tmp, path).expect("rename bundle");
}

// -------------------------------------------------------------------------------------------
// Raw HTTP/1.1 client

/// A complete HTTP response.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// First header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body as (lossy) UTF-8.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Send one request with `Connection: close` and read the whole response. `headers` may
/// override `Host`.
pub async fn request(
    addr: SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Response {
    let fut = async {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream
            .write_all(request_head(addr, method, target, headers).as_bytes())
            .await
            .expect("send request");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.expect("read response");
        raw
    };
    let raw = tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .unwrap_or_else(|_| panic!("{method} {target}: no complete response within 30 s"));
    let head_end =
        find(&raw, b"\r\n\r\n").unwrap_or_else(|| panic!("{method} {target}: malformed response"));
    let (status, headers) = parse_head(&raw[..head_end]);
    let rest = &raw[head_end + 4..];
    let chunked = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked")
    });
    let body = if chunked {
        let mut d = Dechunker::default();
        let mut out = d.feed(rest);
        assert!(
            d.done || method == "HEAD",
            "{method} {target}: truncated chunked body"
        );
        out.shrink_to_fit();
        out
    } else {
        rest.to_vec()
    };
    Response {
        status,
        headers,
        body,
    }
}

fn request_head(addr: SocketAddr, method: &str, target: &str, headers: &[(&str, &str)]) -> String {
    let mut head = format!("{method} {target} HTTP/1.1\r\n");
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        head.push_str(&format!("Host: {addr}\r\n"));
    }
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    head
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn parse_head(head: &[u8]) -> (u16, Vec<(String, String)>) {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status code");
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    (status, headers)
}

/// Incremental `Transfer-Encoding: chunked` decoder.
#[derive(Default)]
struct Dechunker {
    buf: Vec<u8>,
    done: bool,
}

impl Dechunker {
    /// Feed raw bytes; returns the decoded bytes that became available.
    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        while !self.done {
            let Some(line_end) = find(&self.buf, b"\r\n") else {
                break;
            };
            let size_text = String::from_utf8_lossy(&self.buf[..line_end]);
            let size_hex = size_text.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_hex, 16).expect("chunk size");
            if size == 0 {
                self.done = true;
                break;
            }
            let start = line_end + 2;
            if self.buf.len() < start + size + 2 {
                break;
            }
            out.extend_from_slice(&self.buf[start..start + size]);
            self.buf.drain(..start + size + 2);
        }
        out
    }
}

// -------------------------------------------------------------------------------------------
// Server-sent events

/// One parsed SSE event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub id: Option<String>,
    pub data: String,
}

impl SseEvent {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.data).unwrap_or_else(|e| {
            panic!("event {} data is not JSON ({e}): {}", self.event, self.data)
        })
    }
}

/// Something read from an event stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SseItem {
    Event(SseEvent),
    Retry(u64),
    Comment(String),
    /// The server ended the stream.
    End,
}

/// An open event stream; dropping it closes the connection.
pub struct SseStream {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    rx: mpsc::UnboundedReceiver<SseItem>,
    /// Everything received so far (for failure messages).
    pub seen: Vec<SseItem>,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for SseStream {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl SseStream {
    /// Connect and read the response head; events are then parsed in the background.
    pub async fn connect(addr: SocketAddr, target: &str, headers: &[(&str, &str)]) -> SseStream {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        let mut all: Vec<(&str, &str)> = vec![("Accept", "text/event-stream")];
        all.extend_from_slice(headers);
        stream
            .write_all(request_head(addr, "GET", target, &all).as_bytes())
            .await
            .expect("send request");
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        let head_end = loop {
            if let Some(i) = find(&raw, b"\r\n\r\n") {
                break i;
            }
            let n = tokio::time::timeout(EVENT_TIMEOUT, stream.read(&mut buf))
                .await
                .expect("response head in time")
                .expect("read response head");
            assert!(
                n > 0,
                "connection closed before the response head: {}",
                String::from_utf8_lossy(&raw)
            );
            raw.extend_from_slice(&buf[..n]);
        };
        let (status, headers) = parse_head(&raw[..head_end]);
        let rest = raw[head_end + 4..].to_vec();
        let (tx, rx) = mpsc::unbounded_channel();
        let chunked = headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
        let reader = tokio::spawn(read_events(stream, rest, chunked, tx));
        SseStream {
            status,
            headers,
            rx,
            seen: Vec::new(),
            reader,
        }
    }

    /// Next item, or None on timeout.
    pub async fn next_item(&mut self, timeout: Duration) -> Option<SseItem> {
        match tokio::time::timeout(timeout, self.rx.recv()).await {
            Ok(Some(item)) => {
                self.seen.push(item.clone());
                Some(item)
            }
            Ok(None) => Some(SseItem::End),
            Err(_) => None,
        }
    }

    /// Next event (skipping comments and `retry:`), None on timeout or end of stream.
    pub async fn next_event(&mut self, timeout: Duration) -> Option<SseEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next_item(left).await? {
                SseItem::Event(e) => return Some(e),
                SseItem::End => return None,
                _ => {}
            }
        }
    }

    /// Wait for the next event named `name` (others are skipped); panics with the stream
    /// history on timeout or end of stream.
    pub async fn wait_for(&mut self, name: &str) -> SseEvent {
        self.wait_until(name, |_| true).await
    }

    /// Wait for an event named `name` whose JSON satisfies `pred`.
    pub async fn wait_until(
        &mut self,
        name: &str,
        pred: impl Fn(&serde_json::Value) -> bool,
    ) -> SseEvent {
        let deadline = Instant::now() + EVENT_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next_event(left).await {
                Some(e) if e.event == name && pred(&e.json()) => return e,
                Some(_) => {}
                None => panic!(
                    "no matching `{name}` event within {EVENT_TIMEOUT:?}; stream so far: {:#?}",
                    self.seen
                ),
            }
        }
    }

    /// Assert that no event named `name` arrives within `quiet`.
    pub async fn expect_none(&mut self, name: &str, quiet: Duration) {
        let deadline = Instant::now() + quiet;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            match self.next_event(left).await {
                Some(e) if e.event == name => {
                    panic!("unexpected `{name}` event: {e:?}; stream: {:#?}", self.seen)
                }
                Some(_) => {}
                None => return,
            }
        }
    }

    /// Wait until the server ends the stream (skipping events).
    pub async fn wait_end(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next_item(left).await {
                Some(SseItem::End) => return,
                Some(_) => {}
                None => panic!(
                    "the event stream did not end within {timeout:?}; stream: {:#?}",
                    self.seen
                ),
            }
        }
    }
}

/// Background task: de-chunk and parse the event stream into `tx`.
async fn read_events(
    mut stream: TcpStream,
    initial: Vec<u8>,
    chunked: bool,
    tx: mpsc::UnboundedSender<SseItem>,
) {
    let mut dechunk = Dechunker::default();
    let mut text = String::new();
    let mut pending = initial;
    let mut buf = [0u8; 8192];
    loop {
        let decoded = if chunked {
            dechunk.feed(&pending)
        } else {
            std::mem::take(&mut pending)
        };
        pending.clear();
        text.push_str(&String::from_utf8_lossy(&decoded));
        // Events are separated by a blank line (axum uses `\n`).
        while let Some(end) = text.find("\n\n") {
            let block: String = text.drain(..end + 2).collect();
            if let Some(item) = parse_block(&block)
                && tx.send(item).is_err()
            {
                return;
            }
        }
        if dechunk.done {
            break;
        }
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => pending.extend_from_slice(&buf[..n]),
        }
    }
    let _ = tx.send(SseItem::End);
}

fn parse_block(block: &str) -> Option<SseItem> {
    let mut ev = SseEvent::default();
    let mut data_lines: Vec<&str> = Vec::new();
    let mut any = false;
    for line in block.lines() {
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix(':') {
            if !any {
                return Some(SseItem::Comment(comment.trim().to_owned()));
            }
            continue;
        }
        let (field, value) = line
            .split_once(':')
            .map(|(f, v)| (f, v.strip_prefix(' ').unwrap_or(v)))
            .unwrap_or((line, ""));
        any = true;
        match field {
            "event" => ev.event = value.to_owned(),
            "id" => ev.id = Some(value.to_owned()),
            "data" => data_lines.push(value),
            // A `retry:`-only block (what the server sends first).
            "retry" if ev.event.is_empty() && data_lines.is_empty() => {
                return value.parse().ok().map(SseItem::Retry);
            }
            _ => {}
        }
    }
    if !any {
        return None;
    }
    ev.data = data_lines.join("\n");
    if ev.event.is_empty() {
        ev.event = "message".into();
    }
    Some(SseItem::Event(ev))
}
