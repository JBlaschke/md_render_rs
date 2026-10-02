//! Style bundles read in place through tarfs (owner: styles). Contract: docs/DESIGN.md §3.5, §4.
//!
//! A bundle is one `.tar.gz` (default `<cache>/md-render/styles.tar.gz`, D9) holding both
//! flavors' vendored stylesheets, the files they reference, client libraries and a
//! `manifest.json` ([`Manifest`]). It is decompressed into memory once and served from there;
//! nothing is ever unpacked to disk. [`Bundle::fallback`] provides the embedded minimal CSS when
//! no bundle exists (D11), and [`SharedBundle`] lets a running server hot-swap a re-vendored
//! bundle while keeping the old one if the new file is corrupt.

use std::collections::BTreeMap;
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
    /// SHA-256 of the `.tar.gz` bytes this bundle was read from (`None` for the fallback), so
    /// [`SharedBundle::reload`] can detect an unchanged file without decompressing it.
    source_sha256: Option<String>,
    /// Flavors dropped when the bundle was opened because their manifest entry did not match the
    /// archive, with the reason (see [`Bundle::open`]).
    damaged: BTreeMap<Flavor, String>,
}

impl fmt::Debug for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bundle")
            .field("origin", &self.origin)
            .field("etag", &self.etag)
            .field("files", &self.fs.len())
            .field("flavors", &self.manifest.flavors.keys().collect::<Vec<_>>())
            .field("damaged", &self.damaged)
            .finish()
    }
}

/// Largest style bundle md-render reads, compressed or decompressed: 512 MiB. A complete bundle
/// (both flavors with their client libraries) decompresses to about 16 MB; the cap only stops a
/// small hostile `.tar.gz` (a "gzip bomb" expands ~1000-fold) from exhausting memory.
pub const MAX_BUNDLE_SIZE: u64 = 512 << 20;

/// The tarfs limits every bundle is read with ([`MAX_BUNDLE_SIZE`]).
pub(crate) fn bundle_limits() -> tarfs::Limits {
    tarfs::Limits::default().with_max_decompressed_size(MAX_BUNDLE_SIZE)
}

/// Read the `.tar.gz` bytes of a bundle file, refusing files larger than [`MAX_BUNDLE_SIZE`]
/// (`InvalidData`) without reading them whole.
fn read_bundle_file(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let too_large = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "file is larger than the {} MiB limit",
                MAX_BUNDLE_SIZE >> 20
            ),
        )
    };
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_BUNDLE_SIZE {
        return Err(too_large());
    }
    let mut gz = Vec::new();
    // `take`: the file may grow while we read it.
    file.take(MAX_BUNDLE_SIZE + 1).read_to_end(&mut gz)?;
    if gz.len() as u64 > MAX_BUNDLE_SIZE {
        return Err(too_large());
    }
    Ok(gz)
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

/// Check that this md-render reads the manifest's schema version.
fn validate_schema(m: &Manifest) -> anyhow::Result<()> {
    if m.schema != SCHEMA {
        bail!(
            "unsupported bundle schema {} (this md-render reads schema {SCHEMA}); re-run `md-render \
             vendor`",
            m.schema
        );
    }
    Ok(())
}

/// Check a whole manifest (schema version and every listed path).
pub(crate) fn validate_manifest(m: &Manifest, exists: impl Fn(&str) -> bool) -> anyhow::Result<()> {
    validate_schema(m)?;
    for (flavor, fm) in &m.flavors {
        validate_flavor(*flavor, fm, &exists)?;
    }
    Ok(())
}

