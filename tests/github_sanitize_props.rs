//! Property-style sanitizer tests for the GitHub flavor (DESIGN §9; review findings GH-SEC-1
//! and GH-SEC-2).
//!
//! Many generated documents mix unterminated and unclosed raw tags (every HTML formatting
//! element, block tags, table / select / foreign-content contexts) with every Markdown block
//! type. The rendered HTML is parsed again, as a browser would, and the final DOM must hold
//! no event-handler attribute, no `javascript:` / `vbscript:` / `data:text/html` URL in a
//! URL attribute, none of the elements raw HTML must never produce (`<script>`, `<base>`,
//! `<form>`, `<link>`, `<style>`, `<iframe>`, `<object>`, `<embed>`), no `style` attribute
//! other than the ones the renderer itself writes, and no leaked sanitizer marker.
//!
//! The generator is a fixed-seed PRNG, so failures reproduce; `MD_RENDER_PROP_CASES=N`
//! raises the number of random documents (default 1500).

use dom_query::{Document, NodeRef};
use md_render::flavor::{Flavor, Mode};
use md_render::render::{RenderOptions, github};

// ------------------------------------------------------------------------------------------
// generator

/// splitmix64: small, deterministic, good enough for test case generation.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap_or(0)
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// HTML formatting elements (reconstructed by the parser; GH-SEC-2).
const FORMATTING: &[&str] = &[
    "a", "b", "big", "code", "em", "font", "i", "nobr", "s", "small", "strike", "strong", "tt", "u",
];

/// Block, table, form, embedding and foreign-content tags (CommonMark type 6 and 7 starts
/// included; GH-SEC-1).
const OTHER_TAGS: &[&str] = &[
    "div",
    "details",
    "summary",
    "section",
    "article",
    "aside",
    "nav",
    "header",
    "footer",
    "main",
    "center",
    "address",
    "fieldset",
    "legend",
    "figure",
    "figcaption",
    "menu",
    "dialog",
    "search",
    "p",
    "blockquote",
    "pre",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "h1",
    "h3",
    "h6",
    "hr",
    "br",
    "table",
    "caption",
    "colgroup",
    "col",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "td",
    "th",
    "form",
    "base",
    "link",
    "meta",
    "style",
    "script",
    "iframe",
    "object",
    "embed",
    "applet",
    "frame",
    "frameset",
    "noframes",
    "noembed",
    "noscript",
    "textarea",
    "select",
    "option",
    "optgroup",
    "button",
    "input",
    "keygen",
    "template",
    "title",
    "xmp",
    "plaintext",
    "svg",
    "math",
    "foreignObject",
    "img",
    "image",
    "picture",
    "source",
    "video",
    "audio",
    "track",
    "span",
    "kbd",
    "sup",
    "ins",
    "del",
    "q",
    "mark",
    "ruby",
    "math-renderer",
    "markdown-accessiblity-table",
    "g-emoji",
    "x-unknown",
];

/// Hostile attributes. Values are inert placeholders; `STYLE_SENTINEL` and the marker shapes
/// are what the checks look for, so a raw attribute can never be confused with one the
/// renderer writes itself.
const STYLE_SENTINEL: &str = "raw-style-sentinel";

