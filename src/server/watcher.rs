//! The server's single file watcher (DESIGN D13, CR §5).
//!
//! * One `notify` watcher (FSEvents / inotify / ReadDirectoryChangesW), or a `PollWatcher`
//!   with `--poll[=MS]` or when the native watcher cannot be created.
//! * Directories are watched non-recursively and reference-counted: every document with
//!   subscribers, every local image those documents use, the style bundle and `--css` files
//!   each hold one reference on their (canonical) parent directory. Watching directories, not
//!   inodes, survives atomic saves (write temp + rename) and delete + recreate.
//! * Events are debounced by `notify-debouncer-mini` (50 ms) and forwarded as batches of
//!   paths; event kinds are ignored because FSEvents coalesces them unreliably (CR §5).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use notify::{PollWatcher, RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{Config, DebounceEventResult, Debouncer, new_debouncer_opt};
use tokio::sync::mpsc::UnboundedSender;

/// Debounce window (CR §5: 30-50 ms keeps end-to-end latency around 60-80 ms).
pub const DEBOUNCE: Duration = Duration::from_millis(50);

/// Poll interval used when the native watcher is unavailable and no `--poll` was given.
pub const FALLBACK_POLL: Duration = Duration::from_millis(1000);

enum Backend {
    Native(Debouncer<RecommendedWatcher>),
    Poll(Debouncer<PollWatcher>),
}

impl Backend {
    fn watcher(&mut self) -> &mut dyn notify::Watcher {
        match self {
            Backend::Native(d) => d.watcher(),
            Backend::Poll(d) => d.watcher(),
        }
    }
}

struct Inner {
    backend: Backend,
    dirs: HashMap<PathBuf, usize>,
}

/// Reference-counted, non-recursive directory watcher that reports changed paths.
pub struct FsWatcher {
    inner: Mutex<Inner>,
    polling: bool,
}

impl std::fmt::Debug for FsWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsWatcher").field("polling", &self.polling).finish_non_exhaustive()
    }
}

impl FsWatcher {
    /// Create the watcher; changed paths are sent to `tx` in debounced batches.
    ///
    /// Returns the watcher and, when the native backend failed and polling was used instead,
    /// a warning for the user.
    pub fn new(poll: Option<Duration>, tx: UnboundedSender<Vec<PathBuf>>) -> anyhow::Result<(FsWatcher, Option<String>)> {
        let handler = move |res: DebounceEventResult| match res {
            Ok(events) => {
                let paths: Vec<PathBuf> = events.into_iter().map(|e| e.path).collect();
                if !paths.is_empty() {
                    // A send error only means the server is shutting down.
                    let _ = tx.send(paths);
                }
            }
            Err(e) => tracing::debug!("file watcher error: {e}"),
        };
        let mut warning = None;
        let backend = match poll {
            Some(interval) => Backend::Poll(poll_debouncer(interval, handler)?),
            None => {
                let handler2 = handler.clone();
                match new_debouncer_opt::<_, RecommendedWatcher>(Config::default().with_timeout(DEBOUNCE), handler) {
                    Ok(d) => Backend::Native(d),
                    Err(e) => {
                        warning = Some(format!(
                            "native file watching is unavailable ({e}); polling every {} ms instead",
                            FALLBACK_POLL.as_millis()
                        ));
                        Backend::Poll(poll_debouncer(FALLBACK_POLL, handler2)?)
                    }
                }
            }
        };
        let polling = matches!(backend, Backend::Poll(_));
        Ok((FsWatcher { inner: Mutex::new(Inner { backend, dirs: HashMap::new() }), polling }, warning))
    }

    /// Whether this watcher polls (as opposed to native OS notifications).
    pub fn is_polling(&self) -> bool {
        self.polling
    }

    /// Add one reference to `dir` (canonical), starting to watch it on the first reference.
    pub fn add(&self, dir: &Path) -> notify::Result<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(n) = inner.dirs.get_mut(dir) {
            *n += 1;
            return Ok(());
        }
        // PollWatcher silently accepts missing paths; report them like the native backends do.
        if !dir.is_dir() {
            return Err(notify::Error::path_not_found().add_path(dir.to_path_buf()));
        }
        inner.backend.watcher().watch(dir, RecursiveMode::NonRecursive)?;
        inner.dirs.insert(dir.to_path_buf(), 1);
        Ok(())
    }

    /// Drop one reference to `dir`, unwatching it when the last reference goes away.
    pub fn remove(&self, dir: &Path) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let Some(n) = inner.dirs.get_mut(dir) else { return };
        *n -= 1;
        if *n == 0 {
            inner.dirs.remove(dir);
            if let Err(e) = inner.backend.watcher().unwatch(dir) {
                tracing::debug!("unwatching {}: {e}", dir.display());
            }
        }
    }

    /// Current reference count of `dir` (0 when not watched).
    pub fn refs(&self, dir: &Path) -> usize {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.dirs.get(dir).copied().unwrap_or(0)
    }

    /// Is `dir` currently watched?
    pub fn is_watched(&self, dir: &Path) -> bool {
        self.refs(dir) > 0
    }
}

fn poll_debouncer<F>(interval: Duration, handler: F) -> anyhow::Result<Debouncer<PollWatcher>>
where
    F: FnMut(DebounceEventResult) + Send + 'static,
{
    let notify_cfg = notify::Config::default().with_poll_interval(interval);
    let cfg = Config::default().with_timeout(DEBOUNCE).with_notify_config(notify_cfg);
    new_debouncer_opt::<_, PollWatcher>(cfg, handler)
        .map_err(|e| anyhow::anyhow!("creating the polling file watcher: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refcounts_and_reports_changes() {
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (w, warning) = FsWatcher::new(Some(Duration::from_millis(50)), tx).unwrap();
        assert!(warning.is_none());
        assert!(w.is_polling());
        w.add(&canon).unwrap();
        w.add(&canon).unwrap();
        assert_eq!(w.refs(&canon), 2);
        // Give the poller a baseline scan before changing the directory.
        tokio::time::sleep(Duration::from_millis(200)).await;
        std::fs::write(canon.join("a.md"), "x").unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let batch = rx.recv().await.expect("watcher alive");
                if batch.iter().any(|p| p == &canon.join("a.md")) {
                    break;
                }
            }
        })
        .await;
        assert!(got.is_ok(), "no event for a.md");
        w.remove(&canon);
        assert!(w.is_watched(&canon));
        w.remove(&canon);
        assert!(!w.is_watched(&canon));
        w.remove(&canon); // extra removes are harmless
        assert_eq!(w.refs(&canon), 0);
    }

    #[test]
    fn watching_a_missing_directory_fails_cleanly() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (w, _) = FsWatcher::new(Some(Duration::from_millis(50)), tx).unwrap();
        assert!(w.add(Path::new("/definitely/not/here/md-render")).is_err());
        assert!(!w.is_watched(Path::new("/definitely/not/here/md-render")));
    }
}
