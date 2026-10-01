//! End-to-end tests of `md-render vendor` through the public API.
//!
//! The default tests run the real HTTP client ([`UreqFetcher`]) against a small mock HTTP
//! server on 127.0.0.1 (no network): the mock serves a GitHub-like and a GitLab-like site, and a
//! transparent "proxy" [`Fetch`] wrapper maps the pinned library / emoji URLs (jsDelivr,
//! github.githubassets.com) onto it, reporting the original URLs back, so the vendoring code
//! sees exactly what it would see in production.
//!
//! `live_vendor_both_flavors` (ignored) vendors the real sites; run it with
//! `cargo test --test styles_vendor -- --ignored --nocapture`.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use md_render::Flavor;
use md_render::styles::manifest::{role, script};
use md_render::styles::vendor::{
    self, Fetch, Response, USER_AGENT, UreqFetcher, VendorOptions, libs,
};
use md_render::styles::{Bundle, BundleOrigin, SharedBundle, css_references, resolve_bundle_ref};

// ------------------------------------------------------------------------------------------------
// Mock HTTP server

#[derive(Clone, Debug)]
struct Route {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    location: Option<String>,
}

#[derive(Default)]
struct Site {
    routes: Mutex<HashMap<String, Route>>,
    /// (path, User-Agent) of every request, in arrival order.
    log: Mutex<Vec<(String, String)>>,
}

impl Site {
    fn set(&self, path: &str, status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) {
        let route = Route {
            status,
            content_type,
            body: body.into(),
            location: None,
        };
        self.routes
            .lock()
            .expect("routes")
            .insert(path.to_owned(), route);
    }

    fn ok(&self, path: &str, content_type: &'static str, body: impl Into<Vec<u8>>) {
        self.set(path, 200, content_type, body);
    }

    fn redirect(&self, path: &str, to: &str) {
        let route = Route {
            status: 301,
            content_type: "text/plain",
            body: Vec::new(),
            location: Some(to.into()),
        };
        self.routes
            .lock()
            .expect("routes")
            .insert(path.to_owned(), route);
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.log.lock().expect("log").clone()
    }

    fn requested(&self, path: &str) -> usize {
        self.requests().iter().filter(|(p, _)| p == path).count()
    }
}

/// A running mock server: `base` is `http://127.0.0.1:<port>`.
struct Server {
    base: String,
    site: Arc<Site>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let site = Arc::new(Site::default());
        let shared = site.clone();
        // The accept loop lives as long as the test process; every test has its own port.
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let site = shared.clone();
                std::thread::spawn(move || handle(stream, &site));
            }
        });
        Server { base, site }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

