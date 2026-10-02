//! Browser-driven tests of the live client (`assets/client/live.js`, DESIGN §7) in headless
//! Chrome: connection handling across tabs, image cache-busting, banners and the error
//! title, and scroll anchoring.
//!
//! Chrome is driven over the DevTools protocol by `tests/server_browser/cdp.mjs` (Node, no
//! packages). The tests need `node` on `PATH` and Chrome or Chromium: set
//! `MD_RENDER_TEST_CHROME` to the executable, otherwise the usual install locations are
//! tried. Without them each test prints a loud `SKIPPED` line and passes. The pages only
//! talk to the loopback test server (Chrome runs with background networking disabled).

mod server_support;

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use server_support::{Fixture, TestServer, write_atomic};

/// Upper bound for anything a page has to do after a change on disk.
const PAGE_TIMEOUT: Duration = Duration::from_secs(15);

// -------------------------------------------------------------------------------------------
// Browser bridge

fn chrome_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MD_RENDER_TEST_CHROME") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let fixed = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
    ];
    fixed.iter().map(PathBuf::from).find(|p| p.is_file())
}

fn node_works() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// One headless Chrome, driven through the `cdp.mjs` bridge.
struct Browser {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Browser {
    /// Start Chrome, or print why the test is skipped and return None.
    fn launch(test: &str) -> Option<Browser> {
        let Some(chrome) = chrome_path() else {
            eprintln!(
                "SKIPPED {test}: no Chrome/Chromium found (set MD_RENDER_TEST_CHROME to the executable)"
            );
            return None;
        };
        if !node_works() {
            eprintln!("SKIPPED {test}: `node` is not on PATH");
            return None;
        }
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/server_browser/cdp.mjs");
        let mut child = Command::new("node")
            .arg(script)
            .arg(chrome)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start the Chrome bridge");
        let stdin = child.stdin.take().expect("bridge stdin");
        let stdout = child.stdout.take().expect("bridge stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut b = Browser {
            child,
            stdin,
            lines,
        };
        let ready = b.reply(Duration::from_secs(30));
        assert_eq!(ready, Ok(json!("ready")), "the Chrome bridge did not start");
        Some(b)
    }

    fn reply(&mut self, timeout: Duration) -> Result<Value, String> {
        let line = self
            .lines
            .recv_timeout(timeout)
            .map_err(|e| format!("no answer from the Chrome bridge: {e}"))?;
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("{e}: {line}"))?;
        match v.get("error") {
            Some(e) => Err(e.as_str().unwrap_or("error").to_owned()),
            None => Ok(v.get("ok").cloned().unwrap_or(Value::Null)),
        }
    }

    fn cmd(&mut self, cmd: Value) -> Result<Value, String> {
        // Blocking pipe I/O: keep the runtime's other workers serving.
        tokio::task::block_in_place(|| {
            writeln!(self.stdin, "{cmd}").map_err(|e| e.to_string())?;
            self.stdin.flush().map_err(|e| e.to_string())?;
            self.reply(Duration::from_secs(60))
        })
    }

    /// Open `url` in a new tab (a background tab is hidden) and return its id.
    fn open(&mut self, url: &str, background: bool) -> String {
        let id = self
            .cmd(json!({"cmd": "open", "url": url, "background": background}))
            .unwrap_or_else(|e| panic!("opening {url}: {e}"));
        id.as_str().expect("tab id").to_owned()
    }

    /// Bring a tab to the front (the others become hidden).
    fn activate(&mut self, tab: &str) {
        self.cmd(json!({"cmd": "activate", "tab": tab}))
            .expect("activate a tab");
    }

    /// Evaluate `js` in the tab (promises are awaited).
    fn eval(&mut self, tab: &str, js: &str) -> Result<Value, String> {
        self.cmd(json!({"cmd": "eval", "tab": tab, "js": js}))
    }

