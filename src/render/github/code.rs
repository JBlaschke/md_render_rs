//! Code blocks: highlight / clipboard wrappers (GH §7.1, §5.3), ```` ```math ```` (GH §8.6)
//! and enrichment sections for mermaid/geojson/topojson/stl (GH §8.7).

use std::collections::HashSet;

use dom_query::{Document, NodeId};

use super::Ctx;
use super::dom;
use super::math::math_html;
use super::octicons;
use super::util::uuid_like;
use crate::hash::mdr_key;
use crate::html::{push_attr, push_text};
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
    /// The `highlight highlight-<scope>` form was used (known Linguist scope).
    pub highlighted: bool,
}

/// GitHub markup for a code block. `literal` is the block content (normally ending in a
/// newline), `lang` the first word of the info string as written (may be empty), `sp` extra
/// attributes for the outermost element (`data-sourcepos`), `index` the number of enrichment
/// sections emitted before this one.
pub(super) fn block_html(ctx: &Ctx, lang: &str, literal: &str, sp: &str, index: usize) -> Block {
    // RAW / TOKENS drop exactly one final newline (GH §7.1).
    let raw = literal.strip_suffix('\n').unwrap_or(literal);
    if lang == "math" && raw.is_empty() {
        // github.com keeps an empty ```` ```math ```` fence as a `<pre>` holding just the
        // delimiters, no `<math-renderer>` (GH-MATH-1, `cases2/math_blocks2.gh.html`).
        let mut html = String::from("<pre");
        html.push_str(&ctx.mark);
        html.push_str(r#" lang="math" class="notranslate""#);
        html.push_str(sp);
        html.push_str(">$$$$</pre>");
        return Block {
            html,
            ..Block::default()
        };
    }
    if lang == "math" {
        // github.com doubles a backslash that ends a line inside ```` ```math ```` (`\\` at
        // the end of a matrix row becomes `\\\`; `S/ctx/aaddrick_*`, `S/ctx/joseph_*`).
        let tex = raw.replace("\\\n", "\\\\\n");
        let mut html = math_html(ctx, &tex, "$$", true);
        // The sanitizer only trusts a marker that is the first attribute.
        insert_attrs(&mut html, &format!("<math-renderer{}", ctx.mark_tree), sp);
        return Block {
            html,
            math: true,
            ..Block::default()
        };
    }
    if ENRICHMENTS.contains(&lang) {
        let html = enrichment_html(ctx, lang, literal, sp, index);
        return Block {
            html,
            mermaid: lang == "mermaid",
            enrichment: true,
            ..Block::default()
        };
    }
    // The highlighter takes the block content as written (it drops the final newline, and
    // for GitHub's tree-sitter languages every trailing blank line, itself).
    let hl = if lang.is_empty() {
        None
    } else {
        Some(highlight::github(literal, lang))
    };
    let key = mdr_key(&["code", lang, raw]);
    let mut html = String::with_capacity(literal.len() * 2 + 200);
    let mut highlighted = false;
    match hl {
        Some(highlight::GithubCode {
            scope: Some(scope),
            html: tokens,
        }) => {
            highlighted = true;
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
            html.push_str(if ctx.comment {
                r#"><pre class="notranslate">"#
            } else {
                "><pre>"
            });
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
            // The `<code>` carries its own marker so the sanitizer can tell it from a raw
            // formatting element the HTML parser cloned into this trusted subtree.
            html.push_str(r#" class="notranslate"><code"#);
            html.push_str(&ctx.mark);
            html.push_str(if ctx.comment {
                r#" class="notranslate">"#
            } else {
                ">"
            });
            push_text(&mut html, literal);
            html.push_str("</code></pre></div>");
        }
    }
    Block {
        html,
        highlighted,
        ..Block::default()
    }
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
    // github.com leaves an empty line after each section (`</section>\n\n`).
    s.push_str("\n</section>\n");
    s
}

/// Raw HTML `<pre>` elements (after sanitizing): `<pre lang>` is highlighted / turned into
/// math or enrichment like a fence, `<pre><code>` gets the plain clipboard wrapper; a bare
/// `<pre>` stays as is (GH §7.1; `S/ctx/rhysd_*`, `S/pages/lifeparticle_*`). `raw` holds
/// the raw elements the sanitizer kept; generated `<pre>`s (code blocks, enrichments, an
/// empty ```` ```math ```` fence) are left alone.
pub(super) fn raw_pre(
    doc: &Document,
    ctx: &Ctx,
    raw: &HashSet<NodeId>,
    enrichments: &mut usize,
    feats: &mut (bool, bool),
) {
    let plain_ctx = ctx.without_marks();
    for pre in dom::elements_named(doc, "pre") {
        let generated = !raw.contains(&pre.id)
            || pre.parent().is_some_and(|p| {
                p.has_class("highlight")
                    || p.has_class("snippet-clipboard-content")
                    || p.has_class("render-plaintext-hidden")
            });
        if generated {
            continue;
        }
        if let Some(lang) = pre.attr("lang").map(|l| l.to_string()) {
            let lang = lang
                .split_ascii_whitespace()
                .next()
                .unwrap_or("")
                .to_owned();
            let code = pre.text().to_string();
            let is_special = lang == "math" || ENRICHMENTS.contains(&lang.as_str());
            let block = (!lang.is_empty())
                .then(|| block_html(&plain_ctx, &lang, &code, "", *enrichments))
                .filter(|b| is_special || b.highlighted);
            if let Some(block) = block {
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
            div.set_attr(
                "class",
                "snippet-clipboard-content notranslate position-relative overflow-auto",
            );
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
        // An unknown language has no Linguist scope: plain form with `lang` kept (GH §7.1).
        let b = block_html(&ctx(false), "nosuchlang", "fn main() {}\n", "", 0);
        assert!(b.html.starts_with(r#"<div class="snippet-clipboard-content notranslate position-relative overflow-auto" data-snippet-clipboard-copy-content="fn main() {}" data-mdr-key=""#));
        assert!(b.html.ends_with(
            r#"><pre lang="nosuchlang" class="notranslate"><code>fn main() {}
</code></pre></div>"#
        ));
        let b = block_html(&ctx(true), "", "a < b\n", "", 0);
        assert!(b.html.contains(
            r#"<pre class="notranslate"><code class="notranslate">a &lt; b
</code></pre>"#
        ));
    }

    #[test]
    fn math_and_enrichment() {
        let b = block_html(
            &ctx(false),
            "math",
            "x^2\n",
            r#" data-sourcepos="1:1-3:3""#,
            0,
        );
        assert!(b.math);
        assert!(b.html.starts_with(r#"<math-renderer data-sourcepos="1:1-3:3" class="js-display-math" style="display: block" data-run-id=""#));
        assert!(b.html.ends_with(">$$x^2$$</math-renderer>"));

        let b = block_html(&ctx(false), "mermaid", "graph TD\n  A --> B\n", "", 0);
        assert!(b.mermaid && b.enrichment);
        assert!(
            b.html
                .contains(r#"data-json="{&quot;data&quot;:&quot;graph TD\n  A --&gt; B\n&quot;}""#)
        );
        assert!(
            b.html
                .contains("data-plain=\"graph TD\n  A --&gt; B\n\" dir=\"auto\"")
        );
        assert!(b.html.contains(
            "<pre lang=\"mermaid\" aria-label=\"Raw mermaid code\">graph TD\n  A --&gt; B\n</pre>"
        ));
        let other = block_html(&ctx(false), "mermaid", "graph TD\n  A --> B\n", "", 1);
        assert_ne!(b.html, other.html, "distinct data-identity per occurrence");
    }

    /// GH-MATH-1: an empty ```` ```math ```` fence stays a `<pre>` holding the delimiters
    /// (`cases2/math_blocks2.gh.html`), and needs no math library.
    #[test]
    fn empty_math_fence() {
        for comment in [false, true] {
            for literal in ["", "\n"] {
                let b = block_html(&ctx(comment), "math", literal, "", 0);
                assert!(!b.math);
                assert_eq!(b.html, r#"<pre lang="math" class="notranslate">$$$$</pre>"#);
            }
        }
        let b = block_html(&ctx(false), "math", "", r#" data-sourcepos="1:1-2:3""#, 0);
        assert_eq!(
            b.html,
            r#"<pre lang="math" class="notranslate" data-sourcepos="1:1-2:3">$$$$</pre>"#
        );
    }
}