/// The embedded fallback CSS of `flavor` (empty if the asset registry lacks it).
fn fallback_css(flavor: Flavor) -> &'static [u8] {
    crate::assets::get(&format!("fallback-{flavor}.css"))
        .map(|(b, _)| b)
        .unwrap_or_default()
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
    /// its manifest: the schema must be [`SCHEMA`] and every listed path must exist. Neither the
    /// file nor its decompressed contents may exceed [`MAX_BUNDLE_SIZE`].
    ///
    /// Each flavor is validated on its own: a flavor whose entry lists a missing file (or a path
    /// outside `<flavor>/`) is dropped, files and all, and the others stay usable
    /// ([`Bundle::with_fallbacks`] then fills it in; [`load_or_fallback`] and [`flavor_warning`]
    /// say why). Only a bundle without a single intact flavor is rejected.
    pub fn open(path: &Path) -> anyhow::Result<Bundle> {
        let gz = read_bundle_file(path)
            .with_context(|| format!("reading style bundle {}", path.display()))?;
        Self::from_tar_gz_bytes(&gz, BundleOrigin::File(path.to_owned()))
            .with_context(|| format!("invalid style bundle {}", path.display()))
    }

    /// Like [`Bundle::open`] for bundle bytes already in memory. The etag is the SHA-256 of
    /// `gz`.
    pub fn from_tar_gz_bytes(gz: &[u8], origin: BundleOrigin) -> anyhow::Result<Bundle> {
        Self::from_tar_gz_bytes_hashed(gz, sha256_hex(gz), origin)
    }

    /// [`Bundle::from_tar_gz_bytes`] with the SHA-256 of `gz` already computed.
    fn from_tar_gz_bytes_hashed(
        gz: &[u8],
        sha256: String,
        origin: BundleOrigin,
    ) -> anyhow::Result<Bundle> {
        let mut fs = tarfs::InMemoryFileSystem::from_tar_gz_bytes_with_limits(gz, bundle_limits())
            .context("reading the archive")?;
        let raw = fs
            .get(MANIFEST_PATH)
            .with_context(|| format!("no {MANIFEST_PATH} in the archive"))?;
        let mut manifest: Manifest =
            serde_json::from_slice(raw).with_context(|| format!("parsing {MANIFEST_PATH}"))?;
        validate_schema(&manifest)?;
        // Per flavor, as `md-render vendor` does when it keeps the other flavor: one damaged
        // GitLab font must not cost the GitHub pages their styles.
        let mut damaged = BTreeMap::new();
        let mut first_error = None;
        for (&flavor, fm) in &manifest.flavors {
            if let Err(e) = validate_flavor(flavor, fm, |p| fs.contains(p)) {
                damaged.insert(flavor, format!("{e:#}"));
                first_error.get_or_insert(e);
            }
        }
        if let Some(e) = first_error
            && damaged.len() == manifest.flavors.len()
        {
            return Err(e); // nothing usable is left: the bundle as a whole is broken
        }
        for flavor in damaged.keys() {
            manifest.flavors.remove(flavor);
            fs.delete_prefix(flavor.as_str());
        }
        Ok(Bundle {
            fs,
            manifest,
            origin,
            etag: sha256.clone(),
            source_sha256: Some(sha256),
            damaged,
        })
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
            source_sha256: None,
            damaged: BTreeMap::new(),
        }
    }

    fn insert_fallback(
        fs: &mut tarfs::InMemoryFileSystem,
        manifest: &mut Manifest,
        flavor: Flavor,
    ) {
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
        let missing: Vec<Flavor> = Flavor::ALL
            .into_iter()
            .filter(|f| !self.manifest.flavors.contains_key(f))
            .collect();
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

/// The values of every `url()` and `@import` reference in `css`, and of every string argument
/// of `image-set()` / `-webkit-image-set()` (an image URL too), in source order, with CSS
/// escapes resolved. References inside comments and other strings are not reported (CR §7).
pub fn css_references(css: &str) -> Vec<String> {
    vendor::css::scan(css)
        .into_iter()
        .map(|r| r.value)
        .collect()
}

/// Rewrite the value of every reference [`css_references`] reports in `css` for which `replace`
/// returns a new (unescaped) value; it is escaped as the reference's quoting requires. Other
/// bytes are kept verbatim. Export uses this to inline bundle files as `data:` URIs, e.g. with
/// [`resolve_bundle_ref`] and [`Bundle::read`].
pub fn rewrite_css_references(
    css: &str,
    mut replace: impl FnMut(&str) -> Option<String>,
) -> String {
    let refs = vendor::css::scan(css);
    vendor::css::rewrite(css, &refs, |r| replace(&r.value))
}

/// Resolve `reference`, a relative `url()` / `@import` value found in the bundle file `from`,
/// to the bundle path it designates (query and fragment dropped, `%`-escapes decoded, `.` and
/// `..` applied). Vendored stylesheets only contain such relative references (D10).
///
/// Returns `None` for references that do not point into the bundle: absolute URLs (`https:`,
/// `data:`, ...), root-relative (`/x`) and fragment-only (`#f`) references, directories, and
/// paths that climb above the bundle root. The result is not checked for existence.
///
/// ```
/// use md_render::styles::resolve_bundle_ref;
/// let css = "github/assets/global-1.css";
/// assert_eq!(resolve_bundle_ref(css, "../images/a.gif#x").as_deref(), Some("github/images/a.gif"));
/// assert_eq!(resolve_bundle_ref(css, "data:image/png;base64,AA"), None);
/// assert_eq!(resolve_bundle_ref(css, "../../../etc/passwd"), None);
/// ```
pub fn resolve_bundle_ref(from: &str, reference: &str) -> Option<String> {
    let r = reference.trim();
    if r.is_empty() || r.starts_with(['#', '/', '\\']) {
        return None;
    }
    let r = &r[..r.find(['?', '#']).unwrap_or(r.len())];
    // A `:` before the first `/` makes it an absolute URL (RFC 3986 §4.2).
    if r.find(':').is_some_and(|colon| !r[..colon].contains('/')) {
        return None;
    }
    let decoded = percent_encoding::percent_decode_str(r).decode_utf8().ok()?;
    // A trailing `/`, `.` or `..` designates a directory, never a bundle file.
    if matches!(decoded.rsplit('/').next(), None | Some("" | "." | "..")) {
        return None;
    }
    let mut segs: Vec<&str> = from.split('/').collect();
    segs.pop(); // the file name of `from`
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop()?;
            }
            s => segs.push(s),
        }
    }
    let path = segs.join("/");
    tarfs::check_path(&path).is_ok().then_some(path)
}

