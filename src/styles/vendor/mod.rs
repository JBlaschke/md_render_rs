//! `md-render vendor` (owner: styles). Contract: docs/DESIGN.md §3.5, §4; procedures GH §4 and
//! GL §13.
//!
//! For each requested flavor:
//! 1. GET the source page and scrape its `<link rel=stylesheet>` tags (`href` and `data-href`,
//!    with `media`), classify them into roles by file-name stem ([`scrape`]);
//! 2. download every selected stylesheet (any failure is fatal), find each `url()` / `@import`
//!    ([`css`]), resolve it against the stylesheet URL, map it to a bundle path mirroring the
//!    site path ([`paths`]) and rewrite the reference to a *relative* bundle path (D10);
//! 3. download the referenced assets (and `@import`ed stylesheets, recursively) — failures are
//!    recorded in the manifest and are not fatal (GitHub's own CSS references a 404);
//! 4. with `libs`, download the pinned client libraries and GitHub's custom emoji ([`libs`]).
//!
//! Nothing touches the bundle until every flavor has been downloaded; then the bundle is
//! updated with one tarfs read-modify-write (D9) that replaces only the vendored flavors'
//! directories and manifest entries, and is flushed atomically.

mod css;
pub mod fetch;
pub mod libs;
mod paths;
mod scrape;

use std::collections::{BTreeMap, HashSet};
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use anyhow::{Context as _, bail};
use bytes::Bytes;
use url::Url;

pub use fetch::{Fetch, MAX_BODY_BYTES, Response, USER_AGENT, UreqFetcher};

use crate::flavor::Flavor;
use crate::styles::manifest::{MANIFEST_PATH, SCHEMA, role, script};
use crate::styles::{Asset, Failure, FlavorManifest, Manifest, Stylesheet};

/// Default page scraped for GitHub stylesheets.
pub const GITHUB_SOURCE: &str = "https://github.com/joeyespo/grip";
/// Default page scraped for GitLab stylesheets.
pub const GITLAB_SOURCE: &str = "https://gitlab.com/gitlab-org/gitlab-foss";

/// Maximum `@import` nesting followed below a vendored stylesheet.
const MAX_IMPORT_DEPTH: u8 = 4;
/// Parallel downloads.
const WORKERS: usize = 6;

/// Options of one vendoring run.
#[derive(Clone, Debug)]
pub struct VendorOptions {
    pub flavors: Vec<Flavor>,
    pub output: PathBuf,
    pub github_source: String,
    pub gitlab_source: String,
    /// Vendor all 14 GitHub themes instead of light + dark.
    pub all_github_themes: bool,
    /// Also vendor client libraries (MathJax / KaTeX / mermaid) and emoji images.
    pub libs: bool,
    pub quiet: bool,
}

impl Default for VendorOptions {
    /// Both flavors, default sources and bundle path, light + dark, with libraries.
    fn default() -> Self {
        VendorOptions {
            flavors: Flavor::ALL.to_vec(),
            output: crate::styles::default_bundle_path(),
            github_source: GITHUB_SOURCE.to_owned(),
            gitlab_source: GITLAB_SOURCE.to_owned(),
            all_github_themes: false,
            libs: true,
            quiet: false,
        }
    }
}

/// Summary of a vendoring run.
#[derive(Clone, Debug, Default)]
pub struct VendorReport {
    pub output: PathBuf,
    /// Files written for the vendored flavors (stylesheets + assets).
    pub files: usize,
    /// Total size of those files (uncompressed).
    pub bytes: u64,
    /// Non-fatal download failures of this run (also recorded in the manifest).
    pub failures: Vec<Failure>,
}

/// Download the requested flavors' styles into the bundle at `opts.output` (over HTTPS, with
/// [`UreqFetcher`]).
pub fn vendor(opts: &VendorOptions) -> anyhow::Result<VendorReport> {
    vendor_with(opts, &UreqFetcher::new())
}

