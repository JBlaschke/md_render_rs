//! Per-document live state (DESIGN §7): versions, content hashes, the latest render, and
//! fan-out to subscribers.
//!
//! Each document that was requested gets one [`DocEntry`] with a worker task. Anything that
//! might have changed the file (a page load, a new subscriber, a watcher event) calls
//! [`DocEntry::trigger`]; the worker coalesces triggers, re-reads the file, and renders only
//! when the SHA-256 of the bytes changed (so `touch` and duplicate events are free). The
//! render runs in `spawn_blocking`; if another change arrived meanwhile and the file content
//! differs again, the result is dropped as stale. The published state lives in a
//! `tokio::sync::watch` channel (page loads wait on it), events go out through a
//! `tokio::sync::broadcast` channel shared by every subscriber, so all tabs showing the same
//! document share one render.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest as _, Sha256};
use tokio::sync::{Notify, broadcast, watch};

use super::deps;
use super::events::{ErrorKind, WireEvent};
use super::paths;
use super::watcher::FsWatcher;
use crate::render::{RenderOptions, Rendered, Renderer};

/// How long a page load waits for the worker to render the current file.
pub const RENDER_WAIT: Duration = Duration::from_secs(60);

/// FSEvents needs about 500 ms after `watch()` before it delivers events (CR §5); a change
/// made in that window would be missed, so new subscribers trigger one extra re-check.
const RECHECK_AFTER_WATCH: Duration = Duration::from_millis(750);

/// After this many consecutive stale drops a render is published anyway (progress
/// guarantee under continuous writes).
const MAX_STALE_DROPS: u32 = 3;

/// Capacity of each document's event channel (subscribers that lag get a state resync).
const EVENT_CAPACITY: usize = 64;

/// When the registry holds this many documents, idle ones (no subscriber, nobody waiting for
/// a render) are evicted before a new one is added, so requests for many distinct paths
/// (e.g. a crawler probing `/x1.md`, `/x2.md`, ...) cannot grow it without bound.
const MAX_DOCS: usize = 256;

/// Shared rendering context of all documents.
pub struct RenderCtx {
    pub renderer: Arc<dyn Renderer>,
    /// Template for per-document options (`source_path` is filled in per document).
    pub options: RenderOptions,
    /// Canonical served root.
    pub root: PathBuf,
    pub follow_symlinks: bool,
    pub quiet: bool,
    /// The file watcher (None when live updates are off).
    pub watcher: Option<Arc<FsWatcher>>,
}

impl RenderCtx {
    fn options_for(&self, path: &Path) -> RenderOptions {
        RenderOptions {
            source_path: Some(path.to_path_buf()),
            ..self.options.clone()
        }
    }

    fn render_error_kind(&self) -> ErrorKind {
        if self.renderer.name() == "offline" {
            ErrorKind::Render
        } else {
            ErrorKind::Api
        }
    }

    fn log(&self, msg: &str) {
        if !self.quiet {
            eprintln!(" * {msg}");
        }
    }
}

/// A successful render of one version of a document.
#[derive(Debug)]
pub struct Content {
    pub version: u64,
    /// SHA-256 of the source bytes.
    pub hash: [u8; 32],
    pub rendered: Arc<Rendered>,
    /// The serialized `content` event for this version.
    pub event: Arc<WireEvent>,
}

impl Content {
    fn new(version: u64, hash: [u8; 32], rendered: Arc<Rendered>) -> Content {
        let event = Arc::new(WireEvent::content(version, &rendered));
        Content {
            version,
            hash,
            rendered,
            event,
        }
    }
}

/// What the document currently is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DocStatus {
    /// Not looked at yet.
    #[default]
    Pending,
    /// `content` is the current file.
    Ready,
    /// The file does not exist (`content`, if any, is the last good render).
    Missing,
    /// Reading or rendering failed (`content`, if any, is the last good render).
    Failed { kind: ErrorKind, message: String },
}

/// Published state of a document.
#[derive(Clone, Debug, Default)]
pub struct DocSnapshot {
    /// Version of the current state (content, deletion or error).
    pub version: u64,
    /// Highest trigger number fully processed (page loads wait for their trigger).
    pub processed: u64,
    /// The last successful render.
    pub content: Option<Arc<Content>>,
    pub status: DocStatus,
}

