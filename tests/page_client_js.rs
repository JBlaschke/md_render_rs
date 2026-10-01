//! The flavor client scripts (`assets/client/{github,gitlab}.js`) run against real pages under
//! Node, with the small DOM of `tests/page_support/dom_harness.js`: math and mermaid with and
//! without the vendored libraries, the page-lifetime limits during live updates, copy buttons
//! and hash navigation. Prints `SKIPPED (node not on PATH)` and passes when Node is missing.

mod page_support;

use std::path::Path;
use std::process::Command;

use md_render::page::AssetMode;
use md_render::render::{RenderOptions, render};
use md_render::styles::Bundle;
use md_render::{Flavor, Theme};
use page_support::{BundleBuilder, Page, theme};

/// KaTeX double: renders `<span class="katex">` and advances the fake clock by
/// `mdrTest.katexCost` ms per call (scenarios replace `Date` with that clock).
const KATEX_STUB: &[u8] = br#"window.katex = { renderToString: function (tex, opts) {
  var t = window.mdrTest; t.katexCalls = (t.katexCalls || 0) + 1; t.clock = (t.clock || 0) + (t.katexCost || 0);
  return '<span class="katex">' + (opts && opts.displayMode ? 'D:' : 'I:') + tex.replace(/&/g, '&amp;').replace(/</g, '&lt;') + '</span>';
} };"#;

/// mermaid double: one `<svg>` per render call.
const MERMAID_STUB: &[u8] =
    br#"window.mermaid = { initialize: function () {}, render: function (id, src) {
  var t = window.mdrTest; t.mermaidCalls = (t.mermaidCalls || 0) + 1;
  return Promise.resolve({ svg: '<svg id="' + id + '" data-src="' + src.length + '"></svg>' });
} };"#;

/// MathJax double: `tex2mml` returns a `<math>` element holding the TeX as `<mi>`.
const MATHJAX_STUB: &[u8] = br#"window.MathJax = Object.assign(window.MathJax || {}, {
  tex2mml: function (tex, o) { window.mdrTest.mathjaxCalls = (window.mdrTest.mathjaxCalls || 0) + 1;
    return '<math display="' + (o && o.display ? 'block' : 'inline') + '"><mi>' + tex.replace(/</g, '&lt;') + '</mi></math>'; }
});
window.MathJax.startup = { promise: Promise.resolve(), defaultReady: function () {}, document: null };
"#;

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Run `scenario` (a CommonJS module, see dom_harness.js) against `page`; panics with the
/// harness output when it reports a failure.
fn run_scenario(name: &str, page: &str, scenario: &str) {
    if !node_available() {
        eprintln!("SKIPPED (node not on PATH): page_client_js::{name}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let page_path = dir.path().join("page.html");
    let scenario_path = dir.path().join("scenario.js");
    std::fs::write(&page_path, page).unwrap();
    std::fs::write(&scenario_path, scenario).unwrap();
    let harness = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/page_support/dom_harness.js");
    let out = Command::new("node")
        .arg(&harness)
        .arg(&page_path)
        .arg(&scenario_path)
        .output()
        .expect("node runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "{name}: client scenario failed\n{stdout}\n{stderr}"
    );
}

fn rendered(flavor: Flavor, markdown: &str) -> md_render::render::Rendered {
    render(markdown, &RenderOptions::new(flavor))
}

fn page(flavor: Flavor, markdown: &str, bundle: &Bundle, assets: AssetMode) -> String {
    let mut p = Page::new(flavor);
    p.content = rendered(flavor, markdown);
    p.assets = assets;
    p.theme = theme(Theme::Light, "light", "dark");
    p.render(bundle)
}

/// GitLab flavor with stub KaTeX and mermaid libraries (inlined into the export).
fn gitlab_lib_bundle(dir: &Path) -> Bundle {
    BundleBuilder::new()
        .stylesheet(
            Flavor::Gitlab,
            "gitlab/assets/application-1.css",
            "app",
            Some("light"),
            ".md{color:#333}",
        )
        .script(
            Flavor::Gitlab,
            "katex",
            "gitlab/lib/katex/katex.min.js",
            KATEX_STUB,
        )
        .script(
            Flavor::Gitlab,
            "katex-css",
            "gitlab/lib/katex/katex.min.css",
            b".katex{font:normal 1.21em KaTeX_Main}",
        )
        .script(
            Flavor::Gitlab,
            "mermaid",
            "gitlab/lib/mermaid/mermaid.min.js",
            MERMAID_STUB,
        )
        .open(dir)
}

/// GitHub flavor with stub MathJax and mermaid libraries.
fn github_lib_bundle(dir: &Path) -> Bundle {
    BundleBuilder::new()
        .stylesheet(
            Flavor::Github,
            "github/assets/light-1.css",
            "theme",
            Some("light"),
            ":root{--x:1}",
        )
        .script(
            Flavor::Github,
            "mathjax",
            "github/lib/mathjax/es5/tex-chtml-full.js",
            MATHJAX_STUB,
        )
        .script(
            Flavor::Github,
            "mermaid",
            "github/lib/mermaid/mermaid.min.js",
            MERMAID_STUB,
        )
        .open(dir)
}

const MATH_DOC: &str =
    "# Title\n\nHello $x^2$ world and $`y`$.\n\n```math\na = b \\\\\nc = d\n```\n";

/// JS-1: without KaTeX (no bundle yet, `vendor --no-libs`, failed download) the TeX source
/// stays as the renderer wrote it: inline math keeps its `<span>`/`<code>` inside the paragraph
/// and a ```math block keeps its `<pre>`, instead of becoming block `<div>`s.
#[test]
fn gitlab_math_without_katex_keeps_the_source_markup() {
    let html = page(
        Flavor::Gitlab,
        MATH_DOC,
        &Bundle::fallback(),
        AssetMode::Inline,
    );
    run_scenario(
        "gitlab_math_without_katex_keeps_the_source_markup",
        &html,
        r#"module.exports = { async run(window, t) {
  const d = window.document;
  const p = d.querySelector('#md-content p');
  t.equal(p.querySelectorAll('div').length, 0, 'no block element inside the paragraph');
  const inline = p.querySelectorAll('.js-render-math');
  t.equal(inline.map((e) => e.tagName), ['SPAN', 'CODE'], 'inline math keeps its elements');
  t.equal(inline.map((e) => e.textContent), ['x^2', 'y'], 'inline TeX');
  const pre = d.querySelector('pre.js-render-math');
  t.ok(pre, 'the ```math <pre> stays');
  t.ok(pre && pre.textContent.indexOf('a = b \\\\\nc = d') !== -1, 'line breaks of the TeX stay');
  t.equal(d.querySelectorAll('[data-math-style]').filter((e) => e.tagName === 'DIV').length, 0, 'no placeholders');
  t.ok(t.logs.some((l) => /math is shown as TeX source/.test(l.text)), 'the failure is logged');
} };"#,
    );
}