/// [`vendor`] with a caller-supplied HTTP layer (tests use canned responses).
pub fn vendor_with(opts: &VendorOptions, fetcher: &dyn Fetch) -> anyhow::Result<VendorReport> {
    let progress = Progress { quiet: opts.quiet };
    let mut flavors: Vec<Flavor> = Vec::new();
    for f in &opts.flavors {
        if !flavors.contains(f) {
            flavors.push(*f);
        }
    }
    if flavors.is_empty() {
        bail!("no flavor selected to vendor");
    }
    if opts.output.as_os_str().is_empty() {
        bail!("no output file given for the style bundle");
    }

    let mut outputs = Vec::new();
    for &flavor in &flavors {
        let out = vendor_flavor(flavor, opts, fetcher, &progress).with_context(|| {
            let mut msg = format!(
                "vendoring {flavor} styles failed; {} was left unchanged",
                opts.output.display()
            );
            if flavors.len() > 1 {
                msg.push_str(" (use --flavor to vendor one flavor at a time)");
            }
            msg
        })?;
        outputs.push(out);
    }

    write_bundle(&opts.output, &outputs, &progress)?;

    let mut report = VendorReport { output: opts.output.clone(), ..Default::default() };
    for o in &outputs {
        report.files += o.files.len();
        report.bytes += o.files.values().map(|b| b.len() as u64).sum::<u64>();
        report.failures.extend(o.manifest.failures.iter().cloned());
    }
    let on_disk = std::fs::metadata(&opts.output).map(|m| m.len()).unwrap_or(0);
    progress.say(format_args!(
        "wrote {} ({} files, {} uncompressed, {} on disk{})",
        opts.output.display(),
        report.files,
        human_bytes(report.bytes),
        human_bytes(on_disk),
        match report.failures.len() {
            0 => String::new(),
            1 => ", 1 non-fatal failure".to_owned(),
            n => format!(", {n} non-fatal failures"),
        }
    ));
    Ok(report)
}

// ------------------------------------------------------------------------------------------------
// One flavor

/// What one flavor contributes to the bundle.
#[derive(Debug)]
struct FlavorOutput {
    flavor: Flavor,
    manifest: FlavorManifest,
    files: BTreeMap<String, Bytes>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Policy {
    /// Any failure aborts the run (the site's stylesheets).
    Fatal,
    /// Failures are recorded in `FlavorManifest::failures`.
    Record,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The `n`-th selected site stylesheet.
    Sheet(usize),
    /// A stylesheet that is not part of the cascade (`@import` target or library CSS).
    Css { depth: u8, lib: bool },
    /// Anything else, stored verbatim.
    Binary,
}

#[derive(Clone, Debug)]
struct Job {
    url: Url,
    path: String,
    kind: Kind,
    policy: Policy,
    referenced_by: Option<String>,
    script: Option<&'static str>,
}

struct FlavorRun<'a> {
    flavor: Flavor,
    fetcher: &'a dyn Fetch,
    progress: &'a Progress,
    mapper: paths::Mapper,
    queued: HashSet<String>,
    specs: Vec<scrape::SheetSpec>,
    sheets: Vec<Option<Stylesheet>>,
    files: BTreeMap<String, Bytes>,
    assets: Vec<Asset>,
    failures: Vec<Failure>,
    scripts: BTreeMap<String, String>,
}

fn parse_http_url(s: &str, what: &str) -> anyhow::Result<Url> {
    let u = Url::parse(s.trim()).with_context(|| format!("invalid {what} URL {s:?}"))?;
    if !matches!(u.scheme(), "http" | "https") {
        bail!("invalid {what} URL {s:?}: only http and https are supported");
    }
    Ok(u)
}

fn site_name(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Github => "GitHub",
        Flavor::Gitlab => "GitLab",
    }
}

