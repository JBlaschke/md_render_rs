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

This document is the implementer's contract: the decisions (§2), the interfaces between modules
(§3) and the behaviour each part must keep (§4–§9). The user documentation is
[`README.md`](../README.md); the `tarfs` crate is documented in
[`crates/tarfs/README.md`](../crates/tarfs/README.md).

**Research references.** Decisions cite the research notes they rest on, for example "GH §9".
Those notes and the real-site captures behind them live in `.dev/research/`, which is **not
committed**: the captures are third-party content under GPL, AGPL, CC-BY-SA or no license at
all (see D18). The references record where a rule came from; this document and the code
comments state the rules themselves, so a fresh clone does not need the notes to follow them.
**GH** = `github.md` (github.com HTML, CSS and JS), **GL** = `gitlab.md` (gitlab.com), **GRIP** =
`grip.md` (grip's behaviour and the parity checklist, §10), **CR** = `crates.md` (crate
choices), **CQ** = `critique.md` (cross-check of the other four).

**Status.** The tree was implemented by parallel owners, reviewed (103 findings) and fixed in
two waves. Where a decision changed during implementation or review, its entry says
**Changed:** and why. Finding ids (`SEC-6`, `LIVE-3`, ...) refer to the review in
`.dev/review/` (also not committed).

---------------------------------------------------------------------------------------------------

## 1. Workspace layout

```
Cargo.toml                 workspace + package `md-render` (lib + bin); publish = false
crates/tarfs/              TarFS.jl port, no dependency on md-render (§3.11)
src/main.rs                -> md_render::cli::main()
src/lib.rs                 module tree; PREFIX = "/__/md-render", TOOL = "md-render <version>"
src/flavor.rs              Flavor, Mode, Theme, ThemeSel (§3.1)
src/assets.rs              embedded static files (§3.7)
src/render/mod.rs          RenderOptions, Rendered, Renderer, dispatch (§3.2)
src/render/github/         GitHub offline pipeline (D2)
src/render/gitlab/         GitLab offline pipeline: ported glfm formatter + Banzai filters (D3)
src/render/highlight/      syntect/two-face tokenizer; pl/ = GitHub classes, rouge/ = GitLab (D5)
src/render/api.rs          --api renderers (D15, §3.10)
src/styles/                Bundle (tarfs-backed), manifest, fallback, hot-swap (§3.4, §3.5, §4)
src/styles/vendor/         `md-render vendor`: scrape → download → rewrite → bundle (§4)
src/page/                  full-page HTML per flavor/mode/theme/layout, listings, errors (§3.6, §5)
src/export.rs              `md-render export` (§3.9)
src/server/                axum server: routes, path safety, raw files, hub (live state),
                           SSE events, watcher, style reloads (§3.8, §6, §7)
src/cli/                   clap CLI, grip positional rules, argv → plan → run (§8)
assets/client/live.js      live-update client (§7)
assets/client/common.js    helpers shared by the flavor scripts (window.mdRenderCommon)
assets/client/github.js    GitHub page behaviours: clipboard, MathJax, mermaid, hash navigation
assets/client/gitlab.js    GitLab page behaviours: copy-code, KaTeX, mermaid, hash nav, auto dark
assets/fallback/*.css      hand-written fallback CSS (BSD-3-Clause, D11)
assets/favicon.svg         favicon
data/github/               Linguist language table, gemoji table (+ emoji.md: format, provenance)
data/gitlab/               Rouge registry, GitLab emoji table (+ README.md: format, provenance)
tests/                     integration tests; tests/fixtures/<flavor>/ = hand-written inputs (D18)
docs/                      this file, dependency-licenses.{md,py}, licenses/
.github/workflows/ci.yml   fmt, clippy -D warnings, rustdoc -D warnings, tests (Linux, macOS), MSRV
```

Rules that still apply:

* The interfaces in §3 are the contract between modules. Changing one means updating this file
  in the same change.
* Every module has unit tests; behaviour across modules is tested under `tests/`.
* `cargo test` needs no network. Tests that do are `#[ignore]` and say why (§9).
* Shared files (`THIRD_PARTY_NOTICES.md`, `src/assets.rs`): a new ported source or embedded
  dataset gets its own section in the notices; a new embedded asset is one more line in
  `assets::get` and `NAMES`.
* **Changed:** the ownership tags of the implementation phase (`[skeleton]`, `[server]`, ...)
  are gone; the phase is over and every module is maintained as one tree.
* MSRV is Rust 1.88 (let-chains); CI checks it with exactly that toolchain. Both crates have
  `publish = false`: the crates.io name `tarfs` belongs to an unrelated crate, so the sub-crate
  must be renamed before a release.

---------------------------------------------------------------------------------------------------

## 2. Decisions

