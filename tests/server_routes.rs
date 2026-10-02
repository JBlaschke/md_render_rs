//! HTTP surface of the preview server (DESIGN §6): documents, README lookup, listings,
//! redirects, raw files, embedded assets, bundle files, `--css` files and the JSON resync
//! endpoint.

mod server_support;

use std::path::PathBuf;

use md_render::server::Target;
use server_support::{Fixture, write, write_bundle};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_renders_the_readme_of_the_served_directory() {
    let f = Fixture::new();
    f.file("README.md", "Hello from the readme\n\nsecond paragraph");
    let s = f.start(|_| {}).await;
    let r = s.get("/").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    let body = r.text();
    assert!(body.contains("Hello from the readme"), "{body}");
    assert!(body.contains("second paragraph"));
    // Live pages load the live client.
    assert!(
        body.contains("live.js") || body.contains("/__/md-render/events?path="),
        "{body}"
    );
    // The document is also reachable under its own name.
    let r = s.get("/README.md").await;
    assert_eq!(r.status, 200);
    assert!(r.text().contains("Hello from the readme"));
    assert_eq!(
        s.renders(),
        1,
        "one render serves every page load of unchanged content"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_real_offline_renderer_is_wired_in() {
    let f = Fixture::new();
    f.file("README.md", "# Big Title\n\nSome *text*.\n");
    let s = f
        .start(|cfg| cfg.renderer = std::sync::Arc::new(md_render::render::OfflineRenderer))
        .await;
    let r = s.get("/").await;
    assert_eq!(r.status, 200);
    assert!(r.text().contains("Big Title"));
    let (status, json) = s.render_json("README.md").await;
    assert_eq!(status, 200);
    let html = json["html"].as_str().unwrap();
    assert!(
        html.contains("<h1") && html.contains("data-sourcepos"),
        "{html}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn directories_redirect_and_find_their_readme() {
    let f = Fixture::new();
    f.file("docs/readme.markdown", "docs readme");
    f.file("docs/other.md", "other doc");
    let s = f.start(|_| {}).await;

    let r = s.get("/docs").await;
    assert_eq!(r.status, 301);
    assert_eq!(r.header("location"), Some("/docs/"));
    let r = s.get("/docs?x=1").await;
    assert_eq!(
        r.header("location"),
        Some("/docs/?x=1"),
        "the query string is kept"
    );

    let r = s.get("/docs/").await;
    assert_eq!(r.status, 200);
    assert!(r.text().contains("docs readme"));

    // `/file/` -> `/file`, `.` and empty segments collapse.
    let r = s.get("/docs/other.md/").await;
    assert_eq!(
        (r.status, r.header("location")),
        (301, Some("/docs/other.md"))
    );
    let r = s.get("/./docs/other.md").await;
    assert_eq!(
        (r.status, r.header("location")),
        (301, Some("/docs/other.md"))
    );
    let r = s.get("/docs//other.md").await;
    assert_eq!(
        (r.status, r.header("location")),
        (301, Some("/docs/other.md"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_readme_fallbacks_and_listing() {
    let f = Fixture::new();
    f.file("docs/README.md", "docs readme");
    let s = f.start(|_| {}).await;
    let r = s.get("/").await;
    assert_eq!(
        (r.status, r.header("location")),
        (302, Some("/docs/")),
        "docs/README.md is the root README"
    );

    let f = Fixture::new();
    f.file("assets/a.png", b"\x89PNG\r\n\x1a\n");
    f.file("assets/sub/b.txt", "b");
    f.file("assets/.git/config", "secret");
    let s = f.start(|_| {}).await;
    let r = s.get("/assets/").await;
    assert_eq!(r.status, 200);
    let body = r.text();
    assert!(body.contains("a.png") && body.contains("sub"), "{body}");
    assert!(!body.contains(".git"), "listings hide .git: {body}");
    // A root without any README lists the root.
    let r = s.get("/").await;
    assert_eq!(r.status, 200);
    assert!(r.text().contains("assets"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn targets() {
    // A file below the root: `/` redirects to it so relative links resolve.
    let f = Fixture::new();
    f.file("docs/guide.md", "the guide");
    let s = f
        .start(|cfg| cfg.target = Target::File(PathBuf::from("docs/guide.md")))
        .await;
    let r = s.get("/").await;
    assert_eq!(
        (r.status, r.header("location")),
        (302, Some("/docs/guide.md"))
    );

    // A non-Markdown target file is still rendered (it was asked for explicitly).
    let f = Fixture::new();
    f.file("notes.txt", "plain notes");
    let s = f
        .start(|cfg| cfg.target = Target::File(PathBuf::from("notes.txt")))
        .await;
    for path in ["/", "/notes.txt"] {
        let r = s.get(path).await;
        assert_eq!(r.status, 200, "{path}");
        assert!(
            r.header("content-type").unwrap().starts_with("text/html"),
            "{path}"
        );
        assert!(
            r.text()
                .contains("<p data-sourcepos=\"1:1-1:11\">plain notes</p>"),
            "{path}"
        );
    }

    // A directory target below the root.
    let f = Fixture::new();
    f.file("sub/README.md", "sub readme");
    let s = f
        .start(|cfg| cfg.target = Target::Dir(PathBuf::from("sub")))
        .await;
    let r = s.get("/").await;
    assert_eq!((r.status, r.header("location")), (302, Some("/sub/")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_is_rendered_once_without_live_updates() {
    let f = Fixture::new();
    let s = f
        .start(|cfg| {
            cfg.target = Target::Stdin {
                name: "stdin".into(),
                text: "piped text".into(),
            };
        })
        .await;
    let r = s.get("/").await;
    assert_eq!(r.status, 200);
    let body = r.text();
    assert!(body.contains("piped text"), "{body}");
    assert!(
        !body.contains("/__/md-render/events?path="),
        "stdin pages are not live: {body}"
    );
    assert!(
        body.contains("<title>stdin"),
        "the title defaults to the stdin name: {body}"
    );
    s.get("/").await;
    assert_eq!(s.renders(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn title_override() {
    let f = Fixture::new();
    f.file("README.md", "x");
    let s = f.start(|cfg| cfg.title = Some("My <Title>".into())).await;
    let body = s.get("/").await.text();
    assert!(body.contains("<title>My &lt;Title&gt;"), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_documents_and_files() {
    let f = Fixture::new();
    f.file("README.md", "x");
    let s = f.start(|_| {}).await;
    // A missing Markdown document is a styled, live 404 page that recovers on creation.
    let r = s.get("/new.md").await;
    assert_eq!(r.status, 404);
    assert!(r.header("content-type").unwrap().starts_with("text/html"));
    let body = r.text();
    assert!(body.contains("new.md"), "{body}");
    assert!(
        body.contains("live.js") || body.contains("/__/md-render/events?path=new.md"),
        "{body}"
    );
    // Other missing files: styled 404 as well.
    let r = s.get("/missing.png").await;
    assert_eq!(r.status, 404);
    assert!(r.header("content-type").unwrap().starts_with("text/html"));
    let r = s.get("/nodir/").await;
    assert_eq!(r.status, 404);
    // Unknown internal routes are never served from the tree.
    let r = s.get("/__/md-render/nope").await;
    assert_eq!(r.status, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_files_have_correct_content_types() {
    let f = Fixture::new();
    f.file("README.md", "# doc source");
    f.file("notes.txt", "hello world");
    f.file("a.png", b"\x89PNG\r\n\x1a\nfake");
    f.file("data.json", "{}");
    f.file("script.sh", "echo hi");
    f.file("pic.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>");
    f.file("Makefile", "all:\n\techo hi\n");
    f.file("blob", b"\x7fELF\x00\x01\x02");
    let s = f.start(|_| {}).await;
    let cases = [
        ("/notes.txt", "text/plain; charset=utf-8"),
        ("/a.png", "image/png"),
        ("/data.json", "application/json; charset=utf-8"),
        ("/script.sh", "text/plain; charset=utf-8"),
        ("/pic.svg", "image/svg+xml; charset=utf-8"),
        ("/Makefile", "text/plain; charset=utf-8"),
        ("/blob", "application/octet-stream"),
        ("/README.md?raw=1", "text/plain; charset=utf-8"),
    ];
    for (path, ctype) in cases {
        let r = s.get(path).await;
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.header("content-type"), Some(ctype), "{path}");
        assert_eq!(
            r.header("x-content-type-options"),
            Some("nosniff"),
            "{path}"
        );
    }
    assert_eq!(s.get("/README.md?raw").await.text(), "# doc source");
    assert_eq!(s.get("/notes.txt").await.text(), "hello world");

    // Range requests (video seeking) and conditional requests work.
    let r = s.get_with("/notes.txt", &[("Range", "bytes=0-4")]).await;
    assert_eq!(r.status, 206);
    assert_eq!(r.text(), "hello");
    let r = s.get("/notes.txt").await;
    let modified = r.header("last-modified").expect("Last-Modified").to_owned();
    let r = s
        .get_with("/notes.txt", &[("If-Modified-Since", &modified)])
        .await;
    assert_eq!(r.status, 304);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_static_assets() {
    let f = Fixture::new();
    let s = f.start(|_| {}).await;
    let r = s.get("/__/md-render/static/live.js").await;
    assert_eq!(r.status, 200);
    let (bytes, mime) = md_render::assets::get("live.js").unwrap();
    assert_eq!(r.header("content-type"), Some(mime));
    assert_eq!(r.body, bytes);
    assert_eq!(r.header("cache-control"), Some("no-cache"));
    let etag = r.header("etag").expect("ETag").to_owned();
    let r = s
        .get_with("/__/md-render/static/live.js", &[("If-None-Match", &etag)])
        .await;
    assert_eq!(r.status, 304);
    // A different asset has a different (content-based) ETag.
    let other = s.get("/__/md-render/static/github.js").await;
    assert_ne!(other.header("etag"), Some(etag.as_str()));
    assert_eq!(s.get("/__/md-render/static/nope.js").await.status, 404);
    assert_eq!(
        s.get("/__/md-render/static/favicon.svg")
            .await
            .header("content-type"),
        Some("image/svg+xml")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bundle_files_are_served_in_place() {
    let f = Fixture::new();
    f.file("README.md", "x");
    let bundle = f.tmp.join("styles/styles.tar.gz");
    write_bundle(&bundle, "body{color:red}");
    let s = f.start(|_| {}).await;
    let r = s.get("/__/md-render/bundle/github/a.css").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), "body{color:red}");
    assert_eq!(r.header("content-type"), Some("text/css; charset=utf-8"));
    assert_eq!(r.header("cache-control"), Some("no-cache"));
    let etag = r.header("etag").expect("ETag").to_owned();
    let r = s
        .get_with(
            "/__/md-render/bundle/github/a.css",
            &[("If-None-Match", &etag)],
        )
        .await;
    assert_eq!(r.status, 304);
    let r = s.get("/__/md-render/bundle/github/fonts/x.woff2").await;
    assert_eq!(
        (r.status, r.header("content-type")),
        (200, Some("font/woff2"))
    );
    assert_eq!(
        s.get("/__/md-render/bundle/github/nope.css").await.status,
        404
    );
    assert_eq!(
        s.get("/__/md-render/bundle/manifest.json").await.status,
        200
    );
    // The bundle is read in place, never unpacked next to it.
    let entries: Vec<_> = std::fs::read_dir(s.tmp.join("styles"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(entries, ["styles.tar.gz"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_bundle_falls_back_to_embedded_css() {
    let f = Fixture::new();
    f.file("README.md", "x");
    let s = f.start(|_| {}).await;
    let r = s.get("/__/md-render/bundle/github/fallback.css").await;
    assert_eq!(r.status, 200);
    let (css, _) = md_render::assets::get("fallback-github.css").unwrap();
    assert_eq!(r.body, css);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_extra_css_is_served() {
    let f = Fixture::new();
    f.file("README.md", "x");
    let css = f.tmp.join("my style.css");
    server_support::write(&css, "h1{color:blue}");
    let s = f
        .start(|cfg| {
            cfg.extra_css = vec![
                css.display().to_string(),
                "https://example.com/x.css".into(),
            ]
        })
        .await;
    let r = s.get("/__/md-render/css/0/my%20style.css").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("text/css; charset=utf-8"));
    assert_eq!(r.text(), "h1{color:blue}");
    // URLs are only linked, never fetched or served.
    assert_eq!(s.get("/__/md-render/css/1/x.css").await.status, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_endpoint_reports_state_as_json() {
    let f = Fixture::new();
    f.file("README.md", "first");
    let s = f.start(|_| {}).await;
    let (status, v) = s.render_json("README.md").await;
    assert_eq!(status, 200);
    assert_eq!(v["version"], 1);
    assert_eq!(v["html"], "<p data-sourcepos=\"1:1-1:5\">first</p>\n");
    assert_eq!(v["warnings"], serde_json::json!([]));
    assert_eq!(v["server_id"].as_str().map(str::len), Some(16));

    // A change is picked up on the next request even without subscribers.
    server_support::write(&s.path("README.md"), "second");
    let (_, v2) = s.render_json("README.md").await;
    assert_eq!(v2["version"], 2);
    assert!(v2["html"].as_str().unwrap().contains("second"));
    assert_eq!(v2["server_id"], v["server_id"]);

    let (status, v) = s.render_json("nope.md").await;
    assert_eq!(status, 404);
    assert_eq!(v["deleted"], true);
    assert_eq!(v["path"], "nope.md");

    // A render failure keeps the last good content and reports the error.
    server_support::write(&s.path("README.md"), "FAIL");
    let (status, v) = s.render_json("README.md").await;
    assert_eq!(status, 500);
    assert_eq!(v["error"]["kind"], "render");
    assert!(v["html"].as_str().unwrap().contains("second"));
    let page = s.get("/README.md").await;
    assert_eq!(page.status, 500);
    assert!(
        page.text().contains("second"),
        "the page keeps the last good render"
    );

    for bad in ["", "?path=", "?path=notes.txt", "?path=docs/"] {
        let r = s.get(&format!("/__/md-render/render{bad}")).await;
        assert_eq!(r.status, 400, "{bad}: {}", r.text());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bom_is_stripped_and_invalid_utf8_decoded_lossily() {
    let f = Fixture::new();
    f.file("bom.md", b"\xEF\xBB\xBFwith bom");
    f.file("latin1.md", b"caf\xe9 au lait");
    let s = f.start(|_| {}).await;
    let (_, v) = s.render_json("bom.md").await;
    assert_eq!(v["html"], "<p data-sourcepos=\"1:1-1:8\">with bom</p>\n");
    assert_eq!(v["warnings"], serde_json::json!([]));
    let (status, v) = s.render_json("latin1.md").await;
    assert_eq!(status, 200);
    assert!(v["html"].as_str().unwrap().contains("caf\u{FFFD} au lait"));
    let warning = v["warnings"][0].as_str().unwrap();
    assert!(
        warning.contains("not valid UTF-8") && warning.contains("offset 3"),
        "{warning}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn head_requests_work() {
    let f = Fixture::new();
    f.file("README.md", "x");
    let s = f.start(|_| {}).await;
    let r = server_support::request(s.addr, "HEAD", "/", &[]).await;
    assert_eq!(r.status, 200);
    assert!(r.body.is_empty());
}

/// LIVE-7: the banners a page is rendered with carry the marker live.js removes them by,
/// and a page rendered while its document has an error starts with the `(Error) ` title
/// prefix the live client keeps while an error is active.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_banners_and_error_titles_match_the_live_client() {
    for flavor in [
        md_render::flavor::Flavor::Github,
        md_render::flavor::Flavor::Gitlab,
    ] {
        let f = Fixture::new();
        f.file("doc.md", "good");
        f.file("latin1.md", b"caf\xe9");
        let s = f.start(|cfg| cfg.flavor = flavor).await;
        let live_js = s.get("/__/md-render/static/live.js").await.text();
        let selector = "#mdr-banners > .mdr-banner";
        assert!(
            live_js.contains(&format!("'{selector}'")),
            "live.js removes server banners by another selector"
        );

        // A warning banner (invalid UTF-8): inside #mdr-banners, with class mdr-banner.
        let body = s.get("/latin1.md").await.text();
        let banners = body
            .split("id=\"mdr-banners\"")
            .nth(1)
            .unwrap_or_else(|| panic!("{flavor}: no #mdr-banners: {body}"));
        let first = banners.split_once('>').map(|(_, rest)| rest).unwrap_or("");
        assert!(
            first.trim_start().starts_with("<div class=\"")
                && first.split_once('>').unwrap().0.contains(" mdr-banner\""),
            "{flavor}: the banner is not a direct `.mdr-banner` child: {first:.300}"
        );
        assert!(
            body.contains("<title>latin1.md</title>"),
            "{flavor}: {body:.400}"
        );

        // An error page (render failure under the last good content) and a missing page.
        assert!(
            s.get("/doc.md")
                .await
                .text()
                .contains("<title>doc.md</title>")
        );
        write(&s.path("doc.md"), "FAIL");
        let r = s.get("/doc.md").await;
        assert_eq!(r.status, 500);
        let body = r.text();
        assert!(
            body.contains("<title>(Error) doc.md</title>"),
            "{flavor}: {body:.400}"
        );
        assert!(body.contains(" mdr-banner\""), "{flavor}");
        let body = s.get("/gone.md").await.text();
        assert!(body.contains("<title>(Error) gone.md</title>"), "{flavor}");
        write(&s.path("doc.md"), "good again");
        assert!(
            s.get("/doc.md")
                .await
                .text()
                .contains("<title>doc.md</title>")
        );
    }
}