/// `Content-Type` for a bundle file, by extension (`text/*` and JavaScript get
/// `charset=utf-8`; fonts get `font/*` even though GitLab serves them as octet-stream, GL §11.2).
pub fn content_type(path: &str) -> String {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let essence = mime.essence_str();
    if mime.type_() == mime_guess::mime::TEXT || essence == "application/javascript" {
        let essence = if essence == "application/javascript" {
            "text/javascript"
        } else {
            essence
        };
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
        if shown
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+@:=,".contains(c))
        {
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
/// the user when the bundle is missing or unusable, or when a damaged flavor was dropped from it
/// (with the exact command that fixes it). Use [`flavor_warning`] to warn about a single flavor
/// that only has fallback styles.
pub fn load_or_fallback(path: &Path) -> (Bundle, Option<String>) {
    match Bundle::open(path) {
        Ok(b) => {
            let warning = (!b.damaged.is_empty()).then(|| {
                let parts: Vec<String> = b
                    .damaged
                    .iter()
                    .map(|(flavor, why)| damaged_flavor_warning(path, *flavor, why))
                    .collect();
                parts.join("; ")
            });
            (b.with_fallbacks(), warning)
        }
        Err(e) if is_not_found(&e) => (
            Bundle::fallback(),
            Some(format!(
                "no style bundle at {}; using minimal fallback styles (run `{}` to download the \
                 GitHub and GitLab styles)",
                path.display(),
                vendor_command(None, path)
            )),
        ),
        Err(e) => {
            // Suggest only what `md-render vendor` will do: it refuses to overwrite a file it
            // cannot read as an md-render bundle, except at the default location.
            let cmd = vendor_command(None, path);
            let fix = if vendor::accepts_output(path, &e) {
                format!("run `{cmd}` to download them again")
            } else if path.is_file() {
                format!("delete it, then run `{cmd}` to download them again")
            } else {
                // A directory, or a path we cannot even stat: deleting it is not the answer.
                "choose another bundle file; `md-render vendor -o FILE` creates one".to_owned()
            };
            (
                Bundle::fallback(),
                Some(format!("{e:#}; using minimal fallback styles ({fix})")),
            )
        }
    }
}

/// A warning when `flavor` has only fallback styles in a bundle read from a file (D11), saying
/// so when [`Bundle::open`] dropped the flavor as damaged. `None` when the flavor was vendored,
/// or when the whole bundle is the fallback (then [`load_or_fallback`] already returned a
/// warning).
pub fn flavor_warning(bundle: &Bundle, flavor: Flavor) -> Option<String> {
    let BundleOrigin::File(path) = bundle.origin() else {
        return None;
    };
    if let Some(why) = bundle.damaged.get(&flavor) {
        return Some(damaged_flavor_warning(path, flavor, why));
    }
    bundle.is_fallback(flavor).then(|| {
        format!(
            "style bundle {} has no {flavor} styles; using minimal fallback styles (run `{}`)",
            path.display(),
            vendor_command(Some(flavor), path)
        )
    })
}

/// The warning for a flavor that [`Bundle::open`] dropped from the bundle at `path` because of
/// `why`, with the command that repairs it (it re-vendors only that flavor and keeps the rest).
fn damaged_flavor_warning(path: &Path, flavor: Flavor, why: &str) -> String {
    format!(
        "style bundle {} has damaged {flavor} styles ({why}); using minimal fallback styles for \
         {flavor} (run `{}` to repair them)",
        path.display(),
        vendor_command(Some(flavor), path)
    )
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
        SharedBundle {
            path: path.into(),
            current: ArcSwap::from_pointee(bundle),
        }
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
    /// `Ok(false)` when it is unchanged (detected by hashing the file, without decompressing
    /// it). A file that cannot be read or fails validation returns `Err` and the previous bundle
    /// stays in place. A deleted file swaps in the fallback (the same thing a fresh start would
    /// show). Bundles are replaced by an atomic rename (D9), so a reader never sees a half
    /// written file.
    pub fn reload(&self) -> anyhow::Result<bool> {
        let keep = "keeping the previously loaded styles";
        let gz = match read_bundle_file(&self.path) {
            Ok(gz) => gz,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(self.replace(Bundle::fallback()));
            }
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("reading style bundle {}", self.path.display()))
                    .context(keep));
            }
        };
        let sha256 = sha256_hex(&gz);
        if self.current.load().source_sha256.as_deref() == Some(sha256.as_str()) {
            return Ok(false);
        }
        let candidate =
            Bundle::from_tar_gz_bytes_hashed(&gz, sha256, BundleOrigin::File(self.path.clone()))
                .with_context(|| format!("invalid style bundle {}", self.path.display()))
                .context(keep)?
                .with_fallbacks();
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