impl DocSnapshot {
    /// Events that bring a client at `since` up to date: the current content if newer, then
    /// the current deletion/error state if newer.
    pub fn catch_up(&self, key: &str, since: u64) -> Vec<Arc<WireEvent>> {
        let mut out = Vec::new();
        if let Some(c) = &self.content
            && c.version > since
        {
            out.push(c.event.clone());
        }
        if self.version > since {
            match &self.status {
                DocStatus::Missing => out.push(Arc::new(WireEvent::deleted(self.version, key))),
                DocStatus::Failed { kind, message } => {
                    out.push(Arc::new(WireEvent::error(self.version, *kind, message)))
                }
                DocStatus::Pending | DocStatus::Ready => {}
            }
        }
        out
    }
}

/// A local file a document depends on, with its watch target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dep {
    /// URL path the page uses for it (sent in `assets` events).
    pub url: String,
    /// Canonical directory to watch.
    pub dir: PathBuf,
    /// Canonical path events will carry.
    pub file: PathBuf,
}

#[derive(Default)]
struct Tracking {
    subscribers: usize,
    /// Watched directory of the document itself (holds one watcher reference).
    doc_dir: Option<PathBuf>,
    doc_file: Option<PathBuf>,
    /// Latest dependencies (always tracked; watched only while subscribed).
    deps: Vec<Dep>,
    /// Directories this document holds a watcher reference on for its dependencies.
    dep_dirs: BTreeSet<PathBuf>,
}

/// Live state of one document.
pub struct DocEntry {
    /// Root-relative path (`docs/README.md`).
    pub key: String,
    /// Absolute path (served root joined with `key`).
    pub path: PathBuf,
    triggers: AtomicU64,
    versions: AtomicU64,
    wake: Notify,
    /// Set when the entry was evicted from the registry; the worker exits.
    retired: AtomicBool,
    state: watch::Sender<DocSnapshot>,
    events: broadcast::Sender<Arc<WireEvent>>,
    tracking: Mutex<Tracking>,
}

