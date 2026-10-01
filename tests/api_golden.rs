//! Golden tests of the `--api` post-processing against real site output (D18).
//!
//! GitHub: the captured `POST /markdown` (`mode=gfm`) responses of three public READMEs are fed
//! through [`ApiRenderer`] with a canned transport, and the result is compared with the README
//! view of the same files (`application/vnd.github.html` contents API), after a normalization
//! that removes the documented differences (D15, CQ §1.5: soft breaks are `<br>`) and per-render
//! random values. GitLab: captured issue bodies (`:full` pipeline, like the API) must come out
//! without lazy-loading placeholders.
//!
//! The fixtures are not committed: they are read from `$MD_RENDER_GOLDEN_DIR` (default
//! `<repo>/.dev/research`), and each test prints `SKIPPED (set MD_RENDER_GOLDEN_DIR)` when absent.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dom_query::Document;
use md_render::render::api::{
    ApiConfig, ApiRenderer, ApiRequest, ApiResponse, HttpTransport, TransportError,
};
use md_render::render::{RenderOptions, Renderer};
use md_render::{Flavor, Mode};
use regex::Regex;

fn golden_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("MD_RENDER_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".dev/research"));
    if dir.is_dir() {
        Some(dir)
    } else {
        eprintln!("SKIPPED (set MD_RENDER_GOLDEN_DIR): {} does not exist", dir.display());
        None
    }
}

/// Answers every request with the same body.
struct Canned(String);

impl HttpTransport for Canned {
    fn post_json(&self, _: &ApiRequest) -> Result<ApiResponse, TransportError> {
        Ok(ApiResponse { status: 200, headers: vec![], body: self.0.clone() })
    }
}

fn renderer(flavor: Flavor, body: String) -> ApiRenderer {
    let cfg = ApiConfig { flavor, api_url: None, token: None, fallback_offline: false };
    ApiRenderer::with_transport(cfg, Arc::new(Canned(body))).expect("renderer")
}

