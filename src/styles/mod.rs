//! Style bundles read in place through tarfs (owner: styles). Contract: docs/DESIGN.md §3.5, §4.
//!
//! A bundle is one `.tar.gz` (default `<cache>/md-render/styles.tar.gz`, D9) holding both
//! flavors' vendored stylesheets, the files they reference, client libraries and a
//! `manifest.json` ([`Manifest`]). It is decompressed into memory once and served from there;
//! nothing is ever unpacked to disk. [`Bundle::fallback`] provides the embedded minimal CSS when
//! no bundle exists (D11), and [`SharedBundle`] lets a running server hot-swap a re-vendored
//! bundle while keeping the old one if the new file is corrupt.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, bail};
use arc_swap::ArcSwap;
use bytes::Bytes;

use crate::flavor::Flavor;

pub mod manifest;
pub mod vendor;

pub use manifest::{Asset, Failure, FlavorManifest, Manifest, Stylesheet};

use manifest::{MANIFEST_PATH, SCHEMA, role};

/// Where a bundle came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleOrigin {
    /// Read from this `.tar.gz` (missing flavors may have been filled with fallbacks).
    File(PathBuf),
    /// The embedded fallback only.
    Fallback,
}

/// A loaded style bundle: an in-memory tarfs view plus its validated manifest.
///
/// Cheap to share (`Arc<Bundle>`); reads are zero-copy slices of the decompressed archive.
#[derive(Clone)]
pub struct Bundle {
    fs: tarfs::InMemoryFileSystem,
    manifest: Manifest,
    origin: BundleOrigin,
    etag: String,
}

impl fmt::Debug for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bundle")
            .field("origin", &self.origin)
            .field("etag", &self.etag)
            .field("files", &self.fs.len())
            .field("flavors", &self.manifest.flavors.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Lowercase hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for &b in digest.iter() {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0xf)]));
    }
    s
}

/// Check one flavor's manifest entry against the archive: every path lives below `<flavor>/`
/// and exists.
pub(crate) fn validate_flavor(
    flavor: Flavor,
    fm: &FlavorManifest,
    exists: impl Fn(&str) -> bool,
) -> anyhow::Result<()> {
    let prefix = format!("{flavor}/");
    for path in fm.paths() {
        if !path.starts_with(&prefix) {
            bail!("{MANIFEST_PATH} lists {path:?} for {flavor}, outside {prefix}");
        }
        if !exists(path) {
            bail!("{MANIFEST_PATH} lists {path}, which is missing from the archive");
        }
    }
    Ok(())
}

/// Check a whole manifest (schema version and every listed path).
pub(crate) fn validate_manifest(m: &Manifest, exists: impl Fn(&str) -> bool) -> anyhow::Result<()> {
    if m.schema != SCHEMA {
        bail!(
            "unsupported bundle schema {} (this md-render reads schema {SCHEMA}); re-run `md-render \
             vendor`",
            m.schema
        );
    }
    for (flavor, fm) in &m.flavors {
        validate_flavor(*flavor, fm, &exists)?;
    }
    Ok(())
}

/// The embedded fallback CSS of `flavor` (empty if the asset registry lacks it).
fn fallback_css(flavor: Flavor) -> &'static [u8] {
    crate::assets::get(&format!("fallback-{flavor}.css")).map(|(b, _)| b).unwrap_or_default()
}

/// Content hash of the embedded fallback CSS of `flavors` (part of the etag of bundles that use
/// it, so a new binary with different fallback CSS invalidates browser caches).
fn fallback_tag(flavors: &[Flavor]) -> String {
    let mut all = Vec::new();
    for f in flavors {
        all.extend_from_slice(f.as_str().as_bytes());
        all.push(0);
        all.extend_from_slice(fallback_css(*f));
        all.push(0);
    }
    sha256_hex(&all)[..16].to_owned()
}

impl Bundle {
    /// Open a bundle file (decompressed into memory, never unpacked to disk), parse and validate
    /// its manifest: the schema must be [`SCHEMA`] and every listed path must exist.
    pub fn open(path: &Path) -> anyhow::Result<Bundle> {
        let gz = std::fs::read(path)
            .with_context(|| format!("reading style bundle {}", path.display()))?;
        Self::from_tar_gz_bytes(&gz, BundleOrigin::File(path.to_owned()))
            .with_context(|| format!("invalid style bundle {}", path.display()))
    }

    /// Like [`Bundle::open`] for bundle bytes already in memory. The etag is the SHA-256 of
    /// `gz`.
    pub fn from_tar_gz_bytes(gz: &[u8], origin: BundleOrigin) -> anyhow::Result<Bundle> {
        let fs = tarfs::InMemoryFileSystem::from_tar_gz_bytes(gz).context("reading the archive")?;
        let raw = fs.get(MANIFEST_PATH).with_context(|| format!("no {MANIFEST_PATH} in the archive"))?;
        let manifest: Manifest =
            serde_json::from_slice(raw).with_context(|| format!("parsing {MANIFEST_PATH}"))?;
        validate_manifest(&manifest, |p| fs.contains(p))?;
        Ok(Bundle { fs, manifest, origin, etag: sha256_hex(gz) })
    }

