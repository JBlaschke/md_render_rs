//! Path safety and DNS-rebinding protection (DESIGN D14, GRIP C39): traversal in every
//! encoding, symlink escapes, `.git`, and the loopback `Host` allowlist. Requests are sent
//! byte-for-byte over a raw socket so no client normalises the paths first.

mod server_support;

use server_support::{Fixture, TestServer};

const SECRET: &str = "TOP-SECRET-CONTENT";

/// A server whose root sits next to a secret file (`<tmp>/secret.txt`, `<tmp>/outside/`).
async fn server(follow_symlinks: bool) -> TestServer {
    let f = Fixture::new();
    f.file("README.md", "public readme");
    f.file("docs/README.md", "docs");
    f.file(".github/README.md", "github dir readme");
    f.file(".gitignore", "target/");
    f.file(".git/config", SECRET);
    f.file("sub/.git/HEAD", SECRET);
    server_support::write(&f.tmp.join("secret.txt"), SECRET);
    server_support::write(&f.tmp.join("secret.md"), SECRET);
    server_support::write(&f.tmp.join("outside/inner.txt"), SECRET);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(f.tmp.join("secret.txt"), f.root.join("escape.txt")).unwrap();
        std::os::unix::fs::symlink(f.tmp.join("secret.md"), f.root.join("escape.md")).unwrap();
        std::os::unix::fs::symlink(f.tmp.join("outside"), f.root.join("outdir")).unwrap();
        std::os::unix::fs::symlink(f.root.join("README.md"), f.root.join("inside.md")).unwrap();
    }
    f.start(|cfg| cfg.follow_symlinks = follow_symlinks).await
}