fn vendor_flavor(
    flavor: Flavor,
    opts: &VendorOptions,
    fetcher: &dyn Fetch,
    progress: &Progress,
) -> anyhow::Result<FlavorOutput> {
    let fetched_at = rfc3339_utc(SystemTime::now());
    let source = match flavor {
        Flavor::Github => &opts.github_source,
        Flavor::Gitlab => &opts.gitlab_source,
    };
    let page_url = parse_http_url(source, &format!("{flavor} source page"))?;
    progress.say(format_args!("{flavor}: scraping stylesheets from {page_url}"));
    let resp = fetcher.get(page_url.as_str())?;
    if !resp.is_success() {
        bail!("GET {page_url}: HTTP {}", resp.status);
    }
    let final_url = Url::parse(&resp.final_url).unwrap_or_else(|_| page_url.clone());
    let html = String::from_utf8_lossy(&resp.body);
    let page = scrape::scrape_page(&html, &final_url);
    let classified = match flavor {
        Flavor::Github => scrape::classify_github(&page.links, opts.all_github_themes),
        Flavor::Gitlab => scrape::classify_gitlab(&page.links),
    };
    check_roles(flavor, &classified, &final_url, page.links.len(), progress)?;

    let mut run = FlavorRun {
        flavor,
        fetcher,
        progress,
        mapper: paths::Mapper::new(flavor.as_str()),
        queued: HashSet::new(),
        sheets: vec![None; classified.sheets.len()],
        specs: classified.sheets,
        files: BTreeMap::new(),
        assets: Vec::new(),
        failures: Vec::new(),
        scripts: BTreeMap::new(),
    };
    // The site's asset host (github.githubassets.com, gitlab.com) is mirrored at `<flavor>/`.
    let Some(first) = run.specs.first() else {
        bail!("{final_url} links no usable stylesheets");
    };
    let mut site_root = first.url.clone();
    site_root.set_path("/");
    site_root.set_query(None);
    run.mapper.mount(&site_root, flavor.as_str());

    // Fixed locations first, so site paths can never take them.
    let mut later = Vec::new();
    if opts.libs {
        for pkg in libs::packages(flavor) {
            let base = parse_http_url(&pkg.base, "library")?;
            run.mapper.mount(&base, &pkg.prefix);
            for f in &pkg.files {
                let url = parse_http_url(&pkg.url(f), "library")?;
                let path = run.mapper.reserve(&url, &pkg.path(f));
                let kind = if f.css { Kind::Css { depth: 0, lib: true } } else { Kind::Binary };
                later.push(run.job(url, path, kind, None, Some(f.script)));
            }
        }
        if flavor == Flavor::Github {
            let base = parse_http_url(libs::GITHUB_EMOJI_BASE, "emoji")?;
            for name in libs::GITHUB_CUSTOM_EMOJI {
                let url = base.join(&format!("{name}.png")).context("emoji URL")?;
                let path = run.mapper.reserve(&url, &libs::github_emoji_path(name));
                later.push(run.job(url, path, Kind::Binary, None, None));
            }
        }
    }
    let mut jobs = Vec::new();
    for i in 0..run.specs.len() {
        let url = run.specs[i].url.clone();
        let path = run.mapper.map(&url);
        let mut job = run.job(url, path, Kind::Sheet(i), None, None);
        job.policy = Policy::Fatal;
        jobs.push(job);
    }
    if flavor == Flavor::Gitlab {
        // The copy-code button's icon sprite (GL §10.4); scraped because its name is hashed.
        if let Some(sprite) = page.sprites.first() {
            let path = run.mapper.map(sprite);
            jobs.push(run.job(sprite.clone(), path, Kind::Binary, None, Some(script::ICONS)));
        }
    }
    jobs.extend(later);
    run.process(jobs)?;
    Ok(run.finish(fetched_at, final_url))
}

