//! Shared helpers for the page / export integration tests: synthetic style bundles written
//! through tarfs (so `Bundle::open` reads them exactly like a vendored bundle), page builders
//! and small DOM assertions.

#![allow(dead_code)] // each test crate uses a different subset

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dom_query::Document;
use md_render::page::{AssetMode, Banner, Layout, LiveInfo, PageInput, render_page};
use md_render::render::{Features, RenderOptions, Rendered, Renderer};
use md_render::styles::{Asset, Bundle, FlavorManifest, Manifest, Stylesheet};
use md_render::{Flavor, Mode, Theme, ThemeSel};
use sha2::{Digest as _, Sha256};

/// Images above the export limit of the page module for site CSS images (16 KiB).
pub const BIG_IMAGE_LEN: usize = 70 * 1024;

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Builds a bundle `.tar.gz` with a schema-1 manifest whose sha256/size fields are correct.
pub struct BundleBuilder {
    fs: tarfs::InMemoryFileSystem,
    manifest: Manifest,
}

impl Default for BundleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl BundleBuilder {
    pub fn new() -> Self {
        BundleBuilder {
            fs: tarfs::InMemoryFileSystem::new(),
            manifest: Manifest::default(),
        }
    }

    fn flavor_mut(&mut self, flavor: Flavor) -> &mut FlavorManifest {
        self.manifest
            .flavors
            .entry(flavor)
            .or_insert_with(|| FlavorManifest {
                fetched_at: "2026-10-01T00:00:00Z".into(),
                source_page: format!("https://example.invalid/{flavor}"),
                user_agent: "md-render-tests".into(),
                ..Default::default()
            })
    }

    fn put(&mut self, path: &str, bytes: &[u8]) {
        self.fs
            .write_file(path, bytes.to_vec())
            .expect("valid bundle path");
    }

    /// Register a flavor with no files at all.
    pub fn empty_flavor(mut self, flavor: Flavor) -> Self {
        self.flavor_mut(flavor);
        self
    }

    pub fn stylesheet(
        mut self,
        flavor: Flavor,
        path: &str,
        role: &str,
        theme: Option<&str>,
        css: &str,
    ) -> Self {
        self.put(path, css.as_bytes());
        let entry = Stylesheet {
            path: path.into(),
            url: format!("https://example.invalid/{path}"),
            role: role.into(),
            theme: theme.map(Into::into),
            media: Some("all".into()),
            sha256: sha256_hex(css.as_bytes()),
            size: css.len() as u64,
        };
        let fm = self.flavor_mut(flavor);
        if let Some(t) = theme
            && !fm.themes.iter().any(|x| x == t)
        {
            fm.themes.push(t.to_owned());
        }
        fm.stylesheets.push(entry);
        self
    }

    pub fn asset(mut self, flavor: Flavor, path: &str, bytes: &[u8]) -> Self {
        self.put(path, bytes);
        let entry = Asset {
            path: path.into(),
            url: format!("https://example.invalid/{path}"),
            sha256: sha256_hex(bytes),
            size: bytes.len() as u64,
        };
        self.flavor_mut(flavor).assets.push(entry);
        self
    }

    pub fn script(mut self, flavor: Flavor, key: &str, path: &str, bytes: &[u8]) -> Self {
        self.put(path, bytes);
        self.flavor_mut(flavor)
            .scripts
            .insert(key.into(), path.into());
        self
    }

    /// Write `<dir>/styles.tar.gz` and return its path.
    pub fn write(mut self, dir: &Path) -> PathBuf {
        let json = serde_json::to_vec_pretty(&self.manifest).expect("manifest serializes");
        self.put("manifest.json", &json);
        let path = dir.join("styles.tar.gz");
        self.fs.write_tarball_gz(&path).expect("bundle written");
        path
    }

    pub fn open(self, dir: &Path) -> Bundle {
        let path = self.write(dir);
        Bundle::open(&path).expect("synthetic bundle opens")
    }
}

