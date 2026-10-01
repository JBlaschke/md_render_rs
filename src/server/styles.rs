//! Style bundle and `--css` files of a running server (DESIGN §4, D11, GRIP §11.1).
//!
//! The bundle is opened once at startup (vendoring it first with `--auto-vendor` when the
//! flavor is missing) and kept in an `ArcSwap`. With live updates on, the bundle file's
//! directory is watched; a change re-opens the bundle off the reactor, swaps it in, bumps
//! `styles_version` and broadcasts `styles`. A corrupt replacement keeps the old bundle and
//! broadcasts an `error` of kind `bundle`. Local `--css` files are served under
//! `/__/md-render/css/<n>/<name>` and watched the same way.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use arc_swap::ArcSwap;
use tokio::sync::{Notify, broadcast, watch};

use super::events::{ErrorKind, WireEvent};
use super::paths;
use super::watch::FsWatcher;
use crate::PREFIX;
use crate::flavor::Flavor;
use crate::styles::Bundle;

/// Size and modification time of the bundle file: a cheap change detector that works
/// whatever the bundle's own ETag scheme is.
type Fingerprint = (u64, Option<SystemTime>);

fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// One `--css` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtraCss {
    /// URL the page links to.
    pub url: String,
    /// Local file served under `/__/md-render/css/<n>/<name>` (None: external URL).
    pub file: Option<PathBuf>,
}

impl ExtraCss {
    /// Classify `--css` values: URLs (`scheme://`, `//host`, `data:`) and root-relative URLs
    /// pass through; existing local files are served by the server and watched.
    pub fn from_config(entries: &[String]) -> (Vec<ExtraCss>, Vec<String>) {
        let mut out = Vec::new();
        let mut warnings = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            let is_url = entry.contains("://") || entry.starts_with("//") || entry.starts_with("data:");
            let path = Path::new(entry);
            if !is_url && path.is_file() {
                match std::fs::canonicalize(path) {
                    Ok(canon) => {
                        let name = canon.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let url = format!("{PREFIX}/css/{i}/{}", paths::encode_segment(&name));
                        out.push(ExtraCss { url, file: Some(canon) });
                        continue;
                    }
                    Err(e) => warnings.push(format!("--css {entry}: {e}")),
                }
            } else if !is_url && !entry.starts_with('/') {
                warnings.push(format!("--css {entry}: no such file; linking it as a URL"));
            }
            out.push(ExtraCss { url: entry.clone(), file: None });
        }
        (out, warnings)
    }
}

/// How opening the bundle at startup went.
#[derive(Debug)]
pub enum Initial {
    /// The bundle has vendored styles for the flavor.
    Ready(Bundle),
    /// The bundle exists but lacks the flavor (or only has fallback styles for it).
    NoFlavor(Bundle),
    /// There is no bundle file.
    Missing,
    /// The bundle file exists but cannot be read.
    Broken(anyhow::Error),
}

impl Initial {
    /// Open `path` and classify the result for `flavor`.
    pub fn open(path: &Path, flavor: Flavor) -> Initial {
        if !path.exists() {
            return Initial::Missing;
        }
        match Bundle::open(path) {
            Ok(b) if b.flavor(flavor).is_some_and(|f| !f.fallback) => Initial::Ready(b),
            Ok(b) => Initial::NoFlavor(b),
            Err(e) => Initial::Broken(e),
        }
    }

    fn has_flavor(&self) -> bool {
        matches!(self, Initial::Ready(_))
    }
}

/// The exact command that vendors `flavor` into `path` (D11).
pub fn vendor_hint(flavor: Flavor, path: &Path) -> String {
    let mut cmd = format!("md-render vendor --flavor {flavor}");
    if path != crate::styles::default_bundle_path() {
        let p = path.display().to_string();
        let quoted = if p.chars().any(|c| c.is_whitespace() || "'\"$`\\".contains(c)) {
            format!("'{}'", p.replace('\'', r"'\''"))
        } else {
            p
        };
        cmd.push_str(&format!(" -o {quoted}"));
    }
    cmd
}