/// Fail when a stylesheet md-render cannot do without is missing; warn about optional ones.
fn check_roles(
    flavor: Flavor,
    c: &scrape::Classified,
    page: &Url,
    links: usize,
    progress: &Progress,
) -> anyhow::Result<()> {
    let (required, optional): (&[(&str, Option<&str>)], &[(&str, Option<&str>)]) = match flavor {
        Flavor::Github => (
            &[
                (role::THEME, Some("light")),
                (role::THEME, Some("dark")),
                (role::PRIMITIVES, None),
                (role::PRIMER, None),
                (role::GLOBAL, None),
            ],
            &[(role::GITHUB, None), (role::REPOSITORY, None), (role::CODE, None)],
        ),
        Flavor::Gitlab => (
            &[(role::APP, Some("light")), (role::HIGHLIGHT, Some("light"))],
            &[
                (role::APP, Some("dark")),
                (role::TAILWIND, None),
                (role::FONTS, None),
                (role::HIGHLIGHT, Some("dark")),
            ],
        ),
    };
    let describe = |(r, t): &(&str, Option<&str>)| match t {
        Some(t) => format!("{r} ({t})"),
        None => (*r).to_owned(),
    };
    let missing: Vec<String> =
        required.iter().filter(|(r, t)| !c.has(r, *t)).map(describe).collect();
    if !missing.is_empty() {
        bail!(
            "{page} does not link the {} stylesheet(s) md-render needs (it has {links} stylesheet \
             links). The page layout may have changed, or the --{flavor}-source page is not a {} \
             repository page.",
            missing.join(", "),
            site_name(flavor)
        );
    }
    for o in optional.iter().filter(|(r, t)| !c.has(r, *t)) {
        progress.say(format_args!(
            "  warning: {page} has no {} stylesheet; continuing without it",
            describe(o)
        ));
    }
    Ok(())
}