/// The same with a library URL that fails to load (served page, KaTeX file missing).
#[test]
fn gitlab_math_with_failing_katex_download_keeps_the_source_markup() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = gitlab_lib_bundle(dir.path());
    let html = page(
        Flavor::Gitlab,
        MATH_DOC,
        &bundle,
        AssetMode::Server {
            prefix: "/__/md-render".into(),
        },
    );
    run_scenario(
        "gitlab_math_with_failing_katex_download_keeps_the_source_markup",
        &html,
        r#"module.exports = {
  before(window) { window.mdrTest.loadScript = (s) => (/\/lib\//.test(s.src) ? false : undefined); },
  async run(window, t) {
    const d = window.document;
    t.equal(d.querySelector('#md-content p').querySelectorAll('div').length, 0, 'paragraph stays one line');
    t.ok(d.querySelector('pre.js-render-math'), 'the ```math <pre> stays');
    t.ok(t.logs.some((l) => /failed to load/.test(l.text)), 'the failure is logged');
  } };"#,
    );
}

/// With KaTeX, GitLab's placeholders carry their layout classes (render_math.js): inline math
/// is `div.math-content-inline` (display:inline-flex) inside the paragraph.
#[test]
fn gitlab_math_with_katex_renders_into_classed_placeholders() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = gitlab_lib_bundle(dir.path());
    let html = page(Flavor::Gitlab, MATH_DOC, &bundle, AssetMode::Inline);
    run_scenario(
        "gitlab_math_with_katex_renders_into_classed_placeholders",
        &html,
        r#"module.exports = { async run(window, t) {
  const d = window.document;
  const inline = d.querySelectorAll('#md-content p [data-math-style]');
  t.equal(inline.map((e) => e.tagName + '.' + e.className), ['DIV.math-content-inline', 'DIV.math-content-inline'], 'inline placeholders');
  t.equal(inline.map((e) => e.innerHTML), ['<span class="katex">I:x^2</span>', '<span class="katex">I:y</span>'], 'inline KaTeX output');
  const display = d.querySelectorAll('.js-markdown-code [data-math-style="display"]');
  t.equal(display.map((e) => e.className), ['math-content-display'], 'display placeholder');
  t.ok(display[0] && /D:a = b/.test(display[0].innerHTML), 'display KaTeX output');
  t.equal(d.querySelectorAll('.js-render-math').length, 0, 'every source element was replaced');
  t.ok(d.querySelector('.js-markdown-code copy-code button'), 'the copy button of the math block stays');
  t.equal(window.mdrTest.katexCalls, 3, 'one KaTeX call per formula');
} };"#,
    );
}

/// The GitHub flavor in the same situation: `<math-renderer>` keeps its TeX source.
#[test]
fn github_math_without_mathjax_keeps_the_source() {
    let html = page(
        Flavor::Github,
        MATH_DOC,
        &Bundle::fallback(),
        AssetMode::Inline,
    );
    run_scenario(
        "github_math_without_mathjax_keeps_the_source",
        &html,
        r#"module.exports = { async run(window, t) {
  const d = window.document;
  const inline = d.querySelectorAll('#md-content p math-renderer');
  t.equal(inline.map((e) => e.textContent), ['$x^2$', '$y$'], 'inline TeX stays in the paragraph');
  t.ok(t.logs.some((l) => /math is shown as TeX source/.test(l.text)), 'the failure is logged');
} };"#,
    );
}

#[test]
fn github_math_with_mathjax_renders_mathml() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = github_lib_bundle(dir.path());
    let html = page(Flavor::Github, MATH_DOC, &bundle, AssetMode::Inline);
    run_scenario(
        "github_math_with_mathjax_renders_mathml",
        &html,
        r#"module.exports = { async run(window, t) {
  const d = window.document;
  const inline = d.querySelectorAll('#md-content p math-renderer');
  t.equal(inline.map((e) => e.innerHTML), ['<math display="inline"><mi>x^2</mi></math>', '<math display="inline"><mi>y</mi></math>'], 'inline MathML');
  const display = d.querySelectorAll('math-renderer.js-display-math');
  t.ok(display.length === 1 && /<math display="block">/.test(display[0].innerHTML), 'display MathML');
  t.equal(window.mdrTest.mathjaxCalls, 3, 'one tex2mml call per formula');
} };"#,
    );
}