    /// The embedded fallback bundle (minimal CSS for both flavors at `<flavor>/fallback.css`,
    /// role `fallback`, `fallback: true`).
    pub fn fallback() -> Bundle {
        let mut fs = tarfs::InMemoryFileSystem::new();
        let mut manifest = Manifest::default();
        for flavor in Flavor::ALL {
            Self::insert_fallback(&mut fs, &mut manifest, flavor);
        }
        Bundle {
            fs,
            manifest,
            origin: BundleOrigin::Fallback,
            etag: format!("fallback-{}", fallback_tag(&Flavor::ALL)),
        }
    }

    fn insert_fallback(fs: &mut tarfs::InMemoryFileSystem, manifest: &mut Manifest, flavor: Flavor) {
        let css = fallback_css(flavor);
        let path = format!("{flavor}/fallback.css");
        // A constant, valid path: write_file cannot fail here.
        let written = fs.write_file(&path, Bytes::from_static(css));
        debug_assert!(written.is_ok(), "fallback path {path} rejected");
        manifest.flavors.insert(
            flavor,
            FlavorManifest {
                fallback: true,
                stylesheets: vec![Stylesheet {
                    path,
                    role: role::FALLBACK.into(),
                    sha256: sha256_hex(css),
                    size: css.len() as u64,
                    ..Default::default()
                }],
                themes: vec!["light".into(), "dark".into()],
                ..Default::default()
            },
        );
    }

    /// Add fallback entries for flavors this bundle lacks (the etag changes accordingly).
    pub fn with_fallbacks(mut self) -> Bundle {
        let missing: Vec<Flavor> =
            Flavor::ALL.into_iter().filter(|f| !self.manifest.flavors.contains_key(f)).collect();
        if missing.is_empty() {
            return self;
        }
        for &flavor in &missing {
            // The flavor is absent from the manifest; drop any stray files below its prefix so
            // the fallback entry is the only content there.
            self.fs.delete_prefix(flavor.as_str());
            Self::insert_fallback(&mut self.fs, &mut self.manifest, flavor);
        }
        self.etag = format!("{}-fb{}", self.etag, fallback_tag(&missing));
        self
    }

    /// The bundle manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The manifest entry of flavor `f`, if present (vendored or fallback).
    pub fn flavor(&self, f: Flavor) -> Option<&FlavorManifest> {
        self.manifest.flavors.get(&f)
    }

    /// Whether flavor `f` is missing or only has the embedded fallback CSS.
    pub fn is_fallback(&self, f: Flavor) -> bool {
        self.flavor(f).is_none_or(|m| m.fallback)
    }

    /// Zero-copy read of a bundle file by its exact bundle path (`github/assets/x.css`).
    pub fn read(&self, path: &str) -> Option<Bytes> {
        self.fs.read_file(path).ok()
    }

    /// Whether the bundle contains `path`.
    pub fn contains(&self, path: &str) -> bool {
        self.fs.contains(path)
    }

    /// Size of the file at `path`.
    pub fn file_size(&self, path: &str) -> Option<u64> {
        self.fs.file_size(path)
    }

    /// All file paths, sorted (includes `manifest.json`).
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.fs.paths()
    }

    /// Changes whenever the bundle content changes: SHA-256 of the bundle file (plus a hash of
    /// the embedded fallback CSS for flavors filled by [`Bundle::with_fallbacks`]). Safe to use
    /// verbatim inside an HTTP `ETag` (hex digits, `-`, letters only).
    pub fn etag(&self) -> &str {
        &self.etag
    }

    /// Where the bundle came from.
    pub fn origin(&self) -> &BundleOrigin {
        &self.origin
    }
}

/// `Content-Type` for a bundle file, by extension (`text/*` and JavaScript get
/// `charset=utf-8`; fonts get `font/*` even though GitLab serves them as octet-stream, GL §11.2).
pub fn content_type(path: &str) -> String {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let essence = mime.essence_str();
    if mime.type_() == mime_guess::mime::TEXT || essence == "application/javascript" {
        let essence = if essence == "application/javascript" { "text/javascript" } else { essence };
        format!("{essence}; charset=utf-8")
    } else if essence == "application/json" {
        "application/json".to_owned()
    } else {
        essence.to_owned()
    }
}