impl FlavorRun<'_> {
    fn job(
        &mut self,
        url: Url,
        path: String,
        kind: Kind,
        referenced_by: Option<String>,
        script: Option<&'static str>,
    ) -> Job {
        self.queued.insert(path.clone());
        Job { url, path, kind, policy: Policy::Record, referenced_by, script }
    }

    /// Download in waves: the fatal jobs (site stylesheets) first, so a broken site fails before
    /// megabytes of libraries are fetched; then everything else, plus whatever the downloaded
    /// stylesheets reference, until nothing is left.
    fn process(&mut self, mut queue: Vec<Job>) -> anyhow::Result<()> {
        while !queue.is_empty() {
            let wave: Vec<Job> = if queue.iter().any(|j| j.policy == Policy::Fatal) {
                let (fatal, rest): (Vec<Job>, Vec<Job>) =
                    queue.into_iter().partition(|j| j.policy == Policy::Fatal);
                queue = rest;
                fatal
            } else {
                std::mem::take(&mut queue)
            };
            let urls: Vec<&str> = wave.iter().map(|j| j.url.as_str()).collect();
            let results = fetch_all(self.fetcher, &urls);
            for (job, result) in wave.into_iter().zip(results) {
                self.complete(job, result, &mut queue)?;
            }
        }
        Ok(())
    }

    fn complete(
        &mut self,
        job: Job,
        result: anyhow::Result<Response>,
        next: &mut Vec<Job>,
    ) -> anyhow::Result<()> {
        let resp = match result {
            Ok(r) if r.is_success() => r,
            Ok(r) => return self.fail(&job, Some(r.status), format!("HTTP {}", r.status)),
            Err(e) => return self.fail(&job, None, format!("{e:#}")),
        };
        let bytes = match job.kind {
            Kind::Binary => Bytes::from(resp.body),
            Kind::Sheet(_) | Kind::Css { .. } => {
                if resp.mime().as_deref() == Some("text/html") {
                    return self.fail(
                        &job,
                        Some(resp.status),
                        "expected a stylesheet but got an HTML page".to_owned(),
                    );
                }
                let text = match decode_css(resp.body) {
                    Ok(t) => t,
                    Err(e) => return self.fail(&job, Some(resp.status), e),
                };
                let base = Url::parse(&resp.final_url).unwrap_or_else(|_| job.url.clone());
                Bytes::from(self.process_css(&text, &base, &job, next))
            }
        };
        let sha256 = paths::sha256_hex(&bytes);
        let size = bytes.len() as u64;
        match job.kind {
            Kind::Sheet(i) => {
                let spec = &self.specs[i];
                self.sheets[i] = Some(Stylesheet {
                    path: job.path.clone(),
                    url: job.url.to_string(),
                    role: spec.role.to_owned(),
                    theme: spec.theme.clone(),
                    media: spec.media.clone(),
                    sha256,
                    size,
                });
            }
            Kind::Css { .. } | Kind::Binary => self.assets.push(Asset {
                path: job.path.clone(),
                url: job.url.to_string(),
                sha256,
                size,
            }),
        }
        if let Some(key) = job.script {
            self.scripts.insert(key.to_owned(), job.path.clone());
        }
        self.progress.say(format_args!("  {:>3} {:>10}  {}", resp.status, human_bytes(size), job.path));
        self.files.insert(job.path, bytes);
        Ok(())
    }

    fn fail(&mut self, job: &Job, status: Option<u16>, error: String) -> anyhow::Result<()> {
        if job.policy == Policy::Fatal {
            bail!("downloading stylesheet {} failed: {error}", job.url);
        }
        self.progress.say(format_args!(
            "  warning: {} failed: {error}{}",
            job.url,
            job.referenced_by.as_deref().map(|r| format!(" (referenced by {r})")).unwrap_or_default()
        ));
        self.failures.push(Failure {
            url: job.url.to_string(),
            status,
            error,
            referenced_by: job.referenced_by.clone(),
        });
        Ok(())
    }

    /// Rewrite every external reference of the stylesheet `job` (fetched from `base`) to a
    /// relative bundle path and queue the referenced files.
    fn process_css(&mut self, text: &str, base: &Url, job: &Job, next: &mut Vec<Job>) -> String {
        let refs = css::scan(text);
        let (depth, lib) = match job.kind {
            Kind::Css { depth, lib } => (depth, lib),
            _ => (0, false),
        };
        let mapper = &mut self.mapper;
        let queued = &mut self.queued;
        let failures = &mut self.failures;
        css::rewrite(text, &refs, |r| {
            let value = r.value.trim();
            // data: URIs and same-document fragments (`url(#filter)`) stay as they are.
            if value.is_empty()
                || value.starts_with('#')
                || value.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("data:"))
            {
                return None;
            }
            let mut target = base.join(value).ok()?;
            if !matches!(target.scheme(), "http" | "https") {
                return None;
            }
            let fragment = target.fragment().map(str::to_owned);
            target.set_fragment(None);
            let path = mapper.map(&target);
            let mut rel = paths::relative_path(&job.path, &path);
            if let Some(f) = fragment {
                rel.push('#');
                rel.push_str(&f);
            }
            let wanted = !lib || libs::want_lib_css_asset(&path);
            if wanted && queued.insert(path.clone()) {
                let kind = match r.kind {
                    css::RefKind::Url => Some(Kind::Binary),
                    css::RefKind::Import if depth < MAX_IMPORT_DEPTH => {
                        Some(Kind::Css { depth: depth + 1, lib })
                    }
                    css::RefKind::Import => None,
                };
                match kind {
                    Some(kind) => next.push(Job {
                        url: target,
                        path,
                        kind,
                        policy: Policy::Record,
                        referenced_by: Some(job.path.clone()),
                        script: None,
                    }),
                    None => failures.push(Failure {
                        url: target.to_string(),
                        status: None,
                        error: format!("@import nested more than {MAX_IMPORT_DEPTH} levels deep"),
                        referenced_by: Some(job.path.clone()),
                    }),
                }
            }
            Some(rel)
        })
    }

    fn finish(mut self, fetched_at: String, source_page: Url) -> FlavorOutput {
        let stylesheets: Vec<Stylesheet> = self.sheets.into_iter().flatten().collect();
        let themes = match self.flavor {
            Flavor::Github => stylesheets
                .iter()
                .filter(|s| s.role == role::THEME)
                .filter_map(|s| s.theme.clone())
                .collect(),
            Flavor::Gitlab => ["light", "dark"]
                .into_iter()
                .filter(|t| {
                    stylesheets.iter().any(|s| s.role == role::APP && s.theme.as_deref() == Some(t))
                })
                .map(str::to_owned)
                .collect(),
        };
        self.assets.sort_by(|a, b| a.path.cmp(&b.path));
        self.failures.sort_by(|a, b| a.url.cmp(&b.url).then(a.referenced_by.cmp(&b.referenced_by)));
        self.progress.say(format_args!(
            "{}: {} stylesheets, {} other files, {} non-fatal failures",
            self.flavor,
            stylesheets.len(),
            self.assets.len(),
            self.failures.len()
        ));
        FlavorOutput {
            flavor: self.flavor,
            manifest: FlavorManifest {
                fetched_at,
                source_page: source_page.to_string(),
                user_agent: self.fetcher.user_agent().to_owned(),
                fallback: false,
                stylesheets,
                assets: self.assets,
                scripts: self.scripts,
                themes,
                failures: self.failures,
            },
            files: self.files,
        }
    }
}

