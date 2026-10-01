//! Code blocks: highlight / clipboard wrappers (GH §7.1, §5.3), ```` ```math ```` (GH §8.6)
//! and enrichment sections for mermaid/geojson/topojson/stl (GH §8.7).

use dom_query::Document;

use super::Ctx;
use super::ast::math_html;
use super::dom;
use super::octicons;
use super::util::{mdr_key, push_attr, push_text, uuid_like};
use crate::render::highlight;

/// Info-string languages rendered as enrichment sections (GH §8.7).
const ENRICHMENTS: [&str; 4] = ["mermaid", "geojson", "topojson", "stl"];

/// Markup for one code block plus the features it needs.
#[derive(Debug, Default)]
pub(super) struct Block {
    pub html: String,
    pub math: bool,
    pub mermaid: bool,
    pub enrichment: bool,
}

/// GitHub markup for a code block. `literal` is the block content (normally ending in a
/// newline), `lang` the first word of the info string as written (may be empty), `sp` extra
/// attributes for the outermost element (`data-sourcepos`), `index` the number of enrichment
/// sections emitted before this one.
pub(super) fn block_html(ctx: &Ctx, lang: &str, literal: &str, sp: &str, index: usize) -> Block {
    // RAW / TOKENS drop exactly one final newline (GH §7.1).
    let raw = literal.strip_suffix('\n').unwrap_or(literal);
    if lang == "math" {
        let mut html = math_html(ctx, raw, "$$", true);
        insert_attrs(&mut html, "<math-renderer", sp);
        return Block { html, math: true, ..Block::default() };
    }
    if ENRICHMENTS.contains(&lang) {
        let html = enrichment_html(ctx, lang, literal, sp, index);
        return Block { html, mermaid: lang == "mermaid", enrichment: true, ..Block::default() };
    }
    let hl = if lang.is_empty() { None } else { Some(highlight::github(raw, lang)) };
    let key = mdr_key(&["code", lang, raw]);
    let mut html = String::with_capacity(literal.len() * 2 + 200);
    match hl {
        Some(highlight::GithubCode { scope: Some(scope), html: tokens }) => {
            html.push_str("<div");
            html.push_str(&ctx.mark_tree);
            html.push_str(r#" class="highlight highlight-"#);
            push_attr(&mut html, &scope.replace('.', "-"));
            html.push_str(r#" notranslate position-relative overflow-auto" data-snippet-clipboard-copy-content=""#);
            push_attr(&mut html, raw);
            html.push_str(r#"" data-mdr-key=""#);
            html.push_str(&key);
            html.push('"');
            html.push_str(sp);
            html.push_str(if ctx.comment { r#"><pre class="notranslate">"# } else { "><pre>" });
            match tokens {
                Some(t) => html.push_str(&t),
                None => push_text(&mut html, raw),
            }
            html.push_str("</pre></div>");
        }
        _ => {
            html.push_str("<div");
            html.push_str(&ctx.mark_tree);
            html.push_str(r#" class="snippet-clipboard-content notranslate position-relative overflow-auto" data-snippet-clipboard-copy-content=""#);
            push_attr(&mut html, raw);
            html.push_str(r#"" data-mdr-key=""#);
            html.push_str(&key);
            html.push('"');
            html.push_str(sp);
            html.push_str("><pre");
            if !lang.is_empty() {
                html.push_str(r#" lang=""#);
                push_attr(&mut html, lang);
                html.push('"');
            }
            html.push_str(if ctx.comment {
                r#" class="notranslate"><code class="notranslate">"#
            } else {
                r#" class="notranslate"><code>"#
            });
            push_text(&mut html, literal);
            html.push_str("</code></pre></div>");
        }
    }
    Block { html, ..Block::default() }
}

/// Inserts `attrs` (` name="value"…`) right after `tag_start` (`<name`) at the start of `html`.
fn insert_attrs(html: &mut String, tag_start: &str, attrs: &str) {
    if !attrs.is_empty() && html.starts_with(tag_start) {
        html.insert_str(tag_start.len(), attrs);
    }
}

/// GitHub's README-context enrichment section, verbatim apart from `data-identity`
/// (deterministic here) and `data-mdr-key` (GH §8.7).
fn enrichment_html(ctx: &Ctx, kind: &str, code: &str, sp: &str, index: usize) -> String {
    let key = mdr_key(&["enrichment", kind, code]);
    let identity = uuid_like(&[&ctx.run_id, &key, &index.to_string()]);
    let json = serde_json::to_string(&serde_json::json!({ "data": code })).unwrap_or_default();
    let mut s = String::with_capacity(code.len() * 3 + 1500);
    s.push_str("<section");
    s.push_str(&ctx.mark_tree);
    s.push_str(r#" class="js-render-needs-enrichment render-needs-enrichment position-relative" data-identity=""#);
    s.push_str(&identity);
    s.push_str(r#"" data-host="https://viewscreen.githubusercontent.com" data-src="https://viewscreen.githubusercontent.com/markdown/"#);
    s.push_str(kind);
    s.push_str(r#"?docs_host=https%3A%2F%2Fdocs.github.com" data-type=""#);
    s.push_str(kind);
    s.push_str(r#"" aria-label=""#);
    s.push_str(kind);
    s.push_str(r#" rendered output container" data-mdr-key=""#);
    s.push_str(&key);
    s.push('"');
    s.push_str(sp);
    s.push_str(">\n  <div class=\"js-render-enrichment-target\" data-json=\"");
    push_attr(&mut s, &json);
    s.push_str("\" data-plain=\"");
    push_attr(&mut s, code);
    s.push_str("\" dir=\"auto\">\n    <div class=\"render-plaintext-hidden\" dir=\"auto\">\n      <pre lang=\"");
    s.push_str(kind);
    s.push_str("\" aria-label=\"Raw ");
    s.push_str(kind);
    s.push_str(" code\"");
    if ctx.comment {
        s.push_str(" class=\"notranslate\"");
    }
    s.push('>');
    push_text(&mut s, code);
    s.push_str("</pre>\n    </div>\n  </div>\n  ");
    s.push_str(octicons::SPINNER);
    s.push_str("\n</section>");
    s
}

/// Raw HTML `<pre>` elements (after sanitizing): `<pre lang>` is highlighted / turned into
/// math or enrichment like a fence, `<pre><code>` gets the plain clipboard wrapper; a bare
/// `<pre>` stays as is (GH §7.1; `S/ctx/rhysd_*`, `S/pages/lifeparticle_*`).
pub(super) fn raw_pre(doc: &Document, ctx: &Ctx, enrichments: &mut usize, feats: &mut (bool, bool)) {
    let plain_ctx = ctx.without_marks();
    for pre in dom::elements_named(doc, "pre") {
        let generated = pre.parent().is_some_and(|p| {
            p.has_class("highlight")
                || p.has_class("snippet-clipboard-content")
                || p.has_class("render-plaintext-hidden")
        });
        if generated {
            continue;
        }
        if let Some(lang) = pre.attr("lang").map(|l| l.to_string()) {
            let lang = lang.split_ascii_whitespace().next().unwrap_or("").to_owned();
            let code = pre.text().to_string();
            let is_special = lang == "math" || ENRICHMENTS.contains(&lang.as_str());
            let highlighted = !lang.is_empty()
                && !is_special
                && highlight::github(code.strip_suffix('\n').unwrap_or(&code), &lang)
                    .scope
                    .is_some();
            if is_special || highlighted {
                let block = block_html(&plain_ctx, &lang, &code, "", *enrichments);
                if block.enrichment {
                    *enrichments += 1;
                }
                feats.0 |= block.math;
                feats.1 |= block.mermaid;
                pre.replace_with_html(block.html);
                continue;
            }
        }
        let children = pre.element_children();
        let only_code = children.len() == 1 && children[0].has_name("code");
        if only_code {
            let code = pre.text().to_string();
            let raw = code.strip_suffix('\n').unwrap_or(&code);
            let lang = pre.attr("lang").map(|l| l.to_string()).unwrap_or_default();
            let div = doc.tree.new_element("div");
            div.set_attr("class", "snippet-clipboard-content notranslate position-relative overflow-auto");
            div.set_attr("data-snippet-clipboard-copy-content", raw);
            div.set_attr("data-mdr-key", &mdr_key(&["code", &lang, raw]));
            pre.add_class("notranslate");
            pre.insert_before(&div);
            div.append_child(&pre);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(comment: bool) -> Ctx {
        Ctx::for_tests(comment).without_marks()
    }

    #[test]
    fn plain_block_shapes() {
        // The highlight stub returns no scope: plain form with `lang` kept (GH §7.1).
        let b = block_html(&ctx(false), "rust", "fn main() {}\n", "", 0);
        assert!(b.html.starts_with(r#"<div class="snippet-clipboard-content notranslate position-relative overflow-auto" data-snippet-clipboard-copy-content="fn main() {}" data-mdr-key=""#));
        assert!(b.html.ends_with(r#"><pre lang="rust" class="notranslate"><code>fn main() {}
</code></pre></div>"#));
        let b = block_html(&ctx(true), "", "a < b\n", "", 0);
        assert!(b.html.contains(r#"<pre class="notranslate"><code class="notranslate">a &lt; b
</code></pre>"#));
    }

    #[test]
    fn math_and_enrichment() {
        let b = block_html(&ctx(false), "math", "x^2\n", r#" data-sourcepos="1:1-3:3""#, 0);
        assert!(b.math);
        assert!(b.html.starts_with(r#"<math-renderer data-sourcepos="1:1-3:3" class="js-display-math" style="display: block" data-run-id=""#));
        assert!(b.html.ends_with(">$$x^2$$</math-renderer>"));

        let b = block_html(&ctx(false), "mermaid", "graph TD\n  A --> B\n", "", 0);
        assert!(b.mermaid && b.enrichment);
        assert!(b.html.contains(r#"data-json="{&quot;data&quot;:&quot;graph TD\n  A --&gt; B\n&quot;}""#));
        assert!(b.html.contains("data-plain=\"graph TD\n  A --&gt; B\n\" dir=\"auto\""));
        assert!(b.html.contains("<pre lang=\"mermaid\" aria-label=\"Raw mermaid code\">graph TD\n  A --&gt; B\n</pre>"));
        let other = block_html(&ctx(false), "mermaid", "graph TD\n  A --> B\n", "", 1);
        assert_ne!(b.html, other.html, "distinct data-identity per occurrence");
    }
}