/// Canonical form for comparison: drop our own attributes and the documented differences,
/// replace random ids, one tag per line.
fn normalize(html: &str) -> Vec<String> {
    let doc = Document::fragment(html);
    for node in doc.select("[data-mdr-key], [tabindex]").nodes().iter() {
        node.remove_attr("data-mdr-key");
        node.remove_attr("tabindex");
    }
    // D15 / CQ §1.5: `mode=gfm` turns soft breaks into `<br>`; the README view does not.
    doc.select("br").remove();
    // Deliberate difference (GRIP C51): `--api` shows canonical image URLs instead of camo
    // proxies, so apply the same rewrite to the oracle.
    for img in doc.select("img[data-canonical-src]").nodes().iter() {
        let canonical = img.attr("data-canonical-src").map(|v| v.to_string()).unwrap_or_default();
        let src = img.attr("src").map(|v| v.to_string());
        if let Some(a) = img.parent().filter(|p| p.has_name("a")) {
            if a.attr("href").map(|h| h.to_string()) == src {
                a.set_attr("href", &canonical);
            }
        }
        img.set_attr("src", &canonical);
        img.remove_attr("data-canonical-src");
    }
    let html = doc.html_root().inner_html().to_string();
    let rid = Regex::new(r"[0-9a-f]{32}").unwrap();
    let uuid = Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap();
    // Per-render random values (GH §2.3): run ids, enrichment identities, label tooltip ids.
    let label = Regex::new(r#""label-[0-9a-f]{6}""#).unwrap();
    // Signed, time-limited attachment URLs differ per request.
    let jwt = Regex::new(r"jwt=[A-Za-z0-9._-]+").unwrap();
    let html = rid.replace_all(&html, "RID");
    let html = uuid.replace_all(&html, "UUID");
    let html = label.replace_all(&html, "\"label-ID\"");
    let html = jwt.replace_all(&html, "jwt=JWT");
    html.replace('>', ">\n")
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect()
}

/// The inner HTML of the contents-API `article`.
fn article_inner(html: &str) -> String {
    let doc = Document::fragment(html);
    doc.select("article.markdown-body").inner_html().to_string()
}

/// Lines of `got` and `want` that differ, as `(index, got, want)`, after aligning on equal runs.
fn differences(got: &[String], want: &[String]) -> Vec<String> {
    // Classic LCS table; the documents are a few thousand lines at most.
    let (n, m) = (got.len(), want.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if got[i] == want[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && got[i] == want[j] {
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push(format!("+ want[{j}] {}", want[j]));
            j += 1;
        } else {
            out.push(format!("- got[{i}]  {}", got[i]));
            i += 1;
        }
    }
    out
}

#[test]
fn github_readme_mode_matches_the_readme_view() {
    let Some(dir) = golden_dir() else { return };
    let pages = dir.join("github/pages");
    let names = ["dipree_github-markdown", "atapas_markdown-cheatsheet", "lifeparticle_Markdown-Cheatsheet"];
    let mut report = String::new();
    for name in names {
        let read = |suffix: &str| fs::read_to_string(pages.join(format!("{name}.{suffix}")));
        let (Ok(source), Ok(api), Ok(oracle), Ok(req)) =
            (read("README.md"), read("README.api_gfm.html"), read("README.api_html.html"), read("req_gfm.json"))
        else {
            eprintln!("SKIPPED {name}: fixture files missing under {}", pages.display());
            continue;
        };
        let req: serde_json::Value = serde_json::from_str(&req).expect("request json");
        let mut opts = RenderOptions::new(Flavor::Github);
        opts.mode = Mode::Readme;
        opts.context = req["context"].as_str().map(str::to_owned);
        opts.sourcepos = false;
        let out = renderer(Flavor::Github, api).render(&source, &opts).expect("render");
        let got = normalize(&out.html);
        let want = normalize(&article_inner(&oracle));
        let diffs = differences(&got, &want);
        let total = want.len();
        eprintln!("{name}: {} of {total} normalized lines differ", diffs.len());
        for d in diffs.iter().take(60) {
            eprintln!("  {d}");
        }
        if !diffs.is_empty() {
            report.push_str(&format!("{name}: {} differing lines\n", diffs.len()));
        }
    }
    assert!(report.is_empty(), "README-view mismatches:\n{report}");
}

/// Comment mode: `POST /markdown` (`mode=gfm`, `context=cli/cli`) of real issue bodies versus the
/// HTML GitHub stored for them (GraphQL `bodyHTML`, GH §2.4).
#[test]
fn github_comment_mode_matches_stored_comments() {
    let Some(dir) = golden_dir() else { return };
    let comments = dir.join("github/comments");
    let mut report = String::new();
    for issue in ["cli_14520", "cli_14522", "cli_14545", "cli_14554"] {
        let read = |suffix: &str| fs::read_to_string(comments.join(format!("{issue}.{suffix}")));
        let (Ok(source), Ok(api), Ok(stored)) = (read("md"), read("api_gfm.html"), read("bodyHTML.html")) else {
            eprintln!("SKIPPED {issue}: fixture files missing under {}", comments.display());
            continue;
        };
        let mut opts = RenderOptions::new(Flavor::Github);
        opts.mode = Mode::Comment;
        opts.context = Some("cli/cli".into());
        opts.sourcepos = false;
        let out = renderer(Flavor::Github, api).render(&source, &opts).expect("render");
        let diffs = differences(&normalize(&out.html), &normalize(&stored));
        eprintln!("{issue}: {} normalized lines differ", diffs.len());
        for d in diffs.iter().take(40) {
            eprintln!("  {d}");
        }
        if !diffs.is_empty() {
            report.push_str(&format!("{issue}: {} differing lines\n", diffs.len()));
        }
    }
    assert!(report.is_empty(), "stored-comment mismatches:\n{report}");
}

#[test]
fn gitlab_issue_bodies_are_unlazied() {
    let Some(dir) = golden_dir() else { return };
    let samples = dir.join("gitlab/issue_samples");
    let Ok(entries) = fs::read_dir(&samples) else {
        eprintln!("SKIPPED: {} missing", samples.display());
        return;
    };
    let mut checked = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("html") {
            continue;
        }
        let html = fs::read_to_string(&path).expect("sample");
        let body = serde_json::json!({ "html": html }).to_string();
        let out = renderer(Flavor::Gitlab, body)
            .render("ignored", &RenderOptions::new(Flavor::Gitlab))
            .expect("render");
        let doc = Document::fragment(out.html.as_str());
        let name = path.display();
        assert!(!doc.select("img.lazy, img[data-src]").exists(), "{name}: lazy image left");
        assert!(!doc.select(r#"img[src^="data:image/gif"]"#).exists(), "{name}: placeholder left");
        for img in doc.select("img").nodes().iter() {
            let src = img.attr("src").map(|s| s.to_string()).unwrap_or_default();
            assert!(!src.starts_with("/-/"), "{name}: relative upload URL {src}");
        }
        for pre in doc.select("pre.js-syntax-highlight").nodes().iter() {
            assert!(pre.has_class("code-syntax-highlight-theme"), "{name}: {}", pre.html());
        }
        checked += 1;
    }
    eprintln!("checked {checked} GitLab samples");
}