impl std::fmt::Debug for DocEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocEntry")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl DocEntry {
    fn new(key: &str, path: PathBuf) -> DocEntry {
        DocEntry {
            key: key.to_owned(),
            path,
            triggers: AtomicU64::new(0),
            versions: AtomicU64::new(0),
            wake: Notify::new(),
            retired: AtomicBool::new(false),
            state: watch::Sender::new(DocSnapshot::default()),
            events: broadcast::Sender::new(EVENT_CAPACITY),
            tracking: Mutex::new(Tracking::default()),
        }
    }

    /// Ask the worker to re-check the file; returns the trigger number to wait for.
    pub fn trigger(&self) -> u64 {
        let t = self.triggers.fetch_add(1, Ordering::SeqCst) + 1;
        self.wake.notify_one();
        t
    }

    /// Current published state.
    pub fn snapshot(&self) -> DocSnapshot {
        self.state.borrow().clone()
    }

    /// Receive this document's events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<Arc<WireEvent>> {
        self.events.subscribe()
    }

    /// Number of live subscribers.
    pub fn subscribers(&self) -> usize {
        self.lock().subscribers
    }

    /// Stop the worker (the entry was evicted).
    fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tracking> {
        self.tracking.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn next_version(&self) -> u64 {
        self.versions.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn publish(&self, processed: u64, f: impl FnOnce(&mut DocSnapshot)) {
        self.state.send_modify(|s| {
            f(s);
            s.processed = s.processed.max(processed);
        });
    }

    fn broadcast(&self, ev: Arc<WireEvent>) {
        // An error only means nobody is subscribed right now.
        let _ = self.events.send(ev);
    }

    fn add_subscriber(&self, watcher: Option<&FsWatcher>) {
        let mut t = self.lock();
        t.subscribers += 1;
        if t.subscribers > 1 {
            return;
        }
        let Some(w) = watcher else { return };
        if let Some((dir, file)) = paths::watch_target(&self.path) {
            match w.add(&dir) {
                Ok(()) => {
                    t.doc_dir = Some(dir);
                    t.doc_file = Some(file);
                }
                Err(e) => tracing::warn!("cannot watch {}: {e}", dir.display()),
            }
        }
        let wanted: BTreeSet<PathBuf> = t.deps.iter().map(|d| d.dir.clone()).collect();
        for dir in wanted {
            if w.add(&dir).is_ok() {
                t.dep_dirs.insert(dir);
            }
        }
    }

    fn remove_subscriber(&self, watcher: Option<&FsWatcher>) {
        let mut t = self.lock();
        t.subscribers = t.subscribers.saturating_sub(1);
        if t.subscribers > 0 {
            return;
        }
        t.doc_file = None;
        let doc_dir = t.doc_dir.take();
        let dep_dirs = std::mem::take(&mut t.dep_dirs);
        if let Some(w) = watcher {
            for dir in doc_dir.iter().chain(dep_dirs.iter()) {
                w.remove(dir);
            }
        }
    }

    /// Replace the dependency list, updating dependency watches while subscribed.
    fn set_deps(&self, deps: Vec<Dep>, watcher: Option<&FsWatcher>) {
        let mut t = self.lock();
        t.deps = deps;
        if t.subscribers == 0 {
            return;
        }
        let Some(w) = watcher else { return };
        let wanted: BTreeSet<PathBuf> = t.deps.iter().map(|d| d.dir.clone()).collect();
        let held = std::mem::take(&mut t.dep_dirs);
        let mut now = BTreeSet::new();
        for dir in &wanted {
            if held.contains(dir) || w.add(dir).is_ok() {
                now.insert(dir.clone());
            }
        }
        for dir in held.difference(&wanted) {
            w.remove(dir);
        }
        t.dep_dirs = now;
    }

    /// Match changed paths: (re-check the document?, changed dependency URLs).
    fn match_paths(&self, changed: &[PathBuf]) -> (bool, Vec<String>) {
        let t = self.lock();
        if t.subscribers == 0 {
            return (false, Vec::new());
        }
        // A directory-level event (rescan) re-checks the document; the hash makes it cheap.
        let doc = changed
            .iter()
            .any(|p| t.doc_file.as_ref() == Some(p) || t.doc_dir.as_ref() == Some(p));
        let mut urls: Vec<String> = t
            .deps
            .iter()
            .filter(|d| changed.contains(&d.file))
            .map(|d| d.url.clone())
            .collect();
        urls.sort();
        urls.dedup();
        (doc, urls)
    }

    /// Dependencies currently tracked.
    #[cfg(test)]
    pub fn deps(&self) -> Vec<Dep> {
        self.lock().deps.clone()
    }
}

/// Keeps a document subscribed (and its directories watched) while alive.
pub struct Subscription {
    pub entry: Arc<DocEntry>,
    watcher: Option<Arc<FsWatcher>>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.entry.remove_subscriber(self.watcher.as_deref());
    }
}

/// Registry of all documents served so far.
pub struct Hub {
    ctx: Arc<RenderCtx>,
    docs: Mutex<HashMap<String, Arc<DocEntry>>>,
    shutdown: watch::Receiver<bool>,
}

impl Hub {
    pub fn new(ctx: Arc<RenderCtx>, shutdown: watch::Receiver<bool>) -> Hub {
        Hub {
            ctx,
            docs: Mutex::new(HashMap::new()),
            shutdown,
        }
    }

    /// The entry for root-relative `key`, created (with its worker) on first use.
    pub fn entry(&self, key: &str) -> Arc<DocEntry> {
        let mut docs = self.docs.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(e) = docs.get(key) {
            return e.clone();
        }
        if docs.len() >= MAX_DOCS {
            evict_idle(&mut docs);
        }
        let entry = Arc::new(DocEntry::new(key, paths::join_rel(&self.ctx.root, key)));
        tokio::spawn(worker(
            entry.clone(),
            self.ctx.clone(),
            self.shutdown.clone(),
        ));
        docs.insert(key.to_owned(), entry.clone());
        entry
    }

    /// The up-to-date state of `key` (re-reads the file; renders if it changed). None if the
    /// worker did not finish within [`RENDER_WAIT`] or the server is shutting down.
    pub async fn current(&self, key: &str) -> Option<DocSnapshot> {
        let entry = self.entry(key);
        let mut rx = entry.state.subscribe();
        let t = entry.trigger();
        match tokio::time::timeout(RENDER_WAIT, rx.wait_for(|s| s.processed >= t)).await {
            Ok(Ok(s)) => Some(s.clone()),
            _ => None,
        }
    }