/// Load the bundle for a new server: vendor first when asked and needed, print the D11 hint
/// when the flavor's styles are missing, and always end up with a usable bundle.
pub async fn load_initial(path: &Path, flavor: Flavor, auto_vendor: bool, quiet: bool) -> Bundle {
    let mut state = Initial::open(path, flavor);
    if auto_vendor && !state.has_flavor() {
        if !quiet {
            eprintln!(" * Vendoring {flavor} styles into {} (--auto-vendor) ...", path.display());
        }
        let opts = crate::styles::vendor::VendorOptions {
            flavors: vec![flavor],
            output: path.to_path_buf(),
            github_source: crate::styles::vendor::GITHUB_SOURCE.to_owned(),
            gitlab_source: crate::styles::vendor::GITLAB_SOURCE.to_owned(),
            all_github_themes: false,
            libs: true,
            quiet,
        };
        match tokio::task::spawn_blocking(move || crate::styles::vendor::vendor(&opts)).await {
            Ok(Ok(report)) => {
                if !quiet {
                    eprintln!(" * Vendored {} files ({} bytes) into {}", report.files, report.bytes, report.output.display());
                }
                state = Initial::open(path, flavor);
            }
            Ok(Err(e)) => eprintln!("md-render: warning: vendoring {flavor} styles failed: {e:#}"),
            Err(e) => eprintln!("md-render: warning: vendoring {flavor} styles failed: {e}"),
        }
    }
    let hint = vendor_hint(flavor, path);
    match state {
        Initial::Ready(b) => b.with_fallbacks(),
        Initial::NoFlavor(b) => {
            if !quiet {
                eprintln!(
                    "md-render: {} has no vendored {flavor} styles; using minimal fallback styles.\n           Run `{hint}` to download them (or pass --auto-vendor).",
                    path.display()
                );
            }
            b.with_fallbacks()
        }
        Initial::Missing => {
            if !quiet {
                eprintln!(
                    "md-render: no style bundle at {}; using minimal fallback styles.\n           Run `{hint}` to download the {flavor} styles (or pass --auto-vendor).",
                    path.display()
                );
            }
            Bundle::fallback()
        }
        Initial::Broken(e) => {
            // Shown even with --quiet: the user asked for a bundle that cannot be used.
            eprintln!(
                "md-render: warning: {e:#}; using minimal fallback styles.\n           Re-create the bundle with `{hint}`."
            );
            Bundle::fallback()
        }
    }
}

/// Live style state of a server.
pub struct Styles {
    flavor: Flavor,
    path: PathBuf,
    bundle: ArcSwap<Bundle>,
    version: AtomicU64,
    fingerprint: Mutex<Option<Fingerprint>>,
    /// Canonical (dir, file) of the bundle while it is watched.
    watched: Mutex<Option<(PathBuf, PathBuf)>>,
    css: Vec<ExtraCss>,
    /// Canonical (dir, file) of each watched local `--css` file.
    css_watched: Mutex<Vec<(PathBuf, PathBuf)>>,
    wake: Notify,
    global: broadcast::Sender<Arc<WireEvent>>,
    quiet: bool,
}

impl std::fmt::Debug for Styles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Styles").field("path", &self.path).field("version", &self.version()).finish_non_exhaustive()
    }
}

impl Styles {
    pub fn new(
        flavor: Flavor,
        path: PathBuf,
        bundle: Bundle,
        css: Vec<ExtraCss>,
        global: broadcast::Sender<Arc<WireEvent>>,
        quiet: bool,
    ) -> Styles {
        let fp = fingerprint(&path);
        Styles {
            flavor,
            path,
            bundle: ArcSwap::from_pointee(bundle),
            version: AtomicU64::new(0),
            fingerprint: Mutex::new(fp),
            watched: Mutex::new(None),
            css,
            css_watched: Mutex::new(Vec::new()),
            wake: Notify::new(),
            global,
            quiet,
        }
    }

    /// The current bundle.
    pub fn bundle(&self) -> Arc<Bundle> {
        self.bundle.load_full()
    }

