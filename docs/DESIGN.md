# md-render — design

`md-render` is a grip-like local Markdown previewer written in Rust:

1. renders **GitHub-** or **GitLab-**flavoured output (`--flavor github|gitlab`), offline by
   default (comrak + post-processing that reproduces each site's HTML), optionally via the site's
   render API (`--api`);
2. **vendors** each site's real stylesheets (plus the fonts/images they reference and the client
   libraries for math/diagrams) into a single `.tar.gz` bundle, which running servers read **in
   place** through the `tarfs` sub-crate (a Rust port of
   [TarFS.jl](https://github.com/JBlaschke/TarFS.jl)); nothing is ever unpacked to disk;
3. **live-updates** the page as the source changes (file watcher → Server-Sent Events → DOM
   reconciliation that keeps scroll position and client-rendered content).

Research backing every decision lives in `.dev/research/` (not committed):
`github.md`, `gitlab.md`, `grip.md`, `crates.md`, `critique.md`, plus restored artifacts under
`.dev/research/{github,gitlab}/` (see each `MANIFEST*`). Section references below (e.g. "GH §9")
point into those files: **GH** = github.md, **GL** = gitlab.md, **GRIP** = grip.md,
**CR** = crates.md, **CQ** = critique.md.

---------------------------------------------------------------------------------------------------

## 1. Workspace layout and ownership

```
Cargo.toml                    workspace + package `md-render` (lib + bin)
crates/tarfs/                 TarFS.jl port — no dependency on md-render              [tarfs]
src/main.rs                   -> md_render::cli::main()                               [skeleton]
src/lib.rs                    module tree                                             [skeleton]
src/flavor.rs                 Flavor, Mode, Theme, ThemeSel (shared enums)            [skeleton, frozen]
src/assets.rs                 embedded static files registry (include_bytes!)          [skeleton]
src/render/mod.rs             RenderOptions, Rendered, Renderer trait, dispatch       [skeleton, frozen API]
src/render/github/**          GitHub offline pipeline                                 [render-github]
src/render/gitlab/**          GitLab offline pipeline (port of glfm gem + Banzai)     [render-gitlab]
src/render/highlight/**       syntect/two-face tokenizer + per-site class schemes     [highlight]
src/render/api.rs             --api renderers (GitHub /markdown, GitLab /api/v4)      [cli]
src/styles/mod.rs             Bundle (tarfs-backed), load/fallback/hot-swap helpers   [styles]
src/styles/manifest.rs        bundle manifest serde types                              [skeleton, frozen schema]
src/styles/vendor/**          `md-render vendor`: scrape → download → rewrite → bundle [styles]
src/page/**                   full-page HTML (chrome per flavor/mode/theme), listing,
                              error fragments, export                                  [page]
src/export.rs                 `md-render export` (uses page + render)                  [page]
src/server/**                 axum server, routes, SSE live updates, watcher, fs safety [server]
src/cli/**                    clap CLI, grip path/address rules, dispatch               [cli]
assets/client/live.js         live-update client (SSE, reconcile, scroll anchor)      [server]
assets/client/github.js       GitHub page behaviours (hash nav, clipboard, MathJax, mermaid) [page]
assets/client/gitlab.js       GitLab page behaviours (hash nav, copy-code, KaTeX, mermaid, auto dark) [page]
assets/fallback/*.css         minimal MIT fallback CSS when no bundle                  [page]
assets/favicon.svg            favicon                                                  [skeleton]
data/github/**                embedded data (Linguist subset, gemoji subset, octicons) [highlight / render-github]
data/gitlab/**                embedded data (Rouge registry, emoji subset)              [highlight / render-gitlab]
tests/                        integration + golden tests (fixtures in tests/fixtures/)  [each owner]
THIRD_PARTY_NOTICES.md        attributions for ported code and embedded data           [each owner appends]
```

Rules for implementers working in parallel:
* Touch only the files you own (bracketed tag). Shared files (`Cargo.toml`, `THIRD_PARTY_NOTICES.md`,
  `src/assets.rs`) may receive **append-only** edits (one dependency per line, alphabetical within
  its group; one notice section per owner).
* The **frozen** APIs in §3 are the contract between owners. If you truly need to change one, keep
  the old signature compiling and say so in your report.
* Every module ships with unit tests; every owner adds integration tests under `tests/`.
* No network access in `cargo test` by default. Tests that need the network are `#[ignore]` and
  documented.

---------------------------------------------------------------------------------------------------

## 2. Decisions (resolving the research contradictions, CQ §2)

| # | Decision |
|---|---|
| D1 | `comrak = "=0.55.0"` (pinned: GitLab's gem formatter tracks comrak internals; `tagfilter` is gone in 0.56). |
| D2 | GitHub flavor follows **CQ §3.1** pipeline: normalize → (readme) front-matter split → comrak parse (GFM: strikethrough, table, autolink, tasklist, footnotes, math_code, `render.unsafe`, `render.sourcepos`) → AST passes (tagfilter on raw HTML, alert detection) → neutral `create_formatter!` HTML → **dom_query DOM** → sanitize (GitHub raw allowlist ∪ generated attributes) → DOM filters (headings/slugs, footnotes, tasks, alerts+octicons, dollar math, ```math, emoji, code highlight, tables, images, links, `dir="auto"`, id/name prefixing) → serialize via `doc.html_root().inner_html()`. Output contract **GH §9** (`readme` and `comment` contexts). |
| D3 | GitLab flavor follows **GL §2–§6**: front-matter pre-filter (GL §3.2) → `::include` (GL §3.3, confined to the served root) → comrak 0.55 with GitLab's exact option set + **ported** `formatter.rs`/`glfm.rs` (MIT © GitLab B.V., keep notice) → DOM filters ported from `.dev/research/gitlab/tools/glpost.py` in GL's order. Emit the **post-JavaScript** DOM (no lazy images, `code-syntax-highlight-theme` on highlighted `pre`, table wrapper). |
| D4 | One DOM engine: `dom_query 0.28` (html5ever 0.39). No `lol_html`, no `ammonia` (their context rules can't express GitLab's sanitizer; CQ §1.2). Sanitizers are DOM passes. |
| D5 | Highlighting: `syntect 5.3` (`parsing`, `regex-fancy`) + `two-face 0.5.2` syntaxes. GitHub: Linguist lookup (GH §7.2, CQ §1.7 alias table) → `highlight-<tm_scope with . → ->` wrapper + PrettyLights scope→`pl-*` map (GH §7.5/7.6, longest dot-prefix, nested spans, close/reopen at newlines, skip root scope). GitLab: Rouge registry lookup (GL §8.1) → `language-<tag>` + per-line `span#LCn.line` + scope→Rouge short-class rules (GL §8.4, `scope_map.py` RULES; `console` and `diff` special cases). Unknown language: wrapper still emitted, no token spans. |
| D6 | Emoji: GitHub = gemoji data, plain Unicode, 26 names wrapped in `<g-emoji>`, 23 custom → `<img class="emoji">` pointing into the bundle (`github/emoji/<name>.png`), first-match-decides boundary rule (GH §8.5 + CQ §1.6). GitLab = `<gl-emoji>` with data from GitLab `emojis.json` (GL §5.13). Both datasets are embedded as compact JSON under `data/` (MIT; attributed). |
| D7 | Slugs: own implementations. GitHub GH §6.2 (verified 473/473). GitLab = comrak 0.55 anchorizer semantics via the ported formatter (verified 147/147). Never use comrak's `HeadingAdapter`. |
| D8 | Math: GitHub → `<math-renderer>` markup + client MathJax **3.2.0** `es5/tex-chtml-full.js` + `es5/ui/safe.js`, synchronous `tex2mml` (CQ §1.4). GitLab → `js-render-math` markup + client KaTeX **0.16.47** (GL §5.4/§10). Mermaid client-side: GitHub 11.17.2, GitLab 11.16.1 (each flavor vendors its own). |
| D9 | **One bundle file** holds both flavors: default `<cache>/md-render/styles.tar.gz` (`directories` `ProjectDirs::from("", "", "md-render").cache_dir()`), override `--styles FILE` / env `MD_RENDER_STYLES`. Vendoring one flavor is a TarFS **read-modify-write** (`tarfs::open_tarball_gz(out, Some(out), …)`: delete `<flavor>/…`, write new files, update `manifest.json`), flushed atomically, so running servers never see a partial bundle. Layout §4. |
| D10 | `url()` references inside vendored CSS are rewritten to **relative** paths inside the bundle (mirroring site paths), so the bundle can be mounted under any prefix and exported. |
| D11 | Missing bundle (or missing flavor in it): print the exact `md-render vendor --flavor X` command and render with the embedded fallback CSS (`assets/fallback/<flavor>.css`, MIT). No implicit network access. `--auto-vendor` opts into vendoring on startup. |
| D12 | Internal URL namespace `/__/md-render/` (§6). |
| D13 | Live updates: SSE wire format of GRIP §11.1 (named events, JSON payloads, `id: <server_id>:<version>`, `retry: 1000`, keepalive), implemented with per-document `tokio::sync::watch`/`broadcast` + `take_while(shutdown)`; watcher per CR §5 (notify 8.2 `RecommendedWatcher`, parent-directory non-recursive watches, ref-counted, `notify-debouncer-mini` 50 ms; `--poll[=MS]` → `PollWatcher`). Client applies content with **content-keyed top-level block reconciliation** (§7). |
| D14 | Served root = the directory given on the CLI or the file's parent directory; `--root DIR` widens it (e.g. to the git top-level). Repo-root-relative links (`/docs/x.md`) resolve against the root. Path safety per GRIP C39 (no `..`, canonical path must stay under the canonical root, symlink escape blocked unless `--follow-symlinks`, `.git/` denied). Host-header allowlist when bound to loopback. |
| D15 | `--api`: GitHub `POST /markdown` `mode=gfm` (+`context`), then the GitHub DOM filters that are idempotent on API HTML (heading wrappers for readme mode, octicon SVG for `span.octicon-link`, `data-canonical-src` handling); `<br>` on soft breaks in readme mode is a documented difference (CQ §1.5). GitLab `POST /api/v4/markdown` `{text, gfm:true, project}` with `PRIVATE-TOKEN`; un-lazy images, add `code-syntax-highlight-theme`. Tokens: `--token` > `GITHUB_TOKEN`/`GH_TOKEN`/`gh auth token` (GitHub) or `GITLAB_TOKEN`/`glab auth token`-style fallback (GitLab); never printed or logged. On 401/403/429 fall back to offline rendering with an in-page banner. |
| D16 | GitLab `--mode comment`: same renderer output (no `::include`), page chrome = note body (`div.note-text.md`); documented as approximate. |
| D17 | Licensing: project `BSD-3-Clause` (same as TarFS.jl; trivially changeable). Ported/embedded third-party material keeps its MIT notices in `THIRD_PARTY_NOTICES.md`. Vendored site CSS/JS is downloaded by the user and never shipped in the binary. |
| D18 | Golden tests use real site output for public files (fixtures copied into `tests/fixtures/<flavor>/` with provenance + license in `tests/fixtures/<flavor>/FIXTURES.md`), compared with a normalizing DOM diff (CQ §3.7). |

---------------------------------------------------------------------------------------------------

## 3. Frozen interfaces

### 3.1 `src/flavor.rs`
```rust
pub enum Flavor { Github, Gitlab }             // clap ValueEnum + serde lowercase; Flavor::ALL; as_str()
pub enum Mode { Readme, Comment }              // clap ValueEnum; "readme" | "comment"
pub enum Theme { Auto, Light, Dark }           // clap ValueEnum
pub struct ThemeSel { pub theme: Theme, pub light: String, pub dark: String } // GitHub theme ids, default "light"/"dark"
```

### 3.2 `src/render/mod.rs`
```rust
pub struct RenderOptions {
    pub flavor: Flavor,
    pub mode: Mode,
    pub context: Option<String>,      // owner/repo | group/project (API mode; optional offline refs)
    pub source_path: Option<PathBuf>, // the .md on disk (None for stdin) — GitLab ::include, diagnostics
    pub root: Option<PathBuf>,        // served root; ::include must stay inside it
    pub sourcepos: bool,              // emit data-sourcepos on block elements (server: true; export: false)
    pub bundle_url: String,           // URL prefix of bundle files, e.g. "/__/md-render/bundle" (custom emoji etc.)
}
pub struct Rendered {
    pub html: String,                 // inner HTML of the content root (#md-content)
    pub title: Option<String>,        // plain text of the first h1, if any
    pub features: Features,           // which client libraries the page needs
    pub warnings: Vec<String>,        // shown as an in-page banner
}
pub struct Features { pub math: bool, pub mermaid: bool }
pub trait Renderer: Send + Sync {
    fn render(&self, markdown: &str, opts: &RenderOptions) -> anyhow::Result<Rendered>;
    fn name(&self) -> &'static str;
}
pub struct OfflineRenderer;                                    // impl Renderer -> Ok(render(..))
pub fn render(markdown: &str, opts: &RenderOptions) -> Rendered; // dispatch: github::render / gitlab::render
// owned by flavor agents:
pub fn github::render(markdown: &str, opts: &RenderOptions) -> Rendered;
pub fn gitlab::render(markdown: &str, opts: &RenderOptions) -> Rendered;
```
Both renderers are pure and deterministic (same input → same bytes; e.g. footnote/math run ids
derived from content, GH §8.4), never panic on any input (fuzz-ish tests), and must be fast
(steady state < 10 ms for a 20 KB README in release).

### 3.3 `src/render/highlight/mod.rs`
```rust
pub struct GithubCode {
    pub scope: Option<String>,        // Linguist tm_scope, e.g. "source.rust" (None: unknown language)
    pub html: Option<String>,         // escaped code with pl-* spans, lines joined by '\n', no trailing '\n' (None: no grammar)
}
pub fn github(code: &str, lang: &str) -> GithubCode;     // lang = info-string language word, as written
pub struct GitlabCode {
    pub tag: String,                  // Rouge tag ("rust", "javascript", …) or "plaintext"
    pub lines: Vec<String>,           // per source line: escaped HTML with Rouge short-class spans
}
pub fn gitlab(code: &str, lang: &str) -> GitlabCode;
```
Both must be thread-safe (lazy `OnceLock` syntax set / tables) and never panic.

### 3.4 `src/styles/manifest.rs` (bundle `manifest.json`, schema 1)
```rust
pub struct Manifest { pub schema: u32, pub tool: String, pub flavors: BTreeMap<Flavor, FlavorManifest> }
pub struct FlavorManifest {
    pub fetched_at: String,            // RFC 3339 UTC
    pub source_page: String,           // page scraped for <link rel=stylesheet>
    pub user_agent: String,
    pub fallback: bool,                // true = embedded fallback, not vendored
    pub stylesheets: Vec<Stylesheet>,  // in cascade order
    pub assets: Vec<Asset>,            // url() targets, emoji PNGs, …
    pub scripts: BTreeMap<String, String>, // "mathjax" | "mathjax-safe" | "mermaid" | "katex" | "katex-css" -> bundle path
    pub themes: Vec<String>,           // theme ids available (GitHub: "light","dark",…; GitLab: "light","dark")
    pub failures: Vec<Failure>,        // non-fatal download failures
}
pub struct Stylesheet { pub path: String, pub url: String, pub role: String, pub theme: Option<String>,
                        pub media: Option<String>, pub sha256: String, pub size: u64 }
pub struct Asset { pub path: String, pub url: String, pub sha256: String, pub size: u64 }
pub struct Failure { pub url: String, pub status: Option<u16>, pub error: String, pub referenced_by: Option<String> }
```
Roles — GitHub: `theme` (with `theme` id), `primitives`, `primer`, `global`, `github`, `repository`,
`code`. GitLab: `app` (theme light/dark), `tailwind`, `fonts`, `highlight` (theme light/dark).
Fallback: `fallback`. All `path`s are bundle paths (`github/…`, `gitlab/…`).

### 3.5 `src/styles/mod.rs`
```rust
pub struct Bundle { /* tarfs::InMemoryFileSystem + Manifest + origin + etag */ }
pub enum BundleOrigin { File(PathBuf), Fallback }
impl Bundle {
    pub fn open(path: &Path) -> anyhow::Result<Bundle>;    // tarfs open + manifest parse + validation
    pub fn fallback() -> Bundle;                           // embedded fallback CSS for both flavors
    pub fn with_fallbacks(self) -> Bundle;                 // add fallback entries for flavors the bundle lacks
    pub fn manifest(&self) -> &Manifest;
    pub fn flavor(&self, f: Flavor) -> Option<&FlavorManifest>;
    pub fn read(&self, path: &str) -> Option<bytes::Bytes>; // zero-copy
    pub fn etag(&self) -> &str;                            // changes whenever content changes
    pub fn origin(&self) -> &BundleOrigin;
}
pub fn default_bundle_path() -> PathBuf;                    // env MD_RENDER_STYLES, else cache dir
pub fn load_or_fallback(path: &Path) -> (Bundle, Option<String>); // never fails; warning text for the user
// src/styles/vendor/mod.rs
pub struct VendorOptions { pub flavors: Vec<Flavor>, pub output: PathBuf, pub github_source: String,
    pub gitlab_source: String, pub all_github_themes: bool, pub libs: bool, pub quiet: bool }
pub struct VendorReport { pub output: PathBuf, pub files: usize, pub bytes: u64, pub failures: Vec<Failure> }
pub fn vendor(opts: &VendorOptions) -> anyhow::Result<VendorReport>;
```

### 3.6 `src/page/mod.rs`
```rust
pub enum Layout { Readme, Bare }                 // Readme: box with file-name header (GitHub/GitLab chrome); Bare: content only
pub enum AssetMode { Server { prefix: String },  // "/__/md-render": links to /__/md-render/bundle/… and /static/…
                     Inline,                     // export: CSS inlined, url() → data: URIs, libs inlined if needed
                     Directory { href: String, dir: PathBuf } } // export --no-inline: files copied to dir, linked via href
pub struct LiveInfo { pub events_url: String, pub render_url: String, pub doc: String, pub version: u64, pub server_id: String }
pub enum BannerKind { Error, Warning, Info }
pub struct Banner { pub kind: BannerKind, pub message: String }
pub struct PageInput<'a> {
    pub flavor: Flavor, pub mode: Mode, pub theme: &'a ThemeSel, pub layout: Layout,
    pub title: &'a str,                 // <title>
    pub file_name: Option<&'a str>,     // box header text (readme layout)
    pub content: &'a Rendered,
    pub assets: &'a AssetMode,
    pub live: Option<&'a LiveInfo>,
    pub banners: &'a [Banner],
    pub extra_css: &'a [String],        // extra stylesheet URLs (after the bundle CSS)
}
pub fn render_page(input: &PageInput, bundle: &Bundle) -> anyhow::Result<String>;
pub struct ListingEntry { pub name: String, pub href: String, pub is_dir: bool, pub size: Option<u64> }
pub fn listing_html(flavor: Flavor, dir: &str, entries: &[ListingEntry]) -> String; // content fragment
pub fn error_html(flavor: Flavor, status: u16, message: &str) -> String;           // content fragment
```

### 3.7 `src/assets.rs`
`pub fn get(name: &str) -> Option<(&'static [u8], &'static str /*mime*/)>` for
`live.js`, `github.js`, `gitlab.js`, `favicon.svg`, `fallback-github.css`, `fallback-gitlab.css`.

### 3.8 `src/server/mod.rs`
```rust
pub enum Target { File(PathBuf), Dir(PathBuf), Stdin { name: String, text: String } } // paths relative to root
pub struct ServeConfig {
    pub root: PathBuf, pub target: Target, pub host: String, pub port: u16,
    pub flavor: Flavor, pub mode: Mode, pub theme: ThemeSel, pub layout: Layout,
    pub title: Option<String>, pub context: Option<String>,
    pub live: bool, pub open_browser: bool, pub quiet: bool,
    pub bundle_path: PathBuf, pub auto_vendor: bool, pub extra_css: Vec<String>,
    pub renderer: Arc<dyn Renderer>, pub poll: Option<Duration>, pub follow_symlinks: bool,
}
pub struct RunningServer { pub addr: SocketAddr, /* shutdown handle */ }
impl RunningServer { pub async fn shutdown(self); pub async fn wait(self) -> anyhow::Result<()>; }
pub async fn start(cfg: ServeConfig) -> anyhow::Result<RunningServer>; // binds, returns immediately
pub async fn serve(cfg: ServeConfig) -> anyhow::Result<()>;             // start + Ctrl-C + wait
```

### 3.9 `src/export.rs`
```rust
pub enum ExportInput { File(PathBuf), Stdin { name: String, text: String } }
pub enum ExportOutput { File(PathBuf), Stdout }
pub struct ExportConfig { pub input: ExportInput, pub output: ExportOutput, pub inline: bool,
    pub flavor: Flavor, pub mode: Mode, pub theme: ThemeSel, pub layout: Layout,
    pub title: Option<String>, pub context: Option<String>, pub bundle_path: PathBuf,
    pub renderer: Arc<dyn Renderer>, pub quiet: bool }
pub fn export(cfg: &ExportConfig) -> anyhow::Result<()>;
```

### 3.10 `src/render/api.rs`
```rust
pub struct ApiConfig { pub flavor: Flavor, pub api_url: Option<String>, pub token: Option<String>, pub fallback_offline: bool }
pub struct ApiRenderer { /* ureq agent, resolved token, cache keyed by sha256(flavor,mode,context,text) */ }
impl ApiRenderer { pub fn new(cfg: ApiConfig) -> anyhow::Result<Self>; }
impl Renderer for ApiRenderer { /* … */ }
```

---------------------------------------------------------------------------------------------------

## 4. Style bundle

```
manifest.json                        Manifest (§3.4)
github/assets/<css>                  github.githubassets.com/assets/* (light, dark, [all themes], primer-primitives,
                                     primer, global, github, repository, code); *.module.css dropped (GH §4)
github/images/…                      root-relative /images/… referenced by CSS (non-fatal failures recorded)
github/emoji/<name>.png              the 23 custom emoji (GH §8.5)
github/lib/mathjax/es5/tex-chtml-full.js, github/lib/mathjax/es5/ui/safe.js   (MathJax 3.2.0, CQ §1.4)
github/lib/mermaid/mermaid.min.js    (11.17.2)
gitlab/assets/application-*.css, application_dark-*.css, tailwind-*.css, fonts-*.css,
gitlab/assets/highlight/themes/{white,dark}-*.css, gitlab-sans/…, gitlab-mono/…, icon_anchor-*.svg, … (GL §11, §13)
gitlab/lib/katex/{katex.min.js,katex.min.css,fonts/*}   (0.16.47)
gitlab/lib/mermaid/mermaid.min.js    (11.16.1)
```
* Scrape `<link rel=stylesheet>` (both `href` and `data-href`) from a configurable source page
  (GitHub default `https://github.com/joeyespo/grip`, GitLab default
  `https://gitlab.com/gitlab-org/gitlab-foss`) with dom_query; classify by file-name stem into
  roles; hashed names are never hard-coded.
* Download every CSS; non-200 CSS is fatal. Find every `url(...)` (no `@import` expected, handle
  it anyway), resolve against the CSS URL, download (non-200 → `failures`, non-fatal), rewrite to a
  relative bundle path. `data:` URIs untouched. Libraries from jsDelivr at pinned versions.
* `--libs/--no-libs`, `--themes default|all` (GitHub), `--flavor github|gitlab|all`, `-o FILE`,
  `--github-source URL`, `--gitlab-source URL`, User-Agent `md-render/<version>`.
* Written through tarfs (deterministic, atomic). Re-vendoring one flavor preserves the other.
* The server watches the bundle path; on change re-opens it off-thread, swaps it (`ArcSwap`),
  bumps `styles_version`, sends `styles` to all clients; a corrupt new bundle keeps the old one
  and sends `error`.

---------------------------------------------------------------------------------------------------

## 5. Rendering contract per flavor

* GitHub: **GH §9.A** (`readme`, default) and **GH §9.B** (`comment`). Page skeleton GH §3/§9.A.
  Plus: `data-sourcepos` on top-level blocks when `opts.sourcepos` (GitHub itself does not emit it).
* GitLab: **GL §6** output contract, page template **GL §12**, client behaviours **GL §10**.
* Content root element has `id="md-content"` in both flavors' chrome.
* Elements that client code mutates (math, mermaid, code blocks with copy buttons) carry
  `data-mdr-key="<hex hash of their source>"` so the live client can keep already-rendered nodes
  (§7).

---------------------------------------------------------------------------------------------------

## 6. HTTP surface

```
GET /                                   default document (file, dir README, listing, or stdin text)
GET /{*path}                            markdown → rendered page; directory → README or listing
                                        (/dir → 301 /dir/); other files → raw bytes (mime_guess,
                                        text/* with charset=utf-8, Range/ETag via tower-http ServeDir)
GET /__/md-render/static/{name}         embedded assets (src/assets.rs)
GET /__/md-render/bundle/{*path}        bundle files via tarfs (ETag = bundle etag, Cache-Control: no-cache)
GET /__/md-render/events?path=<doc>     SSE live updates
GET /__/md-render/render?path=<doc>     JSON {version, html, title, warnings} (resync)
```
Markdown extensions: `md markdown mdown mkdn mkd mdwn` (+ GitLab `rmd`). README lookup in a
directory: case-insensitive `readme` with those extensions, exact `README.md` first; at the root
also `.github/README.md` then `docs/README.md` (GitHub order).

---------------------------------------------------------------------------------------------------

## 7. Live-update protocol

SSE `GET /__/md-render/events?path=<doc>`; first `retry: 1000`, then events (all `data:` JSON,
`id: <server_id>:<version>`), keepalive comment every 15 s:

| event | data |
|---|---|
| `hello` | `{server_id, version, styles_version}` |
| `rendering` | `{version}` |
| `content` | `{version, html, title, warnings}` |
| `error` | `{version, kind: "read"\|"decode"\|"render"\|"api"\|"bundle", message}` |
| `deleted` | `{path}` |
| `assets` | `{urls: [...], v}` — local images etc. referenced by the doc changed |
| `styles` | `{styles_version}` — bundle re-vendored / `--css` file changed |
| `shutdown` | `{}` |

Server: one watcher; per-document state (latest version, content hash, rendered result) shared by
all subscribers; renders in `spawn_blocking`, once per change (sha256 dedup); stale renders
dropped; on (re)connect the current `content` is sent immediately if the client's version (from
`Last-Event-ID` or the page's `data-version`) is behind.

Client (`assets/client/live.js`, no framework):
* Reconcile: parse the new HTML into a `<template>`; for each new top-level child compute a key =
  its outerHTML with `data-sourcepos="…"` removed; if an unused old top-level node has the same
  key (stored on first render), keep the old node (preserves rendered math/mermaid, `<details>`
  state, loaded images) and only update its `data-sourcepos`; otherwise insert the new node.
  Then dispatch `document` event `mdr:content-updated` with `detail: {root, added: [nodes]}` so
  flavor JS (github.js / gitlab.js) renders math/mermaid/copy buttons in the new nodes only.
* Scroll anchoring: before applying, find the first top-level block intersecting the viewport top,
  remember its source start line and pixel offset; afterwards restore it (pinned-to-bottom
  stays pinned). Fallback: nearest heading id, then scroll ratio.
* `hello` with a different `server_id` than the page was rendered by → `location.reload()`
  keeping the scroll anchor in `sessionStorage`. `styles` → swap each bundle `<link>` for one with
  `?v=<styles_version>` after it loads. `assets` → cache-bust matching `img/source/video`.
  `error`/`deleted` → dismissible banner (`role="alert"`), last good content stays; `(Error) `
  title prefix while active. `pageshow` (bfcache) and `visibilitychange` → resync via `/render`.
  Close the EventSource on `pagehide`.

Flavor JS contract: on `DOMContentLoaded` and on every `mdr:content-updated`, process only the
given nodes: GitHub — clipboard buttons (GH §5.3), `<math-renderer>` via MathJax (GH §8.6/CQ §1.4),
mermaid (GH §8.7), hash navigation with `user-content-` fallback (GH §3.4). GitLab — `copy-code`,
KaTeX (GL §10), mermaid, hash navigation (`getElementById(h) || getElementById('user-content-'+h)`),
`gl-system` auto dark toggle. Library URLs come from `<script id="mdr-config" type="application/json">`
(`{flavor, mode, scripts: {mathjax, mermaid, katex, …}, live: {...}|null}`) emitted by the page.

---------------------------------------------------------------------------------------------------

## 8. CLI (grip-compatible; design B of CR §8)

```
md-render [OPTIONS] [PATH] [ADDRESS]       serve (grip positional rules GRIP §2.3 + IPv6; default localhost:6419)
md-render export [OPTIONS] [PATH] [OUT]    standalone HTML (OUT '-' = stdout; default <PATH stem>.html); also `--export`
md-render vendor [--flavor github|gitlab|all] [-o FILE] [--themes default|all] [--no-libs]
                 [--github-source URL] [--gitlab-source URL]
md-render cache clear | cache path         also grip's `--clear`
global: -f/--flavor github|gitlab (env MD_RENDER_FLAVOR, default github)
        --mode readme|comment (alias --user-content) --context REPO --title TITLE
        --theme auto|light|dark (default auto) --light-theme ID --dark-theme ID
        --layout readme|bare --styles FILE (env MD_RENDER_STYLES) --css URL|FILE (repeatable)
        --api [--api-url URL] [--token TOKEN] --quiet -v/--verbose
serve:  -b/--browser --norefresh (alias --no-reload) --poll[=MS] --root DIR --follow-symlinks --auto-vendor
export: --no-inline
hidden grip compat: --wide (no-op, warns), --user/--pass (error: use --token), -a/-p (hint, exit 2)
```
Post-parse check: a subcommand combined with serve-only positionals/flags is an error with a hint
(`./vendor` to serve a file named `vendor`). Exit codes: 0 ok, 1 runtime error, 2 usage error.

---------------------------------------------------------------------------------------------------

## 9. Testing strategy

* `tarfs`: TarFS.jl-parity tests + system `tar` interop (crates/tarfs/tests).
* Renderers: unit tests per filter; golden tests against real site HTML (`tests/fixtures/<flavor>/`)
  with a normalizing DOM diff; property tests: no panic, deterministic output, sanitizer never
  lets `<script>`/`on*=`/`javascript:` through.
* Highlight: token-class tables per language from the research samples.
* Styles: vendor against a local mock HTTP server (no network) + one `#[ignore]` live test;
  bundle round-trip and read-modify-write preservation of the other flavor.
* Server: integration tests that start the server on port 0, fetch pages/bundle/raw files,
  assert path-traversal and Host-header protections, and observe SSE events after atomic-rename
  edits, deletes and re-creates, and bundle swaps.
* CLI: argv → config table tests (grip resolution table GRIP §2.3).