    /// Subscribe to live updates of `key`.
    pub fn subscribe(&self, key: &str) -> Subscription {
        let entry = self.entry(key);
        entry.add_subscriber(self.ctx.watcher.as_deref());
        entry.trigger();
        if self.ctx.watcher.as_ref().is_some_and(|w| !w.is_polling()) {
            let e = entry.clone();
            tokio::spawn(async move {
                tokio::time::sleep(RECHECK_AFTER_WATCH).await;
                e.trigger();
            });
        }
        Subscription {
            entry,
            watcher: self.ctx.watcher.clone(),
        }
    }

    /// Dispatch a batch of changed (canonical) paths from the watcher.
    pub fn on_paths_changed(&self, changed: &[PathBuf]) {
        let entries: Vec<Arc<DocEntry>> = self
            .docs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        let token = cache_token();
        for e in entries {
            let (doc, urls) = e.match_paths(changed);
            if doc {
                e.trigger();
            }
            if !urls.is_empty() {
                for u in &urls {
                    self.ctx.log(&format!(
                        "Change detected in {}, refreshing images",
                        u.trim_start_matches('/')
                    ));
                }
                e.broadcast(Arc::new(WireEvent::assets(&urls, &token)));
            }
        }
    }

    /// Entry for `key` if it exists.
    #[cfg(test)]
    pub fn get(&self, key: &str) -> Option<Arc<DocEntry>> {
        self.docs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .cloned()
    }

    /// Number of documents in the registry.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.docs.lock().unwrap_or_else(|p| p.into_inner()).len()
    }
}

/// Drop idle entries from the registry (called with the registry lock held).
///
/// The registry and the entry's worker hold one reference each; every other holder (a
/// [`Subscription`], a page load waiting in [`Hub::current`], a re-check timer, a watcher
/// dispatch in progress) owns another, and new references are only handed out under the
/// registry lock, so `strong_count <= 2` means nobody can be using the entry.
fn evict_idle(docs: &mut HashMap<String, Arc<DocEntry>>) {
    docs.retain(|_, e| {
        let idle = Arc::strong_count(e) <= 2 && e.subscribers() == 0;
        if idle {
            e.retire();
        }
        !idle
    });
}

/// Cache-busting token for `assets` events (milliseconds since the Unix epoch).
fn cache_token() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".into())
}

/// SHA-256 of a byte string.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(bytes));
    out
}

/// Decode Markdown source: strip a UTF-8 BOM and decode lossily, returning a warning when
/// the bytes were not valid UTF-8 (GRIP C38).
pub fn decode_markdown(mut bytes: Vec<u8>, name: &str) -> (String, Option<String>) {
    let bom = bytes.starts_with(b"\xEF\xBB\xBF");
    if bom {
        bytes.drain(..3);
    }
    match String::from_utf8(bytes) {
        Ok(s) => (s, None),
        Err(e) => {
            let offset = e.utf8_error().valid_up_to() + if bom { 3 } else { 0 };
            let text = String::from_utf8_lossy(e.as_bytes()).into_owned();
            let warning = format!(
                "{name} is not valid UTF-8 (first invalid byte at offset {offset}); showing a lossy decoding"
            );
            (text, Some(warning))
        }
    }
}

enum Outcome {
    Done,
    Stale,
}

async fn worker(entry: Arc<DocEntry>, ctx: Arc<RenderCtx>, mut shutdown: watch::Receiver<bool>) {
    let mut stale_drops = 0u32;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.wait_for(|stop| *stop) => break,
            _ = entry.wake.notified() => {}
        }
        if entry.retired.load(Ordering::SeqCst) {
            break;
        }
        match process(&entry, &ctx, stale_drops >= MAX_STALE_DROPS).await {
            Outcome::Done => stale_drops = 0,
            Outcome::Stale => stale_drops += 1,
        }
    }
}

