//! The server's single file watcher (DESIGN D13, CR §5).
//!
//! * One native `notify` watcher (FSEvents / inotify / ReadDirectoryChangesW) debounced by
//!   `notify-debouncer-mini` (50 ms); or, with `--poll[=MS]` or when the native watcher
//!   cannot be created, a small poller of our own (see [`Poller`]).
//! * Directories are watched non-recursively and reference-counted: every document with
//!   subscribers, every local image those documents use, the style bundle and `--css` files
//!   each hold one reference on their (canonical) parent directory. Watching directories, not
//!   inodes, survives atomic saves (write temp + rename) and delete + recreate.
//! * Changes are forwarded as batches of paths; event kinds are ignored because FSEvents
//!   coalesces them unreliably (CR §5): consumers re-read whatever a path names.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{Config, DebounceEventResult, Debouncer, new_debouncer_opt};
use tokio::sync::mpsc::UnboundedSender;

/// Debounce window (CR §5: 30-50 ms keeps end-to-end latency around 60-80 ms).
pub(super) const DEBOUNCE: Duration = Duration::from_millis(50);

/// Poll interval used when the native watcher is unavailable and no `--poll` was given.
pub(super) const FALLBACK_POLL: Duration = Duration::from_millis(1000);

enum Backend {
    Native(Debouncer<RecommendedWatcher>),
    Poll(Poller),
}

struct Inner {
    backend: Backend,
    dirs: HashMap<PathBuf, usize>,
}

/// Reference-counted, non-recursive directory watcher that reports changed paths.
pub(crate) struct FsWatcher {
    inner: Mutex<Inner>,
    polling: bool,
}

impl std::fmt::Debug for FsWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsWatcher")
            .field("polling", &self.polling)
            .finish_non_exhaustive()
    }
}

impl FsWatcher {
    /// Create the watcher; changed paths are sent to `tx` in batches.
    ///
    /// Returns the watcher and, when the native backend failed and polling is used instead,
    /// a warning for the user.
    pub(crate) fn new(
        poll: Option<Duration>,
        tx: UnboundedSender<Vec<PathBuf>>,
    ) -> anyhow::Result<(FsWatcher, Option<String>)> {
        let mut warning = None;
        let backend = match poll {
            Some(interval) => Backend::Poll(Poller::new(interval, tx)?),
            None => {
                let native_tx = tx.clone();
                let handler = move |res: DebounceEventResult| match res {
                    Ok(events) => {
                        let paths: Vec<PathBuf> = events.into_iter().map(|e| e.path).collect();
                        if !paths.is_empty() {
                            // A send error only means the server is shutting down.
                            let _ = native_tx.send(paths);
                        }
                    }
                    Err(e) => tracing::debug!("file watcher error: {e}"),
                };
                let cfg = Config::default().with_timeout(DEBOUNCE);
                match new_debouncer_opt::<_, RecommendedWatcher>(cfg, handler) {
                    Ok(d) => Backend::Native(d),
                    Err(e) => {
                        warning = Some(format!(
                            "native file watching is unavailable ({e}); polling every {} ms instead",
                            FALLBACK_POLL.as_millis()
                        ));
                        Backend::Poll(Poller::new(FALLBACK_POLL, tx)?)
                    }
                }
            }
        };
        let polling = matches!(backend, Backend::Poll(_));
        let inner = Inner {
            backend,
            dirs: HashMap::new(),
        };
        Ok((
            FsWatcher {
                inner: Mutex::new(inner),
                polling,
            },
            warning,
        ))
    }