/// 1x1 transparent GIF.
pub const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
/// Not a real PNG; the page module only cares about the extension.
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png";
pub const WOFF2: &[u8] = b"wOF2fake-font";

/// GitLab's icon sprite as vendored: many symbols, the copy button's among them.
pub const GITLAB_SPRITE: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><symbol viewBox="0 0 16 16" id="abuse" xmlns="http://www.w3.org/2000/svg"><path d="M1 1h2"/></symbol><symbol viewBox="0 0 16 16" id="copy-to-clipboard" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" clip-rule="evenodd" d="M4 9.5H2.5v-7h7V4H11V2a1 1 0 0 0-1-1H2a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h2V9.5z"/></symbol></svg>"#;

/// GitHub flavor in the shape vendoring produces (GH §4): theme files (deliberately listed out
/// of cascade order), core files with every kind of `url()`, emoji, MathJax and mermaid.
pub fn github(b: BundleBuilder) -> BundleBuilder {
    let big = vec![0u8; BIG_IMAGE_LEN];
    b.stylesheet(
        Flavor::Github,
        "github/assets/code-c0de.css",
        "code",
        None,
        ".highlight{display:flex}",
    )
    .stylesheet(
        Flavor::Github,
        "github/assets/dark-d4a4.css",
        "theme",
        Some("dark"),
        "[data-color-mode=dark]{--fgColor-default:#f0f6fc}",
    )
    .stylesheet(
        Flavor::Github,
        "github/assets/light-1167.css",
        "theme",
        Some("light"),
        "[data-color-mode=light]{--fgColor-default:#1f2328}",
    )
    .stylesheet(
        Flavor::Github,
        "github/assets/dark_dimmed-d1d1.css",
        "theme",
        Some("dark_dimmed"),
        "[data-dark-theme=dark_dimmed]{--x:1}",
    )
    .stylesheet(
        Flavor::Github,
        "github/assets/primer-primitives-0001.css",
        "primitives",
        None,
        ":root{--base-size-8:8px}",
    )
    .stylesheet(
        Flavor::Github,
        "github/assets/primer-0002.css",
        "primer",
        None,
        ".select{background:url(\"data:image/svg+xml;base64,PHN2Zy8+\")}",
    )
    .stylesheet(
        Flavor::Github,
        "github/assets/global-0003.css",
        "global",
        None,
        ".spin{background:url(../images/spinners/octocat-spinner-16px.gif)}\
             .gone{background:url(/images/never-vendored.png)}\
             .ext{background:url(https://example.com/x.png)}",
    )
    .stylesheet(
        Flavor::Github,
        "github/assets/github-0004.css",
        "github",
        None,
        ".promo{background-image:url('../images/modules/promo.png')}",
    )
    .stylesheet(
        Flavor::Github,
        "github/assets/repository-0005.css",
        "repository",
        None,
        "@import \"repo-extra.css\";.r{color:red}",
    )
    .asset(
        Flavor::Github,
        "github/assets/repo-extra.css",
        b".extra{background:url(../images/x.svg#frag)}",
    )
    .asset(
        Flavor::Github,
        "github/images/x.svg",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .asset(
        Flavor::Github,
        "github/images/spinners/octocat-spinner-16px.gif",
        GIF,
    )
    .asset(Flavor::Github, "github/images/modules/promo.png", &big)
    .asset(Flavor::Github, "github/emoji/octocat.png", PNG)
    .script(
        Flavor::Github,
        "mathjax",
        "github/lib/mathjax/es5/tex-chtml-full.js",
        b"window.MJ = '</script><!--';",
    )
    .script(
        Flavor::Github,
        "mathjax-safe",
        "github/lib/mathjax/es5/ui/safe.js",
        b"window.MJSAFE = 1;",
    )
    .script(
        Flavor::Github,
        "mermaid",
        "github/lib/mermaid/mermaid.min.js",
        b"window.mermaid = {};",
    )
}

/// GitLab flavor in the shape vendoring produces (GL §11, §13).
pub fn gitlab(b: BundleBuilder) -> BundleBuilder {
    b.stylesheet(Flavor::Gitlab, "gitlab/assets/highlight/themes/dark-5555.css", "highlight", Some("dark"), ".code.dark{color:#c5c8c6}")
        .stylesheet(
            Flavor::Gitlab,
            "gitlab/assets/application_dark-2222.css",
            "app",
            Some("dark"),
            ":root.gl-dark{--gl-text-color-default:#ececef}",
        )
        .stylesheet(
            Flavor::Gitlab,
            "gitlab/assets/application-1111.css",
            "app",
            Some("light"),
            ".md h1 a.anchor::after{background-image:url(./icon_anchor-abc.svg)}",
        )
        .stylesheet(Flavor::Gitlab, "gitlab/assets/tailwind-3333.css", "tailwind", None, ".gl-sr-only{position:absolute}")
        .stylesheet(
            Flavor::Gitlab,
            "gitlab/assets/fonts-4444.css",
            "fonts",
            None,
            "@font-face{font-family:\"GitLab Sans\";src:url(./gitlab-sans/GitLabSans-abc.woff2) format(\"woff2\")}",
        )
        .stylesheet(Flavor::Gitlab, "gitlab/assets/highlight/themes/white-6666.css", "highlight", Some("light"), ".code.white{color:#3a383d}")
        .asset(Flavor::Gitlab, "gitlab/assets/icon_anchor-abc.svg", b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>")
        .asset(Flavor::Gitlab, "gitlab/assets/gitlab-sans/GitLabSans-abc.woff2", WOFF2)
        .asset(Flavor::Gitlab, "gitlab/assets/icons-0123456789abcdef.svg", b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>")
        .asset(Flavor::Gitlab, "gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff2", WOFF2)
        .asset(Flavor::Gitlab, "gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff", b"wOFFfake-legacy-font")
        .asset(Flavor::Gitlab, "gitlab/lib/katex/fonts/KaTeX_Main-Regular.ttf", b"fake-truetype-font")
        .script(Flavor::Gitlab, "katex", "gitlab/lib/katex/katex.min.js", b"window.katex = {};")
        .script(
            Flavor::Gitlab,
            "katex-css",
            "gitlab/lib/katex/katex.min.css",
            b"@font-face{font-family:KaTeX_Main;src:url(fonts/KaTeX_Main-Regular.woff2) format(\"woff2\"),\
              url(fonts/KaTeX_Main-Regular.woff) format(\"woff\"),url(fonts/KaTeX_Main-Regular.ttf) format(\"truetype\")}",
        )
        .script(Flavor::Gitlab, "mermaid", "gitlab/lib/mermaid/mermaid.min.js", b"window.mermaid = {};")
}

/// Both flavors.
pub fn full_bundle(dir: &Path) -> Bundle {
    gitlab(github(BundleBuilder::new())).open(dir)
}

pub fn theme(theme: Theme, light: &str, dark: &str) -> ThemeSel {
    ThemeSel {
        theme,
        light: light.into(),
        dark: dark.into(),
    }
}

/// Typical rendered content per flavor; `sourcepos` adds `data-sourcepos` like the server does.
pub fn content(flavor: Flavor, features: Features) -> Rendered {
    let html = match flavor {
        Flavor::Github => concat!(
            "<div class=\"markdown-heading\" dir=\"auto\" data-sourcepos=\"1:1-1:7\"><h1 class=\"heading-element\" dir=\"auto\">Title</h1>",
            "<a id=\"user-content-title\" class=\"anchor\" aria-label=\"Permalink: Title\" href=\"#title\"></a></div>\n",
            "<p dir=\"auto\" data-sourcepos=\"3:1-3:20\">Hi <img class=\"emoji\" title=\":octocat:\" alt=\":octocat:\" ",
            "src=\"/__/md-render/bundle/github/emoji/octocat.png\" height=\"20\" width=\"20\" align=\"absmiddle\"> ",
            "<a href=\"docs/x.md\">rel</a> <img src=\"https://example.com/remote.png\" alt=\"remote\"></p>\n",
            "<div class=\"highlight highlight-source-rust notranslate position-relative overflow-auto\" dir=\"auto\" ",
            "data-snippet-clipboard-copy-content=\"fn main() {}\" data-sourcepos=\"5:1-7:3\"><pre>fn main() {}</pre></div>\n",
        )
        .to_owned(),
        Flavor::Gitlab => concat!(
            "<h1 id=\"user-content-title\" data-sourcepos=\"1:1-1:7\" dir=\"auto\">Title<a href=\"#title\" ",
            "aria-label=\"Link to heading 'Title'\" data-heading-content=\"Title\" class=\"anchor\"></a></h1>\n",
            "<p data-sourcepos=\"3:1-3:5\" dir=\"auto\">Hello</p>\n",
            "<div class=\"gl-relative markdown-code-block js-markdown-code\"><pre data-sourcepos=\"5:1-7:3\" ",
            "class=\"code highlight js-syntax-highlight language-rust code-syntax-highlight-theme\" v-pre=\"true\">",
            "<code><span id=\"LC1\" class=\"line\" data-lang=\"rust\">fn main() {}</span></code></pre>",
            "<copy-code></copy-code><insert-code-snippet></insert-code-snippet></div>\n",
        )
        .to_owned(),
    };
    Rendered {
        html,
        title: Some("Title".into()),
        features,
        warnings: Vec::new(),
    }
}

pub fn server() -> AssetMode {
    AssetMode::Server {
        prefix: "/__/md-render".into(),
    }
}

pub fn live_info() -> LiveInfo {
    LiveInfo {
        events_url: "/__/md-render/events?path=docs/README.md".into(),
        render_url: "/__/md-render/render?path=docs/README.md".into(),
        doc: "docs/README.md".into(),
        version: 7,
        server_id: "abc123".into(),
    }
}

/// Everything a page test varies, with sensible defaults.
pub struct Page<'a> {
    pub flavor: Flavor,
    pub mode: Mode,
    pub theme: ThemeSel,
    pub layout: Layout,
    pub title: &'a str,
    pub file_name: Option<&'a str>,
    pub content: Rendered,
    pub assets: AssetMode,
    pub live: Option<LiveInfo>,
    pub banners: Vec<Banner>,
    pub extra_css: Vec<String>,
    /// `PageInput::bundle_path` (`None`: the bundle's own file).
    pub bundle_path: Option<PathBuf>,
}

impl<'a> Page<'a> {
    pub fn new(flavor: Flavor) -> Self {
        Page {
            flavor,
            mode: Mode::Readme,
            theme: ThemeSel::default(),
            layout: Layout::Readme,
            title: "README.md",
            file_name: Some("README.md"),
            content: content(flavor, Features::default()),
            assets: server(),
            live: None,
            banners: Vec::new(),
            extra_css: Vec::new(),
            bundle_path: None,
        }
    }

    pub fn render(&self, bundle: &Bundle) -> String {
        let input = PageInput {
            flavor: self.flavor,
            mode: self.mode,
            theme: &self.theme,
            layout: self.layout,
            title: self.title,
            file_name: self.file_name,
            content: &self.content,
            assets: &self.assets,
            live: self.live.as_ref(),
            banners: &self.banners,
            extra_css: &self.extra_css,
            bundle_path: self.bundle_path.as_deref(),
        };
        render_page(&input, bundle).expect("page renders")
    }
}

/// Collects what a `tracing` subscriber writes.
#[derive(Clone, Default)]
pub struct LogCapture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'w> tracing_subscriber::fmt::MakeWriter<'w> for LogCapture {
    type Writer = LogCapture;
    fn make_writer(&'w self) -> LogCapture {
        self.clone()
    }
}

/// Warnings (and errors) logged on this thread while `f` runs.
pub fn warnings_during<T>(f: impl FnOnce() -> T) -> (T, String) {
    let capture = LogCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_writer(capture.clone())
        .finish();
    let value = tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8_lossy(&capture.0.lock().unwrap()).into_owned();
    (value, text)
}

/// `href`s of the stylesheet `<link>`s, in document order.
pub fn stylesheet_hrefs(doc: &Document) -> Vec<String> {
    doc.select("link[rel=stylesheet]")
        .nodes()
        .iter()
        .filter_map(|n| n.attr("href"))
        .map(|s| s.to_string())
        .collect()
}

/// `src`s of the `<script src>` elements, in document order.
pub fn script_srcs(doc: &Document) -> Vec<String> {
    doc.select("script[src]")
        .nodes()
        .iter()
        .filter_map(|n| n.attr("src"))
        .map(|s| s.to_string())
        .collect()
}

/// The parsed `mdr-config` JSON block.
pub fn config(doc: &Document) -> serde_json::Value {
    let sel = doc.select("script#mdr-config[type=\"application/json\"]");
    assert_eq!(sel.length(), 1, "exactly one mdr-config block");
    serde_json::from_str(&sel.text()).expect("mdr-config is valid JSON")
}

pub fn attr(doc: &Document, selector: &str, name: &str) -> Option<String> {
    doc.select(selector).attr(name).map(|s| s.to_string())
}

/// Every `url(...)` argument in `css`, unquoted (raw, escapes kept). Comments and strings
/// outside `url()` are skipped, as a CSS parser does.
pub fn css_urls(css: &str) -> Vec<String> {
    let re = regex::Regex::new(
        r#"(?i)/\*[\s\S]*?\*/|url\(\s*(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|((?:\\.|[^)"'\s\\])*))\s*\)|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'"#,
    )
    .expect("regex");
    re.captures_iter(css)
        .filter_map(|c| {
            c.get(1)
                .or(c.get(2))
                .or(c.get(3))
                .map(|m| m.as_str().to_owned())
        })
        .collect()
}

/// Assert a self-contained document: no `<link>`/`<script>`/`<img>`/`url()` reference leaves the
/// file (only `data:` URIs, fragments, and content links the author wrote).
pub fn assert_self_contained(html: &str) {
    let doc = Document::from(html);
    for n in doc.select("link").nodes() {
        let href = n.attr("href").map(|s| s.to_string()).unwrap_or_default();
        assert!(href.starts_with("data:"), "external <link href={href:?}>");
    }
    assert_eq!(
        doc.select("script[src]").length(),
        0,
        "no <script src> in an inline export"
    );
    for n in doc.select("[src]").nodes() {
        let src = n.attr("src").map(|s| s.to_string()).unwrap_or_default();
        assert!(!src.starts_with("/__/"), "internal URL left in src={src:?}");
    }
    for n in doc.select("style").nodes() {
        for url in css_urls(&n.text()) {
            assert!(
                url.starts_with("data:") || url.starts_with('#'),
                "url({url}) in an inlined stylesheet"
            );
        }
    }
    assert!(
        !html.contains("/__/md-render/"),
        "internal route left in the export"
    );
}

/// A renderer double that returns fixed output and records the options it was called with.
pub struct FixedRenderer {
    pub output: Rendered,
    pub seen: Mutex<Vec<(String, RenderOptions)>>,
    pub fail: bool,
}

impl FixedRenderer {
    pub fn new(output: Rendered) -> Self {
        FixedRenderer {
            output,
            seen: Mutex::new(Vec::new()),
            fail: false,
        }
    }
}

impl Renderer for FixedRenderer {
    fn render(&self, markdown: &str, opts: &RenderOptions) -> anyhow::Result<Rendered> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push((markdown.to_owned(), opts.clone()));
        }
        if self.fail {
            anyhow::bail!("synthetic renderer failure");
        }
        let mut out = self.output.clone();
        // Like the real renderers, honour the bundle URL for custom emoji.
        // `data-sourcepos` is deliberately kept: the page must strip it from exports anyway.
        out.html = out.html.replace("/__/md-render/bundle", &opts.bundle_url);
        Ok(out)
    }

    fn name(&self) -> &'static str {
        "fixed"
    }
}