    /// Bumped on every bundle swap and `--css` change.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Extra stylesheet URLs for the page.
    pub fn extra_css_urls(&self) -> Vec<String> {
        self.css.iter().map(|c| c.url.clone()).collect()
    }

    /// The local file behind `/__/md-render/css/<index>/...`.
    pub fn css_file(&self, index: usize) -> Option<&Path> {
        self.css.get(index)?.file.as_deref()
    }

    /// Stylesheet URLs a page of this flavor uses with the current bundle (sent in `styles`).
    pub fn hrefs(&self) -> Vec<String> {
        let bundle = self.bundle();
        let mut out: Vec<String> = bundle
            .flavor(self.flavor)
            .map(|m| m.stylesheets.iter().map(|s| format!("{PREFIX}/bundle/{}", s.path)).collect())
            .unwrap_or_default();
        out.extend(self.extra_css_urls());
        out
    }

    /// Start watching the bundle file and local `--css` files.
    pub fn watch(&self, watcher: &FsWatcher) {
        if let Some((dir, file)) = paths::watch_target(&self.path) {
            match watcher.add(&dir) {
                Ok(()) => *lock(&self.watched) = Some((dir, file)),
                Err(e) => tracing::debug!("not watching the style bundle directory {}: {e}", dir.display()),
            }
        }
        let mut css = lock(&self.css_watched);
        for c in &self.css {
            if let Some(f) = &c.file
                && let Some((dir, file)) = paths::watch_target(f)
                && watcher.add(&dir).is_ok()
            {
                css.push((dir, file));
            }
        }
    }

    /// Ask the reload task to re-check the bundle file.
    pub fn recheck(&self) {
        self.wake.notify_one();
    }

    /// Dispatch changed paths from the watcher.
    pub fn on_paths_changed(&self, changed: &[PathBuf]) {
        if let Some((dir, file)) = lock(&self.watched).as_ref()
            && changed.iter().any(|p| p == file || p == dir)
        {
            self.wake.notify_one();
        }
        let css_changed = lock(&self.css_watched).iter().any(|(_, f)| changed.contains(f));
        if css_changed {
            let v = self.version.fetch_add(1, Ordering::SeqCst) + 1;
            self.log("Change detected in --css stylesheet, refreshing styles");
            let _ = self.global.send(Arc::new(WireEvent::styles(v, &self.hrefs())));
        }
    }

    fn log(&self, msg: &str) {
        if !self.quiet {
            eprintln!(" * {msg}");
        }
    }