fn attrs() -> Vec<String> {
    vec![
        r#"onclick="x""#.into(),
        r#"onmouseover="x""#.into(),
        "onfocus=x".into(),
        r#"ONTOGGLE="x""#.into(),
        format!(r#"style="{STYLE_SENTINEL}""#),
        r#"href="javascript:x""#.into(),
        r#"href=" JaVa&#x09;ScRiPt:x""#.into(),
        r#"href="vbscript:x""#.into(),
        r#"href="data:text/html,x""#.into(),
        r#"src="javascript:x""#.into(),
        r#"src="data:text/html,x""#.into(),
        r#"srcset="javascript:x 1x, a.png 2x""#.into(),
        r#"action="javascript:x""#.into(),
        r#"formaction="javascript:x""#.into(),
        r#"xlink:href="javascript:x""#.into(),
        r#"data="data:text/html,x""#.into(),
        r#"poster="javascript:x""#.into(),
        r#"background="javascript:x""#.into(),
        r#"cite="javascript:x""#.into(),
        r#"longdesc="javascript:x""#.into(),
        "autofocus".into(),
        r#"tabindex="1""#.into(),
        "open".into(),
        r#"class="markdown-alert""#.into(),
        r#"id="x""#.into(),
        r#"data-mdr-g="0123456789abcdef""#.into(),
        r#"data-mdr-g="0123456789abcdef-t""#.into(),
    ]
}

/// Contexts that change how later markup is parsed.
const CONTEXTS: &[&str] = &[
    "<table>",
    "<table><tr>",
    "<table><tbody><tr><td>",
    "<table><caption>",
    "<table><colgroup>",
    "<select>",
    "<svg>",
    "<math>",
    "<math><mtext>",
    "<svg><foreignObject>",
    "<template>",
    "<noscript>",
    "<frameset>",
    "<b><i><em><strong><s><code><tt><strike><a href=\"javascript:x\">",
    "",
];

/// One Markdown block of every kind the formatter writes.
const BLOCKS: &[&str] = &[
    "paragraph text",
    "**bold** _em_ `code` ~~del~~",
    "# heading one",
    "### heading three",
    "###### heading six",
    "- bullet item\n- second",
    "1. ordered\n2. second",
    "- [ ] todo\n- [x] done",
    "> quoted text",
    "> [!NOTE]\n> an alert body",
    "> [!TIP]\n> $$e=mc^2$$",
    "```\nplain code\n```",
    "```js\nlet a = 1;\n```",
    "```math\n\\frac{1}{2}\n```",
    "```mermaid\ngraph TD\n  A --> B\n```",
    "```geojson\n{\"type\":\"Point\"}\n```",
    "```nosuchlang\nunknown\n```",
    "| a | b |\n|---|---|\n| 1 | 2 |",
    "text with inline $x^2$ math",
    "a paragraph of only $$x^2$$",
    "ref[^1] here\n\n[^1]: a footnote",
    "---",
    "![alt](img.png)",
    "[a link](https://example.com/)",
    ":smile: :octocat:",
    "www.localhost http://localhost:3000",
    "",
];

/// Builds one pseudo-random document.
fn generate(rng: &mut Rng) -> String {
    let mut doc = String::new();
    let fragments = 2 + rng.below(6);
    for _ in 0..fragments {
        match rng.below(10) {
            // An unterminated raw start tag (no closing `>`): absorbs what follows (GH-SEC-1).
            0..=2 => {
                let tag = if rng.below(2) == 0 {
                    rng.pick(FORMATTING)
                } else {
                    rng.pick(OTHER_TAGS)
                };
                let n = rng.below(3);
                let mut s = format!("{}<{tag}", rng.pick(CONTEXTS));
                let pool = attrs();
                for _ in 0..n {
                    s.push(' ');
                    s.push_str(rng.pick(&pool));
                }
                doc.push_str(&s);
                doc.push('\n');
            }
            // An unclosed (but terminated) raw start tag: stays in the formatting list and is
            // reconstructed into later subtrees (GH-SEC-2).
            3..=5 => {
                let tag = if rng.below(2) == 0 {
                    rng.pick(FORMATTING)
                } else {
                    rng.pick(OTHER_TAGS)
                };
                let n = rng.below(3);
                let mut s = format!("{}<{tag}", rng.pick(CONTEXTS));
                let pool = attrs();
                for _ in 0..n {
                    s.push(' ');
                    s.push_str(rng.pick(&pool));
                }
                s.push('>');
                s.push_str("raw");
                doc.push_str(&s);
            }
            // A plain Markdown block.
            _ => {
                doc.push_str(rng.pick(BLOCKS));
            }
        }
        doc.push_str("\n\n");
    }
    doc
}

// ------------------------------------------------------------------------------------------
// invariants

/// Elements raw HTML must never be able to produce in the final DOM.
const FORBIDDEN_ELEMENTS: &[&str] = &[
    "script", "base", "form", "link", "style", "iframe", "object", "embed",
];

/// URL-bearing attribute names whose scheme must stay safe: the ones the sanitizer
/// scheme-checks and that are live on a kept element. `action`/`formaction`/`poster`/`data`/
/// `background` are excluded: they are either stripped entirely or inert, because the
/// elements that would act on them (`<form>`, `<object>`, …) are never kept — github.com
/// keeps a dead `action="javascript:…"` on a `<section>` too. Attributes that legitimately
/// carry arbitrary code text (e.g. `data-snippet-clipboard-copy-content`) are not URLs and
/// are excluded as well.
const URL_ATTRS: &[&str] = &["href", "src", "srcset", "cite", "longdesc", "xlink:href"];

/// The scheme of `url`, with tabs/newlines/leading controls stripped as browsers do.
fn scheme(url: &str) -> Option<String> {
    let cleaned: String = url
        .trim_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control())
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let end = cleaned.find([':', '/', '?', '#'])?;
    (cleaned.as_bytes()[end] == b':').then(|| cleaned[..end].to_ascii_lowercase())
}

