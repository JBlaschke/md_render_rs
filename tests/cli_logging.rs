//! The CLI's log subscriber never prints API tokens, even at the most verbose level: ureq's
//! TRACE output ("wire level debug. NOT REDACTED") is dropped, its redacted DEBUG output is kept.
//!
//! This file is its own test binary because it installs the global subscriber (and with it the
//! `log` → `tracing` bridge that carries ureq's records).

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use md_render::cli::run::log_subscriber;
use md_render::render::api::{ApiRequest, HttpTransport, UreqTransport};
use tracing_subscriber::util::SubscriberInitExt as _;

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

#[test]
fn tokens_never_reach_the_log() {
    const SECRET: &str = "ghp_do-not-log-me-0123456789";
    let capture = Capture::default();
    // `-vv` and `RUST_LOG=trace`: everything else is logged.
    log_subscriber(Some("trace"), false, 2, capture.clone(), false)
        .try_init()
        .expect("first subscriber in this test binary");

    // A one-shot local HTTP server.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        // Read until the end of the JSON body (the request is small).
        while !request.ends_with(b"}") {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 9\r\nConnection: close\r\n\r\n<p>ok</p>")
            .unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });

    let request = ApiRequest {
        url: format!("http://{addr}/markdown"),
        headers: vec![
            ("Authorization".into(), format!("Bearer {SECRET}")),
            ("PRIVATE-TOKEN".into(), SECRET.into()),
        ],
        body: serde_json::json!({ "text": "x", "mode": "gfm" }),
    };
    let response = UreqTransport::new()
        .post_json(&request)
        .expect("local request");
    assert_eq!(
        (response.status, response.body.as_str()),
        (200, "<p>ok</p>")
    );
    let received = server.join().unwrap();
    assert!(
        received.contains(SECRET),
        "the token must still be sent: {received}"
    );

    let log = String::from_utf8_lossy(&capture.0.lock().unwrap()).into_owned();
    // ureq's TRACE output is a hex + ASCII dump in 16-byte rows, so a leaked token appears in
    // pieces: look for every 6-character fragment of it.
    let chars: Vec<char> = SECRET.chars().collect();
    for window in chars.windows(6) {
        let fragment: String = window.iter().collect();
        assert!(
            !log.contains(&fragment),
            "token fragment {fragment:?} leaked into the log:\n{log}"
        );
    }
    assert!(
        !log.lines().any(|l| l.contains("TRACE ureq")),
        "ureq TRACE output logged:\n{log}"
    );
    // The bridge works and ureq's (redacted) DEBUG output still gets through.
    assert!(
        log.contains("DEBUG ureq"),
        "no ureq output captured:\n{log}"
    );
}