    /// Reload loop: runs until shutdown.
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.wait_for(|stop| *stop) => break,
                _ = self.wake.notified() => {}
            }
            self.reload().await;
        }
    }

    /// Re-open the bundle if the file changed; swap it in or report why not.
    pub async fn reload(&self) {
        let now = fingerprint(&self.path);
        {
            let mut fp = lock(&self.fingerprint);
            if *fp == now {
                return;
            }
            *fp = now;
        }
        if now.is_none() {
            self.log(&format!("style bundle {} was removed; keeping the loaded styles", self.path.display()));
            return;
        }
        let path = self.path.clone();
        let opened = tokio::task::spawn_blocking(move || Bundle::open(&path)).await;
        match opened {
            Ok(Ok(b)) => {
                if b.flavor(self.flavor).is_none_or(|f| f.fallback) && !self.quiet {
                    eprintln!(
                        "md-render: the new style bundle has no vendored {} styles; run `{}`",
                        self.flavor,
                        vendor_hint(self.flavor, &self.path)
                    );
                }
                self.bundle.store(Arc::new(b.with_fallbacks()));
                let v = self.version.fetch_add(1, Ordering::SeqCst) + 1;
                self.log(&format!("Style bundle {} changed, refreshing styles", self.path.display()));
                let _ = self.global.send(Arc::new(WireEvent::styles(v, &self.hrefs())));
            }
            Ok(Err(e)) => {
                let message = format!("{e:#}; keeping the previously loaded styles");
                self.log(&format!("style bundle error: {message}"));
                let _ = self.global.send(Arc::new(WireEvent::global_error(ErrorKind::Bundle, &message)));
            }
            Err(e) => {
                let message = format!("re-opening the style bundle failed: {e}");
                self.log(&message);
                let _ = self.global.send(Arc::new(WireEvent::global_error(ErrorKind::Bundle, &message)));
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::styles::{FlavorManifest, Manifest, Stylesheet};

    pub(crate) fn write_bundle(path: &Path, css: &str) {
        let mut fs = tarfs::InMemoryFileSystem::new();
        let mut m = Manifest::default();
        m.flavors.insert(
            Flavor::Github,
            FlavorManifest {
                stylesheets: vec![Stylesheet { path: "github/a.css".into(), role: "github".into(), ..Default::default() }],
                ..Default::default()
            },
        );
        fs.write_file("manifest.json", serde_json::to_vec(&m).unwrap()).unwrap();
        fs.write_file("github/a.css", css.as_bytes().to_vec()).unwrap();
        fs.write_tarball_gz(path).unwrap();
    }

    #[test]
    fn extra_css_classification() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("my style.css");
        std::fs::write(&file, "body{}").unwrap();
        let entries = vec![
            "https://example.com/a.css".to_owned(),
            file.display().to_string(),
            "/local/root.css".to_owned(),
            "missing.css".to_owned(),
        ];
        let (css, warnings) = ExtraCss::from_config(&entries);
        assert_eq!(css[0], ExtraCss { url: "https://example.com/a.css".into(), file: None });
        assert_eq!(css[1].url, "/__/md-render/css/1/my%20style.css");
        assert!(css[1].file.is_some());
        assert_eq!(css[2].url, "/local/root.css");
        assert_eq!(css[3].url, "missing.css");
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn hint_mentions_custom_paths() {
        let hint = vendor_hint(Flavor::Gitlab, Path::new("/tmp/my bundle.tar.gz"));
        assert_eq!(hint, "md-render vendor --flavor gitlab -o '/tmp/my bundle.tar.gz'");
        let default = vendor_hint(Flavor::Github, &crate::styles::default_bundle_path());
        assert_eq!(default, "md-render vendor --flavor github");
    }

    #[test]
    fn initial_classification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.tar.gz");
        assert!(matches!(Initial::open(&path, Flavor::Github), Initial::Missing));
        std::fs::write(&path, b"not a tarball").unwrap();
        assert!(matches!(Initial::open(&path, Flavor::Github), Initial::Broken(_)));
        write_bundle(&path, "a{}");
        assert!(matches!(Initial::open(&path, Flavor::Github), Initial::Ready(_)));
        assert!(matches!(Initial::open(&path, Flavor::Gitlab), Initial::NoFlavor(_)));
    }

    #[tokio::test]
    async fn reload_swaps_and_keeps_old_on_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.tar.gz");
        write_bundle(&path, "a{}");
        let (tx, mut rx) = broadcast::channel(8);
        let styles = Styles::new(Flavor::Github, path.clone(), load_initial(&path, Flavor::Github, false, true).await, vec![], tx, true);
        assert_eq!(styles.hrefs(), ["/__/md-render/bundle/github/a.css"]);
        // Unchanged file: nothing happens.
        styles.reload().await;
        assert_eq!(styles.version(), 0);

        write_bundle(&path, "a{color:red}/* longer */");
        styles.reload().await;
        assert_eq!(styles.version(), 1);
        assert_eq!(rx.recv().await.unwrap().name, "styles");
        assert_eq!(&styles.bundle().read("github/a.css").unwrap()[..], b"a{color:red}/* longer */");

        std::fs::write(&path, b"garbage, definitely not gzip").unwrap();
        styles.reload().await;
        assert_eq!(styles.version(), 1);
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.name, "error");
        assert!(ev.data.contains("\"bundle\""));
        assert_eq!(&styles.bundle().read("github/a.css").unwrap()[..], b"a{color:red}/* longer */");
    }
}
