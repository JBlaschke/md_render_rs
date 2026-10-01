//! The `--api` renderer through its public interface with an injected transport (no network):
//! request building, caching, error classification and the offline fallback (D15, GH §2.1,
//! GL §9, GRIP C57).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use md_render::render::api::{
    ApiConfig, ApiError, ApiRenderer, ApiRequest, ApiResponse, HttpTransport, TransportError,
    USER_AGENT,
};
use md_render::render::{RenderOptions, Renderer};
use md_render::{Flavor, Mode};

/// Records requests; answers from a queue (then with a transport error).
#[derive(Default)]
struct Script {
    answers: Mutex<VecDeque<Result<ApiResponse, TransportError>>>,
    seen: Mutex<Vec<ApiRequest>>,
}

impl Script {
    fn new(answers: Vec<Result<ApiResponse, TransportError>>) -> Arc<Script> {
        Arc::new(Script {
            answers: Mutex::new(answers.into()),
            seen: Mutex::default(),
        })
    }

    fn seen(&self) -> Vec<ApiRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl HttpTransport for Script {
    fn post_json(&self, request: &ApiRequest) -> Result<ApiResponse, TransportError> {
        self.seen.lock().unwrap().push(request.clone());
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(TransportError("script exhausted".into())))
    }
}

fn response(
    status: u16,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<ApiResponse, TransportError> {
    Ok(ApiResponse {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        body: body.to_owned(),
    })
}

fn config(flavor: Flavor, token: Option<&str>, fallback_offline: bool) -> ApiConfig {
    ApiConfig {
        flavor,
        api_url: None,
        token: token.map(str::to_owned),
        fallback_offline,
    }
}

fn options(flavor: Flavor, mode: Mode, context: Option<&str>) -> RenderOptions {
    let mut o = RenderOptions::new(flavor);
    o.mode = mode;
    o.context = context.map(str::to_owned);
    o
}

#[test]
fn github_request() {
    let script = Script::new(vec![response(200, &[], "<p>hi</p>")]);
    let r = ApiRenderer::with_transport(
        config(Flavor::Github, Some("ghp_secret"), false),
        script.clone(),
    )
    .unwrap();
    assert_eq!(r.name(), "api");
    r.render("hi", &options(Flavor::Github, Mode::Comment, Some("o/r")))
        .unwrap();
    let req = &script.seen()[0];
    assert_eq!(req.url, "https://api.github.com/markdown");
    assert_eq!(
        req.body,
        serde_json::json!({"text": "hi", "mode": "gfm", "context": "o/r"})
    );
    assert_eq!(req.header("Accept"), Some("application/vnd.github+json"));
    assert_eq!(req.header("X-GitHub-Api-Version"), Some("2022-11-28"));
    assert_eq!(req.header("User-Agent"), Some(USER_AGENT));
    assert_eq!(req.header("Authorization"), Some("Bearer ghp_secret"));
    // Neither the request nor the renderer reveal the token in debug output.
    let shown = format!("{req:?} {r:?}");
    assert!(!shown.contains("ghp_secret"), "{shown}");
}

#[test]
fn github_enterprise_and_gitlab_endpoints() {
    let ghe = ApiConfig {
        api_url: Some("https://ghe.corp/api/v3".into()),
        ..config(Flavor::Github, None, false)
    };
    assert_eq!(
        ApiRenderer::with_transport(ghe, Script::new(vec![]))
            .unwrap()
            .endpoint(),
        "https://ghe.corp/api/v3/markdown"
    );
    let gl = ApiRenderer::with_transport(config(Flavor::Gitlab, None, false), Script::new(vec![]))
        .unwrap();
    assert_eq!(gl.endpoint(), "https://gitlab.com/api/v4/markdown");
    let own = ApiConfig {
        api_url: Some("https://git.example.org/".into()),
        ..config(Flavor::Gitlab, None, false)
    };
    assert_eq!(
        ApiRenderer::with_transport(own, Script::new(vec![]))
            .unwrap()
            .endpoint(),
        "https://git.example.org/api/v4/markdown"
    );
}

#[test]
fn gitlab_request_and_postprocessing() {
    let html = r#"<p><img src="data:image/gif;base64,R0lGOD" class="lazy gfm" data-src="/-/project/1/uploads/x/a.png" alt="a"></p><div class="gl-relative markdown-code-block js-markdown-code"><pre class="code highlight js-syntax-highlight language-rust" v-pre="true"><code><span id="LC1" class="line" lang="rust">fn</span></code></pre><copy-code></copy-code></div>"#;
    let body = serde_json::json!({ "html": html }).to_string();
    let script = Script::new(vec![response(200, &[], &body)]);
    let r = ApiRenderer::with_transport(
        config(Flavor::Gitlab, Some("glpat-x"), false),
        script.clone(),
    )
    .unwrap();
    let out = r
        .render(
            "![a](a.png)",
            &options(Flavor::Gitlab, Mode::Readme, Some("g/p")),
        )
        .unwrap();
    let req = &script.seen()[0];
    assert_eq!(
        req.body,
        serde_json::json!({"text": "![a](a.png)", "gfm": true, "project": "g/p"})
    );
    assert_eq!(req.header("PRIVATE-TOKEN"), Some("glpat-x"));
    assert!(
        out.html
            .contains(r#"src="https://gitlab.com/-/project/1/uploads/x/a.png""#),
        "{}",
        out.html
    );
    assert!(
        !out.html.contains("data-src") && !out.html.contains("R0lGOD"),
        "{}",
        out.html
    );
    assert!(
        out.html.contains("code-syntax-highlight-theme"),
        "{}",
        out.html
    );
}

#[test]
fn identical_documents_are_rendered_once() {
    let script = Script::new(vec![
        response(200, &[], "<p>x</p>"),
        response(200, &[], "<p>y</p>"),
    ]);
    let r =
        ApiRenderer::with_transport(config(Flavor::Github, None, false), script.clone()).unwrap();
    let o = options(Flavor::Github, Mode::Readme, None);
    let first = r.render("x", &o).unwrap();
    assert_eq!(r.render("x", &o).unwrap(), first);
    assert_eq!(script.seen().len(), 1);
    r.render("x", &options(Flavor::Github, Mode::Comment, None))
        .unwrap();
    assert_eq!(script.seen().len(), 2);
}

#[test]
fn rate_limits_fall_back_offline_with_the_reset_time() {
    let reset = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600)
        .to_string();
    let script = Script::new(vec![response(
        403,
        &[
            ("X-RateLimit-Limit", "60"),
            ("X-RateLimit-Remaining", "0"),
            ("X-RateLimit-Reset", &reset),
        ],
        r#"{"message":"API rate limit exceeded for 203.0.113.9.","documentation_url":"https://docs.github.com/rest"}"#,
    )]);
    let r =
        ApiRenderer::with_transport(config(Flavor::Github, None, true), script.clone()).unwrap();
    let out = r
        .render("# Title\n", &options(Flavor::Github, Mode::Readme, None))
        .unwrap();
    assert!(
        out.html.contains("Title"),
        "offline rendering expected: {}",
        out.html
    );
    let warning = &out.warnings[0];
    assert!(
        warning
            .starts_with("GitHub API rate limit exceeded (60 requests per window); it resets at "),
        "{warning}"
    );
    assert!(
        warning.contains("(in 1 h 0 min)") || warning.contains("(in 59 min)"),
        "{warning}"
    );
    assert!(warning.contains("GITHUB_TOKEN"), "{warning}");
    // Further renders do not hit the API until the reset.
    r.render("# Other\n", &options(Flavor::Github, Mode::Readme, None))
        .unwrap();
    assert_eq!(script.seen().len(), 1);
}

/// A canned answer and a predicate on the error it must produce.
type ErrorCase = (Result<ApiResponse, TransportError>, fn(&ApiError) -> bool);

#[test]
fn errors_without_fallback_are_returned() {
    let cases: Vec<ErrorCase> = vec![
        (
            response(401, &[], r#"{"message":"Bad credentials"}"#),
            |e| {
                matches!(
                    e,
                    ApiError::Unauthorized {
                        had_token: true,
                        ..
                    }
                )
            },
        ),
        (response(429, &[("Retry-After", "30")], ""), |e| {
            matches!(
                e,
                ApiError::RateLimited {
                    retry_after: Some(30),
                    ..
                }
            )
        }),
        (
            response(
                403,
                &[("X-RateLimit-Remaining", "4999")],
                r#"{"message":"Forbidden"}"#,
            ),
            |e| matches!(e, ApiError::Forbidden { .. }),
        ),
        (response(502, &[], "<html>bad gateway</html>"), |e| {
            matches!(e, ApiError::Status { status: 502, .. })
        }),
        (Err(TransportError("connection refused".into())), |e| {
            matches!(e, ApiError::Network { .. })
        }),
    ];
    for (answer, check) in cases {
        let r = ApiRenderer::with_transport(
            config(Flavor::Github, Some("t"), false),
            Script::new(vec![answer]),
        )
        .unwrap();
        let err = r
            .render("x", &options(Flavor::Github, Mode::Comment, None))
            .unwrap_err();
        let api = err
            .downcast_ref::<ApiError>()
            .unwrap_or_else(|| panic!("not an ApiError: {err:#}"));
        assert!(check(api), "{api:?}");
        assert!(
            format!("{err:#}").starts_with("rendering through the GitHub API: "),
            "{err:#}"
        );
    }
    // A GitLab 200 without an `html` string is a bad response.
    let r = ApiRenderer::with_transport(
        config(Flavor::Gitlab, None, false),
        Script::new(vec![response(200, &[], "{}")]),
    )
    .unwrap();
    let err = r
        .render("x", &options(Flavor::Gitlab, Mode::Readme, None))
        .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<ApiError>(),
            Some(ApiError::BadResponse { .. })
        ),
        "{err:#}"
    );
}