| # | Decision |
|---|---|
| D1 | `comrak = "=0.55.0"` (pinned: GitLab's gem formatter tracks comrak internals; `tagfilter` is gone in 0.56). `html5ever = "0.39.0"` is pinned to the minor version `dom_query` uses, because `render/gitlab/parse.rs` implements html5ever's `TreeSink` by delegating to `dom_query::Document`. |
| D2 | GitHub flavor, **CQ §3.1** pipeline: normalize line endings → (readme) YAML front matter split off (its lines blanked so comrak's line numbers stay the file's) → comrak parse (GFM: strikethrough, table, autolink, tasklist, footnotes, math_code, `render.unsafe`, `render.sourcepos`) → AST passes (duplicate footnote definitions merged as cmark-gfm does; bare URLs comrak misses but cmark-gfm links, e.g. short hosts and `www.localhost`; tagfilter on raw HTML) → formatter output (GitHub's shapes for footnotes, code, ```` ```math ````, enrichment sections) → **nesting budget** → **dom_query DOM** → sanitizer → DOM filters (alerts, dollar math, task lists, headings/slugs, raw-text autolinks, raw `<pre>`, emoji, tables, images, links, details, comment-mode code classes, `dir="auto"`) → top-level `data-sourcepos`, octicon expansion, serialization. Output contract **GH §9** (`readme` and `comment`). **Changed (sanitizer):** instead of "GitHub raw allowlist ∪ generated attributes", every element our formatter writes carries a per-render marker attribute (`data-mdr-g`, value derived from the input) as its **first** attribute: `TOKEN` = keep all attributes, `TOKEN-t` = trusted subtree (octicons, highlighted code, math). Everything else is raw HTML and goes through GitHub's allowlist (unknown elements unwrapped, attributes filtered per element, `id`/`name` prefixed `user-content-`, URL schemes restricted, comments dropped). The union scheme let raw look-alikes (`<div class="markdown-alert">`, `<input>`) survive; the marker scheme also covers two bypasses found in review: an unterminated raw tag that swallows the next generated tag's marker (GH-SEC-1), and HTML5 formatting-element reconstruction cloning raw `<b onclick>` into a trusted subtree (GH-SEC-2: unmarked formatting elements inside trusted subtrees are treated as raw). **Changed (budget, GH-PERF-1):** html5ever's tree builder is quadratic in nesting depth; `render/github/budget.rs` estimates the cost in one linear scan and a document above `MAX_NESTING_COST` (≈ 50 ms of tree building) renders as escaped source in `<pre><code>` plus a warning. |
| D3 | GitLab flavor, **GL §2–§6**: front-matter pre-filter (GL §3.2) → `::include` (GL §3.3, readme mode only) → comrak 0.55 with GitLab's exact option set + **ported** `formatter.rs`/`glfm.rs` of gitlab-glfm-markdown 0.0.43 (MIT, notice kept) → `ParseHtmlFilter` with GitLab's nesting limit → Banzai filters in GitLab's order (sanitizer included, syntax highlighting last). Emit the **post-JavaScript** DOM (no lazy images, `code-syntax-highlight-theme` on highlighted `pre`, table wrapper). **Added (GL-PERF-2):** gitlab.com parses with `HTML::Pipeline::HTML5Patch` (`config/initializers/html_pipeline_html5_patch.rb`, gitlabhq `8396d286`; not among the restored research sources, a copy is in `.dev/fix/render-gitlab/`): Nokogiri HTML5 with `max_tree_depth: 4096`, and a deeper document is replaced by `NESTING_TOO_DEEP_MESSAGE`. `parse.rs` does the same (it measures depth in the tree being built, so foster parenting and the adoption agency can cut a few levels off differently), which also bounds html5ever's quadratic cost. **Added (`::include` hardening, SEC-3, GL-INC-1/2/3):** confined to the canonical served root; any `.git` path component (any case, before and after canonicalization) renders GitLab's `not found`; on case-insensitive file systems a spelling that differs from the directory entries in ASCII case is `not found` too (GitLab matches the git tree exactly); at most 32 directives (`asciidoc_max_includes`), 10 MiB per file and 10 MiB of expanded text in total; `http(s)://` targets become links (URI includes are off on gitlab.com). Includes read the working tree, so uncommitted files are found (restricting to git-tracked files was rejected: it needs a subprocess per include and changes without a file event). **Added (GL-SAN-1, GL-LINK-1):** the link sanitizer models Addressable 2.8.7 (`NORMPATH`, `normalize_component`, `normalize_path`) and the external-link filter models Ruby uri 1.1.1 (`URI::MailTo`, `URI::FTP`, empty userinfo); not modelled: host-normalization failures, LDAP/WS/WSS/File classes, the 4 KiB normalization cap. |
| D4 | One DOM engine: `dom_query 0.28` (html5ever 0.39). No `lol_html`, no `ammonia` (their context rules can't express GitLab's sanitizer; CQ §1.2). Sanitizers are DOM passes. GitLab's parse step drives html5ever's tree builder directly (D1) so it can stop at the depth limit. |
| D5 | Highlighting: `syntect 5.3` (`parsing`, `regex-fancy`, pure Rust) + `two-face 0.5.2` syntaxes. GitHub: Linguist lookup (GH §7.2, CQ §1.7 alias table) → `highlight-<tm_scope with . → ->` wrapper + PrettyLights scope→`pl-*` map (GH §7.5/7.6, longest dot-prefix, nested spans, close/reopen at newlines, skip root scope), plus per-language text rules where two-face's scopes differ from Linguist's grammars; `text.shell-session` and `source.diff` are ports of Linguist's grammars; a `source.js` block that contains JSX is tokenized with two-face's TypeScriptReact grammar (`highlight/jsx.rs`, GH-HL-1). GitLab: Rouge registry lookup (GL §8.1) → `language-<tag>` + per-line `span#LCn.line` + scope→Rouge short-class rules (GL §8.4); `diff`, `console` and `docker` are ports of the Rouge lexers; `rouge/words.rs` reclassifies identifiers with the Rouge lexers' word tables. Unknown language: wrapper still emitted, no token spans. Results are memoized (`highlight/memo.rs`, ≤ 2048 entries / 32 MiB). Fidelity floors are enforced by `tests/highlight_golden.rs` (§9). |
| D6 | Emoji: GitHub = gemoji data, plain Unicode, 26 names wrapped in `<g-emoji>`, 23 custom → `<img class="emoji">` pointing into the bundle (`github/emoji/<name>.png`), first-match-decides boundary rule (GH §8.5 + CQ §1.6). GitLab = `<gl-emoji>` with data from GitLab's emoji database v4 (`/-/emojis/4/emojis.json`) and `fixtures/emojis/aliases.json` (GL §5.13). Both datasets are embedded as compact JSON under `data/` (MIT; attributed). |
| D7 | Slugs: own implementations. GitHub GH §6.2 (verified 473/473). GitLab = comrak 0.55 anchorizer semantics via the ported formatter (verified 147/147). Never use comrak's `HeadingAdapter`. |
| D8 | Math: GitHub → `<math-renderer>` markup + client MathJax **3.2.0** `es5/tex-chtml-full.js` + `es5/ui/safe.js`, synchronous `tex2mml` (CQ §1.4). GitLab → `js-render-math` markup + client KaTeX **0.16.47** (GL §5.4/§10). Mermaid client-side: GitHub 11.17.2, GitLab 11.16.1 (each flavor vendors its own). Without the library (no bundle, `--no-libs`) math and diagrams stay as their source (JS-1). GitLab's page-lifetime limits (50 diagrams, 2000 ms of KaTeX) count only connected elements, so live updates do not exhaust them (Q-LIVE-GL-LIMITS). |
| D9 | **One bundle file** holds both flavors: default `<cache>/md-render/styles.tar.gz` (`directories` `ProjectDirs::from("", "", "md-render").cache_dir()`), override `--styles FILE` / env `MD_RENDER_STYLES`. Vendoring one flavor is a TarFS **read-modify-write** (`tarfs::open_tarball_gz(out, Some(out), …)`: delete `<flavor>/…`, write new files, update `manifest.json`), flushed atomically, so running servers never see a partial bundle. Layout §4. **Added:** reading is capped at `styles::MAX_BUNDLE_SIZE` = 512 MiB decompressed (tarfs `Limits`, SEC-4); `vendor` refuses, before any download, to overwrite a file that is not an md-render bundle or was written by a newer md-render, except an unreadable bundle at the default location (STY-1); **changed (STY-2):** a flavor whose manifest entry does not match the archive is dropped (and replaced by the fallback) instead of rejecting the whole bundle. |
| D10 | `url()` references inside vendored CSS are rewritten to **relative** paths inside the bundle (mirroring site paths), so the bundle can be mounted under any prefix and exported. String arguments of `image-set()` / `-webkit-image-set()` count as references too (VEN-1). One CSS tokenizer (`styles/vendor/css.rs`) serves vendoring and export. |
| D11 | Missing bundle (or missing flavor in it): print the exact `md-render vendor …` command (with `-o PATH` when the bundle is not the default one) and render with the embedded fallback CSS (`assets/fallback/<flavor>.css`). No implicit network access. `--auto-vendor` opts into vendoring the served flavor on startup. **Changed:** the fallback CSS is md-render's own work under BSD-3-Clause, not MIT third-party CSS; it reuses only the sites' public class and custom-property names. Export, server and `vendor` all build that hint with `styles::vendor_command` (STY-1). |
| D12 | Internal URL namespace `/__/md-render/` (§6). |
| D13 | Live updates: SSE wire format of GRIP §11.1 (named events, JSON payloads, `id: <server_id>:<version>`, `retry: 1000`, keepalive), implemented with per-document `tokio::sync::watch`/`broadcast` + shutdown; one watcher per server: notify 8.2 `RecommendedWatcher`, parent-directory non-recursive watches, ref-counted, `notify-debouncer-mini` 50 ms. Client applies content with **content-keyed top-level block reconciliation** (§7). **Changed:** `--poll[=MS]` (default 300 ms; also the 1 s fallback when the native watcher fails) uses md-render's own poller, not notify's `PollWatcher`, which compares whole-second modification times: the poller lists each watched directory and compares size, full-resolution mtime and inode. **Changed:** versions come from one server-wide counter, so a document re-created after eviction never reissues a version a client already holds (LIVE-8). |
| D14 | Served root = the directory given on the CLI or the file's parent directory; `--root DIR` widens it (e.g. to the git top-level). Repo-root-relative links (`/docs/x.md`) resolve against the root. Path safety per GRIP C39: the URL path is percent-decoded once; NUL, backslash and `..` segments are rejected; the canonical path must stay under the canonical root (symlink escape blocked unless `--follow-symlinks`); `.git` (any case) is denied and hidden from listings; FIFOs, sockets and devices are refused. **Changed (CLI-18):** the file named on the command line is served even when it is a symlink that resolves outside the root (its own images and includes still follow the policy). Host-header allowlist (DNS rebinding): see D19. |
| D15 | `--api`: GitHub `POST /markdown` `mode=gfm` (+`context`), then the GitHub DOM filters that are idempotent on API HTML (heading wrappers for readme mode, octicon SVG for `span.octicon-link`, `data-canonical-src` handling); `<br>` on soft breaks in readme mode is a documented difference (CQ §1.5); readme-mode YAML front matter is rendered offline and not sent. GitLab `POST /api/v4/markdown` `{text, gfm:true, project}` with `PRIVATE-TOKEN`; un-lazy images, add `code-syntax-highlight-theme`. Tokens: `--token` > `GITHUB_TOKEN` > `GH_TOKEN` > `gh auth token --hostname HOST` for github.com and `*.ghe.com`; **changed (CLI-3):** any other GitHub host (Enterprise Server) gets `GH_ENTERPRISE_TOKEN` > `GITHUB_ENTERPRISE_TOKEN` > `gh auth token --hostname HOST`, never the github.com token; GitLab: `--token` > `GITLAB_TOKEN` > `glab config get token --host HOST`. Tokens are never printed or logged; ureq's wire-level TRACE output is filtered at every verbosity. **Changed (SEC-2, CLI-2, CLI-6):** redirects are never followed (a 3xx is an error naming the redacted `Location`); credentials inside `--api-url` are a usage error; URLs in messages are redacted. When serving, every API error falls back to offline rendering with an in-page banner, and the API is skipped for 15 s after a network error, until the reset time after a rate limit, and for the session after a rejected token; exports fail instead. Responses are cached by `sha256(flavor, mode, context, text)`, ≤ 128 entries / 32 MiB. GitHub documents over 400 KiB are not sent. |
| D16 | GitLab `--mode comment`: same renderer output (no `::include`), page chrome = note body (`div.note-text.md`); documented as approximate. |
| D17 | Licensing: project `BSD-3-Clause` (same as TarFS.jl). Ported and embedded third-party material is listed in `THIRD_PARTY_NOTICES.md`, one section per upstream with the verified copyright line, license texts once at the end. **Changed:** not all of it is MIT: Addressable is Apache-2.0, Ruby uri BSD-2-Clause, the Unicode emoji data Unicode-3.0, diff.tmbundle the TextMate bundle license. The licenses of the statically linked Rust crates are summarized in `docs/dependency-licenses.md` (generated by `docs/dependency-licenses.py` from `cargo metadata`), two-face's syntax licenses in `docs/licenses/two-face-acknowledgements.md`. Vendored site CSS/JS is downloaded by the user and never shipped in the binary. |
| D18 | Golden tests compare against real site output for public files, using a normalizing DOM diff (CQ §3.7). The fixtures are **not committed** (several are GPL/AGPL/CC-BY-SA or unlicensed — see `.dev/research/{github,gitlab}/FIXTURES.md`): golden tests read them from `$MD_RENDER_GOLDEN_DIR` (default `<repo>/.dev/research`; layout `github/{ctx,pages,comments,probes}`, `gitlab/{samples,issue_samples,rouge_samples}`) and print a loud `SKIPPED (set MD_RENDER_GOLDEN_DIR)` line when absent. Our own small hand-written inputs with expected outputs live in `tests/fixtures/<flavor>/` (committed). Current state: GitHub 43/43, GitLab blobs 10/10, GitLab issues 12/16. Of the other four, three are renders gitlab.com cached under older GitLab versions (pre-17.0 heading markup, older comrak source positions) and one needs GitLab services (PlantUML, group custom emoji, a `+` reference); the test reports them as known residuals. |
| D19 | **Added (SEC-5):** Host-header allowlist against DNS rebinding. Both the authority of an absolute-form request line and every `Host` header must name the server, or the request gets `403`. On loopback binds the names are `localhost`, `127.0.0.1`, `[::1]`, `*.localhost` (RFC 6761) and the configured host, with the bound port (or none). **Changed:** non-loopback binds were first left unchecked (proxies, tunnels, Codespaces and Docker port mappings use names md-render cannot predict); they are now checked as well, and `--allow-host NAME` (repeatable) adds names. A wildcard bind prints `Running on all addresses (…)` and the machine's network address, so it is never silent (CLI-19). |
| D20 | **Added (SEC-6):** raw files whose type the browser runs as an active document (`text/html`, `text/xml`, `application/xml`, `text/xsl`, `*+xml` incl. SVG) are served with `Content-Security-Policy: sandbox allow-scripts allow-popups allow-forms` (no `allow-same-origin`). The sites serve raw files from a separate origin; here they share the preview origin, so script in an untrusted SVG opened from a README could otherwise read every file under the root. The sandbox gives the document an opaque origin: its scripts, popups and forms work, same-origin fetches, CORS web fonts and module scripts do not. `<img src=x.svg>` is unaffected. Rendered pages and internal routes get no CSP. |
| D21 | **Added (Q-LOG-1, CLI-23):** diagnostics go through `tracing`. At the default verbosity a custom formatter prints `md-render: warning: …`, `md-render: error: …` and `md-render: …` with fields folded into the text; `-v` switches to tracing's own format. Filters: `-q` = `error`; default `warn,md_render=info,tarfs=info`; `-v` debug for md-render; `-vv` trace for md-render; `-vvv` trace for everything; `RUST_LOG` overrides. The server's grip-style ` * Running on …` / ` * Change detected …` lines are info-level output hidden by `-q`. Access log (CLI-7): target `md_render::access`, `METHOD /path?query -> STATUS (ms)`; 4xx/5xx at INFO (shown by default), the rest at DEBUG. |
| D22 | **Added:** resource bounds against hostile or runaway input, each with a user-visible message: GitHub nesting budget (D2), GitLab tree depth 4096 and 10 000 links per document (GitLab's `SanitizeLinkFilter` limit), `::include` limits (D3), bundle decompression cap (D9), API cache and response size (D15, 32 MiB each), highlight memo (D5), at most 256 live documents per server (idle ones evicted), GitHub's 400 KiB API limit. Still super-linear, outside our code: deeply nested `>` blockquotes inside comrak 0.55 (50 000 levels ≈ 2 s). |

---------------------------------------------------------------------------------------------------

## 3. Interfaces

These signatures are what other modules and the integration tests use. Changing one means
updating this section.

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
    pub context: Option<String>,      // owner/repo | group/project (sent with --api; unused offline)
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
pub fn github::render(markdown: &str, opts: &RenderOptions) -> Rendered;
pub fn gitlab::render(markdown: &str, opts: &RenderOptions) -> Rendered;
pub fn gitlab::included_files(markdown: &str, opts: &RenderOptions) -> Vec<PathBuf>; // ::include targets (live tracking)
```
Both renderers are pure and deterministic (same input → same bytes; e.g. footnote/math run ids
derived from content, GH §8.4), never panic on any input (property tests), and must be fast
(steady state < 10 ms for a 20 KB README in release). Pathological input ends in a bounded
fallback with a warning (D22), never in unbounded time.

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
Both must be thread-safe (lazy `OnceLock` syntax set / tables) and never panic; a syntect error
or panic inside a grammar is contained and the block is emitted unhighlighted.

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
    pub scripts: BTreeMap<String, String>, // key -> bundle path (keys below)
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
Fallback: `fallback`. Script keys: `mathjax`, `mathjax-safe`, `mermaid`, `katex`, `katex-css` and,
**added**, `icons` (GitLab's `icons-<hash>.svg` sprite for the copy-code button; not a script, but
recorded so pages find it without knowing the hashed name). All `path`s are bundle paths
(`github/…`, `gitlab/…`).

### 3.5 `src/styles/mod.rs`
```rust
pub const MAX_BUNDLE_SIZE: u64 = 512 << 20;                 // decompressed-size cap (D9)
pub struct Bundle { /* tarfs::InMemoryFileSystem + Manifest + origin + etag + damaged flavors */ }
pub enum BundleOrigin { File(PathBuf), Fallback }
impl Bundle {
    pub fn open(path: &Path) -> anyhow::Result<Bundle>;    // tarfs open (capped) + manifest parse + per-flavor validation
    pub fn from_tar_gz_bytes(gz: &[u8], origin: BundleOrigin) -> anyhow::Result<Bundle>;
    pub fn fallback() -> Bundle;                           // embedded fallback CSS for both flavors
    pub fn with_fallbacks(self) -> Bundle;                 // add fallback entries for flavors the bundle lacks
    pub fn manifest(&self) -> &Manifest;
    pub fn flavor(&self, f: Flavor) -> Option<&FlavorManifest>;
    pub fn is_fallback(&self, f: Flavor) -> bool;
    pub fn read(&self, path: &str) -> Option<bytes::Bytes>; // zero-copy
    pub fn contains(&self, path: &str) -> bool;  pub fn file_size(&self, path: &str) -> Option<u64>;
    pub fn paths(&self) -> impl Iterator<Item = &str>;
    pub fn etag(&self) -> &str;                            // changes whenever content changes
    pub fn origin(&self) -> &BundleOrigin;
}
pub struct SharedBundle { /* ArcSwap<Bundle> */ }           // get(), reload() (keeps the old one on error), replace()
pub fn default_bundle_path() -> PathBuf;                    // env MD_RENDER_STYLES, else cache dir
pub fn vendor_command(flavor: Option<Flavor>, path: &Path) -> String; // the exact fix-it command (with -o when needed)
pub fn load_or_fallback(path: &Path) -> (Bundle, Option<String>); // never fails; warning text for the user
pub fn flavor_warning(bundle: &Bundle, flavor: Flavor) -> Option<String>; // "no / damaged X styles; run …"
// src/styles/vendor/mod.rs
pub struct VendorOptions { pub flavors: Vec<Flavor>, pub output: PathBuf, pub github_source: String,
    pub gitlab_source: String, pub all_github_themes: bool, pub libs: bool, pub quiet: bool }
pub struct VendorReport { pub output: PathBuf, pub files: usize, pub bytes: u64, pub failures: Vec<Failure> }
pub fn vendor(opts: &VendorOptions) -> anyhow::Result<VendorReport>; // prints progress, failures and the summary unless quiet
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
    pub extra_css: &'a [String],        // extra stylesheets after the bundle CSS (see below)
}
pub fn render_page(input: &PageInput, bundle: &Bundle) -> anyhow::Result<String>;
pub struct ListingEntry { pub name: String, pub href: String, pub is_dir: bool, pub size: Option<u64> }
pub fn listing_html(flavor: Flavor, dir: &str, entries: &[ListingEntry]) -> String; // content fragment
pub fn error_html(flavor: Flavor, status: u16, message: &str) -> String;           // content fragment
```
**Changed (CLI-12):** `extra_css` entries are URLs (always linked) or local CSS files: Server
mode links them (the server serves them, §6); Inline and Directory modes embed a local file as
`<style data-mdr-style=extra data-mdr-file=NAME>`, with its relative `url()`/`@import` targets
as `data:` URIs. `render_page` therefore fails on I/O errors only: copying files in Directory
mode, or reading a local extra stylesheet in the export modes. A requested GitHub theme id the
bundle lacks falls back to light/dark with a warning naming `vendor --themes all` (PAGE-1).

### 3.7 `src/assets.rs`
`pub fn get(name: &str) -> Option<(&'static [u8], &'static str /*mime*/)>` and `pub const NAMES`
for `live.js`, `github.js`, `gitlab.js`, **`common.js`** (added: the helpers both flavor scripts
share, Q-DUP-JS), `favicon.svg`, `fallback-github.css`, `fallback-gitlab.css`. Served with a
content-hash ETag and `Cache-Control: no-cache`, so a rebuilt binary is never answered from a
stale browser cache.

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
    pub allow_hosts: Vec<String>,     // --allow-host (D19)
}
impl ServeConfig { pub fn new(root: PathBuf, target: Target, renderer: Arc<dyn Renderer>) -> ServeConfig; } // grip defaults
pub struct RunningServer { pub addr: SocketAddr, /* url, shutdown handle, tasks */ }
impl RunningServer { pub fn url(&self) -> &str; pub fn shutdown_handle(&self) -> ShutdownHandle;
                     pub async fn shutdown(self); pub async fn wait(self) -> anyhow::Result<()>; }
pub async fn start(cfg: ServeConfig) -> anyhow::Result<RunningServer>; // binds, returns immediately
pub async fn serve(cfg: ServeConfig) -> anyhow::Result<()>;             // start + Ctrl-C/SIGTERM + wait
```
`start` binds before it returns. `localhost` binds both loopback families on the same port; if
the other family's address is taken, it fails like a port in use (CLI-4), and with port 0 it
picks another port (up to 8 tries). Request handlers never run blocking filesystem calls on
the reactor (canonicalize, metadata, `read_dir`, README lookup): they go through
`server::blocking` (Q-ASYNC-1). Renders run on detached threads, so a slow render does not
hold up shutdown; a second Ctrl-C exits with status 130 (LIVE-11).

### 3.9 `src/export.rs`
```rust
pub enum ExportInput { File(PathBuf), Stdin { name: String, text: String } }
pub enum ExportOutput { File(PathBuf), Stdout }
pub struct ExportConfig { pub input: ExportInput, pub output: ExportOutput, pub inline: bool,
    pub flavor: Flavor, pub mode: Mode, pub theme: ThemeSel, pub layout: Layout,
    pub title: Option<String>, pub context: Option<String>, pub bundle_path: PathBuf,
    pub extra_css: Vec<String>,       // added (CLI-12): --css, embedded or linked (§3.6)
    pub renderer: Arc<dyn Renderer>, pub quiet: bool }
pub fn export(cfg: &ExportConfig) -> anyhow::Result<()>;
```
Exports render with `sourcepos: false` and the default bundle route, which the page then
resolves inside the export. Document-relative URLs are rebased when OUT is in another
directory than the source (EXP-1); document images are never embedded. Images that only site
CSS references and that exceed 16 KiB (raster) or 64 KiB (SVG) are neither inlined nor copied
(EXP-2: they are page decoration). The default `<title>` and header text is the file name;
`--title` replaces both (CLI-9, CLI-24). The output is written atomically; a symlinked output
has its target replaced (link kept), a replaced file keeps its permission bits, and an
unwritable existing file is refused (CLI-20).

### 3.10 `src/render/api.rs`
```rust
pub struct ApiConfig { pub flavor: Flavor, pub api_url: Option<String>, pub token: Option<String>, pub fallback_offline: bool }
pub struct ApiRenderer { /* ureq agent (no redirects), resolved token, response cache */ }
impl ApiRenderer { pub fn new(cfg: ApiConfig) -> anyhow::Result<Self>; }
impl Renderer for ApiRenderer { /* … */ }
```
`fallback_offline` is true when serving and false for exports (D15). The transport is
injectable, so tests never touch the network.

### 3.11 `crates/tarfs` (added)
```rust
pub struct InMemoryFileSystem { /* one decompressed Bytes buffer + path index */ } // Send + Sync; alias TarFs
#[non_exhaustive] pub struct Limits { pub max_decompressed_size: Option<u64> }       // Limits::default() = unlimited
impl Limits { pub const fn with_max_decompressed_size(self, bytes: u64) -> Self; }
impl InMemoryFileSystem {
    pub fn new() -> Self;
    pub fn open(path) -> Result<Self>;  pub fn open_with_limits(path, Limits) -> Result<Self>;
    pub fn from_tar_gz_reader(r) / from_tar_gz_reader_with_limits(r, Limits) / from_tar_gz_bytes(gz)
        / from_tar_gz_bytes_with_limits(gz, Limits) / from_tar_bytes(buf) -> Result<Self>;
    pub fn read_file(&self, p) -> Result<Bytes>;  get, read_to_string, contains, len, is_empty,
        paths, iter, file_size, read_dir;
    pub fn write_file(&mut self, p, c) -> Result<()>;  delete_file -> bool;  delete_prefix -> usize;
    pub fn write_tarball(&self, w) / write_tarball_extended(&self, w) -> Result<W>;
    pub fn write_tarball_gz(&self, p) / write_tarball_gz_extended(&self, p) -> Result<()>; // atomic
}
pub fn open_tarball_gz<T, E, F>(path, from: Option<&Path>, f: F) -> Result<(T, InMemoryFileSystem), E>;
pub fn check_path(p: &str) -> Result<()>;
pub enum Error { Io, NotFound, InvalidPath { path, reason }, PathTooLong, EntryTooLarge { path, size },
                 Corrupt, TooLarge { limit }, Utf8 }
```
Deliberate differences from TarFS.jl, all documented in the crate README: reads accept
multi-member gzip and all-zero padding after the last member (TAR-3), drop empty and `.` path
segments, skip `._*` members only when they carry the AppleDouble magic `00 05 16 07` (other
`._*` files are ordinary files, TAR-1), and reject unsafe or non-UTF-8 member paths and sparse
files; both writers refuse a filesystem in which a path is both a file and a parent directory
(`a` and `a/b`) with `InvalidPath`, which TarFS.jl writes anyway (TAR-2); `_with_limits` readers
stop with `TooLarge` (SEC-4).

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
gitlab/assets/highlight/themes/{white,dark}-*.css, gitlab-sans/…, gitlab-mono/…, icons-<hash>.svg, … (GL §11, §13)
gitlab/lib/katex/{katex.min.js,katex.min.css,fonts/*.woff2}   (0.16.47)
gitlab/lib/mermaid/mermaid.min.js    (11.16.1)
```
* Scrape `<link rel=stylesheet>` (both `href` and `data-href`) from a configurable source page
  (GitHub default `https://github.com/joeyespo/grip`, GitLab default
  `https://gitlab.com/gitlab-org/gitlab-foss`) with dom_query; classify by file-name stem into
  roles; hashed names are never hard-coded. GitLab's `icons-<hash>.svg` sprite is found in
  `<use href>` or `gon.sprite_icons` on the page.
* Download every CSS; non-200 CSS is fatal. Find every `url(...)`, `@import` and `image-set()`
  string, resolve it against the CSS URL, download it (non-200 → `failures`, non-fatal), rewrite
  it to a relative bundle path. `data:` URIs untouched. Libraries from jsDelivr at pinned versions;
  KaTeX's `.woff`/`.ttf` fallbacks are not vendored (browsers that run KaTeX use `.woff2`).
* `--libs/--no-libs`, `--themes default|all` (GitHub), `--flavor github|gitlab|all` (default
  `all`; `MD_RENDER_FLAVOR` does not apply), `-o FILE`, `--github-source URL`,
  `--gitlab-source URL`, User-Agent `md-render/<version>`.
* Nothing touches the bundle until every requested flavor has downloaded; then one tarfs
  read-modify-write replaces only those flavors (deterministic, atomic). The overwrite policy
  (D9) is checked before the first download.
* The server watches the bundle's directory (re-trying every 2 s while it does not exist, e.g.
  before the first `vendor`); on change it re-opens the bundle off the reactor, swaps it
  (`ArcSwap`), bumps `styles_version` and sends `styles` to all clients. **Changed (STY-2):** a
  bundle with at least one intact flavor is swapped in (damaged flavors dropped and reported); a
  bundle with none keeps the old one and sends an `error` of kind `bundle`.
* A wrong or missing bundle never stops md-render: pages fall back per flavor (D11) and the
  message names the command that fixes it.

---------------------------------------------------------------------------------------------------

## 5. Rendering contract per flavor

* GitHub: **GH §9.A** (`readme`, default) and **GH §9.B** (`comment`). Page skeleton GH §3/§9.A.
  Plus: `data-sourcepos` on top-level blocks when `opts.sourcepos` (GitHub itself does not emit it).
* GitLab: **GL §6** output contract, page template **GL §12**, client behaviours **GL §10**.
* Content root element has `id="md-content"` in both flavors' chrome.
* Elements that client code mutates (math, mermaid, code blocks with copy buttons) carry
  `data-mdr-key="<hex hash of their source>"` so the live client can keep already-rendered nodes
  (§7).
* `<html data-icon-sprite>` (GitLab, added in PAGE-2): absent = no sprite, the copy button uses
  the Octicon fallback; a non-empty value = the sprite URL (`<use href="URL#copy-to-clipboard">`);
  `""` = the `copy-to-clipboard` `<symbol>` is in the page itself (inline exports carry it in a
  hidden `<svg>`, rebuilt from the sprite's viewBox and path data only).
* Banners rendered by the server are `#mdr-banners > .mdr-banner`; live.js removes and adds
  banners with exactly that selector (LIVE-7).
* A document page served while the document is failed or missing has the title prefix
  `(Error) ` (`routes.rs` `ERROR_TITLE_PREFIX`, which must match `live.js`), and the client
  treats it as an active error until newer content arrives.

---------------------------------------------------------------------------------------------------

## 6. HTTP surface

```
GET /                                   default document (file, dir README, listing, or stdin text)
GET /{*path}                            markdown → rendered page; directory → README or listing
                                        (/dir → 301 /dir/, /file/ → 301 /file); other files → raw bytes
GET /{*path}?raw=1                      a Markdown file's source as text/plain
GET /__/md-render/static/{name}         embedded assets (§3.7)
GET /__/md-render/bundle/{*path}        bundle files via tarfs (ETag = bundle etag, Cache-Control: no-cache)
GET /__/md-render/css/{index}/{name}    a local --css file (served and watched; Cache-Control: no-cache)
GET /__/md-render/events?path=<doc>     SSE live updates (§7)
GET /__/md-render/render?path=<doc>     JSON resync (§7)
```
* Every route is behind the Host guard (D19) and the access log (D21). An unknown path under
  `/__/md-render/` is a 404 and never shadowed by a user file.
* `<doc>` is a root-relative document path, or a directory URL with a trailing slash
  (`dir/`, `/` for the root) for a live listing (LIVE-9). An empty `path` is a 400.
* **Changed:** raw files are served with tower-http's `ServeFile` (Range, `Last-Modified` and
  `If-Modified-Since`, an ETag from size and mtime with `If-None-Match` → 304),
  `Cache-Control: no-cache` so edited images revalidate, and
  `X-Content-Type-Options: nosniff`. Content-Type: `mime_guess`; textual types get
  `charset=utf-8`; script types browsers would download (`x-sh`, `x-python`, …) are shown as
  `text/plain`; extension-less files are sniffed (NUL byte or invalid UTF-8 in the first 8 KiB →
  `application/octet-stream`, else `text/plain`). Active document types get the CSP sandbox (D20).
* A missing Markdown path renders a 404 page that includes the live client, so it turns into
  the document when the file appears (GRIP C58); `.git` paths and symlink escapes are 403
  pages.
* Markdown extensions: `md markdown mdown mkdn mkd mdwn` (+ GitLab `rmd`). README lookup in a
  directory: case-insensitive `readme` with those extensions, exact `README.md` first, then by
  extension order. **Changed (CLI-17):** for the served root the GitHub flavor uses GitHub's
  order, `.github/README*`, then the root, then `docs/README*`; the GitLab flavor uses the root
  only. (The first version looked at the root first for both; export and serve now share one
  lookup, so they always pick the same file.)
* Listings: directories first, then case-insensitive by name; `..` below the root; `.git` and
  symlinks that escape the root (unless `--follow-symlinks`) hidden.

---------------------------------------------------------------------------------------------------

## 7. Live-update protocol

SSE `GET /__/md-render/events?path=<doc>`; first `retry: 1000`, then events (all `data:` JSON,
`id: <server_id>:<version>`), keepalive comment `: ping` every 15 s:

| event | data |
|---|---|
| `hello` | `{server_id, version, styles_version}` |
| `rendering` | `{version}` |
| `content` | `{version, html, title, warnings}` |
| `error` | `{version, kind: "read"\|"render"\|"api"\|"bundle", message}` (kind `bundle` is global and has no `version`) |
| `deleted` | `{version, path}` |
| `assets` | `{urls: [...], v}` — local images etc. referenced by the doc changed |
| `styles` | `{styles_version, hrefs: [...]}` — bundle re-vendored / `--css` file changed; `hrefs` = the stylesheet URLs the page should now use |
| `reload` | `{}` — load the page again (added: a listed directory got a README) |
| `shutdown` | `{}` |

**Changed:** the kind `decode` was dropped from the protocol: the server decodes lossily (BOM
stripped, invalid UTF-8 replaced) and reports the position as a content warning instead
(GRIP C38). Events that publish no document state (`hello`, `rendering`, `assets`, `styles`,
`reload`, `shutdown`) carry the last version the subscriber received in their `id`, so a
`Last-Event-ID` never claims a state the client has not seen.

`/render` returns the `content` object plus `server_id` (200). A missing document adds
`deleted: true` and `path` (404), a failed one `error: {kind, message}` (500), each next to the
last good content when there is one; 503 while nothing has been rendered yet.

Server:
* One watcher; per-document state (latest version, input hash, rendered result) shared by all
  subscribers. A document entry is created on first request; idle entries are evicted above 256
  documents. Versions come from one server-wide counter (D13).
* A worker coalesces triggers (page load, new subscriber, watcher event), re-reads the inputs
  and renders only when their SHA-256 changed. The inputs are the document bytes plus, for
  GitLab, every `::include`d file, which is also watched (LIVE-1). Renders run off the reactor
  and **every finished render is published**; a change that arrived meanwhile leaves a wake-up
  permit, so the newer bytes are rendered right after (LIVE-13; dropping "stale" renders froze the
  preview while saving often).
* A document that suddenly reads as empty is re-read every 50 ms for up to 1.5 s before the
  empty render is published (truncate-then-write saves, LIVE-2; 250 ms was too short for a
  writer pausing 0.4 s); new content ends the wait at once, so normal edits stay instant and a
  file that was really emptied is shown after the hold. Inputs that already failed to render
  publish no new error when re-checked.
* Media dependencies (`img`, `source`, `video`, `audio`, `track`, `input[type=image]`: `src`,
  `srcset`, `poster`) inside the root are watched; a change sends `assets`. Dependencies (and
  documents) whose directory does not exist yet are watched through their nearest existing
  ancestor (LIVE-4, LIVE-5). A symlinked document's link is watched as well as its target
  (LIVE-10).
* A directory listing is an entry too (key `dir/`, or `/`): its render is the listing fragment,
  pushed as `content` when entries change; a README appearing sends `reload` (LIVE-9).
* On (re)connect the current `content` is sent immediately if the client's version (from
  `Last-Event-ID`, or the `v` + `sid` query parameters when they name this process) is behind.
* FSEvents may drop changes made right after a watch is added (CR §5): new subscribers trigger
  one extra re-check after 750 ms.

Client (`assets/client/live.js`, no framework):
* Reconcile: parse the new HTML into a `<template>`; for each new top-level child compute a key =
  its outerHTML with `data-sourcepos="…"` removed; if an unused old top-level node has the same
  key (stored on first render), keep the old node (preserves rendered math/mermaid, `<details>`
  state, loaded images) and only update its `data-sourcepos`; otherwise insert the new node.
  Then dispatch `document` event `mdr:content-updated` with `detail: {root, added: [nodes]}` so
  flavor JS renders math/mermaid/copy buttons in the new nodes only. Several `content` events in
  one animation frame apply only the last.
* Scroll anchoring: before applying, take the first top-level block intersecting the viewport
  top, its source start line (its own `data-sourcepos`, else the first one inside it: GitLab code,
  table, mermaid and raw-HTML wrappers, LIVE-12) and pixel offset; afterwards restore it
  (pinned-to-bottom stays pinned). Fallback: nearest heading id, then scroll ratio. Updates to
  empty content keep the last anchor taken from non-empty content.
* `hello` with a different `server_id` than the page was rendered by → `location.reload()`
  keeping the scroll anchor in `sessionStorage`. `styles` → swap each bundle and `--css` `<link>`
  for one with `?v=<styles_version>`, removing the old one once the new one loaded; a stylesheet
  that fails to load (a re-vendored, renamed hashed file) reloads the page, and a `hello` with a
  newer `styles_version` swaps too (missed while disconnected). `assets` →
  cache-bust matching `img/source/video`; the latest token per URL is also applied to media in
  blocks inserted later (LIVE-6). `error`/`deleted` → dismissible banner (`role="alert"`), last good
  content stays; `(Error) ` title prefix while an error is active. `reload` → reload.
* **Changed (LIVE-3):** browsers allow about six HTTP/1.1 connections per server, and each open
  stream holds one. The stream is closed on `pagehide` **and 5 s after the page is hidden**
  (`HIDDEN_GRACE_MS`); a page opened in a background tab does not connect until shown; showing
  the page reconnects and resyncs via `/render`; `pageshow` (bfcache) resyncs too. More than about
  five *visible* previews of one server can still stall; sharing one connection between tabs
  (BroadcastChannel/SharedWorker) would need a multi-document events endpoint and was not done.

Flavor JS contract: on `DOMContentLoaded` and on every `mdr:content-updated`, process only the
given nodes. **Added:** `common.js` loads first and provides `window.mdRenderCommon` (config,
collect/claim of not-yet-rendered elements, lazy library loading by URL or from an export's inert
`<script type="text/plain" id="mdr-lib-NAME">`, clipboard with the legacy fallback, hash
navigation with the `user-content-` fallback). GitHub — clipboard buttons (GH §5.3),
`<math-renderer>` via MathJax (GH §8.6/CQ §1.4), mermaid (GH §8.7; geoJSON/topoJSON/STL shown as
code), hash navigation (also `name=` targets). GitLab — `copy-code` (sprite per §5), KaTeX
(GL §10), mermaid, hash navigation (`getElementById(h) || getElementById('user-content-'+h)`),
`gl-system` auto dark toggle. Configuration comes from
`<script id="mdr-config" type="application/json">`
(`{flavor, layout, mode, theme, themes: {light, dark}, scripts: {mathjax, mermaid, katex, …},
live: {...}|null}`) emitted by the page.

---------------------------------------------------------------------------------------------------

## 8. CLI (grip-compatible; design B of CR §8)

```
md-render [OPTIONS] [PATH] [ADDRESS]       serve (grip positional rules GRIP §2.3 + IPv6; default localhost:6419)
md-render export [OPTIONS] [PATH] [OUT]    standalone HTML (OUT '-' = stdout; default <PATH stem>.html); also `--export`
md-render vendor [-f github|gitlab|all] [-o FILE] [--themes default|all] [--no-libs]
                 [--github-source URL] [--gitlab-source URL]
md-render cache clear | cache path         also grip's `--clear`
global: -f/--flavor github|gitlab (env MD_RENDER_FLAVOR, default github; vendor: all)
        --mode readme|comment (alias --user-content) --context REPO --title TITLE
        --theme auto|light|dark|<GitHub theme id> (default auto) --light-theme ID --dark-theme ID
        --layout readme|bare --styles FILE (env MD_RENDER_STYLES) --css URL|FILE (repeatable)
        --api [--api-url URL] [--token TOKEN] -q/--quiet -v/--verbose (repeatable)
serve:  -b/--browser --norefresh (alias --no-reload) --poll[=MS] --root DIR --follow-symlinks
        --allow-host NAME (repeatable, D19) --auto-vendor
export: --no-inline
hidden grip compat: --wide (no-op, warns), --user/--pass (usage error: use --token), -a/-p (hint, exit 2)
```
* A lone positional is the ADDRESS iff it contains a valid port (path-and-address rules, ported
  exactly); `./8080` and `./vendor` force a path. **Added:** an ADDRESS whose host is all digits
  is a usage error (`. 70000`: port out of range; `70000:80`: not a host), because the resolver
  would read it as an integer IPv4 address (`70000` = `0.1.17.112`).
* Post-parse checks: a subcommand combined with serve-only positionals/flags is a usage error
  with a hint (`./vendor` to serve a file named `vendor`), re-rendered with the subcommand's
  usage line (CLI-14). A bare `md-render vendor`/`export` next to a file of that name warns
  before any network access (CLI-21). `vendor`, `cache` and `--clear` warn by name about the
  rendering/API options they ignore (CLI-11); `--context`, `--api-url` and `--token` without
  `--api` warn (CLI-22). **Changed (CLI-13):** an all-digit argument right after a bare `--poll`
  is a usage error (`--poll=500`, or the address before the option), because it would otherwise
  silently become the PATH or ADDRESS. When planning fails, the warnings collected so far are
  printed before the error (unless `-q`), so the CLI-21 hint is not lost when, say, a bare
  `md-render export` finds no README.
* GitLab accepts only the `light` and `dark` theme ids.
* `cache clear` deletes md-render's cache directory (a symlinked one is unlinked, its target
  kept); a `--styles` bundle outside it is never deleted and is named in the message.
* Exit codes: 0 ok, 1 runtime error, 2 usage error, 130 second Ctrl-C during shutdown.
* Reading stdin on a terminal prints `Reading Markdown from stdin (press … when done)`.

---------------------------------------------------------------------------------------------------

## 9. Testing strategy

* `cargo test --workspace` needs no network and no external tools; tests that need something
  optional print a loud `SKIPPED (…)` line and pass.
* `tarfs`: TarFS.jl-parity tests (a golden fixture produced by TarFS.jl), system `tar` interop
  (bsdtar, GNU tar when installed), a corruption fuzz loop, README examples as doctests; the live
  Julia cross-check is `#[ignore]` (`cargo test -p tarfs --test julia -- --ignored`).
* Renderers: unit tests per filter; hand-written inputs with expected output in
  `tests/fixtures/<flavor>/` (committed); property tests: no panic, deterministic output,
  sanitizer never lets `<script>`/`on*=`/`javascript:` through (`MD_RENDER_PROP_CORPUS=DIR` runs
  them over a directory of `.md` files).
* **Golden tests (D18)** against real site HTML from `$MD_RENDER_GOLDEN_DIR` with a normalizing
  DOM diff: `tests/github_golden.rs` (43 files), `tests/gitlab_golden.rs` (10 blobs, 16 issues),
  `tests/api_golden.rs` (`--api` post-processing on captured API responses). Inside code blocks
  only text and line structure are compared unless `GOLDEN_STRICT_HIGHLIGHT=1`.
* Highlight: token tables per language and `tests/highlight_golden.rs`, per-character agreement
  with the sites; the floors are GitHub ≥ 91 % class path / 94 % colour on the tuning set
  (≥ 88 % / 90 % held out) and GitLab ≥ 85 % exact / 91 % family, with `diff`, `console`,
  `dockerfile`, `json` and `plaintext` exact. Lowering a floor is a deliberate change.
* Styles: vendor against a local mock HTTP server (no network) + one `#[ignore]` live test;
  bundle round-trip, read-modify-write preservation of the other flavor, damaged flavors, the
  decompression cap, the overwrite policy.
* Server: integration tests start the server on port 0, fetch pages/bundle/raw files, assert
  path-traversal, `.git`, symlink, Host-header and CSP protections, and observe SSE events after
  atomic-rename edits, deletes and re-creates, include edits, listing changes and bundle swaps.
* Client JS: `tests/page_client_js.rs` runs github.js/gitlab.js under Node with a small DOM
  harness (`tests/page_support/dom_harness.js`); `tests/server_browser.rs` drives headless Chrome
  over the DevTools protocol (`tests/server_browser/cdp.mjs`; `MD_RENDER_TEST_CHROME` selects the
  binary) for connection handling across tabs, cache-busting, banners and scroll anchoring.
* `--api`: a fake transport for unit tests; `tests/api_live.rs` is `#[ignore]` (network, and
  `GITLAB_TOKEN` for GitLab).
* CLI: argv → plan table tests (grip resolution table GRIP §2.3), binary tests for messages and
  exit codes.
* Docs: `tests/docs_notices.rs` keeps `THIRD_PARTY_NOTICES.md` well-formed (balanced code fences,
  a license and copyright line in every upstream section, the summary table matching the
  sections, in-page links resolving).
* CI (`.github/workflows/ci.yml`): `cargo fmt --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps`, `cargo test
  --workspace` on Linux and macOS, `cargo check` with Rust 1.88.