fn dangerous_url(value: &str) -> bool {
    scheme(value).is_some_and(|s| matches!(s.as_str(), "javascript" | "vbscript")) || {
        // `data:` is allowed only for non-HTML media; `data:text/html` is an XSS vector.
        let lower = value
            .trim_start()
            .chars()
            .filter(|c| !c.is_ascii_whitespace() && !c.is_ascii_control())
            .collect::<String>()
            .to_ascii_lowercase();
        lower.starts_with("data:text/html")
    }
}

fn check(input: &str, html: &str) {
    let doc = Document::fragment(html);
    for el in doc
        .html_root()
        .descendants()
        .into_iter()
        .filter(NodeRef::is_element)
    {
        let Some(q) = el.qual_name_ref() else {
            continue;
        };
        let name = q.local.to_string();
        assert!(
            !FORBIDDEN_ELEMENTS.contains(&name.as_str()),
            "<{name}> survived\n  input: {input:?}\n  html: {html}"
        );
        let attrs = el
            .element_ref()
            .map(|e| e.attrs.clone())
            .unwrap_or_default();
        for a in &attrs {
            let an = a.name.local.to_string();
            let qualified = if a.name.ns.is_empty() {
                an.clone()
            } else {
                format!("{}:{an}", a.name.prefix.as_deref().unwrap_or("ns"))
            };
            let val = a.value.to_string();
            // No event-handler attributes.
            assert!(
                !an.to_ascii_lowercase().starts_with("on"),
                "event handler {an:?} on <{name}>\n  input: {input:?}\n  html: {html}"
            );
            // No leaked sanitizer marker.
            assert_ne!(
                an, "data-mdr-g",
                "marker leaked on <{name}>\n  input: {input:?}\n  html: {html}"
            );
            // No raw `style` attribute (the renderer's own styles never carry the sentinel).
            if an == "style" {
                assert!(
                    !val.contains(STYLE_SENTINEL),
                    "raw style on <{name}>\n  input: {input:?}\n  html: {html}"
                );
            }
            // No dangerous scheme in a URL attribute (incl. xlink:href on foreign content).
            if URL_ATTRS.contains(&an.as_str())
                || URL_ATTRS.contains(&qualified.to_ascii_lowercase().as_str())
            {
                if an == "srcset" {
                    for cand in val
                        .split(',')
                        .filter_map(|c| c.split_ascii_whitespace().next())
                    {
                        assert!(
                            !dangerous_url(cand),
                            "dangerous srcset {cand:?} on <{name}>\n  input: {input:?}\n  html: {html}"
                        );
                    }
                } else {
                    assert!(
                        !dangerous_url(&val),
                        "dangerous {an}={val:?} on <{name}>\n  input: {input:?}\n  html: {html}"
                    );
                }
            }
        }
    }
}

fn opts(mode: Mode, sourcepos: bool) -> RenderOptions {
    let mut o = RenderOptions::new(Flavor::Github);
    o.mode = mode;
    o.sourcepos = sourcepos;
    o.bundle_url = "/__/md-render/bundle".into();
    o
}

#[test]
fn generated_documents_are_sanitized() {
    let cases: usize = std::env::var("MD_RENDER_PROP_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    let mut rng = Rng(0x1234_5678_9abc_def0);
    for _ in 0..cases {
        let input = generate(&mut rng);
        for mode in [Mode::Readme, Mode::Comment] {
            for sp in [false, true] {
                let html = github::render(&input, &opts(mode, sp)).html;
                check(&input, &html);
            }
        }
    }
}

/// The specific vectors from GH-SEC-1 / GH-SEC-2, pinned so they always run.
#[test]
fn known_vectors_are_sanitized() {
    let vectors = [
        "<details open ontoggle=\"x\"\n\npara\n",
        "<div onclick=\"x\"\n\npara\n",
        "<base href=\"https://evil.example/\"\n\npara\n",
        "<form action=\"https://evil.example/\"\n\npara\n",
        "<link rel=\"stylesheet\" href=\"https://evil.example/s.css\"\n\npara\n",
        "<table>\n\nx <b onmouseover=\"x\" style=\"position:fixed\">y\n\n```\ncode\n```\n",
        "<table>\n\nx <b tabindex=\"1\" autofocus onfocus=\"x\" style=\"animation:a 1s\" onanimationstart=\"x\">y\n\n```\ncode\n```\n",
        "<table>\n\n<a href=\"javascript:x\" onclick=\"x\">z\n\n```js\ncode\n```\n",
    ];
    for v in vectors {
        for mode in [Mode::Readme, Mode::Comment] {
            let html = github::render(v, &opts(mode, false)).html;
            check(v, &html);
        }
    }
}