    /// Whether this watcher polls (as opposed to native OS notifications).
    pub(crate) fn is_polling(&self) -> bool {
        self.polling
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Add one reference to `dir` (canonical), starting to watch it on the first reference.
    pub(crate) fn add(&self, dir: &Path) -> notify::Result<()> {
        let mut inner = self.lock();
        if let Some(n) = inner.dirs.get_mut(dir) {
            *n += 1;
            return Ok(());
        }
        if !dir.is_dir() {
            return Err(notify::Error::path_not_found().add_path(dir.to_path_buf()));
        }
        match &mut inner.backend {
            Backend::Native(d) => d.watcher().watch(dir, RecursiveMode::NonRecursive)?,
            Backend::Poll(p) => p.add(dir),
        }
        inner.dirs.insert(dir.to_path_buf(), 1);
        Ok(())
    }

    /// Drop one reference to `dir`, unwatching it when the last reference goes away.
    pub(crate) fn remove(&self, dir: &Path) {
        let mut inner = self.lock();
        let Some(n) = inner.dirs.get_mut(dir) else {
            return;
        };
        *n -= 1;
        if *n > 0 {
            return;
        }
        inner.dirs.remove(dir);
        match &mut inner.backend {
            Backend::Native(d) => {
                if let Err(e) = d.watcher().unwatch(dir) {
                    // E.g. the directory was deleted and inotify already dropped the watch.
                    tracing::debug!("unwatching {}: {e}", dir.display());
                }
            }
            Backend::Poll(p) => p.remove(dir),
        }
    }

    /// Current reference count of `dir` (0 when not watched).
    #[cfg(test)]
    pub(crate) fn refs(&self, dir: &Path) -> usize {
        self.lock().dirs.get(dir).copied().unwrap_or(0)
    }

    /// Is `dir` currently watched?
    #[cfg(test)]
    pub(crate) fn is_watched(&self, dir: &Path) -> bool {
        self.refs(dir) > 0
    }
}

/// What a directory entry looked like at the last poll.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    is_dir: bool,
    /// Inode (Unix): a rename-over save always changes it.
    ino: u64,
}

impl Stamp {
    fn of(path: &Path) -> Option<Stamp> {
        // Follow symlinks (a symlinked document changes when its target does); a dangling
        // link is still an entry.
        let meta = std::fs::metadata(path)
            .or_else(|_| std::fs::symlink_metadata(path))
            .ok()?;
        #[cfg(unix)]
        let ino = std::os::unix::fs::MetadataExt::ino(&meta);
        #[cfg(not(unix))]
        let ino = 0;
        Some(Stamp {
            len: meta.len(),
            modified: meta.modified().ok(),
            is_dir: meta.is_dir(),
            ino,
        })
    }
}

/// Entries of one directory (None: the directory cannot be read, e.g. it was deleted).
type Listing = Option<HashMap<OsString, Stamp>>;

fn scan(dir: &Path) -> Listing {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut out = HashMap::new();
    for entry in entries.flatten() {
        if let Some(stamp) = Stamp::of(&entry.path()) {
            out.insert(entry.file_name(), stamp);
        }
    }
    Some(out)
}

/// Paths that differ between two listings of `dir` (the directory itself when it appeared
/// or disappeared).
fn diff(dir: &Path, old: &Listing, new: &Listing) -> Vec<PathBuf> {
    match (old, new) {
        (Some(old), Some(new)) => {
            let mut out: Vec<PathBuf> = new
                .iter()
                .filter(|(name, stamp)| old.get(*name) != Some(*stamp))
                .map(|(name, _)| dir.join(name))
                .collect();
            out.extend(
                old.keys()
                    .filter(|name| !new.contains_key(*name))
                    .map(|name| dir.join(name)),
            );
            out
        }
        (None, None) => Vec::new(),
        // Appeared or vanished: report the directory and everything in it.
        (Some(entries), None) | (None, Some(entries)) => {
            let mut out = vec![dir.to_path_buf()];
            out.extend(entries.keys().map(|name| dir.join(name)));
            out
        }
    }
}

/// A precise polling watcher (`--poll`, network file systems).
///
/// notify's `PollWatcher` compares modification times in whole seconds unless it hashes
/// every file's contents on every poll, so a save within the same second as the previous
/// write goes unnoticed. This poller instead lists each watched directory every interval
/// and compares size, full-resolution modification time and inode of every entry; it never
/// reads file contents.
struct Poller {
    dirs: Arc<Mutex<HashMap<PathBuf, Listing>>>,
    /// Dropping it stops the polling thread.
    _stop: Sender<()>,
}

impl Poller {
    fn new(interval: Duration, tx: UnboundedSender<Vec<PathBuf>>) -> anyhow::Result<Poller> {
        let dirs: Arc<Mutex<HashMap<PathBuf, Listing>>> = Arc::default();
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let shared = dirs.clone();
        std::thread::Builder::new()
            .name("md-render poll".into())
            .spawn(move || {
                // `recv_timeout` doubles as the interval timer and the stop signal.
                while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(interval) {
                    let changed = poll_once(&shared);
                    if !changed.is_empty() && tx.send(changed).is_err() {
                        break; // the server is gone
                    }
                }
            })
            .map_err(|e| anyhow::anyhow!("starting the polling file watcher: {e}"))?;
        Ok(Poller { dirs, _stop: stop })
    }

    fn add(&self, dir: &Path) {
        let listing = scan(dir);
        lock(&self.dirs).insert(dir.to_path_buf(), listing);
    }

