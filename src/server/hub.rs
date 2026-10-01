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
use crate::flavor::{Flavor, Mode};
use crate::render::{RenderOptions, Rendered, Renderer};

/// How long a page load waits for the worker to render the current file.
pub const RENDER_WAIT: Duration = Duration::from_secs(60);

/// FSEvents needs about 500 ms after `watch()` before it delivers events (CR §5); a change
/// made in that window would be missed, so new subscribers trigger one extra re-check.
const RECHECK_AFTER_WATCH: Duration = Duration::from_millis(750);

/// How long a document that suddenly reads as empty gets to receive its new content before
/// the empty render is published (truncate-then-write saves).
const EMPTY_SETTLE: Duration = Duration::from_millis(250);

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
    /// Dedup hash of every input of the render: the source bytes and, for a GitLab
    /// document with `::include` directives, the included files ([`Inputs`]).
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

/// What a dependency is to the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepKind {
    /// Loaded by the page (an image, a video): a change sends an `assets` event.
    Asset,
    /// Part of the Markdown source (a GitLab `::include` file): a change re-renders.
    Source,
}

/// A local file a document depends on, with its watch target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dep {
    /// URL path the page uses for it (sent in `assets` events; the root-relative path for
    /// [`DepKind::Source`]).
    pub url: String,
    /// Canonical directory to watch.
    pub dir: PathBuf,
    /// Canonical path events will carry.
    pub file: PathBuf,
    pub kind: DepKind,
    /// The file's directory does not exist yet: `dir` is its nearest existing ancestor and
    /// `file` the canonical path it will have (re-resolved when an ancestor changes).
    pub pending: bool,
}

/// Result of matching a batch of changed paths against one document.
#[derive(Debug, Default, PartialEq, Eq)]
struct Matched {
    /// Re-check (and possibly re-render) the document.
    doc: bool,
    /// Dependency URLs whose files changed (`assets` event).
    urls: Vec<String>,
    /// A directory on the way to a pending dependency changed: re-resolve those.
    pending: bool,
}

/// What to watch for the document itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DocWatch {
    /// Directories to watch (canonical).
    dirs: BTreeSet<PathBuf>,
    /// Paths whose events re-check the document: the directory entry it is requested by
    /// and, for a symlinked document, the file the link points to.
    files: BTreeSet<PathBuf>,
    /// The canonical path the document will have once its missing directories exist; an
    /// event on any of its ancestors re-checks the document.
    pending: Option<PathBuf>,
}

impl DocWatch {
    /// The watch for the document at `path` (absolute, below `root`): the directory of its
    /// entry and of its symlink target, or, while its directory does not exist, the nearest
    /// existing ancestor directory inside the root.
    fn of(path: &Path, root: &Path, follow_symlinks: bool) -> DocWatch {
        let mut w = DocWatch::default();
        let contained = |dir: &Path| paths::check_contained(root, dir, follow_symlinks).is_ok();
        for (dir, file) in [paths::entry_watch_target(path), paths::watch_target(path)]
            .into_iter()
            .flatten()
        {
            if contained(&dir) {
                w.dirs.insert(dir);
                w.files.insert(file);
            }
        }
        if w.dirs.is_empty()
            && let Some((dir, file)) = paths::ancestor_watch_target(root, path, follow_symlinks)
        {
            w.dirs.insert(dir);
            w.pending = Some(file);
        }
        w
    }

    fn matches(&self, p: &Path) -> bool {
        // A directory-level event (rescan, a watched directory deleted) re-checks too; the
        // hash makes that cheap.
        self.files.contains(p)
            || self.dirs.contains(p)
            || self.pending.as_ref().is_some_and(|f| f.starts_with(p))
    }
}

#[derive(Default)]
struct Tracking {
    subscribers: usize,
    /// What is watched for the document itself (one watcher reference per directory).
    doc: DocWatch,
    /// Latest dependencies (always tracked; watched only while subscribed).
    deps: Vec<Dep>,
    /// Directories this document holds a watcher reference on for its dependencies.
    dep_dirs: BTreeSet<PathBuf>,
}

