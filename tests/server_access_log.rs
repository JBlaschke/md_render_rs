//! The access log (CLI-7, GRIP C24): one line per request under the `md_render::access`
//! target, failed requests at INFO (shown by default), the others at DEBUG (`-v`).
//!
//! A current-thread runtime runs the server on the test thread, so the thread-local test
//! subscriber sees the server's events.

mod server_support;

use std::sync::{Arc, Mutex};

use server_support::Fixture;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt as _;

/// Collects everything the subscriber writes.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'w> tracing_subscriber::fmt::MakeWriter<'w> for Capture {
    type Writer = Capture;
    fn make_writer(&'w self) -> Capture {
        self.clone()
    }
}

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn subscriber(filter: &str, capture: &Capture) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(capture.clone())
            .with_ansi(false)
            .with_filter(tracing_subscriber::EnvFilter::new(filter)),
    )
}

#[tokio::test]
async fn requests_are_logged() {
    // The default filter (`md_render=info`): failed requests only.
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(subscriber("warn,md_render=info", &capture));
    let f = Fixture::new();
    f.file("README.md", "hello");
    let s = f.start(|_| {}).await;
    assert_eq!(s.get("/").await.status, 200);
    assert_eq!(s.get("/missing.png?x=1").await.status, 404);
    assert_eq!(
        s.get_with("/", &[("Host", "evil.example")]).await.status,
        403
    );
    let log = capture.text();
    assert!(
        log.contains("md_render::access") && log.contains("GET /missing.png?x=1 -> 404 Not Found"),
        "{log}"
    );
    assert!(log.contains("GET / -> 403 Forbidden"), "{log}");
    assert!(
        !log.contains("GET / -> 200"),
        "successful requests are logged by default: {log}"
    );
    drop(_guard);

    // `-v` (`md_render=debug`): every request.
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(subscriber("info,md_render=debug", &capture));
    assert_eq!(s.get("/README.md").await.status, 200);
    let log = capture.text();
    assert!(log.contains("GET /README.md -> 200 OK"), "{log}");
}