/// One worker iteration: re-read, dedup by hash, render, publish.
async fn process(entry: &DocEntry, ctx: &RenderCtx, force: bool) -> Outcome {
    let t0 = entry.triggers.load(Ordering::SeqCst);
    let snap = entry.snapshot();
    let bytes = match read_contained(ctx, &entry.path).await {
        Ok(b) => b,
        Err(e) if paths::is_not_found(&e) => {
            if snap.status == DocStatus::Missing {
                entry.publish(t0, |_| {});
            } else {
                if snap.status != DocStatus::Pending {
                    ctx.log(&format!(
                        "{} was deleted; waiting for it to reappear",
                        entry.key
                    ));
                }
                let v = entry.next_version();
                entry.publish(t0, |s| {
                    s.version = v;
                    s.status = DocStatus::Missing;
                });
                entry.broadcast(Arc::new(WireEvent::deleted(v, &entry.key)));
            }
            return Outcome::Done;
        }
        Err(e) => {
            let message = format!("cannot read {}: {e}", entry.key);
            let same = matches!(&snap.status, DocStatus::Failed { kind: ErrorKind::Read, message: m } if *m == message);
            if same {
                entry.publish(t0, |_| {});
            } else {
                ctx.log(&message);
                let v = entry.next_version();
                let msg = message.clone();
                entry.publish(t0, |s| {
                    s.version = v;
                    s.status = DocStatus::Failed {
                        kind: ErrorKind::Read,
                        message: msg,
                    };
                });
                entry.broadcast(Arc::new(WireEvent::error(v, ErrorKind::Read, &message)));
            }
            return Outcome::Done;
        }
    };

    let hash = sha256(&bytes);
    if let Some(c) = &snap.content
        && c.hash == hash
    {
        if snap.status == DocStatus::Ready {
            entry.publish(t0, |_| {}); // no-op save, `touch`, duplicate event
            return Outcome::Done;
        }
        // Same bytes as the last good render (recreated file, reverted error): reuse it.
        ctx.log(&format!("Change detected in {}, refreshing", entry.key));
        let v = entry.next_version();
        let content = Arc::new(Content::new(v, hash, c.rendered.clone()));
        let ev = content.event.clone();
        entry.publish(t0, |s| {
            s.version = v;
            s.content = Some(content);
            s.status = DocStatus::Ready;
        });
        entry.broadcast(ev);
        return Outcome::Done;
    }

    if snap.status != DocStatus::Pending {
        ctx.log(&format!("Change detected in {}, refreshing", entry.key));
    }
    let v = entry.next_version();
    entry.broadcast(Arc::new(WireEvent::rendering(v)));

    let (text, decode_warning) = decode_markdown(bytes, &entry.key);
    let renderer = ctx.renderer.clone();
    let opts = ctx.options_for(&entry.path);
    let key = entry.key.clone();
    let root = ctx.root.clone();
    let follow = ctx.follow_symlinks;
    let joined = tokio::task::spawn_blocking(move || {
        let result = renderer.render(&text, &opts);
        let deps = match &result {
            Ok(r) => resolve_deps(&root, follow, deps::scan(&r.html, &key)),
            Err(_) => Vec::new(),
        };
        (result, deps)
    })
    .await;

    // Drop stale results: a newer change arrived while rendering and the file differs again.
    if !force && entry.triggers.load(Ordering::SeqCst) != t0 {
        let now = read_contained(ctx, &entry.path)
            .await
            .ok()
            .map(|b| sha256(&b));
        if now != Some(hash) {
            return Outcome::Stale;
        }
    }

    match joined {
        Ok((Ok(mut rendered), deps)) => {
            if let Some(w) = decode_warning {
                rendered.warnings.insert(0, w);
            }
            let content = Arc::new(Content::new(v, hash, Arc::new(rendered)));
            let ev = content.event.clone();
            entry.publish(t0, |s| {
                s.version = v;
                s.content = Some(content);
                s.status = DocStatus::Ready;
            });
            entry.broadcast(ev);
            entry.set_deps(deps, ctx.watcher.as_deref());
        }
        Ok((Err(e), _)) => {
            let kind = ctx.render_error_kind();
            let message = format!("{e:#}");
            ctx.log(&format!("rendering {} failed: {message}", entry.key));
            publish_failure(entry, t0, v, kind, message);
        }
        Err(join) => {
            let message = if join.is_panic() {
                "the renderer panicked".to_owned()
            } else {
                join.to_string()
            };
            ctx.log(&format!("rendering {} failed: {message}", entry.key));
            publish_failure(entry, t0, v, ErrorKind::Render, message);
        }
    }
    Outcome::Done
}

