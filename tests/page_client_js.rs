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

/// A scenario prelude for live updates of a one-block document: `UPDATE` is the top-level HTML
/// of the block (`__N__` stands for the update number), and `liveUpdate(window, t, i)` applies
/// version `i` like live.js's reconcile does: the new node is inserted, every old top-level node
/// (the previous block, alerts the flavor script added) is removed, and `mdr:content-updated`
/// is dispatched with the added node.
fn live_prelude(flavor: Flavor, markdown_template: &str) -> String {
    let update = rendered(flavor, markdown_template).html;
    format!(
        r#"const UPDATE = {};
async function liveUpdate(window, t, i) {{
  const d = window.document;
  const content = d.getElementById('md-content');
  const old = content.childNodes.slice();
  const holder = d.createElement('div');
  holder.innerHTML = UPDATE.replace(/__N__/g, String(i));
  const next = holder.firstElementChild;
  content.insertBefore(next, content.firstChild);
  old.forEach((n) => n.remove());
  d.dispatchEvent({{ type: 'mdr:content-updated', detail: {{ root: content, added: [next] }} }});
  await t.flush(2);
}}
"#,
        serde_json::to_string(update.trim()).unwrap()
    )
}

/// Q-LIVE-GL-LIMITS: GitLab's "50 diagrams per page" limit counts the diagrams on the page, not
/// every diagram drawn since the page loaded: 60 live edits of one diagram keep rendering it.
#[test]
fn gitlab_mermaid_limit_counts_only_live_diagrams() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = gitlab_lib_bundle(dir.path());
    let doc = "```mermaid\ngraph TD\n  A --> B__N__\n```\n";
    let html = page(Flavor::Gitlab, doc, &bundle, AssetMode::Inline);
    let scenario = format!(
        r#"{}
module.exports = {{ async run(window, t) {{
  const d = window.document;
  t.equal(d.querySelectorAll('.md-render-mermaid').length, 1, 'initial diagram');
  for (let i = 1; i <= 60; i++) await liveUpdate(window, t, i);
  t.equal(d.querySelectorAll('#md-content .js-markdown-code').length, 1, 'one diagram on the page');
  t.equal(d.querySelectorAll('.md-render-mermaid').length, 1, 'it is rendered');
  t.equal(d.querySelectorAll('.gl-alert').length, 0, 'no performance warning');
  t.equal(window.mdrTest.mermaidCalls, 61, 'every version was drawn');
}} }};"#,
        live_prelude(Flavor::Gitlab, doc)
    );
    run_scenario(
        "gitlab_mermaid_limit_counts_only_live_diagrams",
        &html,
        &scenario,
    );
}

/// The 50-diagram limit itself still applies to the diagrams on the page.
#[test]
fn gitlab_mermaid_limit_applies_per_page() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = gitlab_lib_bundle(dir.path());
    let doc: String = (0..52)
        .map(|i| format!("```mermaid\ngraph TD\n  A --> B{i}\n```\n\n"))
        .collect();
    let html = page(Flavor::Gitlab, &doc, &bundle, AssetMode::Inline);
    run_scenario(
        "gitlab_mermaid_limit_applies_per_page",
        &html,
        r#"module.exports = { async run(window, t) {
  const d = window.document;
  t.equal(d.querySelectorAll('.md-render-mermaid').length, 50, '50 diagrams are drawn');
  const alerts = d.querySelectorAll('.gl-alert');
  t.equal(alerts.length, 2, 'the others get a warning');
  alerts[0].querySelector('button').click();
  await t.flush();
  t.equal(d.querySelectorAll('.md-render-mermaid').length, 51, '"Display" draws one more');
} };"#,
    );
}

/// Q-LIVE-GL-LIMITS: the 2000 ms KaTeX budget covers the math on the page: 30 live edits of a
/// formula that takes 100 ms keep rendering it (the old page-lifetime total stopped at 20).
#[test]
fn gitlab_math_time_budget_counts_only_live_math() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = gitlab_lib_bundle(dir.path());
    let doc = "```math\nx^{__N__}\n```\n";
    let html = page(Flavor::Gitlab, doc, &bundle, AssetMode::Inline);
    let scenario = format!(
        r#"{}
module.exports = {{
  before(window) {{
    window.mdrTest.clock = 0;
    window.mdrTest.katexCost = 100;
    window.Date = {{ now: () => window.mdrTest.clock }};
  }},
  async run(window, t) {{
    const d = window.document;
    for (let i = 1; i <= 30; i++) await liveUpdate(window, t, i);
    const math = d.querySelectorAll('#md-content [data-math-style]');
    t.equal(math.length, 1, 'one formula on the page');
    t.ok(math[0] && /class="katex"/.test(math[0].innerHTML), 'it is rendered: ' + (math[0] && math[0].innerHTML));
    t.equal(d.querySelectorAll('.gl-alert').length, 0, 'no performance warning');
    t.equal(window.mdrTest.katexCalls, 31, 'every version was rendered');
  }}
}};"#,
        live_prelude(Flavor::Gitlab, doc)
    );
    run_scenario(
        "gitlab_math_time_budget_counts_only_live_math",
        &html,
        &scenario,
    );
}