#[test]
fn network_failures_fall_back_offline() {
    let r = ApiRenderer::with_transport(
        config(Flavor::Gitlab, None, true),
        Script::new(vec![Err(TransportError("dns error: no such host".into()))]),
    )
    .unwrap();
    let out = r
        .render("*hi*", &options(Flavor::Gitlab, Mode::Readme, None))
        .unwrap();
    assert!(out.html.contains("hi"));
    assert!(
        out.warnings[0]
            .starts_with("Could not reach the GitLab API at https://gitlab.com/api/v4/markdown"),
        "{:?}",
        out.warnings
    );
    assert!(
        out.warnings[0].ends_with("Showing the offline rendering instead."),
        "{:?}",
        out.warnings
    );
}

#[test]
fn renderer_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ApiRenderer>();
    let script = Script::new(
        (0..8)
            .map(|i| response(200, &[], &format!("<p>{i}</p>")))
            .collect(),
    );
    let r: Arc<dyn Renderer> =
        Arc::new(ApiRenderer::with_transport(config(Flavor::Github, None, false), script).unwrap());
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let r = Arc::clone(&r);
            std::thread::spawn(move || {
                r.render(
                    &format!("doc {i}"),
                    &options(Flavor::Github, Mode::Comment, None),
                )
                .is_ok()
            })
        })
        .collect();
    assert!(handles.into_iter().all(|h| h.join().unwrap()));
}