/// Stylesheets must be UTF-8 (both sites serve UTF-8); a BOM is dropped.
fn decode_css(body: Vec<u8>) -> Result<String, String> {
    let mut s = String::from_utf8(body).map_err(|e| format!("stylesheet is not valid UTF-8 ({e})"))?;
    if s.starts_with('\u{feff}') {
        s.drain(..3);
    }
    Ok(s)
}

/// GET all `urls` with up to [`WORKERS`] parallel requests; results in input order.
fn fetch_all(fetcher: &dyn Fetch, urls: &[&str]) -> Vec<anyhow::Result<Response>> {
    if urls.len() <= 1 {
        return urls.iter().map(|u| fetcher.get(u)).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<anyhow::Result<Response>>>> =
        urls.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(urls.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(url) = urls.get(i) else { break };
                    let result = fetcher.get(url);
                    *slots[i].lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(result);
                }
            });
        }
    });
    slots
        .into_iter()
        .zip(urls)
        .map(|(slot, url)| {
            slot.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .unwrap_or_else(|| Err(anyhow::anyhow!("GET {url}: download did not run")))
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// Writing the bundle

/// Decide whether the existing `output` can be read-modify-written. Refuses to overwrite files
/// that are not md-render bundles (a typo in `-o` must not destroy unrelated data), except a
/// corrupt bundle at the default cache location, which is simply replaced.
fn existing_source<'p>(output: &'p Path, progress: &Progress) -> anyhow::Result<Option<&'p Path>> {
    let meta = match std::fs::metadata(output) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("checking {}", output.display())),
    };
    if !meta.is_file() {
        bail!("{} exists and is not a regular file", output.display());
    }
    match tarfs::InMemoryFileSystem::open(output) {
        Ok(fs) if fs.contains(MANIFEST_PATH) || fs.is_empty() => Ok(Some(output)),
        Ok(_) => bail!(
            "refusing to overwrite {}: it is a tar.gz archive but not an md-render style bundle \
             (no {MANIFEST_PATH}); delete it or choose another output file",
            output.display()
        ),
        Err(e) if output == crate::styles::default_bundle_path() => {
            progress.say(format_args!(
                "  warning: the existing bundle {} is unreadable ({e}); replacing it",
                output.display()
            ));
            Ok(None)
        }
        Err(e) => bail!(
            "refusing to overwrite {}: it is not a readable md-render style bundle ({e}); delete \
             it or choose another output file",
            output.display()
        ),
    }
}