impl Tracking {
    /// Switch the document's watch to `want`, keeping the references balanced. Returns
    /// whether a directory started being watched. When a directory cannot be watched the
    /// previous ones are kept, so the next change still re-checks (and retries).
    fn set_doc_watch(&mut self, want: DocWatch, w: &FsWatcher) -> bool {
        let mut added = false;
        let mut failed = false;
        let mut held = BTreeSet::new();
        for dir in &want.dirs {
            if self.doc.dirs.contains(dir) {
                held.insert(dir.clone());
                continue;
            }
            match w.add(dir) {
                Ok(()) => {
                    held.insert(dir.clone());
                    added = true;
                }
                Err(e) => {
                    tracing::warn!("cannot watch {}: {e}", dir.display());
                    failed = true;
                }
            }
        }
        for dir in &self.doc.dirs {
            if held.contains(dir) {
                continue;
            }
            if failed {
                held.insert(dir.clone());
            } else {
                w.remove(dir);
            }
        }
        self.doc = DocWatch { dirs: held, ..want };
        added
    }

    /// Watch exactly the directories of the current dependencies.
    fn sync_dep_dirs(&mut self, w: &FsWatcher) {
        let wanted: BTreeSet<PathBuf> = self.deps.iter().map(|d| d.dir.clone()).collect();
        let held = std::mem::take(&mut self.dep_dirs);
        let mut now = BTreeSet::new();
        for dir in &wanted {
            if held.contains(dir) || w.add(dir).is_ok() {
                now.insert(dir.clone());
            }
        }
        for dir in held.difference(&now) {
            w.remove(dir);
        }
        self.dep_dirs = now;
    }
}