fn assert_no_leak(path: &str, r: &server_support::Response) {
    assert!(
        !r.text().contains(SECRET),
        "{path} leaked the secret: {} {}",
        r.status,
        r.text()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn traversal_is_rejected_in_every_encoding() {
    let s = server(false).await;
    let attempts = [
        "/../secret.txt",
        "/docs/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2E%2E/secret.txt",
        "/.%2e/secret.txt",
        "/docs/%2e%2e/%2e%2e/secret.txt",
        "/docs%2f..%2f..%2fsecret.txt",
        "/docs%2F%2E%2E%2F%2E%2E%2Fsecret.txt",
        "/..%5csecret.txt",
        "/docs%5c..%5c..%5csecret.txt",
        "/README.md%00.txt",
        "/../secret.md",
        "/%2e%2e/secret.md?raw=1",
    ];
    for path in attempts {
        let r = s.get(path).await;
        assert_eq!(r.status, 400, "{path}: {}", r.text());
        assert_no_leak(path, &r);
    }
    // A raw backslash is refused by the HTTP parser or by us; either way nothing leaks.
    let r = s.get("/..\\secret.txt").await;
    assert!(r.status >= 400, "raw backslash: {}", r.status);
    assert_no_leak("raw backslash", &r);
    // Decoding happens exactly once: `%252e%252e` is a literal directory name `%2e%2e`.
    let r = s.get("/%252e%252e/secret.txt").await;
    assert_eq!(r.status, 404);
    assert_no_leak("double encoding", &r);
    // Absolute-looking paths stay inside the root.
    for path in ["//etc/passwd", "/etc/passwd", "///etc//passwd"] {
        let r = s.get(path).await;
        assert_eq!(r.status, 404, "{path}");
        assert!(!r.text().contains("root:"), "{path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_endpoints_validate_their_path_parameter() {
    let s = server(false).await;
    for q in [
        "../secret.md",
        "%2e%2e/secret.md",
        "docs/../../secret.md",
        "a\\b.md",
        "x%00.md",
    ] {
        for endpoint in ["render", "events"] {
            let r = s.get(&format!("/__/md-render/{endpoint}?path={q}")).await;
            assert_eq!(r.status, 400, "{endpoint}?path={q}: {}", r.text());
            assert_no_leak(q, &r);
        }
    }
    for q in [".git/x.md", "sub/.git/HEAD.md"] {
        for endpoint in ["render", "events"] {
            let r = s.get(&format!("/__/md-render/{endpoint}?path={q}")).await;
            assert_eq!(r.status, 403, "{endpoint}?path={q}");
        }
    }
    // A leading slash is accepted and means the same document.
    let (status, v) = s.render_json("/README.md").await;
    assert_eq!(status, 200);
    assert!(v["html"].as_str().unwrap().contains("public readme"));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symlink_escapes_are_blocked() {
    let s = server(false).await;
    for path in [
        "/escape.txt",
        "/escape.md",
        "/escape.md?raw=1",
        "/outdir/",
        "/outdir",
        "/outdir/inner.txt",
        "/outdir/new.md",
    ] {
        let r = s.get(path).await;
        assert_eq!(r.status, 403, "{path}: {}", r.text());
        assert_no_leak(path, &r);
    }
    let (status, _) = s.render_json("escape.md").await;
    assert_eq!(status, 403);
    // Symlinks that stay inside the root are fine.
    let r = s.get("/inside.md").await;
    assert_eq!(r.status, 200);
    assert!(r.text().contains("public readme"));
    // Escaping links are not even listed.
    let r = s.get("/?listing").await;
    assert_no_leak("/", &r);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_symlinks_allows_links_out_of_the_root() {
    let s = server(true).await;
    let r = s.get("/escape.txt").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), SECRET);
    let r = s.get("/outdir/inner.txt").await;
    assert_eq!(r.status, 200);
    // `..` stays forbidden even then.
    assert_eq!(s.get("/../secret.txt").await.status, 400);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_document_replaced_by_an_escaping_symlink_is_not_read() {
    let s = server(false).await;
    assert_eq!(s.get("/docs/README.md").await.status, 200);
    std::fs::remove_file(s.path("docs/README.md")).unwrap();
    std::os::unix::fs::symlink(s.tmp.join("secret.md"), s.path("docs/README.md")).unwrap();
    let r = s.get("/docs/README.md").await;
    assert_eq!(r.status, 403);
    assert_no_leak("/docs/README.md", &r);
    let (_, v) = s.render_json("docs/README.md").await;
    assert_no_leak(
        "render",
        &server_support::Response {
            status: 0,
            headers: vec![],
            body: v.to_string().into_bytes(),
        },
    );
}

/// CLI-18: the file named as the server's target is served even when it is a symlink to a
/// file outside the root (the user chose it; grip and `export` render it too), with live
/// updates; other escaping symlinks stay forbidden, and a directory's README that escapes
/// is a 403 that says how to allow it, not a 500.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicitly_named_symlinked_target_is_served() {
    let f = Fixture::new();
    server_support::write(&f.tmp.join("notes/real.md"), "real notes");
    server_support::write(&f.tmp.join("other.md"), SECRET);
    server_support::write(&f.tmp.join("elsewhere/README.md"), SECRET);
    std::os::unix::fs::symlink(f.tmp.join("notes/real.md"), f.root.join("link.md")).unwrap();
    std::os::unix::fs::symlink(f.tmp.join("other.md"), f.root.join("other.md")).unwrap();
    std::fs::create_dir(f.root.join("sub")).unwrap();
    std::os::unix::fs::symlink(
        f.tmp.join("elsewhere/README.md"),
        f.root.join("sub/README.md"),
    )
    .unwrap();
    let s = f
        .start(|cfg| cfg.target = md_render::server::Target::File("link.md".into()))
        .await;
    for path in ["/", "/link.md"] {
        let r = s.get(path).await;
        assert_eq!(r.status, 200, "{path}: {}", r.text());
        assert!(r.text().contains("real notes"), "{path}");
    }
    let (status, v) = s.render_json("link.md").await;
    assert_eq!(status, 200, "{v}");
    let mut sse = s.events("link.md", "", &[]).await;
    assert_eq!(sse.status, 200);
    sse.wait_for("hello").await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    server_support::write_atomic(&s.tmp.join("notes/real.md"), "edited notes");
    sse.wait_until("content", |c| {
        c["html"]
            .as_str()
            .is_some_and(|h| h.contains("edited notes"))
    })
    .await;

    for path in ["/other.md", "/other.md?raw=1"] {
        let r = s.get(path).await;
        assert_eq!(r.status, 403, "{path}");
        assert_no_leak(path, &r);
    }
    let r = s.get("/sub/").await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_no_leak("/sub/", &r);
    assert!(r.text().contains("--follow-symlinks"), "{}", r.text());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_directories_are_never_served() {
    let s = server(false).await;
    for path in [
        "/.git/config",
        "/.git/",
        "/.git",
        "/.GIT/config",
        "/sub/.git/HEAD",
        "/%2egit/config",
        "/sub/.Git/",
    ] {
        let r = s.get(path).await;
        assert_eq!(r.status, 403, "{path}");
        assert_no_leak(path, &r);
    }
    let listing = s.get("/sub/").await;
    assert_eq!(listing.status, 200);
    assert!(!listing.text().contains(".git"), "{}", listing.text());
    // Other dot files and `.github` are served normally.
    assert_eq!(s.get("/.gitignore").await.text(), "target/");
    let r = s.get("/.github/").await;
    assert_eq!(r.status, 200);
    assert!(r.text().contains("github dir readme"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_header_allowlist_on_loopback() {
    let s = server(false).await;
    let port = s.addr.port();
    for host in [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
        "localhost".into(),
        format!("app.localhost:{port}"),
        format!("LOCALHOST:{port}"),
    ] {
        let r = s.get_with("/README.md", &[("Host", &host)]).await;
        assert_eq!(r.status, 200, "Host: {host}");
    }
    for host in [
        format!("evil.example:{port}"),
        "evil.example".into(),
        format!("localhost:{}", port.wrapping_add(1)),
        format!("127.0.0.1.evil.example:{port}"),
        format!("10.0.0.1:{port}"),
    ] {
        for path in [
            "/README.md",
            "/__/md-render/static/live.js",
            "/__/md-render/render?path=README.md",
            "/__/md-render/events?path=README.md",
        ] {
            let r = s.get_with(path, &[("Host", &host)]).await;
            assert_eq!(r.status, 403, "Host: {host} {path}");
            assert!(!r.text().contains("public readme"));
        }
    }
}

/// SEC-5: an absolute-form request line does not override a foreign `Host` header (nor the
/// other way round), and a second `Host` header is checked too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_guard_checks_every_authority() {
    let s = server(false).await;
    let port = s.addr.port();
    let local = format!("localhost:{port}");
    let evil = format!("evil.example:{port}");
    let cases: [(String, Vec<(&str, &str)>); 3] = [
        (
            format!("http://{local}/README.md"),
            vec![("Host", evil.as_str())],
        ),
        (
            format!("http://{evil}/README.md"),
            vec![("Host", local.as_str())],
        ),
        (
            "/README.md".to_owned(),
            vec![("Host", local.as_str()), ("Host", evil.as_str())],
        ),
    ];
    for (target, headers) in &cases {
        let r = s.get_with(target, headers).await;
        assert_ne!(r.status, 200, "{target} {headers:?}");
        assert!(!r.text().contains("public readme"), "{target} {headers:?}");
    }
    let r = s
        .get_with(
            &format!("http://{local}/README.md"),
            &[("Host", local.as_str())],
        )
        .await;
    assert_eq!(r.status, 200);
}

/// SEC-6: raw HTML, SVG and XML files are active documents. Served from the preview origin
/// they could read every file of the root (a README image opens its SVG in a new tab), so
/// they get a sandbox CSP without `allow-same-origin`: an opaque origin, scripts still run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_active_documents_are_sandboxed() {
    let f = Fixture::new();
    f.file("README.md", "![logo](logo.svg)");
    f.file("logo.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>");
    f.file("report.html", "<!doctype html><script>1</script>");
    f.file(
        "page.xhtml",
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"/>",
    );
    f.file("feed.xml", "<feed/>");
    f.file("pic.png", b"\x89PNG\r\n\x1a\n");
    f.file("notes.txt", "text");
    f.file("data.json", "{}");
    let s = f.start(|_| {}).await;
    for path in ["/logo.svg", "/report.html", "/page.xhtml", "/feed.xml"] {
        let r = s.get(path).await;
        assert_eq!(r.status, 200, "{path}");
        let csp = r
            .header("content-security-policy")
            .unwrap_or_else(|| panic!("{path} has no Content-Security-Policy"));
        let tokens: Vec<&str> = csp.split([' ', ';']).collect();
        assert!(
            tokens.contains(&"sandbox") && tokens.contains(&"allow-scripts"),
            "{path}: {csp}"
        );
        assert!(!csp.contains("allow-same-origin"), "{path}: {csp}");
    }
    for path in [
        "/",
        "/README.md",
        "/README.md?raw=1",
        "/pic.png",
        "/notes.txt",
        "/data.json",
        "/__/md-render/static/live.js",
    ] {
        let r = s.get(path).await;
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.header("content-security-policy"), None, "{path}");
    }
}
