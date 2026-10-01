//! Live tests against the real GitHub and GitLab markdown APIs.
//!
//! All are `#[ignore]` because they need the network (and, for GitLab, a token). Run them with
//! `cargo test --test api_live -- --ignored`. Tokens are taken from the environment
//! (`GITHUB_TOKEN`/`GH_TOKEN`/`gh auth token`, `GITLAB_TOKEN`) by [`ApiRenderer::new`] and are
//! never printed.

use md_render::render::api::{ApiConfig, ApiRenderer};
use md_render::render::{RenderOptions, Renderer};
use md_render::{Flavor, Mode};

const DOC: &str = "# Live test :rocket:\n\nSome *text* with `code`.\n\n```rust\nfn main() {}\n```\n\n- [x] done\n";

fn options(flavor: Flavor, mode: Mode) -> RenderOptions {
    let mut o = RenderOptions::new(flavor);
    o.mode = mode;
    o
}

#[test]
#[ignore = "network: POST https://api.github.com/markdown (works unauthenticated, 60 requests/hour)"]
fn github_live_readme_mode() {
    let cfg = ApiConfig { flavor: Flavor::Github, api_url: None, token: None, fallback_offline: false };
    let r = ApiRenderer::new(cfg).expect("renderer");
    let out = r.render(DOC, &options(Flavor::Github, Mode::Readme)).expect("GitHub API render");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert!(out.html.contains(r#"<div class="markdown-heading" dir="auto"><h1 class="heading-element" dir="auto">"#), "{}", out.html);
    assert!(out.html.contains(r#"id="user-content-live-test-rocket""#), "{}", out.html);
    assert!(out.html.contains("highlight-source-rust") && out.html.contains("data-snippet-clipboard-copy-content"), "{}", out.html);
    assert!(out.html.contains("task-list-item"), "{}", out.html);
}

#[test]
#[ignore = "network: POST https://api.github.com/markdown with context"]
fn github_live_comment_mode() {
    let cfg = ApiConfig { flavor: Flavor::Github, api_url: None, token: None, fallback_offline: false };
    let r = ApiRenderer::new(cfg).expect("renderer");
    let mut o = options(Flavor::Github, Mode::Comment);
    o.context = Some("joeyespo/grip".into());
    let out = r.render(DOC, &o).expect("GitHub API render");
    assert!(out.html.contains(r#"<h1 dir="auto">"#), "{}", out.html);
    assert!(out.html.contains(r#"<code class="notranslate">code</code>"#), "{}", out.html);
}

#[test]
#[ignore = "network + GITLAB_TOKEN: POST https://gitlab.com/api/v4/markdown (anonymous requests get 401)"]
fn gitlab_live() {
    if std::env::var_os("GITLAB_TOKEN").is_none() {
        eprintln!("SKIPPED: set GITLAB_TOKEN");
        return;
    }
    let cfg = ApiConfig { flavor: Flavor::Gitlab, api_url: None, token: None, fallback_offline: false };
    let r = ApiRenderer::new(cfg).expect("renderer");
    let out = r.render(DOC, &options(Flavor::Gitlab, Mode::Readme)).expect("GitLab API render");
    assert!(out.html.contains("code-syntax-highlight-theme"), "{}", out.html);
    assert!(!out.html.contains(r#"class="lazy"#), "{}", out.html);
}

#[test]
#[ignore = "network: unauthenticated GitLab request must fall back offline with a warning"]
fn gitlab_live_unauthorized_falls_back() {
    let cfg = ApiConfig { flavor: Flavor::Gitlab, api_url: None, token: Some("invalid-token".into()), fallback_offline: true };
    let r = ApiRenderer::new(cfg).expect("renderer");
    let out = r.render(DOC, &options(Flavor::Gitlab, Mode::Readme)).expect("fallback render");
    assert!(out.warnings.iter().any(|w| w.contains("unauthorized")), "{:?}", out.warnings);
}
