//! Style bundles read in place through tarfs (owner: styles). Contract: docs/DESIGN.md §3.5, §4.
//!
//! SKELETON PLACEHOLDER: functional minimum so other owners can integrate.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use bytes::Bytes;

use crate::flavor::Flavor;

pub mod manifest;
pub mod vendor;

pub use manifest::{Asset, Failure, FlavorManifest, Manifest, Stylesheet};

/// Where a bundle came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleOrigin {
    File(PathBuf),
    Fallback,
}

/// A loaded style bundle.
#[derive(Clone, Debug)]
pub struct Bundle {
    fs: tarfs::InMemoryFileSystem,
    manifest: Manifest,
    origin: BundleOrigin,
    etag: String,
}

impl Bundle {
    /// Open a bundle file (decompressed into memory, never unpacked to disk).
    pub fn open(path: &Path) -> anyhow::Result<Bundle> {
        let fs = tarfs::InMemoryFileSystem::open(path)
            .with_context(|| format!("reading style bundle {}", path.display()))?;
        let manifest: Manifest = serde_json::from_slice(
            fs.get(manifest::MANIFEST_PATH).context("bundle has no manifest.json")?,
        )
        .context("parsing bundle manifest.json")?;
        let etag = format!("{:x}", fs.len());
        Ok(Bundle { fs, manifest, origin: BundleOrigin::File(path.to_owned()), etag })
    }

    /// The embedded fallback bundle (minimal CSS for both flavors).
    pub fn fallback() -> Bundle {
        let mut fs = tarfs::InMemoryFileSystem::new();
        let mut manifest = Manifest::default();
        for flavor in Flavor::ALL {
            let name = format!("fallback-{flavor}.css");
            let (css, _) = crate::assets::get(&name).expect("embedded fallback css");
            let path = format!("{flavor}/fallback.css");
            fs.write_file(&path, Bytes::from_static(css)).expect("valid path");
            manifest.flavors.insert(
                flavor,
                FlavorManifest {
                    fallback: true,
                    stylesheets: vec![Stylesheet { path, role: "fallback".into(), ..Default::default() }],
                    themes: vec!["light".into(), "dark".into()],
                    ..Default::default()
                },
            );
        }
        Bundle { fs, manifest, origin: BundleOrigin::Fallback, etag: "fallback".into() }
    }

    /// Add fallback entries for flavors this bundle lacks.
    pub fn with_fallbacks(mut self) -> Bundle {
        let fb = Bundle::fallback();
        for flavor in Flavor::ALL {
            if !self.manifest.flavors.contains_key(&flavor) {
                let m = fb.manifest.flavors[&flavor].clone();
                for s in &m.stylesheets {
                    if let Some(b) = fb.read(&s.path) {
                        let _ = self.fs.write_file(&s.path, b);
                    }
                }
                self.manifest.flavors.insert(flavor, m);
            }
        }
        self
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn flavor(&self, f: Flavor) -> Option<&FlavorManifest> {
        self.manifest.flavors.get(&f)
    }

    /// Zero-copy read of a bundle file.
    pub fn read(&self, path: &str) -> Option<Bytes> {
        self.fs.read_file(path).ok()
    }

    /// Changes whenever the bundle content changes.
    pub fn etag(&self) -> &str {
        &self.etag
    }

    pub fn origin(&self) -> &BundleOrigin {
        &self.origin
    }
}

/// Default bundle location: `$MD_RENDER_STYLES`, else `<cache dir>/md-render/styles.tar.gz`.
pub fn default_bundle_path() -> PathBuf {
    if let Some(p) = std::env::var_os("MD_RENDER_STYLES") {
        return PathBuf::from(p);
    }
    directories::ProjectDirs::from("", "", "md-render")
        .map(|d| d.cache_dir().join("styles.tar.gz"))
        .unwrap_or_else(|| PathBuf::from("md-render-styles.tar.gz"))
}

/// Load a bundle, filling missing flavors with the fallback. Never fails; returns a warning for
/// the user when something is missing.
pub fn load_or_fallback(path: &Path) -> (Bundle, Option<String>) {
    match Bundle::open(path) {
        Ok(b) => (b.with_fallbacks(), None),
        Err(e) => (
            Bundle::fallback(),
            Some(format!("{e:#}; using minimal fallback styles (run `md-render vendor` to download site styles)")),
        ),
    }
}