/// Live state of one document.
pub struct DocEntry {
    /// Root-relative path (`docs/README.md`).
    pub key: String,
    /// Absolute path (served root joined with `key`).
    pub path: PathBuf,
    triggers: AtomicU64,
    /// Version counter shared by every document of the server, so versions are unique per
    /// server process: an entry re-created after eviction never hands out a version a
    /// client already holds (LIVE-8). Clients only compare versions; gaps are harmless.
    versions: Arc<AtomicU64>,
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
    fn new(key: &str, path: PathBuf, versions: Arc<AtomicU64>) -> DocEntry {
        DocEntry {
            key: key.to_owned(),
            path,
            triggers: AtomicU64::new(0),
            versions,
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

    fn add_subscriber(&self, ctx: &RenderCtx) {
        let mut t = self.lock();
        t.subscribers += 1;
        if t.subscribers > 1 {
            return;
        }
        let Some(w) = ctx.watcher.as_deref() else {
            return;
        };
        let want = DocWatch::of(&self.path, &ctx.root, ctx.follow_symlinks);
        t.set_doc_watch(want, w);
        t.sync_dep_dirs(w);
    }

    fn remove_subscriber(&self, watcher: Option<&FsWatcher>) {
        let mut t = self.lock();
        t.subscribers = t.subscribers.saturating_sub(1);
        if t.subscribers > 0 {
            return;
        }
        let doc = std::mem::take(&mut t.doc);
        let dep_dirs = std::mem::take(&mut t.dep_dirs);
        if let Some(w) = watcher {
            for dir in doc.dirs.iter().chain(dep_dirs.iter()) {
                w.remove(dir);
            }
        }
    }

    /// Re-evaluate the document's own watch while subscribed: its directory may have been
    /// created (a page opened before `newdir/` existed) or deleted, or a symlinked document
    /// re-pointed. Returns whether a new directory started being watched.
    fn refresh_doc_watch(&self, ctx: &RenderCtx) -> bool {
        let Some(w) = ctx.watcher.as_deref() else {
            return false;
        };
        if self.subscribers() == 0 {
            return false;
        }
        let want = DocWatch::of(&self.path, &ctx.root, ctx.follow_symlinks);
        let mut t = self.lock();
        if t.subscribers == 0 || t.doc == want {
            return false;
        }
        t.set_doc_watch(want, w)
    }

    /// Replace the dependency list, updating dependency watches while subscribed.
    fn set_deps(&self, deps: Vec<Dep>, watcher: Option<&FsWatcher>) {
        let mut t = self.lock();
        t.deps = deps;
        if t.subscribers == 0 {
            return;
        }
        if let Some(w) = watcher {
            t.sync_dep_dirs(w);
        }
    }

    /// Match changed paths against the document, its sources and its dependencies.
    fn match_paths(&self, changed: &[PathBuf]) -> Matched {
        let t = self.lock();
        if t.subscribers == 0 {
            return Matched::default();
        }
        // A change of a file the source includes re-renders as well.
        let doc = changed.iter().any(|p| {
            t.doc.matches(p)
                || t.deps
                    .iter()
                    .any(|d| d.kind == DepKind::Source && (d.file == *p || d.dir == *p))
        });
        let mut urls: Vec<String> = t
            .deps
            .iter()
            .filter(|d| d.kind == DepKind::Asset && !d.pending && changed.contains(&d.file))
            .map(|d| d.url.clone())
            .collect();
        urls.sort();
        urls.dedup();
        let pending = t
            .deps
            .iter()
            .any(|d| d.pending && changed.iter().any(|p| d.file.starts_with(p) || d.dir == *p));
        Matched { doc, urls, pending }
    }

    /// Re-resolve dependencies whose directory did not exist (LIVE-5), moving their watch
    /// to the real directory once it does. Returns the URLs of those whose file exists now
    /// (the page shows a broken image for them) and the URL and path of those whose
    /// directory appeared but whose file has not (yet).
    fn refresh_pending_deps(&self, ctx: &RenderCtx) -> (Vec<String>, Vec<(String, PathBuf)>) {
        let mut appeared = Vec::new();
        let mut waiting = Vec::new();
        let Some(w) = ctx.watcher.as_deref() else {
            return (appeared, waiting);
        };
        let mut t = self.lock();
        if t.subscribers == 0 {
            return (appeared, waiting);
        }
        let mut changed = false;
        for d in t.deps.iter_mut().filter(|d| d.pending) {
            let Some((dir, file, pending)) = dep_target(&ctx.root, ctx.follow_symlinks, &d.file)
            else {
                continue;
            };
            if (&dir, &file, pending) == (&d.dir, &d.file, d.pending) {
                continue;
            }
            changed = true;
            (d.dir, d.file, d.pending) = (dir, file, pending);
            if !d.pending {
                if d.file.is_file() {
                    appeared.push(d.url.clone());
                } else {
                    waiting.push((d.url.clone(), d.file.clone()));
                }
            }
        }
        if changed {
            t.sync_dep_dirs(w);
        }
        (appeared, waiting)
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
    /// The server-wide version counter (see [`DocEntry`]).
    versions: Arc<AtomicU64>,
}

impl Hub {
    pub fn new(ctx: Arc<RenderCtx>, shutdown: watch::Receiver<bool>) -> Hub {
        Hub {
            ctx,
            docs: Mutex::new(HashMap::new()),
            shutdown,
            versions: Arc::new(AtomicU64::new(0)),
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
        let entry = Arc::new(DocEntry::new(
            key,
            paths::join_rel(&self.ctx.root, key),
            self.versions.clone(),
        ));
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
        entry.add_subscriber(&self.ctx);
        entry.trigger();
        recheck_after_watch(&entry, &self.ctx);
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
            let Matched {
                doc,
                mut urls,
                pending,
            } = e.match_paths(changed);
            if doc {
                e.trigger();
            }
            if pending {
                let (appeared, waiting) = e.refresh_pending_deps(&self.ctx);
                urls.extend(appeared);
                urls.sort();
                urls.dedup();
                self.recheck_waiting_deps(&e, waiting);
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

    /// A dependency whose directory was just created and watched may itself be created
    /// before FSEvents delivers events for that directory (CR §5): look once more a little
    /// later and refresh it if it exists by then.
    fn recheck_waiting_deps(&self, entry: &Arc<DocEntry>, waiting: Vec<(String, PathBuf)>) {
        if waiting.is_empty() || !self.ctx.watcher.as_ref().is_some_and(|w| !w.is_polling()) {
            return;
        }
        let e = entry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RECHECK_AFTER_WATCH).await;
            let urls: Vec<String> = waiting
                .into_iter()
                .filter(|(_, file)| file.is_file())
                .map(|(url, _)| url)
                .collect();
            if !urls.is_empty() {
                e.broadcast(Arc::new(WireEvent::assets(&urls, &cache_token())));
            }
        });
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

/// FSEvents may miss changes made right after a directory starts being watched (CR §5):
/// re-check the document once more a little later.
fn recheck_after_watch(entry: &Arc<DocEntry>, ctx: &RenderCtx) {
    if ctx.watcher.as_ref().is_some_and(|w| !w.is_polling()) {
        let e = entry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RECHECK_AFTER_WATCH).await;
            e.trigger();
        });
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

/// The Markdown source of one render: raw bytes, or text already decoded while gathering.
enum Source {
    Bytes(Vec<u8>),
    Text(String, Option<String>),
}

impl Source {
    /// The decoded text and the invalid-UTF-8 warning, if any.
    fn into_text(self, key: &str) -> (String, Option<String>) {
        match self {
            Source::Bytes(b) => decode_markdown(b, key),
            Source::Text(t, w) => (t, w),
        }
    }
}

/// Everything a render reads (GL §3.3: included files belong to the document).
struct Inputs {
    source: Source,
    /// Files read by GitLab `::include` directives (canonical, inside the root), in order.
    includes: Vec<PathBuf>,
    /// Dedup hash: SHA-256 of the source bytes when nothing is included; otherwise a hash
    /// over that and every included file's path and bytes, so a page load or a watcher event
    /// re-renders when only an included file changed.
    hash: [u8; 32],
}

impl Inputs {
    async fn gather(bytes: Vec<u8>, key: &str, opts: &RenderOptions) -> Result<Inputs, String> {
        let doc_hash = sha256(&bytes);
        if !may_include(&bytes, opts) {
            return Ok(Inputs {
                source: Source::Bytes(bytes),
                includes: Vec::new(),
                hash: doc_hash,
            });
        }
        // Reading the included files is blocking I/O.
        let (key, opts) = (key.to_owned(), opts.clone());
        tokio::task::spawn_blocking(move || {
            let (text, warning) = decode_markdown(bytes, &key);
            let includes = crate::render::gitlab::included_files(&text, &opts);
            let hash = inputs_hash(doc_hash, &includes);
            Inputs {
                source: Source::Text(text, warning),
                includes,
                hash,
            }
        })
        .await
        .map_err(|e| format!("reading the included files failed: {e}"))
    }
}

/// Can this document `::include` files (GitLab readme mode with a directive in the text)?
fn may_include(bytes: &[u8], opts: &RenderOptions) -> bool {
    const DIRECTIVE: &[u8] = b"::include{file=";
    opts.flavor == Flavor::Gitlab
        && opts.mode == Mode::Readme
        && bytes.windows(DIRECTIVE.len()).any(|w| w == DIRECTIVE)
}

/// Combined dedup hash of a document and the files it includes.
fn inputs_hash(doc_hash: [u8; 32], includes: &[PathBuf]) -> [u8; 32] {
    if includes.is_empty() {
        return doc_hash;
    }
    let mut h = Sha256::new();
    h.update(doc_hash);
    for p in includes {
        h.update(p.as_os_str().as_encoded_bytes());
        h.update([0]);
        match std::fs::read(p) {
            Ok(b) => h.update(sha256(&b)),
            Err(_) => h.update([0xff]), // vanished meanwhile: differs from any content
        }
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// Watch entries for the files a document includes (a change re-renders the document).
fn source_deps(root: &Path, includes: &[PathBuf]) -> Vec<Dep> {
    includes
        .iter()
        .filter_map(|file| {
            let dir = file.parent()?.to_path_buf();
            let rel = file.strip_prefix(root).ok()?;
            let url = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            Some(Dep {
                url,
                dir,
                file: file.clone(),
                kind: DepKind::Source,
                pending: false,
            })
        })
        .collect()
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
        if entry.refresh_doc_watch(&ctx) {
            recheck_after_watch(&entry, &ctx);
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
    let mut read = read_contained(ctx, &entry.path).await;
    if matches!(&read, Ok(b) if b.is_empty())
        && snap
            .content
            .as_ref()
            .is_some_and(|c| !c.rendered.html.is_empty())
    {
        // A document that just became empty is probably being rewritten in place (truncate,
        // then write: `gen.py > README.md`, slow writers, network file systems). Publishing
        // the empty render would throw away the reader's scroll position and every
        // client-rendered block, so give the writer a moment first.
        tokio::time::sleep(EMPTY_SETTLE).await;
        read = read_contained(ctx, &entry.path).await;
    }
    let bytes = match read {
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

    let opts = ctx.options_for(&entry.path);
    let inputs = match Inputs::gather(bytes, &entry.key, &opts).await {
        Ok(i) => i,
        Err(message) => {
            ctx.log(&format!("rendering {} failed: {message}", entry.key));
            let v = entry.next_version();
            publish_failure(entry, t0, v, ErrorKind::Render, message);
            return Outcome::Done;
        }
    };
    let hash = inputs.hash;
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

    let renderer = ctx.renderer.clone();
    let key = entry.key.clone();
    let root = ctx.root.clone();
    let follow = ctx.follow_symlinks;
    let Inputs {
        source, includes, ..
    } = inputs;
    let joined = tokio::task::spawn_blocking(move || {
        let (text, decode_warning) = source.into_text(&key);
        let result = renderer.render(&text, &opts);
        let deps = match &result {
            Ok(r) => {
                let mut deps = resolve_deps(&root, follow, deps::scan(&r.html, &key));
                deps.extend(source_deps(&root, &includes));
                deps
            }
            Err(_) => Vec::new(),
        };
        (result, deps, decode_warning)
    })
    .await;

    // Drop stale results: a newer change arrived while rendering and the file differs again.
    if !force && entry.triggers.load(Ordering::SeqCst) != t0 {
        let now = match read_contained(ctx, &entry.path).await {
            Ok(b) => Inputs::gather(b, &entry.key, &ctx.options_for(&entry.path))
                .await
                .ok()
                .map(|i| i.hash),
            Err(_) => None,
        };
        if now != Some(hash) {
            return Outcome::Stale;
        }
    }

    match joined {
        Ok((Ok(mut rendered), deps, decode_warning)) => {
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
        Ok((Err(e), _, _)) => {
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
    // Only regular files: a FIFO or device node would block the reader forever.
    if !tokio::fs::metadata(&canon).await?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
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
            let (dir, file, pending) =
                dep_target(root, follow_symlinks, &paths::join_rel(root, &r.rel))?;
            Some(Dep {
                url: r.url,
                dir,
                file,
                kind: DepKind::Asset,
                pending,
            })
        })
        .collect()
}

/// Watch target of a dependency inside the root: (directory, file, pending). A file whose
/// directory does not exist yet is watched through its nearest existing ancestor.
fn dep_target(root: &Path, follow_symlinks: bool, abs: &Path) -> Option<(PathBuf, PathBuf, bool)> {
    match paths::watch_target(abs) {
        Some((dir, file)) => {
            paths::check_contained(root, &dir, follow_symlinks).ok()?;
            paths::check_contained(root, &file, follow_symlinks).ok()?;
            Some((dir, file, false))
        }
        None => {
            let (dir, file) = paths::ancestor_watch_target(root, abs, follow_symlinks)?;
            Some((dir, file, true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// LIVE-8: a document re-created after eviction continues above every version handed out
    /// before, so a client reconnecting with its old version does not skip new content.
    #[tokio::test]
    async fn versions_survive_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (hub, _stop) = hub(&root, Arc::new(Counting(AtomicUsize::new(0))));
        let mut last = 0;
        for i in 0..5 {
            std::fs::write(root.join("a.md"), format!("edit {i}")).unwrap();
            last = hub.current("a.md").await.unwrap().version;
        }
        assert!(last >= 5);
        for i in 0..MAX_DOCS {
            hub.current(&format!("x{i}.md")).await.unwrap();
        }
        assert!(hub.get("a.md").is_none(), "a.md was evicted");
        std::fs::write(root.join("a.md"), "edit after eviction").unwrap();
        let s = hub.current("a.md").await.unwrap();
        assert!(s.version > last, "{} <= {last}", s.version);
        assert_eq!(
            s.content.unwrap().rendered.html,
            "<p>edit after eviction</p>"
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
        assert_eq!(deps[0].kind, DepKind::Asset);
        // Workers stop on shutdown.
        tx.send(true).unwrap();
    }

    fn gitlab_hub(root: &Path) -> (Hub, watch::Sender<bool>) {
        let (tx, rx) = watch::channel(false);
        let mut options = RenderOptions::new(Flavor::Gitlab);
        options.root = Some(root.to_path_buf());
        let ctx = RenderCtx {
            renderer: Arc::new(crate::render::OfflineRenderer),
            options,
            root: root.to_path_buf(),
            follow_symlinks: false,
            quiet: true,
            watcher: None,
        };
        (Hub::new(Arc::new(ctx), rx), tx)
    }

    /// LIVE-1 / Q-INCLUDE-1: an edit of an `::include`d file re-renders the document on
    /// the next page load, and the included files are tracked as source dependencies.
    #[tokio::test]
    async fn gitlab_includes_are_inputs_of_the_render() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("parts")).unwrap();
        std::fs::write(
            root.join("doc.md"),
            "# Doc\n\n::include{file=inc.md}\n\n::include{file=parts/two.md}\n",
        )
        .unwrap();
        std::fs::write(root.join("inc.md"), "FIRST_VERSION\n").unwrap();
        std::fs::write(root.join("parts/two.md"), "OTHER_PART\n").unwrap();
        let (hub, _stop) = gitlab_hub(&root);

        let s = hub.current("doc.md").await.unwrap();
        let html = &s.content.as_ref().unwrap().rendered.html;
        assert!(
            html.contains("FIRST_VERSION") && html.contains("OTHER_PART"),
            "{html}"
        );
        let v1 = s.version;
        let deps = hub.get("doc.md").unwrap().deps();
        let sources: Vec<_> = deps
            .iter()
            .filter(|d| d.kind == DepKind::Source)
            .map(|d| (d.url.as_str(), d.file.clone()))
            .collect();
        assert_eq!(
            sources,
            [
                ("inc.md", root.join("inc.md")),
                ("parts/two.md", root.join("parts/two.md"))
            ]
        );

        // Unchanged inputs: no new version.
        assert_eq!(hub.current("doc.md").await.unwrap().version, v1);

        // Only the included file changes: the reload shows it.
        std::fs::write(root.join("inc.md"), "SECOND_VERSION\n").unwrap();
        let s = hub.current("doc.md").await.unwrap();
        assert!(s.version > v1);
        let html = &s.content.as_ref().unwrap().rendered.html;
        assert!(html.contains("SECOND_VERSION"), "stale include: {html}");
        let v2 = s.version;
        std::fs::write(root.join("parts/two.md"), "CHANGED_PART\n").unwrap();
        let s = hub.current("doc.md").await.unwrap();
        assert!(s.version > v2);
        let html = &s.content.as_ref().unwrap().rendered.html;
        assert!(html.contains("CHANGED_PART"), "stale include: {html}");
    }

    /// LIVE-2: a truncate-then-write save does not publish an empty render in between.
    #[tokio::test]
    async fn truncate_then_write_saves_skip_the_empty_render() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let doc = root.join("a.md");
        std::fs::write(&doc, "full one").unwrap();
        let (hub, _stop) = hub(&root, Arc::new(Counting(AtomicUsize::new(0))));
        let v1 = hub.current("a.md").await.unwrap().version;
        let mut rx = hub.get("a.md").unwrap().subscribe_events();

        std::fs::write(&doc, "").unwrap(); // truncated; the new bytes follow 100 ms later
        let writer = {
            let doc = doc.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                std::fs::write(&doc, "full two").unwrap();
            })
        };
        let s = hub.current("a.md").await.unwrap();
        writer.await.unwrap();
        let s = if s.content.as_ref().unwrap().rendered.html.contains("two") {
            s
        } else {
            hub.current("a.md").await.unwrap()
        };
        assert!(s.version > v1);
        assert_eq!(s.content.as_ref().unwrap().rendered.html, "<p>full two</p>");
        let mut contents = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if ev.name == "content" {
                contents.push(ev.data.clone());
            }
        }
        assert!(
            contents.iter().all(|d| !d.contains(r#""html":"<p></p>""#)),
            "an empty render was published: {contents:?}"
        );

        // A file that stays empty is published as empty (after the settle delay).
        std::fs::write(&doc, "").unwrap();
        let s = hub.current("a.md").await.unwrap();
        assert_eq!(s.content.as_ref().unwrap().rendered.html, "<p></p>");
    }

    #[test]
    fn source_dependencies_trigger_the_document() {
        let entry = DocEntry::new("doc.md", PathBuf::from("/r/doc.md"), Arc::default());
        entry.lock().subscribers = 1;
        entry.set_deps(
            vec![
                Dep {
                    url: "/a.png".into(),
                    dir: "/r/img".into(),
                    file: "/r/img/a.png".into(),
                    kind: DepKind::Asset,
                    pending: false,
                },
                Dep {
                    url: "inc.md".into(),
                    dir: "/r/parts".into(),
                    file: "/r/parts/inc.md".into(),
                    kind: DepKind::Source,
                    pending: false,
                },
                Dep {
                    url: "/new/dir/b.png".into(),
                    dir: "/r".into(),
                    file: "/r/new/dir/b.png".into(),
                    kind: DepKind::Asset,
                    pending: true,
                },
            ],
            None,
        );
        let m = |p: &str| entry.match_paths(&[p.into()]);
        let matched = |doc, urls: &[&str], pending| Matched {
            doc,
            urls: urls.iter().map(|u| (*u).to_owned()).collect(),
            pending,
        };
        assert_eq!(m("/r/parts/inc.md"), matched(true, &[], false));
        assert_eq!(m("/r/img/a.png"), matched(false, &["/a.png"], false));
        assert_eq!(m("/r/img"), matched(false, &[], false));
        // A directory on the way to a pending dependency, not the dependency itself.
        assert_eq!(m("/r/new"), matched(false, &[], true));
        assert_eq!(m("/r/other"), matched(false, &[], false));
        assert!(!may_include(
            b"::include{file=x}",
            &RenderOptions::new(Flavor::Github)
        ));
        assert!(may_include(
            b"a\n::include{file=x}",
            &RenderOptions::new(Flavor::Gitlab)
        ));
        let mut comment = RenderOptions::new(Flavor::Gitlab);
        comment.mode = Mode::Comment;
        assert!(!may_include(b"::include{file=x}", &comment));
    }
}