    fn remove(&self, dir: &Path) {
        lock(&self.dirs).remove(dir);
    }
}

/// Re-list every watched directory; returns the changed paths.
fn poll_once(dirs: &Mutex<HashMap<PathBuf, Listing>>) -> Vec<PathBuf> {
    // Scan without holding the lock, so `add` / `remove` never wait for the file system.
    let watched: Vec<PathBuf> = lock(dirs).keys().cloned().collect();
    let mut changed = Vec::new();
    for dir in watched {
        let new = scan(&dir);
        let mut map = lock(dirs);
        // Skip directories unwatched meanwhile.
        if let Some(old) = map.get_mut(&dir) {
            changed.extend(diff(&dir, old, &new));
            *old = new;
        }
    }
    changed
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn expect_path(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<PathBuf>>, want: &Path) {
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let batch = rx.recv().await.expect("watcher alive");
                if batch.iter().any(|p| p == want) {
                    break;
                }
            }
        })
        .await;
        assert!(got.is_ok(), "no event for {}", want.display());
    }

    #[tokio::test]
    async fn refcounts_and_reports_changes() {
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (w, warning) = FsWatcher::new(Some(Duration::from_millis(30)), tx).unwrap();
        assert!(warning.is_none());
        assert!(w.is_polling());
        w.add(&canon).unwrap();
        w.add(&canon).unwrap();
        assert_eq!(w.refs(&canon), 2);
        std::fs::write(canon.join("a.md"), "x").unwrap();
        expect_path(&mut rx, &canon.join("a.md")).await;
        w.remove(&canon);
        assert!(w.is_watched(&canon));
        w.remove(&canon);
        assert!(!w.is_watched(&canon));
        w.remove(&canon); // extra removes are harmless
        assert_eq!(w.refs(&canon), 0);
    }

    #[tokio::test]
    async fn polling_sees_same_second_rename_over_saves() {
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        let doc = canon.join("doc.md");
        std::fs::write(&doc, "one").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (w, _) = FsWatcher::new(Some(Duration::from_millis(20)), tx).unwrap();
        w.add(&canon).unwrap();
        for content in ["two", "six", "ten"] {
            // Same size, written immediately after the previous save.
            std::fs::write(canon.join(".tmp"), content).unwrap();
            std::fs::rename(canon.join(".tmp"), &doc).unwrap();
            expect_path(&mut rx, &doc).await;
        }
        std::fs::remove_file(&doc).unwrap();
        expect_path(&mut rx, &doc).await;
    }

    #[test]
    fn listing_diffs() {
        let dir = Path::new("/d");
        let stamp = |len| Stamp {
            len,
            modified: None,
            is_dir: false,
            ino: 1,
        };
        let old: Listing = Some(
            [
                (OsString::from("a"), stamp(1)),
                (OsString::from("b"), stamp(1)),
            ]
            .into(),
        );
        let new: Listing = Some(
            [
                (OsString::from("a"), stamp(2)),
                (OsString::from("c"), stamp(1)),
            ]
            .into(),
        );
        let mut changed = diff(dir, &old, &new);
        changed.sort();
        assert_eq!(changed, [dir.join("a"), dir.join("b"), dir.join("c")]);
        assert!(diff(dir, &old, &old).is_empty());
        let mut gone = diff(dir, &old, &None);
        gone.sort();
        assert_eq!(gone, [dir.to_path_buf(), dir.join("a"), dir.join("b")]);
        assert!(diff(dir, &None, &None).is_empty());
    }

    #[test]
    fn watching_a_missing_directory_fails_cleanly() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (w, _) = FsWatcher::new(Some(Duration::from_millis(50)), tx).unwrap();
        assert!(w.add(Path::new("/definitely/not/here/md-render")).is_err());
        assert!(!w.is_watched(Path::new("/definitely/not/here/md-render")));
    }

    #[tokio::test]
    async fn native_watcher_reports_atomic_saves() {
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        let doc = canon.join("doc.md");
        std::fs::write(&doc, "one").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (w, warning) = FsWatcher::new(None, tx).unwrap();
        if warning.is_some() {
            return; // no native backend on this system; polling is covered above
        }
        w.add(&canon).unwrap();
        // FSEvents needs ~0.5 s after `watch()` before it delivers events (CR §5).
        tokio::time::sleep(Duration::from_millis(700)).await;
        std::fs::write(canon.join(".tmp"), "two").unwrap();
        std::fs::rename(canon.join(".tmp"), &doc).unwrap();
        expect_path(&mut rx, &doc).await;
    }
}