/// The manifest already in the bundle (or a fresh one).
fn existing_manifest(
    fs: &mut tarfs::InMemoryFileSystem,
    output: &Path,
    progress: &Progress,
) -> anyhow::Result<Manifest> {
    let Some(raw) = fs.get(MANIFEST_PATH) else { return Ok(Manifest::default()) };
    match serde_json::from_slice::<Manifest>(raw) {
        Ok(m) if m.schema == SCHEMA => Ok(m),
        Ok(m) if m.schema > SCHEMA => bail!(
            "{} was written by a newer md-render (bundle schema {}, this version writes schema \
             {SCHEMA}); refusing to downgrade it",
            output.display(),
            m.schema
        ),
        other => {
            let why = match other {
                Ok(m) => format!("old bundle schema {}", m.schema),
                Err(e) => format!("unreadable {MANIFEST_PATH}: {e}"),
            };
            progress.say(format_args!(
                "  warning: {} has an {why}; starting a fresh bundle",
                output.display()
            ));
            let all: Vec<String> = fs.paths().map(str::to_owned).collect();
            for p in all {
                fs.delete_file(&p);
            }
            Ok(Manifest::default())
        }
    }
}

fn write_bundle(output: &Path, outputs: &[FlavorOutput], progress: &Progress) -> anyhow::Result<()> {
    if let Some(dir) = output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let from = existing_source(output, progress)?;
    tarfs::open_tarball_gz(output, from, |fs| -> anyhow::Result<()> {
        let mut manifest = existing_manifest(fs, output, progress)?;
        for o in outputs {
            fs.delete_prefix(o.flavor.as_str());
            for (path, bytes) in &o.files {
                fs.write_file(path, bytes.clone()).with_context(|| format!("adding {path}"))?;
            }
            manifest.flavors.insert(o.flavor, o.manifest.clone());
        }
        // Keep the other flavors only if they are intact (a damaged entry would make the whole
        // bundle fail validation when servers open it).
        let kept: Vec<Flavor> = manifest
            .flavors
            .keys()
            .copied()
            .filter(|f| !outputs.iter().any(|o| o.flavor == *f))
            .collect();
        for flavor in kept {
            let ok = manifest
                .flavors
                .get(&flavor)
                .is_some_and(|fm| crate::styles::validate_flavor(flavor, fm, |p| fs.contains(p)).is_ok());
            if !ok {
                progress.say(format_args!(
                    "  warning: dropping the incomplete {flavor} styles from {} (run `md-render \
                     vendor --flavor {flavor}` to restore them)",
                    output.display()
                ));
                manifest.flavors.remove(&flavor);
                fs.delete_prefix(flavor.as_str());
            }
        }
        manifest.schema = SCHEMA;
        manifest.tool = crate::TOOL.to_owned();
        crate::styles::validate_manifest(&manifest, |p| fs.contains(p))?;
        let json = serde_json::to_vec_pretty(&manifest).context("serializing manifest.json")?;
        fs.write_file(MANIFEST_PATH, json)?;
        Ok(())
    })
    .with_context(|| format!("writing style bundle {}", output.display()))?;
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// Small helpers

/// Progress lines on stderr unless quiet.
struct Progress {
    quiet: bool,
}

impl Progress {
    fn say(&self, msg: impl Display) {
        if !self.quiet {
            eprintln!("{msg}");
        }
    }
}

/// `1234567` → `1.2 MB`.
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u + 1 < UNITS.len() {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

/// RFC 3339 UTC timestamp with second precision, e.g. `2026-10-01T10:00:00Z`.
fn rfc3339_utc(t: SystemTime) -> String {
    let secs = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    };
    let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", sod / 3600, sod % 3600 / 60, sod % 60)
}

#[cfg(test)]
mod tests;