/// The budget itself still applies to the math on the page.
#[test]
fn gitlab_math_time_budget_applies_per_page() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = gitlab_lib_bundle(dir.path());
    let doc: String = (0..25).map(|i| format!("$`x_{{{i}}}`$\n\n")).collect();
    let html = page(Flavor::Gitlab, &doc, &bundle, AssetMode::Inline);
    run_scenario(
        "gitlab_math_time_budget_applies_per_page",
        &html,
        r#"module.exports = {
  before(window) {
    window.mdrTest.clock = 0;
    window.mdrTest.katexCost = 100;
    window.Date = { now: () => window.mdrTest.clock };
  },
  async run(window, t) {
    const d = window.document;
    t.equal(window.mdrTest.katexCalls, 20, '2000 ms of rendering');
    t.equal(d.querySelectorAll('.gl-alert').length, 5, 'the rest gets a warning');
  }
};"#,
    );
}

/// PAGE-2: in a self-contained GitLab export the copy button draws GitLab's own
/// copy-to-clipboard icon from the symbol the page carries (like a served page does from the
/// sprite file), not the Octicon fallback; without a vendored sprite the fallback stays.
#[test]
fn gitlab_inline_export_copy_button_uses_the_gitlab_icon() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = BundleBuilder::new()
        .stylesheet(
            Flavor::Gitlab,
            "gitlab/assets/application-1.css",
            "app",
            Some("light"),
            ".md{}",
        )
        .asset(
            Flavor::Gitlab,
            "gitlab/assets/icons-0123abcd.svg",
            page_support::GITLAB_SPRITE,
        )
        .open(dir.path());
    let code = "# T\n\n```rust\nfn main() {}\n```\n";
    let with_sprite = page(Flavor::Gitlab, code, &bundle, AssetMode::Inline);
    run_scenario(
        "gitlab_inline_export_copy_button_uses_the_gitlab_icon",
        &with_sprite,
        r##"module.exports = { async run(window, t) {
  const d = window.document;
  const button = d.querySelector('copy-code button');
  t.ok(button, 'a copy button');
  const html = button ? button.innerHTML : '';
  t.ok(/<use href="#copy-to-clipboard"/.test(html), 'GitLab icon from the in-page symbol: ' + html);
  t.ok(d.getElementById('copy-to-clipboard'), 'the symbol is in the page');
} };"##,
    );
    let fallback = page(Flavor::Gitlab, code, &Bundle::fallback(), AssetMode::Inline);
    run_scenario(
        "gitlab_inline_export_copy_button_without_sprite",
        &fallback,
        r##"module.exports = { async run(window, t) {
  const html = window.document.querySelector('copy-code button').innerHTML;
  t.ok(/M0 6\.75C0 5\.784/.test(html) && !/<use/.test(html), 'Octicon fallback: ' + html);
} };"##,
    );
}

/// Q-DUP-JS: both flavors' hash navigation comes from common.js (one implementation instead
/// of two copies that had drifted) with each site's target rule: a `#title` link reaches
/// `user-content-title` on load, on click and after a live update, and GitHub also finds
/// `<a name="user-content-…">` targets. Pages load common.js before the flavor script.
#[test]
fn hash_navigation_from_the_shared_helpers() {
    let markdown = "# Title\n\nSee [the title](#title) and [x](#x).\n\n<a name=\"x\"></a>X\n";
    for flavor in [Flavor::Github, Flavor::Gitlab] {
        for assets in [
            AssetMode::Inline,
            AssetMode::Server {
                prefix: "/__/md-render".into(),
            },
        ] {
            let html = page(flavor, markdown, &Bundle::fallback(), assets.clone());
            let at = |name: &str| {
                html.find(&format!("static/{name}\""))
                    .or_else(|| html.find(&format!("data-mdr-static=\"{name}\"")))
                    .unwrap_or_else(|| panic!("{flavor} {assets:?}: {name} is loaded"))
            };
            assert!(
                at("common.js") < at(&format!("{flavor}.js")),
                "{flavor} {assets:?}: common.js comes first"
            );
            let by_name = flavor == Flavor::Github;
            run_scenario(
                &format!("hash_navigation_from_the_shared_helpers ({flavor}, {assets:?})"),
                &html,
                &format!(
                    r##"module.exports = {{
  before(window) {{ window.location.hash = '#title'; }},
  async run(window, t) {{
    const d = window.document;
    const target = d.getElementById('user-content-title');
    t.ok(target, 'prefixed heading id');
    const scrolled = () => window.mdrTest.scrolledTo.map((s) => s.el);
    t.ok(scrolled().includes(target), 'scrolled to the prefixed target on load');
    window.mdrTest.scrolledTo.length = 0;
    window.location.hash = '';
    d.querySelector('#md-content a[href="#title"]').click();
    t.equal(window.location.hash, '#title', 'the hash follows the click');
    t.ok(scrolled().includes(target), 'scrolled on click');
    window.mdrTest.scrolledTo.length = 0;
    d.querySelector('#md-content a[href="#x"]').click();
    const named = d.getElementsByName('user-content-x')[0];
    t.equal(!!named && scrolled().includes(named), {by_name}, 'name targets (GitHub only)');
    // A live update that brings the heading follows the hash into the new content.
    window.location.hash = '#title';
    window.mdrTest.scrolledTo.length = 0;
    const content = d.getElementById('md-content');
    let block = target;
    while (block.parentNode && block.parentNode !== content) block = block.parentNode;
    d.dispatchEvent({{ type: 'mdr:content-updated', detail: {{ root: content, added: [block] }} }});
    await t.flush(2);
    t.ok(scrolled().includes(target), 'follows the hash into updated content');
    t.ok(window.mdRenderCommon && typeof window.mdRenderCommon.loadLibrary === 'function', 'shared helpers');
  }}
}};"##
                ),
            );
        }
    }
}