    /// Poll `js` until it returns something truthy; None on timeout.
    fn wait(&mut self, tab: &str, js: &str, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(v) = self.eval(tab, js)
                && truthy(&v)
            {
                return Some(v);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Like [`Browser::wait`], panicking with `what` on timeout.
    fn expect(&mut self, tab: &str, js: &str, what: &str) -> Value {
        if let Some(v) = self.wait(tab, js, PAGE_TIMEOUT) {
            return v;
        }
        let page = self.eval(
            tab,
            "JSON.stringify({url: location.href, ready: document.readyState, title: document.title, \
             status: document.getElementById('mdr-live-status')?.dataset.state || null, \
             content: (document.getElementById('md-content')?.textContent || '').slice(0, 300)})",
        );
        panic!("timed out waiting until {what} ({js}); page: {page:?}")
    }

    /// Wait for the live client to be connected (its status dot says so).
    fn expect_connected(&mut self, tab: &str) {
        self.expect(
            tab,
            "document.readyState === 'complete' && \
             document.getElementById('mdr-live-status')?.dataset.state === 'connected'",
            "the live client is connected",
        );
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "{}", json!({"cmd": "quit"}));
        let _ = self.stdin.flush();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn url(s: &TestServer, path: &str) -> String {
    format!("http://{}{path}", s.addr)
}

// -------------------------------------------------------------------------------------------
// Tests

/// LIVE-3: browsers allow about six HTTP/1.1 connections per server. Hidden tabs must not
/// keep their event streams, or a seventh tab (and every request of the visible ones) waits
/// forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hidden_tabs_give_back_their_connections() {
    let Some(mut b) = Browser::launch("hidden_tabs_give_back_their_connections") else {
        return;
    };
    let f = Fixture::new();
    f.file("README.md", "readme");
    f.file("other.md", "other");
    let s = f.start(|_| {}).await;

    // Tabs opened in the background: they are hidden from the start.
    let first = b.open(&url(&s, "/"), false);
    b.expect_connected(&first);
    for i in 0..6 {
        b.open(&url(&s, &format!("/other.md?bg={i}")), true);
    }
    let seventh = b.open(&url(&s, "/other.md?bg=last"), true);
    b.expect(
        &seventh,
        "document.readyState === 'complete' && !!document.getElementById('md-content')",
        "a seventh tab loads",
    );
    let fetched = b
        .eval(
            &first,
            "(async () => { const c = new AbortController(); setTimeout(() => c.abort(), 5000); \
             try { return (await fetch('/__/md-render/render?path=README.md', \
             {signal: c.signal, cache: 'no-store'})).status; } catch (e) { return e.name; } })()",
        )
        .expect("fetch from the visible tab");
    assert_eq!(fetched, json!(200), "a request of the visible tab stalled");

    // Tabs that were each visible for a moment: after the grace period only the visible
    // one keeps a stream, so a seventh one loads (at most after that grace period).
    let mut tabs = Vec::new();
    for i in 0..7 {
        tabs.push(b.open(&url(&s, &format!("/other.md?fg={i}")), false));
        std::thread::sleep(Duration::from_millis(300));
    }
    let last = tabs.last().unwrap().clone();
    b.expect_connected(&last);

    // A suspended tab catches up when it is shown again.
    std::thread::sleep(Duration::from_secs(6));
    write_atomic(&s.path("other.md"), "other CHANGED");
    std::thread::sleep(Duration::from_millis(500));
    b.activate(&tabs[0]);
    b.expect(
        &tabs[0],
        "document.getElementById('md-content').textContent.includes('CHANGED')",
        "the tab shown again catches up",
    );
    // ... and receives live updates again.
    b.expect_connected(&tabs[0]);
    write_atomic(&s.path("other.md"), "other AGAIN");
    b.expect(
        &tabs[0],
        "document.getElementById('md-content').textContent.includes('AGAIN')",
        "the tab shown again gets live updates",
    );
}

fn svg(width: u32) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"50\">\
         <rect width=\"{width}\" height=\"50\" fill=\"teal\"/></svg>"
    )
}

/// The natural width of the first image in the content once it has loaded (0 before).
const IMG_WIDTH: &str = "(() => { const i = document.querySelector('#md-content img'); \
     return i && i.complete ? i.naturalWidth : 0; })()";

