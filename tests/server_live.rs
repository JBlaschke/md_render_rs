//! Live updates over SSE (DESIGN §7, GRIP §11.1): wire format, atomic saves, deletion and
//! re-creation, shared renders, reconnect catch-up, image dependencies, style bundle and
//! `--css` swaps, polling, and graceful shutdown.

mod server_support;

use std::time::{Duration, Instant};

use server_support::{EVENT_TIMEOUT, Fixture, SseItem, write, write_atomic, write_bundle};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_starts_with_retry_and_hello() {
    let f = Fixture::new();
    f.file("README.md", "hello");
    let s = f.start(|_| {}).await;
    let (_, state) = s.render_json("README.md").await;
    let sid = state["server_id"].as_str().unwrap().to_owned();

    let mut sse = s.events("README.md", "", &[]).await;
    assert_eq!(sse.status, 200);
    let ctype = sse
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone());
    assert_eq!(ctype.as_deref(), Some("text/event-stream"));
    assert_eq!(
        sse.next_item(EVENT_TIMEOUT).await,
        Some(SseItem::Retry(1000))
    );
    let hello = sse.next_event(EVENT_TIMEOUT).await.expect("hello");
    assert_eq!(hello.event, "hello");
    let data = hello.json();
    assert_eq!(data["server_id"], sid.as_str());
    assert_eq!(data["version"], 1);
    assert_eq!(data["styles_version"], 0);
    // A client without a version is behind: the current content follows at once, and every
    // event id is `<server_id>:<version>`.
    assert_eq!(hello.id.as_deref(), Some(format!("{sid}:0").as_str()));
    let content = sse.next_event(EVENT_TIMEOUT).await.expect("content");
    assert_eq!(content.event, "content");
    assert_eq!(content.id.as_deref(), Some(format!("{sid}:1").as_str()));
    let c = content.json();
    assert_eq!(c["version"], 1);
    assert_eq!(c["html"], "<p data-sourcepos=\"1:1-1:5\">hello</p>\n");
    assert_eq!(c["warnings"], serde_json::json!([]));
    assert!(c.get("title").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn atomic_rename_saves_push_new_content() {
    let f = Fixture::new();
    f.file("doc.md", "one");
    let s = f.start(|_| {}).await;
    let (mut sse, hello) = s.subscribe("doc.md").await;
    let v0 = hello["version"].as_u64().unwrap();
    // Nothing is replayed to a client that is up to date.
    sse.expect_none("content", Duration::from_millis(200)).await;

    write_atomic(&s.path("doc.md"), "two");
    let rendering = sse.wait_for("rendering").await;
    let content = sse.wait_for("content").await.json();
    assert!(content["version"].as_u64().unwrap() > v0);
    assert_eq!(rendering.json()["version"], content["version"]);
    assert!(
        content["html"].as_str().unwrap().contains("two"),
        "{content}"
    );

    // In-place writes work too.
    write(&s.path("doc.md"), "three");
    let content = sse
        .wait_until("content", |c| {
            c["html"].as_str().is_some_and(|h| h.contains("three"))
        })
        .await;
    assert!(content.json()["version"].as_u64().unwrap() > v0 + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_saves_do_not_rerender() {
    let f = Fixture::new();
    f.file("doc.md", "same");
    let s = f.start(|_| {}).await;
    let (mut sse, _) = s.subscribe("doc.md").await;
    let before = s.renders();
    write_atomic(&s.path("doc.md"), "same"); // `touch`-like save: identical bytes
    sse.expect_none("content", Duration::from_millis(1500))
        .await;
    assert_eq!(s.renders(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleted_then_recreated() {
    let f = Fixture::new();
    f.file("doc.md", "alive");
    let s = f.start(|_| {}).await;
    let (mut sse, _) = s.subscribe("doc.md").await;

    std::fs::remove_file(s.path("doc.md")).unwrap();
    let deleted = sse.wait_for("deleted").await.json();
    assert_eq!(deleted["path"], "doc.md");
    // The page now reports the deletion (and keeps the last good content for the client).
    let (status, state) = s.render_json("doc.md").await;
    assert_eq!(status, 404);
    assert_eq!(state["deleted"], true);
    assert!(state["html"].as_str().unwrap().contains("alive"));

    write_atomic(&s.path("doc.md"), "reborn");
    let content = sse.wait_for("content").await.json();
    assert!(content["html"].as_str().unwrap().contains("reborn"));
    assert!(content["version"].as_u64().unwrap() > deleted["version"].as_u64().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_document_page_recovers_when_created() {
    let f = Fixture::new();
    let s = f.start(|_| {}).await;
    assert_eq!(s.get("/later.md").await.status, 404);
    let (mut sse, hello) = s.subscribe("later.md").await;
    assert_eq!(hello["version"], 1, "the deletion state is version 1");
    write(&s.path("later.md"), "here now");
    let content = sse.wait_for("content").await.json();
    assert!(content["html"].as_str().unwrap().contains("here now"));
    assert_eq!(s.get("/later.md").await.status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn errors_are_events_and_the_next_good_render_recovers() {
    let f = Fixture::new();
    f.file("doc.md", "good");
    let s = f.start(|_| {}).await;
    let (mut sse, _) = s.subscribe("doc.md").await;
    write_atomic(&s.path("doc.md"), "FAIL");
    let err = sse.wait_for("error").await.json();
    assert_eq!(err["kind"], "render");
    assert!(err["message"].as_str().unwrap().contains("refuses"));
    write_atomic(&s.path("doc.md"), "good again");
    let content = sse.wait_for("content").await.json();
    assert!(content["version"].as_u64().unwrap() > err["version"].as_u64().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_subscribers_share_one_render() {
    let f = Fixture::new();
    f.file("doc.md", "v1");
    let s = f.start(|_| {}).await;
    let (mut a, _) = s.subscribe("doc.md").await;
    let (mut b, _) = s.subscribe("doc.md").await;
    let before = s.renders();
    write_atomic(&s.path("doc.md"), "v2");
    let ca = a.wait_for("content").await;
    let cb = b.wait_for("content").await;
    assert_eq!(ca.data, cb.data, "both tabs get the same render");
    assert_eq!(ca.id, cb.id);
    // Let any duplicate watcher events settle: still exactly one render for the change.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(s.renders(), before + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnecting_clients_catch_up() {
    let f = Fixture::new();
    f.file("doc.md", "first");
    let s = f.start(|_| {}).await;
    let (_, state) = s.render_json("doc.md").await;
    let sid = state["server_id"].as_str().unwrap().to_owned();
    assert_eq!(state["version"], 1);
    // The document changes while nobody is connected.
    write(&s.path("doc.md"), "second");
    let (_, state) = s.render_json("doc.md").await;
    assert_eq!(state["version"], 2);

    // Behind according to Last-Event-ID: the current content is replayed after hello.
    let last = format!("{sid}:1");
    let mut sse = s.events("doc.md", "", &[("Last-Event-ID", &last)]).await;
    sse.wait_for("hello").await;
    let content = sse
        .next_event(EVENT_TIMEOUT)
        .await
        .expect("replayed content");
    assert_eq!(content.event, "content");
    assert!(content.json()["html"].as_str().unwrap().contains("second"));
    drop(sse);

    // Up to date (header or ?v=): nothing is replayed.
    let last = format!("{sid}:2");
    let mut sse = s.events("doc.md", "", &[("Last-Event-ID", &last)]).await;
    sse.wait_for("hello").await;
    sse.expect_none("content", Duration::from_millis(300)).await;
    let mut sse = s.events("doc.md", &format!("&v=2&sid={sid}"), &[]).await;
    sse.wait_for("hello").await;
    sse.expect_none("content", Duration::from_millis(300)).await;

    // Ids and versions of another server process mean nothing here.
    let mut sse = s
        .events(
            "doc.md",
            "&v=99&sid=0123456789abcdef",
            &[("Last-Event-ID", "0123456789abcdef:99")],
        )
        .await;
    let hello = sse.wait_for("hello").await.json();
    assert_eq!(hello["server_id"], sid.as_str());
    sse.wait_for("content").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_images_send_assets_events() {
    let f = Fixture::new();
    f.file(
        "docs/doc.md",
        "intro\n![](img/a.png)\n![](https://example.com/remote.png)",
    );
    f.file("docs/img/a.png", b"\x89PNG one");
    let s = f.start(|_| {}).await;
    let (mut sse, _) = s.subscribe("docs/doc.md").await;
    write_atomic(&s.path("docs/img/a.png"), b"\x89PNG two");
    let assets = sse.wait_for("assets").await.json();
    assert_eq!(assets["urls"], serde_json::json!(["/docs/img/a.png"]));
    assert!(assets["v"].as_str().is_some_and(|v| !v.is_empty()));
    // The document itself did not change.
    sse.expect_none("content", Duration::from_millis(300)).await;
}

/// LIVE-5: an image in a directory that does not exist when the document is rendered is
/// still a dependency: creating the directory and the image sends `assets`, and later
/// changes to the image do too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn images_in_directories_created_later_send_assets_events() {
    let f = Fixture::new();
    f.file("img.md", "intro\n![](imgs/deep/b.svg)");
    let s = f.start(|_| {}).await;
    let (mut sse, _) = s.subscribe("img.md").await;
    std::fs::create_dir_all(s.path("imgs/deep")).unwrap();
    write(&s.path("imgs/deep/b.svg"), "<svg/>");
    let assets = sse.wait_for("assets").await.json();
    assert_eq!(assets["urls"], serde_json::json!(["/imgs/deep/b.svg"]));
    // Once the directory exists the image is watched like any other.
    tokio::time::sleep(Duration::from_millis(900)).await;
    write_atomic(&s.path("imgs/deep/b.svg"), "<svg></svg>");
    let assets = sse.wait_for("assets").await.json();
    assert_eq!(assets["urls"], serde_json::json!(["/imgs/deep/b.svg"]));
    sse.expect_none("content", Duration::from_millis(300)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacing_the_bundle_sends_styles_and_a_corrupt_one_keeps_the_old() {
    let f = Fixture::new();
    f.file("doc.md", "styled");
    let bundle = f.tmp.join("styles/styles.tar.gz");
    write_bundle(&bundle, "p{color:red}");
    let s = f.start(|_| {}).await;
    let (mut sse, hello) = s.subscribe("doc.md").await;
    assert_eq!(hello["styles_version"], 0);

    write_bundle(&bundle, "p{color:green}/* re-vendored */");
    let styles = sse.wait_for("styles").await.json();
    assert_eq!(styles["styles_version"], 1);
    assert_eq!(
        styles["hrefs"],
        serde_json::json!(["/__/md-render/bundle/github/a.css"])
    );
    let r = s.get("/__/md-render/bundle/github/a.css").await;
    assert_eq!(r.text(), "p{color:green}/* re-vendored */");

    // A broken replacement is reported and the loaded styles stay.
    let tmp = bundle.with_file_name(".broken.tmp");
    write(&tmp, b"definitely not a gzip tarball");
    std::fs::rename(&tmp, &bundle).unwrap();
    let err = sse.wait_for("error").await.json();
    assert_eq!(err["kind"], "bundle");
    let r = s.get("/__/md-render/bundle/github/a.css").await;
    assert_eq!(r.text(), "p{color:green}/* re-vendored */");
    // New subscribers learn the current styles version from hello.
    let (_, hello) = s.subscribe("doc.md").await;
    assert_eq!(hello["styles_version"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bundle_vendored_while_serving_replaces_the_fallback() {
    let f = Fixture::new();
    f.file("doc.md", "x");
    let bundle = f.tmp.join("styles/styles.tar.gz");
    std::fs::create_dir_all(bundle.parent().unwrap()).unwrap();
    let s = f.start(|_| {}).await;
    let (mut sse, _) = s.subscribe("doc.md").await;
    assert_eq!(s.get("/__/md-render/bundle/github/a.css").await.status, 404);
    write_bundle(&bundle, "q{}");
    sse.wait_for("styles").await;
    assert_eq!(
        s.get("/__/md-render/bundle/github/a.css").await.text(),
        "q{}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_css_changes_send_styles() {
    let f = Fixture::new();
    f.file("doc.md", "x");
    let css = f.tmp.join("extra.css");
    write(&css, "a{}");
    let s = f
        .start(|cfg| cfg.extra_css = vec![css.display().to_string()])
        .await;
    let (mut sse, _) = s.subscribe("doc.md").await;
    write_atomic(&css, "a{color:red}");
    let styles = sse.wait_for("styles").await.json();
    assert!(styles["styles_version"].as_u64().unwrap() >= 1);
    assert!(
        styles["hrefs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h == "/__/md-render/css/0/extra.css")
    );
    assert_eq!(
        s.get("/__/md-render/css/0/extra.css").await.text(),
        "a{color:red}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polling_watcher() {
    let f = Fixture::new();
    f.file("doc.md", "polled");
    let s = f
        .start(|cfg| cfg.poll = Some(Duration::from_millis(100)))
        .await;
    let (mut sse, _) = s.subscribe("doc.md").await;
    write_atomic(&s.path("doc.md"), "polled again");
    let c = sse.wait_for("content").await.json();
    assert!(c["html"].as_str().unwrap().contains("polled again"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn norefresh_disables_the_event_stream() {
    let f = Fixture::new();
    f.file("README.md", "static");
    let s = f.start(|cfg| cfg.live = false).await;
    let page = s.get("/").await;
    assert_eq!(page.status, 200);
    assert!(!page.text().contains("/__/md-render/events?path="));
    assert_eq!(
        s.get("/__/md-render/events?path=README.md").await.status,
        404
    );
    // Pages still re-read the file on every load.
    write(&s.path("README.md"), "changed");
    assert!(s.get("/").await.text().contains("changed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_shutdown_ends_every_stream() {
    let f = Fixture::new();
    f.file("doc.md", "x");
    let mut s = f.start(|_| {}).await;
    let (mut a, _) = s.subscribe("doc.md").await;
    let (mut b, _) = s.subscribe("doc.md").await;
    let started = Instant::now();
    s.shutdown().await;
    // Open SSE streams would otherwise block axum's graceful shutdown (CR §3); the server
    // force-closes after 5 s, so finishing well before that proves the streams ended.
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "shutdown took {:?}",
        started.elapsed()
    );
    for sse in [&mut a, &mut b] {
        sse.wait_for("shutdown").await;
        sse.wait_end(EVENT_TIMEOUT).await;
    }
    // The listener is gone.
    assert!(tokio::net::TcpStream::connect(s.addr).await.is_err());
}

/// LIVE-4: a document whose directory does not exist yet is watched through its nearest
/// existing ancestor, so creating the directory and the file pushes the content, and later
/// saves keep arriving (also after the directory is deleted and created again).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_document_in_a_directory_created_later() {
    let f = Fixture::new();
    let s = f.start(|_| {}).await;
    let (mut sse, _) = s.subscribe("newdir/sub/new.md").await;
    std::fs::create_dir_all(s.path("newdir/sub")).unwrap();
    write(&s.path("newdir/sub/new.md"), "hello");
    sse.wait_until("content", |c| {
        c["html"].as_str().is_some_and(|h| h.contains("hello"))
    })
    .await;
    write_atomic(&s.path("newdir/sub/new.md"), "hello again");
    sse.wait_until("content", |c| {
        c["html"]
            .as_str()
            .is_some_and(|h| h.contains("hello again"))
    })
    .await;

    // A second tab subscribing now gets the live updates too.
    let (mut second, _) = s.subscribe("newdir/sub/new.md").await;
    write_atomic(&s.path("newdir/sub/new.md"), "third save");
    for sse in [&mut sse, &mut second] {
        sse.wait_until("content", |c| {
            c["html"].as_str().is_some_and(|h| h.contains("third save"))
        })
        .await;
    }

    // The directory goes away and comes back.
    std::fs::remove_dir_all(s.path("newdir")).unwrap();
    sse.wait_for("deleted").await;
    std::fs::create_dir_all(s.path("newdir/sub")).unwrap();
    write(&s.path("newdir/sub/new.md"), "reborn");
    sse.wait_until("content", |c| {
        c["html"].as_str().is_some_and(|h| h.contains("reborn"))
    })
    .await;
}

/// LIVE-1 / Q-INCLUDE-1: editing a GitLab `::include`d file pushes new content, and a page
/// load shows it too (the dedup hash covers the included files).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gitlab_included_files_are_watched() {
    let f = Fixture::new();
    f.file("README.md", "# GL doc\n\n::include{file=inc.md}\n\nafter\n");
    f.file("inc.md", "Included **ONE**\n");
    let s = f
        .start(|cfg| {
            cfg.flavor = md_render::flavor::Flavor::Gitlab;
            cfg.renderer = std::sync::Arc::new(md_render::render::OfflineRenderer);
        })
        .await;
    let (mut sse, _) = s.subscribe("README.md").await;
    write_atomic(&s.path("inc.md"), "Included **TWO**\n");
    let content = sse
        .wait_until("content", |c| {
            c["html"].as_str().is_some_and(|h| h.contains("TWO"))
        })
        .await
        .json();
    assert!(!content["html"].as_str().unwrap().contains("ONE"));
    // An in-place write is seen as well, and the page itself is never stale.
    write(&s.path("inc.md"), "Included **THREE**\n");
    sse.wait_until("content", |c| {
        c["html"].as_str().is_some_and(|h| h.contains("THREE"))
    })
    .await;
    write(&s.path("inc.md"), "Included **FOUR**\n");
    assert!(s.get("/").await.text().contains("FOUR"));
}

/// LIVE-10: re-pointing a symlinked document (`ln -sfn`, a checkout that changes the link)
/// pushes the new target's content, and the new target is watched from then on.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repointing_a_symlinked_document() {
    let f = Fixture::new();
    f.file("real/one.md", "Real one");
    f.file("other/two.md", "Real two");
    std::os::unix::fs::symlink("real/one.md", f.root.join("link.md")).unwrap();
    let s = f.start(|_| {}).await;
    let (mut sse, _) = s.subscribe("link.md").await;
    write_atomic(&s.path("real/one.md"), "Real one edited");
    sse.wait_until("content", |c| {
        c["html"]
            .as_str()
            .is_some_and(|h| h.contains("Real one edited"))
    })
    .await;

    // Atomically replace the link (like `ln -sfn` with a rename).
    let tmp = s.path(".link.tmp");
    std::os::unix::fs::symlink("other/two.md", &tmp).unwrap();
    std::fs::rename(&tmp, s.path("link.md")).unwrap();
    sse.wait_until("content", |c| {
        c["html"].as_str().is_some_and(|h| h.contains("Real two"))
    })
    .await;
    // Edits of the new target arrive; the old target no longer matters.
    write_atomic(&s.path("other/two.md"), "Real two edited");
    sse.wait_until("content", |c| {
        c["html"]
            .as_str()
            .is_some_and(|h| h.contains("Real two edited"))
    })
    .await;
}