fn handle(mut stream: TcpStream, site: &Site) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let target = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_owned();
    let mut user_agent = String::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) if line == "\r\n" || line == "\n" => break,
            Ok(_) => match line.split_once(':') {
                Some((name, value)) if name.eq_ignore_ascii_case("user-agent") => {
                    user_agent = value.trim().to_owned();
                }
                _ => {}
            },
        }
    }
    site.log
        .lock()
        .expect("log")
        .push((target.clone(), user_agent));
    let route = site
        .routes
        .lock()
        .expect("routes")
        .get(&target)
        .cloned()
        .unwrap_or(Route {
            status: 404,
            content_type: "text/plain",
            body: b"not found".to_vec(),
            location: None,
        });
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        route.status,
        route.content_type,
        route.body.len()
    );
    if let Some(l) = &route.location {
        head.push_str(&format!("Location: {l}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&route.body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

/// A [`Fetch`] that sends production URLs with a known prefix to the mock server and reports
/// the production URL back, so the pipeline cannot tell the difference.
struct Proxy {
    inner: UreqFetcher,
    /// (production prefix, mock prefix)
    map: Vec<(String, String)>,
}

impl Proxy {
    fn new(server: &Server) -> Proxy {
        Proxy {
            inner: UreqFetcher::new(),
            map: vec![
                (libs::JSDELIVR_NPM.to_owned(), server.url("/cdn/npm/")),
                (libs::GITHUB_EMOJI_BASE.to_owned(), server.url("/emoji/")),
            ],
        }
    }
}

impl Fetch for Proxy {
    fn get(&self, url: &str) -> anyhow::Result<Response> {
        let to_mock = self.map.iter().find_map(|(prod, mock)| {
            url.strip_prefix(prod.as_str())
                .map(|rest| format!("{mock}{rest}"))
        });
        let mut resp = self.inner.get(to_mock.as_deref().unwrap_or(url))?;
        if to_mock.is_some() {
            for (prod, mock) in &self.map {
                if let Some(rest) = resp.final_url.strip_prefix(mock.as_str()) {
                    resp.final_url = format!("{prod}{rest}");
                    break;
                }
            }
        }
        Ok(resp)
    }

    fn user_agent(&self) -> &str {
        self.inner.user_agent()
    }
}

// ------------------------------------------------------------------------------------------------
// Fixtures

const GH_HASH: &str = "0123456789abcdef";
const GL_HASH: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn gh_css(name: &str) -> String {
    format!("/assets/{name}-{GH_HASH}.css")
}

fn gl_css(name: &str) -> String {
    format!("/assets/{name}-{GL_HASH}.css")
}

/// GitHub-like repository page at `/github/o/r` (reached through a redirect from `/github/old`).
fn github_site(server: &Server) {
    let s = &server.site;
    let mut links = String::new();
    for t in ["light", "light_high_contrast", "dark", "dark_dimmed"] {
        links.push_str(&format!(
            r#"<link crossorigin="anonymous" media="all" rel="stylesheet" href="{}" />"#,
            gh_css(t)
        ));
    }
    // A lazily enabled theme (data-href only).
    links.push_str(&format!(
        r#"<link data-color-theme="dark_tritanopia" media="all" rel="stylesheet" data-href="{}" />"#,
        gh_css("dark_tritanopia")
    ));
    for n in [
        "primer-primitives",
        "primer",
        "global",
        "github",
        "repository",
        "code",
    ] {
        links.push_str(&format!(
            r#"<link media="all" rel="stylesheet" href="{}" />"#,
            gh_css(n)
        ));
    }
    links.push_str(
        r#"<link media="all" rel="stylesheet" href="/assets/app.0123456789abcdef.module.css" />"#,
    );
    s.ok(
        "/github/o/r",
        "text/html; charset=utf-8",
        format!("<!DOCTYPE html><html><head>{links}</head><body></body></html>"),
    );
    s.redirect("/github/old", "/github/o/r");

    for t in [
        "light",
        "light_high_contrast",
        "dark",
        "dark_dimmed",
        "dark_tritanopia",
    ] {
        s.ok(
            &gh_css(t),
            "text/css",
            format!("[data-color-mode=light][data-light-theme={t}]{{--fgColor-default:#111}}"),
        );
    }
    s.ok(
        &gh_css("primer-primitives"),
        "text/css",
        "@layer primer-css-base;:root{--base-size-8:.5rem}",
    );
    s.ok(
        &gh_css("primer"),
        "text/css",
        r#".markdown-body{font-size:16px}select{background-image:url("data:image/svg+xml;base64,PHN2Zz4=")}"#,
    );
    s.ok(
        &gh_css("global"),
        "text/css",
        "/* url(/images/in-a-comment.png) */\
         .a{background:url(/images/spinners/octocat-spinner-32.gif)}\
         .b{background:url('/images/modules/pulls/progressive-disclosure-line.svg')}\
         .c{mask:url(images/modules/signup/icons/alert-fill-12.svg)}\
         .d{background:URL( \"/images/spinners/octocat-spinner-32.gif#again\" )}\
         .e{content:\"url(/images/in-a-string.png)\"}",
    );
    s.ok(
        &gh_css("github"),
        "text/css",
        ".x{background:url(/images/redirected.png)}",
    );
    s.ok(&gh_css("repository"), "text/css", ".y{}");
    s.ok(&gh_css("code"), "text/css", ".z{}");
    s.ok(
        "/images/spinners/octocat-spinner-32.gif",
        "image/gif",
        b"GIF89a-spinner".to_vec(),
    );
    s.ok(
        "/images/modules/pulls/progressive-disclosure-line.svg",
        "image/svg+xml",
        "<svg/>",
    );
    // `/assets/images/modules/signup/icons/alert-fill-12.svg` is a 404 (as on github.com).
    s.redirect("/images/redirected.png", "/images/real.png");
    s.ok("/images/real.png", "image/png", b"PNG-real".to_vec());

    let mj = format!("/cdn/npm/mathjax@{}/", libs::MATHJAX_VERSION);
    s.ok(
        &format!("{mj}es5/tex-chtml-full.js"),
        "application/javascript",
        "/* mathjax */",
    );
    s.ok(
        &format!("{mj}es5/ui/safe.js"),
        "application/javascript",
        "/* safe */",
    );
    s.ok(
        &format!(
            "/cdn/npm/mermaid@{}/dist/mermaid.min.js",
            libs::GITHUB_MERMAID_VERSION
        ),
        "application/javascript",
        "/* mermaid gh */",
    );
    for name in libs::GITHUB_CUSTOM_EMOJI {
        if name != "trollface" {
            s.ok(
                &format!("/emoji/{name}.png"),
                "image/png",
                format!("PNG-{name}"),
            );
        }
    }
}

/// GitLab-like project page at `/gitlab/g/p`.
fn gitlab_site(server: &Server) {
    let s = &server.site;
    let page = format!(
        r#"<!DOCTYPE html><html class="gl-system ui-neutral"><head>
<link rel="stylesheet" href="{app}" media="(prefers-color-scheme: light)" />
<link rel="stylesheet" href="{dark}" media="(prefers-color-scheme: dark)" />
<link rel="stylesheet" href="/assets/page_bundles/project-{GL_HASH}.css" />
<link rel="stylesheet" href="{tw}" />
<link rel="stylesheet" href="{fonts}" />
<link rel="stylesheet" href="/assets/highlight/themes/white-{GL_HASH}.css" media="(prefers-color-scheme: light)" />
<link rel="stylesheet" href="/assets/highlight/themes/dark-{GL_HASH}.css" media="(prefers-color-scheme: dark)" />
</head><body><svg><use href="/assets/icons-{GL_HASH}.svg#branch"></use></svg></body></html>"#,
        app = gl_css("application"),
        dark = gl_css("application_dark"),
        tw = gl_css("tailwind"),
        fonts = gl_css("fonts"),
    );
    s.ok("/gitlab/g/p", "text/html; charset=utf-8", page);
    let anchor = format!(".md h1 a.anchor::after{{background:url(./icon_anchor-{GL_HASH}.svg)}}");
    s.ok(
        &gl_css("application"),
        "text/css; charset=utf-8",
        format!("{anchor}.md{{color:#333}}"),
    );
    s.ok(
        &gl_css("application_dark"),
        "text/css; charset=utf-8",
        format!("{anchor}.md{{color:#eee}}"),
    );
    s.ok(
        &gl_css("tailwind"),
        "text/css",
        ".gl-sr-only{position:absolute}",
    );
    s.ok(
        &gl_css("fonts"),
        "text/css",
        format!(
            "@font-face{{font-family:GitLab Sans;src:url(./gitlab-sans/GitLabSans-{GL_HASH}.woff2) format(\"woff2\")}}"
        ),
    );
    s.ok(
        &format!("/assets/highlight/themes/white-{GL_HASH}.css"),
        "text/css",
        format!(".code-syntax-highlight-theme .x{{background:url(/assets/icons-stacked-{GL_HASH}.svg#git)}}"),
    );
    s.ok(
        &format!("/assets/highlight/themes/dark-{GL_HASH}.css"),
        "text/css",
        ".code-syntax-highlight-theme{}",
    );
    s.ok(
        &format!("/assets/icon_anchor-{GL_HASH}.svg"),
        "image/svg+xml",
        "<svg id=anchor/>",
    );
    s.ok(
        &format!("/assets/gitlab-sans/GitLabSans-{GL_HASH}.woff2"),
        "application/octet-stream",
        b"wOF2sans".to_vec(),
    );
    s.ok(
        &format!("/assets/icons-stacked-{GL_HASH}.svg"),
        "image/svg+xml",
        "<svg id=stacked/>",
    );
    s.ok(
        &format!("/assets/icons-{GL_HASH}.svg"),
        "image/svg+xml",
        "<svg id=sprite/>",
    );

    let k = format!("/cdn/npm/katex@{}/dist/", libs::KATEX_VERSION);
    s.ok(
        &format!("{k}katex.min.js"),
        "application/javascript",
        "/* katex */",
    );
    let mut katex_css = String::from(".katex{font:normal 1.21em KaTeX_Main}");
    for f in ["KaTeX_Main-Regular", "KaTeX_AMS-Regular"] {
        katex_css.push_str(&format!(
            "@font-face{{font-family:{f};src:url(fonts/{f}.woff2) format(\"woff2\"),url(fonts/{f}.woff) format(\"woff\"),url(fonts/{f}.ttf) format(\"truetype\")}}"
        ));
        s.ok(
            &format!("{k}fonts/{f}.woff2"),
            "font/woff2",
            format!("wOF2-{f}"),
        );
    }
    s.ok(&format!("{k}katex.min.css"), "text/css", katex_css);
    s.ok(
        &format!(
            "/cdn/npm/mermaid@{}/dist/mermaid.min.js",
            libs::GITLAB_MERMAID_VERSION
        ),
        "application/javascript",
        "/* mermaid gl */",
    );
}

fn options(server: &Server, out: &Path, flavors: &[Flavor]) -> VendorOptions {
    VendorOptions {
        flavors: flavors.to_vec(),
        output: out.to_owned(),
        github_source: server.url("/github/old"),
        gitlab_source: server.url("/gitlab/g/p"),
        all_github_themes: false,
        libs: true,
        quiet: true,
    }
}

/// Every relative reference in every vendored stylesheet (and `@import`ed or library CSS)
/// resolves to a file in the bundle, or to a download recorded as failed, or to a KaTeX
/// `.woff`/`.ttf` fallback that is deliberately not vendored. Returns the number of resolved
/// references.
fn assert_references_resolve(bundle: &Bundle) -> usize {
    let mut resolved = 0;
    for (flavor, fm) in &bundle.manifest().flavors {
        let failed: Vec<&str> = fm.failures.iter().map(|f| f.url.as_str()).collect();
        let css_files = bundle
            .paths()
            .filter(|p| p.starts_with(flavor.as_str()) && p.ends_with(".css"));
        for css_path in css_files.map(str::to_owned).collect::<Vec<_>>() {
            let css = bundle.read(&css_path).expect("listed file");
            let css = std::str::from_utf8(&css).expect("utf-8 css");
            for r in css_references(css) {
                if r.starts_with("data:") || r.starts_with('#') {
                    continue;
                }
                assert!(
                    !r.starts_with('/') && !r.contains("://"),
                    "{css_path}: reference {r:?} was not rewritten to a relative path"
                );
                let target = resolve_bundle_ref(&css_path, &r)
                    .unwrap_or_else(|| panic!("{css_path}: {r:?} does not resolve"));
                if bundle.read(&target).is_some() {
                    resolved += 1;
                } else {
                    let skipped_font = css_path.contains("/lib/katex/")
                        && (target.ends_with(".woff") || target.ends_with(".ttf"));
                    let file = target.rsplit('/').next().unwrap_or_default();
                    let recorded = failed.iter().any(|u| u.ends_with(file));
                    assert!(
                        skipped_font || recorded,
                        "{css_path}: {r:?} -> {target} is neither in the bundle nor a recorded failure"
                    );
                }
            }
        }
    }
    resolved
}

fn text(bundle: &Bundle, path: &str) -> String {
    let b = bundle
        .read(path)
        .unwrap_or_else(|| panic!("{path} missing"));
    String::from_utf8(b.to_vec()).expect("utf-8")
}

// ------------------------------------------------------------------------------------------------
// Tests

#[test]
fn vendors_github_through_real_http() {
    let server = Server::start();
    github_site(&server);
    let dir = tempfile::tempdir().expect("tempdir");
    // Parent directories are created.
    let out = dir.path().join("nested/cache/styles.tar.gz");
    let report = vendor::vendor_with(
        &options(&server, &out, &[Flavor::Github]),
        &Proxy::new(&server),
    )
    .expect("vendor");
    assert_eq!(report.output, out);

    let bundle = Bundle::open(&out).expect("open");
    assert_eq!(bundle.origin(), &BundleOrigin::File(out.clone()));
    let m = bundle.manifest();
    assert_eq!(m.schema, md_render::styles::manifest::SCHEMA);
    assert_eq!(m.tool, md_render::TOOL);
    assert!(m.flavors.get(&Flavor::Gitlab).is_none());
    let gh = bundle.flavor(Flavor::Github).expect("github");
    assert!(!gh.fallback);
    assert_eq!(
        gh.source_page,
        server.url("/github/o/r"),
        "redirect target is recorded"
    );
    assert_eq!(gh.user_agent, USER_AGENT);
    assert!(
        gh.fetched_at.ends_with('Z') && gh.fetched_at.len() == 20,
        "{}",
        gh.fetched_at
    );

    // Roles in cascade order; only light + dark by default; *.module.css never fetched.
    let roles: Vec<(&str, Option<&str>, &str)> = gh
        .stylesheets
        .iter()
        .map(|s| (s.role.as_str(), s.theme.as_deref(), s.path.as_str()))
        .collect();
    assert_eq!(
        roles,
        vec![
            (
                role::THEME,
                Some("light"),
                "github/assets/light-0123456789abcdef.css"
            ),
            (
                role::THEME,
                Some("dark"),
                "github/assets/dark-0123456789abcdef.css"
            ),
            (
                role::PRIMITIVES,
                None,
                "github/assets/primer-primitives-0123456789abcdef.css"
            ),
            (
                role::PRIMER,
                None,
                "github/assets/primer-0123456789abcdef.css"
            ),
            (
                role::GLOBAL,
                None,
                "github/assets/global-0123456789abcdef.css"
            ),
            (
                role::GITHUB,
                None,
                "github/assets/github-0123456789abcdef.css"
            ),
            (
                role::REPOSITORY,
                None,
                "github/assets/repository-0123456789abcdef.css"
            ),
            (role::CODE, None, "github/assets/code-0123456789abcdef.css"),
        ]
    );
    assert!(
        gh.stylesheets.iter().all(|s| s.media.is_none()),
        "media=all is the default"
    );
    assert_eq!(gh.themes, vec!["light", "dark"]);
    assert_eq!(
        server
            .site
            .requested("/assets/app.0123456789abcdef.module.css"),
        0
    );
    assert_eq!(server.site.requested(&gh_css("dark_dimmed")), 0);

    // url() rewriting: root-relative -> ../images, quoted forms keep their quotes, fragments
    // survive, comments and strings are left alone, data: untouched.
    assert_eq!(
        text(&bundle, "github/assets/global-0123456789abcdef.css"),
        "/* url(/images/in-a-comment.png) */\
         .a{background:url(../images/spinners/octocat-spinner-32.gif)}\
         .b{background:url('../images/modules/pulls/progressive-disclosure-line.svg')}\
         .c{mask:url(images/modules/signup/icons/alert-fill-12.svg)}\
         .d{background:URL( \"../images/spinners/octocat-spinner-32.gif#again\" )}\
         .e{content:\"url(/images/in-a-string.png)\"}"
    );
    assert!(
        text(&bundle, "github/assets/primer-0123456789abcdef.css")
            .contains("data:image/svg+xml;base64,PHN2Zz4=")
    );
    assert_eq!(
        server
            .site
            .requested("/images/spinners/octocat-spinner-32.gif"),
        1,
        "deduplicated"
    );
    assert_eq!(server.site.requested("/images/in-a-comment.png"), 0);
    // A redirected asset is stored under the path of the URL the CSS references.
    assert_eq!(
        text(&bundle, "github/assets/github-0123456789abcdef.css"),
        ".x{background:url(../images/redirected.png)}"
    );
    assert_eq!(
        bundle.read("github/images/redirected.png").as_deref(),
        Some(&b"PNG-real"[..])
    );

    // Every stored file is described with its real hash and size.
    for s in &gh.stylesheets {
        let b = bundle.read(&s.path).expect("stylesheet");
        assert_eq!(s.size, b.len() as u64, "{}", s.path);
        assert_eq!(s.sha256.len(), 64);
        assert!(s.url.starts_with(&server.base), "{}", s.url);
    }
    for a in &gh.assets {
        assert_eq!(bundle.file_size(&a.path), Some(a.size), "{}", a.path);
    }

    // Libraries at the DESIGN §4 locations, with production URLs recorded.
    let scripts: BTreeMap<&str, &str> = gh
        .scripts
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        scripts,
        BTreeMap::from([
            (script::MATHJAX, "github/lib/mathjax/es5/tex-chtml-full.js"),
            (script::MATHJAX_SAFE, "github/lib/mathjax/es5/ui/safe.js"),
            (script::MERMAID, "github/lib/mermaid/mermaid.min.js"),
        ])
    );
    let mathjax = gh
        .assets
        .iter()
        .find(|a| a.path.ends_with("tex-chtml-full.js"))
        .expect("asset");
    assert_eq!(
        mathjax.url,
        "https://cdn.jsdelivr.net/npm/mathjax@3.2.0/es5/tex-chtml-full.js"
    );

    // 22 emoji present; trollface is a 404 and is recorded, not fatal.
    let emoji: Vec<&str> = gh
        .assets
        .iter()
        .filter(|a| a.path.starts_with("github/emoji/"))
        .map(|a| a.path.as_str())
        .collect();
    assert_eq!(emoji.len(), 22);
    assert_eq!(
        bundle.read("github/emoji/octocat.png").as_deref(),
        Some(&b"PNG-octocat"[..])
    );
    let failures: Vec<(&str, Option<u16>, Option<&str>)> = gh
        .failures
        .iter()
        .map(|f| (f.url.as_str(), f.status, f.referenced_by.as_deref()))
        .collect();
    let alert = server.url("/assets/images/modules/signup/icons/alert-fill-12.svg");
    assert_eq!(
        failures,
        vec![
            (
                alert.as_str(),
                Some(404),
                Some("github/assets/global-0123456789abcdef.css")
            ),
            (
                "https://github.githubassets.com/images/icons/emoji/trollface.png",
                Some(404),
                None
            ),
        ]
    );
    assert_eq!(report.failures, gh.failures);
    assert_eq!(report.files, gh.stylesheets.len() + gh.assets.len());
    assert_eq!(
        bundle.paths().count(),
        report.files + 1,
        "plus manifest.json"
    );

    assert!(assert_references_resolve(&bundle) >= 4);

    // Every request carried the md-render User-Agent.
    let requests = server.site.requests();
    assert!(!requests.is_empty());
    for (path, ua) in &requests {
        assert_eq!(ua, USER_AGENT, "{path}");
    }
}

#[test]
fn all_github_themes_include_lazy_data_href_links() {
    let server = Server::start();
    github_site(&server);
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    let mut opts = options(&server, &out, &[Flavor::Github]);
    opts.all_github_themes = true;
    opts.libs = false;
    vendor::vendor_with(&opts, &Proxy::new(&server)).expect("vendor");
    let bundle = Bundle::open(&out).expect("open");
    let gh = bundle.flavor(Flavor::Github).expect("github");
    assert_eq!(
        gh.themes,
        vec![
            "light",
            "light_high_contrast",
            "dark",
            "dark_dimmed",
            "dark_tritanopia"
        ]
    );
    let dimmed = gh
        .themed_stylesheet(role::THEME, "dark_tritanopia")
        .expect("data-href theme");
    assert_eq!(
        dimmed.path,
        "github/assets/dark_tritanopia-0123456789abcdef.css"
    );
    // Without libs: no scripts, no emoji.
    assert!(gh.scripts.is_empty());
    assert!(
        !bundle
            .paths()
            .any(|p| p.starts_with("github/emoji/") || p.starts_with("github/lib/"))
    );
    assert!(
        server
            .site
            .requests()
            .iter()
            .all(|(p, _)| !p.starts_with("/cdn/") && !p.starts_with("/emoji/"))
    );
}

#[test]
fn vendors_gitlab_and_preserves_github_on_read_modify_write() {
    let server = Server::start();
    github_site(&server);
    gitlab_site(&server);
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    let proxy = Proxy::new(&server);

    vendor::vendor_with(&options(&server, &out, &[Flavor::Github]), &proxy).expect("github");
    let first = Bundle::open(&out).expect("open");
    vendor::vendor_with(&options(&server, &out, &[Flavor::Gitlab]), &proxy).expect("gitlab");
    let both = Bundle::open(&out).expect("open");

    // GitHub is untouched: same manifest entry, same bytes.
    assert_eq!(both.flavor(Flavor::Github), first.flavor(Flavor::Github));
    let gh_paths: Vec<&str> = first.paths().filter(|p| p.starts_with("github/")).collect();
    assert_eq!(
        gh_paths,
        both.paths()
            .filter(|p| p.starts_with("github/"))
            .collect::<Vec<_>>()
    );
    for p in gh_paths {
        assert_eq!(first.read(p), both.read(p), "{p}");
    }

    let gl = both.flavor(Flavor::Gitlab).expect("gitlab");
    let sheets: Vec<(&str, Option<&str>, Option<&str>)> = gl
        .stylesheets
        .iter()
        .map(|s| (s.role.as_str(), s.theme.as_deref(), s.media.as_deref()))
        .collect();
    assert_eq!(
        sheets,
        vec![
            (
                role::APP,
                Some("light"),
                Some("(prefers-color-scheme: light)")
            ),
            (
                role::APP,
                Some("dark"),
                Some("(prefers-color-scheme: dark)")
            ),
            (role::TAILWIND, None, None),
            (role::FONTS, None, None),
            (
                role::HIGHLIGHT,
                Some("light"),
                Some("(prefers-color-scheme: light)")
            ),
            (
                role::HIGHLIGHT,
                Some("dark"),
                Some("(prefers-color-scheme: dark)")
            ),
        ]
    );
    assert_eq!(gl.themes, vec!["light", "dark"]);
    assert!(gl.failures.is_empty(), "{:?}", gl.failures);
    assert_eq!(
        server
            .site
            .requested(&format!("/assets/page_bundles/project-{GL_HASH}.css")),
        0
    );

    // Absolute /assets/ references in the highlight theme become relative (GL §12).
    let white = format!("gitlab/assets/highlight/themes/white-{GL_HASH}.css");
    assert_eq!(
        text(&both, &white),
        format!(
            ".code-syntax-highlight-theme .x{{background:url(../../icons-stacked-{GL_HASH}.svg#git)}}"
        )
    );
    assert!(
        text(&both, &format!("gitlab/assets/fonts-{GL_HASH}.css"))
            .contains(&format!("url(gitlab-sans/GitLabSans-{GL_HASH}.woff2)"))
    );
    assert!(
        both.read(&format!(
            "gitlab/assets/gitlab-sans/GitLabSans-{GL_HASH}.woff2"
        ))
        .is_some()
    );

    // Libraries and the copy-button sprite.
    let scripts: BTreeMap<&str, &str> = gl
        .scripts
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let sprite = format!("gitlab/assets/icons-{GL_HASH}.svg");
    assert_eq!(
        scripts,
        BTreeMap::from([
            (script::ICONS, sprite.as_str()),
            (script::KATEX, "gitlab/lib/katex/katex.min.js"),
            (script::KATEX_CSS, "gitlab/lib/katex/katex.min.css"),
            (script::MERMAID, "gitlab/lib/mermaid/mermaid.min.js"),
        ])
    );
    assert_eq!(
        text(&both, "gitlab/lib/mermaid/mermaid.min.js"),
        "/* mermaid gl */"
    );
    assert_eq!(
        text(&both, "github/lib/mermaid/mermaid.min.js"),
        "/* mermaid gh */"
    );
    // KaTeX: the woff2 fonts its CSS references, next to it; no woff/ttf requests.
    for f in ["KaTeX_Main-Regular", "KaTeX_AMS-Regular"] {
        assert!(
            both.read(&format!("gitlab/lib/katex/fonts/{f}.woff2"))
                .is_some(),
            "{f}"
        );
    }
    assert!(
        !server
            .site
            .requests()
            .iter()
            .any(|(p, _)| p.ends_with(".woff") || p.ends_with(".ttf"))
    );

    assert!(assert_references_resolve(&both) >= 8);

    // Re-vendoring GitHub replaces its tree (stale files go) and keeps GitLab.
    server.site.ok(
        &gh_css("global"),
        "text/css",
        ".a{background:url(/images/new.png)}",
    );
    server
        .site
        .ok("/images/new.png", "image/png", b"PNG-new".to_vec());
    vendor::vendor_with(&options(&server, &out, &[Flavor::Github]), &proxy).expect("github again");
    let again = Bundle::open(&out).expect("open");
    assert_eq!(again.flavor(Flavor::Gitlab), both.flavor(Flavor::Gitlab));
    assert!(again.read("github/images/new.png").is_some());
    assert!(
        again
            .read("github/images/spinners/octocat-spinner-32.gif")
            .is_none(),
        "stale file removed"
    );
    assert!(
        !again
            .flavor(Flavor::Github)
            .expect("github")
            .failures
            .iter()
            .any(|f| f.url.contains("alert-fill"))
    );
    assert_ne!(again.etag(), both.etag());
}

#[test]
fn vendors_both_flavors_in_one_run() {
    let server = Server::start();
    github_site(&server);
    gitlab_site(&server);
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    let report = vendor::vendor_with(
        &options(
            &server,
            &out,
            &[Flavor::Gitlab, Flavor::Github, Flavor::Gitlab],
        ),
        &Proxy::new(&server),
    )
    .expect("vendor");
    let bundle = Bundle::open(&out).expect("open");
    assert_eq!(
        bundle
            .manifest()
            .flavors
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        Flavor::ALL.to_vec()
    );
    let listed: usize = bundle
        .manifest()
        .flavors
        .values()
        .map(|fm| fm.stylesheets.len() + fm.assets.len())
        .sum();
    assert_eq!(report.files, listed);
    let bytes: u64 = bundle
        .paths()
        .filter(|p| *p != "manifest.json")
        .filter_map(|p| bundle.file_size(p))
        .sum();
    assert_eq!(report.bytes, bytes);
    // A complete bundle needs no fallbacks.
    let filled = bundle.clone().with_fallbacks();
    assert_eq!(filled.etag(), bundle.etag());
}

#[test]
fn stylesheet_failure_is_fatal_and_leaves_the_bundle_alone() {
    let server = Server::start();
    github_site(&server);
    gitlab_site(&server);
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    let proxy = Proxy::new(&server);
    vendor::vendor_with(&options(&server, &out, &[Flavor::Gitlab]), &proxy).expect("gitlab");
    let before = std::fs::read(&out).expect("read");

    server
        .site
        .set(&gh_css("primer"), 500, "text/plain", "boom");
    let e = vendor::vendor_with(&options(&server, &out, &Flavor::ALL), &proxy).expect_err("fatal");
    let msg = format!("{e:#}");
    assert!(
        msg.contains("HTTP 500") && msg.contains("primer-0123456789abcdef.css"),
        "{msg}"
    );
    assert!(msg.contains("left unchanged"), "{msg}");
    assert_eq!(
        std::fs::read(&out).expect("read"),
        before,
        "bundle untouched"
    );
    // No library was downloaded once the stylesheets failed.
    assert!(
        !server
            .site
            .requests()
            .iter()
            .any(|(p, _)| p.contains("mathjax"))
    );

    // A challenge page instead of CSS is fatal too.
    server.site.ok(
        &gh_css("primer"),
        "text/html",
        "<html>Just a moment...</html>",
    );
    let e =
        vendor::vendor_with(&options(&server, &out, &[Flavor::Github]), &proxy).expect_err("html");
    assert!(format!("{e:#}").contains("HTML page"), "{e:#}");

    // A page without the required stylesheets explains itself.
    server
        .site
        .ok("/github/o/r", "text/html", "<html><head></head></html>");
    let e =
        vendor::vendor_with(&options(&server, &out, &[Flavor::Github]), &proxy).expect_err("roles");
    assert!(format!("{e:#}").contains("does not link the"), "{e:#}");

    // An unreachable source page.
    let mut opts = options(&server, &out, &[Flavor::Github]);
    opts.github_source = "http://127.0.0.1:1/nothing".into();
    assert!(vendor::vendor_with(&opts, &proxy).is_err());
    assert_eq!(
        std::fs::read(&out).expect("read"),
        before,
        "bundle still untouched"
    );
}

#[test]
fn refuses_to_overwrite_unrelated_files() {
    let server = Server::start();
    gitlab_site(&server);
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("README.md");
    std::fs::write(&out, "# not a bundle").expect("write");
    let e = vendor::vendor_with(
        &options(&server, &out, &[Flavor::Gitlab]),
        &Proxy::new(&server),
    )
    .expect_err("refuse");
    assert!(format!("{e:#}").contains("refusing to overwrite"), "{e:#}");
    assert_eq!(
        std::fs::read_to_string(&out).expect("read"),
        "# not a bundle"
    );
}

#[test]
fn running_server_picks_up_a_revendored_bundle() {
    let server = Server::start();
    github_site(&server);
    gitlab_site(&server);
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    let proxy = Proxy::new(&server);

    let (shared, warning) = SharedBundle::load(&out);
    assert!(warning.expect("no bundle yet").contains("md-render vendor"));
    assert_eq!(shared.get().origin(), &BundleOrigin::Fallback);

    vendor::vendor_with(&options(&server, &out, &[Flavor::Github]), &proxy).expect("github");
    assert!(shared.reload().expect("reload"));
    let gh_only = shared.get();
    assert!(!gh_only.is_fallback(Flavor::Github));
    assert!(
        gh_only.is_fallback(Flavor::Gitlab),
        "missing flavor filled with the fallback"
    );
    assert!(gh_only.read("gitlab/fallback.css").is_some());
    assert!(!shared.reload().expect("unchanged"));

    vendor::vendor_with(&options(&server, &out, &[Flavor::Gitlab]), &proxy).expect("gitlab");
    assert!(shared.reload().expect("reload"));
    assert!(!shared.get().is_fallback(Flavor::Gitlab));
    // The old snapshot stays fully readable for in-flight requests.
    assert!(gh_only.read("github/lib/mermaid/mermaid.min.js").is_some());

    std::fs::write(&out, b"\x1f\x8b garbage").expect("corrupt");
    assert!(shared.reload().is_err());
    assert!(
        !shared.get().is_fallback(Flavor::Gitlab),
        "a corrupt file keeps the last good bundle"
    );
}

/// Vendors the real sites into a temporary bundle and checks the invariants. Needs network
/// access (github.com, github.githubassets.com, gitlab.com, cdn.jsdelivr.net), so it is
/// ignored by default: `cargo test --test styles_vendor -- --ignored --nocapture`.
/// `examples/vendor_live.rs` does the same into a file that is kept.
#[test]
#[ignore = "network: vendors github.com and gitlab.com"]
fn live_vendor_both_flavors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("styles.tar.gz");
    let opts = VendorOptions {
        output: out.clone(),
        quiet: false,
        ..VendorOptions::default()
    };
    let report = vendor::vendor(&opts).expect("vendor");
    let bundle = Bundle::open(&out).expect("open");
    let gh = bundle.flavor(Flavor::Github).expect("github");
    for (r, t) in [
        (role::THEME, Some("light")),
        (role::THEME, Some("dark")),
        (role::PRIMER, None),
        (role::GLOBAL, None),
    ] {
        assert!(
            gh.stylesheets
                .iter()
                .any(|s| s.role == r && s.theme.as_deref() == t),
            "{r} {t:?}"
        );
    }
    for k in [script::MATHJAX, script::MATHJAX_SAFE, script::MERMAID] {
        assert!(gh.script(k).is_some(), "github {k}");
    }
    let emoji = bundle
        .paths()
        .filter(|p| p.starts_with("github/emoji/"))
        .count();
    assert_eq!(emoji, libs::GITHUB_CUSTOM_EMOJI.len());
    let gl = bundle.flavor(Flavor::Gitlab).expect("gitlab");
    for (r, t) in [
        (role::APP, Some("light")),
        (role::APP, Some("dark")),
        (role::FONTS, None),
        (role::HIGHLIGHT, Some("light")),
    ] {
        assert!(
            gl.stylesheets
                .iter()
                .any(|s| s.role == r && s.theme.as_deref() == t),
            "{r} {t:?}"
        );
    }
    for k in [
        script::KATEX,
        script::KATEX_CSS,
        script::MERMAID,
        script::ICONS,
    ] {
        assert!(gl.script(k).is_some(), "gitlab {k}");
    }
    assert!(
        bundle
            .paths()
            .filter(|p| p.starts_with("gitlab/lib/katex/fonts/"))
            .count()
            >= 20
    );
    let resolved = assert_references_resolve(&bundle);
    println!(
        "{} files, {} bytes, {resolved} resolved references, failures: {:#?}",
        report.files, report.bytes, report.failures
    );
}
