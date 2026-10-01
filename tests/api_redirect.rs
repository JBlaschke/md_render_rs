//! The real HTTP transport never follows a redirect from a render API (SEC-2, CLI-1, CLI-2).
//!
//! `POST /markdown` and `POST /api/v4/markdown` never redirect on a working instance. Following a
//! 301/302/303 turns the POST into a GET to another host: ureq drops `Authorization` on such a
//! hop but keeps GitLab's custom `PRIVATE-TOKEN` header, and the redirect target's page would be
//! accepted as the rendered document. Two local listeners stand in for the API and the redirect
//! target; no external network is used.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use md_render::render::api::{
    ApiConfig, ApiError, ApiRenderer, ApiRequest, HttpTransport as _, UreqTransport,
};
use md_render::render::{RenderOptions, Renderer as _};
use md_render::{Flavor, Mode};

/// Read one HTTP/1.1 request head (and its body, if any); return the head lines.
fn read_request(stream: &mut TcpStream) -> Vec<String> {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut head = Vec::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end().to_owned();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        head.push(line);
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);
    head
}

/// A server that answers every request with `response` and records the request heads.
fn server(response: String) -> (u16, Arc<Mutex<Vec<Vec<String>>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let head = read_request(&mut stream);
            log.lock().unwrap().push(head);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (port, seen)
}

fn ok_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn redirect_response(location: &str) -> String {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

/// The API at `api` answers 302 to a second server that would hand out a "rendered" page.
struct Redirect {
    api_url: String,
    target_seen: Arc<Mutex<Vec<Vec<String>>>>,
    api_seen: Arc<Mutex<Vec<Vec<String>>>>,
}

fn redirecting_api(location_suffix: &str) -> Redirect {
    let (target_port, target_seen) = server(ok_response(r#"{"html":"<p>captured</p>"}"#));
    let location = format!("http://user:hunter2@localhost:{target_port}/login{location_suffix}");
    let (api_port, api_seen) = server(redirect_response(&location));
    Redirect {
        api_url: format!("http://127.0.0.1:{api_port}"),
        target_seen,
        api_seen,
    }
}

#[test]
fn the_transport_returns_redirects_instead_of_following_them() {
    let r = redirecting_api("");
    let request = ApiRequest {
        url: format!("{}/api/v4/markdown", r.api_url),
        headers: vec![("PRIVATE-TOKEN".into(), "glpat-not-a-real-token".into())],
        body: serde_json::json!({"text": "# x", "gfm": true}),
    };
    let response = UreqTransport::new().post_json(&request).unwrap();
    assert_eq!(response.status, 302);
    assert_eq!(r.api_seen.lock().unwrap().len(), 1);
    assert!(
        r.target_seen.lock().unwrap().is_empty(),
        "the redirect target was contacted: {:?}",
        r.target_seen.lock().unwrap()
    );
}

fn render_through(flavor: Flavor, api_url: &str) -> anyhow::Result<md_render::render::Rendered> {
    let cfg = ApiConfig {
        flavor,
        api_url: Some(api_url.to_owned()),
        token: Some("tok-not-a-real-token".into()),
        fallback_offline: false,
    };
    let renderer = ApiRenderer::with_transport(cfg, Arc::new(UreqTransport::new()))?;
    let mut opts = RenderOptions::new(flavor);
    opts.mode = Mode::Readme;
    renderer.render("# Hello\n", &opts)
}

#[test]
fn a_redirecting_api_is_an_error_and_its_target_never_sees_the_token() {
    for flavor in [Flavor::Gitlab, Flavor::Github] {
        let r = redirecting_api("?session=s3cr3t-query");
        let err = render_through(flavor, &r.api_url).expect_err("a redirect must not render");
        let api_err = err
            .downcast_ref::<ApiError>()
            .unwrap_or_else(|| panic!("{flavor:?}: not an ApiError: {err:#}"));
        assert!(
            matches!(api_err, ApiError::Status { status: 302, .. }),
            "{flavor:?}: {api_err:?}"
        );
        assert!(
            r.target_seen.lock().unwrap().is_empty(),
            "{flavor:?}: the redirect target was contacted: {:?}",
            r.target_seen.lock().unwrap()
        );
        // The message names where the API redirected to, without credentials or query.
        let msg = format!("{err:#}");
        assert!(msg.contains("HTTP 302"), "{msg}");
        assert!(msg.contains("redirected to http://***@localhost:"), "{msg}");
        assert!(msg.contains("/login"), "{msg}");
        assert!(!msg.contains("hunter2") && !msg.contains("user:"), "{msg}");
        assert!(!msg.contains("s3cr3t-query"), "{msg}");
        assert!(msg.contains("--api-url"), "{msg}");
    }
}

#[test]
fn a_redirect_falls_back_offline_when_serving() {
    let r = redirecting_api("");
    let cfg = ApiConfig {
        flavor: Flavor::Github,
        api_url: Some(r.api_url.clone()),
        token: None,
        fallback_offline: true,
    };
    let renderer = ApiRenderer::with_transport(cfg, Arc::new(UreqTransport::new())).unwrap();
    let out = renderer
        .render("# Hello\n", &RenderOptions::new(Flavor::Github))
        .unwrap();
    assert!(out.html.contains("Hello") && !out.html.contains("captured"));
    assert!(out.warnings[0].contains("HTTP 302"), "{:?}", out.warnings);
    assert!(r.target_seen.lock().unwrap().is_empty());
}