/// Default bundle location: `$MD_RENDER_STYLES` (if set and non-empty), else
/// `<cache dir>/md-render/styles.tar.gz`.
pub fn default_bundle_path() -> PathBuf {
    if let Some(p) = std::env::var_os("MD_RENDER_STYLES").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    directories::ProjectDirs::from("", "", "md-render")
        .map(|d| d.cache_dir().join("styles.tar.gz"))
        .unwrap_or_else(|| PathBuf::from("md-render-styles.tar.gz"))
}

/// The `md-render vendor` command that (re)creates `path` (for `flavor`, or all flavors).
pub fn vendor_command(flavor: Option<Flavor>, path: &Path) -> String {
    let mut cmd = String::from("md-render vendor");
    if let Some(f) = flavor {
        cmd.push_str(&format!(" --flavor {f}"));
    }
    if path != default_bundle_path() {
        let shown = path.display().to_string();
        if shown.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+@:=,".contains(c)) {
            cmd.push_str(&format!(" -o {shown}"));
        } else {
            cmd.push_str(&format!(" -o '{}'", shown.replace('\'', r"'\''")));
        }
    }
    cmd
}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::NotFound)
}

/// Load a bundle, filling missing flavors with the fallback. Never fails; returns a warning for
/// the user when the bundle is missing or unusable (with the exact command that fixes it).
/// Use [`flavor_warning`] to warn about a single flavor that only has fallback styles.
pub fn load_or_fallback(path: &Path) -> (Bundle, Option<String>) {
    match Bundle::open(path) {
        Ok(b) => (b.with_fallbacks(), None),
        Err(e) if is_not_found(&e) => (
            Bundle::fallback(),
            Some(format!(
                "no style bundle at {}; using minimal fallback styles (run `{}` to download the \
                 GitHub and GitLab styles)",
                path.display(),
                vendor_command(None, path)
            )),
        ),
        Err(e) => (
            Bundle::fallback(),
            Some(format!(
                "{e:#}; using minimal fallback styles (run `{}` to download them again)",
                vendor_command(None, path)
            )),
        ),
    }
}

/// A warning when `flavor` has only fallback styles in a bundle read from a file (D11). `None`
/// when the flavor was vendored, or when the whole bundle is the fallback (then
/// [`load_or_fallback`] already returned a warning).
pub fn flavor_warning(bundle: &Bundle, flavor: Flavor) -> Option<String> {
    let BundleOrigin::File(path) = bundle.origin() else { return None };
    bundle.is_fallback(flavor).then(|| {
        format!(
            "style bundle {} has no {flavor} styles; using minimal fallback styles (run `{}`)",
            path.display(),
            vendor_command(Some(flavor), path)
        )
    })
}

/// A bundle shared by a running server and swapped atomically when the file changes (DESIGN §4,
/// last bullet). Readers call [`SharedBundle::get`] per request; [`SharedBundle::reload`] runs
/// off the request path (it decompresses the archive).
pub struct SharedBundle {
    path: PathBuf,
    current: ArcSwap<Bundle>,
}

impl fmt::Debug for SharedBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedBundle")
            .field("path", &self.path)
            .field("current", &*self.current.load())
            .finish()
    }
}

impl SharedBundle {
    /// Load `path` with [`load_or_fallback`] semantics (never fails; the warning is returned).
    pub fn load(path: impl Into<PathBuf>) -> (SharedBundle, Option<String>) {
        let path = path.into();
        let (bundle, warning) = load_or_fallback(&path);
        (SharedBundle::new(path, bundle), warning)
    }

    /// Share an already loaded `bundle` that [`SharedBundle::reload`] re-reads from `path`.
    pub fn new(path: impl Into<PathBuf>, bundle: Bundle) -> SharedBundle {
        SharedBundle { path: path.into(), current: ArcSwap::from_pointee(bundle) }
    }

    /// The bundle file this holder watches.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The current bundle (a cheap `Arc` clone; it stays valid across swaps).
    pub fn get(&self) -> Arc<Bundle> {
        self.current.load_full()
    }

    /// The current bundle's etag.
    pub fn etag(&self) -> String {
        self.current.load().etag().to_owned()
    }

    /// Re-read the bundle file. Returns `Ok(true)` when the content changed and was swapped in,
    /// `Ok(false)` when it is unchanged. A file that cannot be read or fails validation returns
    /// `Err` and the previous bundle stays in place. A deleted file swaps in the fallback (the
    /// same thing a fresh start would show).
    pub fn reload(&self) -> anyhow::Result<bool> {
        let candidate = match std::fs::metadata(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Bundle::fallback(),
            _ => Bundle::open(&self.path)
                .context("keeping the previously loaded styles")?
                .with_fallbacks(),
        };
        Ok(self.replace(candidate))
    }

    /// Swap in `bundle`; returns whether its etag differs from the current one.
    pub fn replace(&self, bundle: Bundle) -> bool {
        if self.current.load().etag() == bundle.etag() {
            return false;
        }
        self.current.store(Arc::new(bundle));
        true
    }
}

#[cfg(test)]
mod tests;