/// LIVE-6: an image refreshed by an `assets` event stays new when the block around it is
/// edited later (the re-inserted `<img>` must not fall back to the stale cached URL).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refreshed_images_stay_fresh_when_their_block_changes() {
    let Some(mut b) = Browser::launch("refreshed_images_stay_fresh_when_their_block_changes")
    else {
        return;
    };
    let f = Fixture::new();
    f.file("doc.md", "# Doc\n\n![img](pic.svg)\n\nafter\n");
    f.file("pic.svg", svg(100));
    let s = f
        .start(|cfg| cfg.renderer = Arc::new(md_render::render::OfflineRenderer))
        .await;
    let tab = b.open(&url(&s, "/doc.md"), false);
    b.expect_connected(&tab);
    assert_eq!(b.expect(&tab, IMG_WIDTH, "the image loads"), json!(100));

    write_atomic(&s.path("pic.svg"), svg(200));
    b.expect(
        &tab,
        &format!("{IMG_WIDTH} === 200"),
        "the changed image is shown",
    );

    write_atomic(
        &s.path("doc.md"),
        "# Doc\n\n![img changed alt](pic.svg)\n\nafter\n",
    );
    b.expect(
        &tab,
        "document.querySelector('#md-content img')?.alt === 'img changed alt'",
        "the edited block is applied",
    );
    std::thread::sleep(Duration::from_millis(500));
    let width = b.expect(&tab, IMG_WIDTH, "the re-inserted image loads");
    assert_eq!(
        width,
        json!(200),
        "the re-inserted image shows the old file"
    );
}

/// Banners in the page: those the server rendered and those live.js added.
const BANNERS: &str = "({server: document.querySelectorAll('#mdr-banners > .mdr-banner').length, \
     live: document.querySelectorAll('.mdr-live-banner').length, title: document.title})";

/// LIVE-7: banners the server rendered into the page go away once fresh content is applied,
/// and a page loaded while its document has an error shows the `(Error) ` title until then.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn banners_and_error_titles_from_page_load_are_cleared_by_fresh_content() {
    let Some(mut b) =
        Browser::launch("banners_and_error_titles_from_page_load_are_cleared_by_fresh_content")
    else {
        return;
    };
    let f = Fixture::new();
    f.file("doc.md", "good");
    f.file("latin1.md", b"caf\xe9");
    let s = f.start(|_| {}).await;
    assert_eq!(s.get("/doc.md").await.status, 200);
    write_atomic(&s.path("doc.md"), "FAIL");

    // Loaded with a render error: error banner and title prefix.
    let tab = b.open(&url(&s, "/doc.md"), false);
    b.expect_connected(&tab);
    let state = b.eval(&tab, BANNERS).unwrap();
    assert_eq!(
        state,
        json!({"server": 1, "live": 0, "title": "(Error) doc.md"})
    );
    // The re-check that follows the subscription does not announce the same error again
    // (which would add a second, live banner).
    std::thread::sleep(Duration::from_millis(1500));
    let state = b.eval(&tab, BANNERS).unwrap();
    assert_eq!(
        state,
        json!({"server": 1, "live": 0, "title": "(Error) doc.md"})
    );
    write_atomic(&s.path("doc.md"), "fixed");
    b.expect(
        &tab,
        "document.getElementById('md-content').textContent.includes('fixed')",
        "the fixed document is shown",
    );
    let state = b.eval(&tab, BANNERS).unwrap();
    assert_eq!(state, json!({"server": 0, "live": 0, "title": "doc.md"}));

    // Loaded with a warning (invalid UTF-8): replaced by the warnings of fresh content, never
    // shown twice.
    let tab = b.open(&url(&s, "/latin1.md"), false);
    b.expect_connected(&tab);
    let state = b.eval(&tab, BANNERS).unwrap();
    assert_eq!(state, json!({"server": 1, "live": 0, "title": "latin1.md"}));
    write_atomic(&s.path("latin1.md"), b"caf\xe9 still latin-1");
    b.expect(
        &tab,
        "document.getElementById('md-content').textContent.includes('still')",
        "the edit is shown",
    );
    let state = b.eval(&tab, BANNERS).unwrap();
    assert_eq!(state, json!({"server": 0, "live": 1, "title": "latin1.md"}));
    write_atomic(&s.path("latin1.md"), "café fixed");
    b.expect(
        &tab,
        "document.getElementById('md-content').textContent.includes('fixed')",
        "the fixed document is shown",
    );
    let state = b.eval(&tab, BANNERS).unwrap();
    assert_eq!(state, json!({"server": 0, "live": 0, "title": "latin1.md"}));
}