/// Read a document, refusing files that (now) resolve outside the served root, e.g. a
/// document replaced by a symlink to `/etc/passwd` after the page was opened.
async fn read_contained(ctx: &RenderCtx, path: &Path) -> std::io::Result<Vec<u8>> {
    let canon = tokio::fs::canonicalize(path).await?;
    if let Err(e) = paths::check_contained(&ctx.root, &canon, ctx.follow_symlinks) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            e.to_string(),
        ));
    }
    tokio::fs::read(&canon).await
}

fn publish_failure(entry: &DocEntry, t0: u64, v: u64, kind: ErrorKind, message: String) {
    let ev = Arc::new(WireEvent::error(v, kind, &message));
    entry.publish(t0, |s| {
        s.version = v;
        s.status = DocStatus::Failed { kind, message };
    });
    entry.broadcast(ev);
}

/// Resolve scanned dependency references to canonical watch targets inside the root.
fn resolve_deps(root: &Path, follow_symlinks: bool, refs: Vec<deps::DepRef>) -> Vec<Dep> {
    refs.into_iter()
        .filter_map(|r| {
            let (dir, file) = paths::watch_target(&paths::join_rel(root, &r.rel))?;
            paths::check_contained(root, &dir, follow_symlinks).ok()?;
            paths::check_contained(root, &file, follow_symlinks).ok()?;
            Some(Dep {
                url: r.url,
                dir,
                file,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flavor::Flavor;
    use std::sync::atomic::AtomicUsize;

    struct Counting(AtomicUsize);
    impl Renderer for Counting {
        fn render(&self, md: &str, _: &RenderOptions) -> anyhow::Result<Rendered> {
            self.0.fetch_add(1, Ordering::SeqCst);
            if md.contains("FAIL") {
                anyhow::bail!("cannot render this");
            }
            Ok(Rendered {
                html: format!("<p>{}</p>", md.trim()),
                ..Default::default()
            })
        }
        fn name(&self) -> &'static str {
            "offline"
        }
    }

    fn hub(root: &Path, renderer: Arc<Counting>) -> (Hub, watch::Sender<bool>) {
        let (tx, rx) = watch::channel(false);
        let ctx = RenderCtx {
            renderer,
            options: RenderOptions::new(Flavor::Github),
            root: root.to_path_buf(),
            follow_symlinks: false,
            quiet: true,
            watcher: None,
        };
        (Hub::new(Arc::new(ctx), rx), tx)
    }

    #[test]
    fn decoding() {
        assert_eq!(
            decode_markdown(b"\xEF\xBB\xBF# x".to_vec(), "a.md"),
            ("# x".into(), None)
        );
        let (text, w) = decode_markdown(b"caf\xe9".to_vec(), "a.md");
        assert_eq!(text, "caf\u{FFFD}");
        assert!(w.unwrap().contains("offset 3"));
        let (_, w) = decode_markdown(b"\xEF\xBB\xBFcaf\xe9".to_vec(), "a.md");
        assert!(w.unwrap().contains("offset 6"));
    }

    #[test]
    fn sha256_known_value() {
        let h = sha256(b"abc");
        assert_eq!(h[..4], [0xba, 0x78, 0x16, 0xbf]);
    }

    #[test]
    fn catch_up_events() {
        let r = Arc::new(Rendered::default());
        let snap = DocSnapshot {
            version: 5,
            processed: 0,
            content: Some(Arc::new(Content::new(3, [0; 32], r))),
            status: DocStatus::Missing,
        };
        let names = |since| {
            snap.catch_up("a.md", since)
                .iter()
                .map(|e| e.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(0), ["content", "deleted"]);
        assert_eq!(names(3), ["deleted"]);
        assert!(names(5).is_empty());
    }

    #[tokio::test]
    async fn renders_once_per_change_and_tracks_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let counter = Arc::new(Counting(AtomicUsize::new(0)));
        let (hub, _stop) = hub(&root, counter.clone());

        let s = hub.current("a.md").await.unwrap();
        assert_eq!(s.status, DocStatus::Missing);
        assert_eq!(s.version, 1);

        std::fs::write(root.join("a.md"), "hello").unwrap();
        let s = hub.current("a.md").await.unwrap();
        assert_eq!(s.status, DocStatus::Ready);
        assert_eq!(s.version, 2);
        assert_eq!(s.content.as_ref().unwrap().rendered.html, "<p>hello</p>");
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);

        // Unchanged bytes: no render, no new version.
        let s = hub.current("a.md").await.unwrap();
        assert_eq!(s.version, 2);
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);

        // Delete, then recreate with the same bytes: the old render is reused.
        std::fs::remove_file(root.join("a.md")).unwrap();
        let s = hub.current("a.md").await.unwrap();
        assert_eq!((s.status.clone(), s.version), (DocStatus::Missing, 3));
        std::fs::write(root.join("a.md"), "hello").unwrap();
        let s = hub.current("a.md").await.unwrap();
        assert_eq!((s.status.clone(), s.version), (DocStatus::Ready, 4));
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);

        // A render failure keeps the last good content.
        std::fs::write(root.join("a.md"), "FAIL").unwrap();
        let s = hub.current("a.md").await.unwrap();
        assert!(
            matches!(&s.status, DocStatus::Failed { kind: ErrorKind::Render, message } if message.contains("cannot render"))
        );
        assert_eq!(s.content.as_ref().unwrap().rendered.html, "<p>hello</p>");
        assert_eq!(s.version, 5);
    }

    #[tokio::test]
    async fn idle_documents_are_evicted_at_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (hub, _stop) = hub(&root, Arc::new(Counting(AtomicUsize::new(0))));
        for i in 0..MAX_DOCS {
            hub.current(&format!("d{i}.md")).await.unwrap();
        }
        assert_eq!(hub.len(), MAX_DOCS);
        let sub = hub.subscribe("d0.md");
        let s = hub.current("new.md").await.unwrap();
        assert_eq!(s.status, DocStatus::Missing);
        // Everything idle went; the subscribed document and the new one stay.
        assert_eq!(hub.len(), 2);
        assert!(hub.get("d0.md").is_some());
        drop(sub);
        // An evicted document is simply re-created on its next use.
        assert_eq!(
            hub.current("d1.md").await.unwrap().status,
            DocStatus::Missing
        );
    }

    #[tokio::test]
    async fn events_reach_subscribers() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("b.md"), "one").unwrap();
        let (hub, _stop) = hub(&root, Arc::new(Counting(AtomicUsize::new(0))));
        let sub = hub.subscribe("b.md");
        let mut rx = sub.entry.subscribe_events();
        assert_eq!(sub.entry.subscribers(), 1);
        let mut names = Vec::new();
        while names.last() != Some(&"content") {
            let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            names.push(ev.name);
        }
        assert_eq!(names, ["rendering", "content"]);
        drop(sub);
        assert_eq!(hub.get("b.md").unwrap().subscribers(), 0);
    }

    #[tokio::test]
    async fn dependencies_are_tracked() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("img")).unwrap();
        std::fs::write(root.join("img/a.png"), "png").unwrap();
        struct Img;
        impl Renderer for Img {
            fn render(&self, _: &str, _: &RenderOptions) -> anyhow::Result<Rendered> {
                Ok(Rendered {
                    html: r#"<img src="img/a.png"><img src="https://x/y.png">"#.into(),
                    ..Default::default()
                })
            }
            fn name(&self) -> &'static str {
                "test"
            }
        }
        let (tx, rx) = watch::channel(false);
        let ctx = RenderCtx {
            renderer: Arc::new(Img),
            options: RenderOptions::new(Flavor::Github),
            root: root.clone(),
            follow_symlinks: false,
            quiet: true,
            watcher: None,
        };
        let hub = Hub::new(Arc::new(ctx), rx);
        std::fs::write(root.join("d.md"), "x").unwrap();
        hub.current("d.md").await.unwrap();
        let deps = hub.get("d.md").unwrap().deps();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].url, "/img/a.png");
        assert_eq!(deps[0].file, root.join("img/a.png"));
        // Workers stop on shutdown.
        tx.send(true).unwrap();
    }
}
