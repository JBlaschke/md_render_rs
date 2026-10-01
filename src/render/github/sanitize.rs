//! The raw-HTML sanitizer (GH §8.11), run once on the parsed formatter output (CQ §3.1 step 4).
//!
//! Elements written by our formatter carry the per-render marker attribute (see
//! [`super::MARK_ATTR`]): they keep all attributes (`TOKEN`), or are trusted with their whole
//! subtree (`TOKEN-t`: octicons, highlighted code, math). Everything else is raw HTML and goes
//! through GitHub's allowlist: unknown elements are unwrapped (content kept), attributes are
//! filtered per element, `id`/`name` get the `user-content-` prefix, URL attributes are
//! restricted to safe schemes, and comments are dropped. Unlike a plain "allowlist ∪ generated
//! attributes" scheme this keeps raw lookalikes (`<div class="markdown-alert">`, `<input>`)
//! from surviving, as on github.com.

use std::collections::HashSet;

use dom_query::{Document, NodeData, NodeId, NodeRef};

use super::{Ctx, MARK_ATTR, SOURCEPOS_COMMENT, dom};

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Elements kept from raw HTML (GH §8.11, `S/probes/tags.gfm.html`).
const KEPT: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "picture",
    "pre",
    "q",
    "rp",
    "rt",
    "ruby",
    "s",
    "samp",
    "section",
    "source",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "tt",
    "ul",
    "var",
    "video",
];

/// Attributes allowed on every kept element (GH §8.11, `S/probes/attrs.gfm.html`).
const GENERIC: &[&str] = &[
    "abbr",
    "accept",
    "accept-charset",
    "accesskey",
    "action",
    "align",
    "alt",
    "axis",
    "cellpadding",
    "cellspacing",
    "char",
    "charoff",
    "charset",
    "checked",
    "clear",
    "color",
    "cols",
    "colspan",
    "compact",
    "coords",
    "datetime",
    "dir",
    "disabled",
    "enctype",
    "for",
    "frame",
    "headers",
    "height",
    "hreflang",
    "hspace",
    "id",
    "ismap",
    "itemprop",
    "label",
    "lang",
    "maxlength",
    "media",
    "method",
    "multiple",
    "name",
    "nohref",
    "noshade",
    "nowrap",
    "open",
    "prompt",
    "readonly",
    "rev",
    "rows",
    "rowspan",
    "rules",
    "scope",
    "selected",
    "shape",
    "size",
    "span",
    "start",
    "summary",
    "tabindex",
    "title",
    "type",
    "usemap",
    "valign",
    "value",
    "width",
];

const ARIA: &[&str] = &["aria-describedby", "aria-label", "aria-labelledby"];

/// Element-specific extras (GH §8.11 "Element-specific").
fn extra_allowed(element: &str, attr: &str) -> bool {
    match element {
        "a" => attr == "href" || ARIA.contains(&attr),
        "img" => matches!(attr, "src" | "longdesc") || ARIA.contains(&attr),
        "div" => matches!(attr, "itemscope" | "itemtype"),
        "table" | "ol" => ARIA.contains(&attr),
        "video" => matches!(attr, "src" | "controls" | "muted") || ARIA.contains(&attr),
        "source" => attr == "srcset",
        "blockquote" | "q" | "del" | "ins" => attr == "cite",
        _ => false,
    }
}

/// Schemes allowed in `a[href]`; `src`-like attributes only allow http(s) (GH §8.9, §8.11).
const HREF_SCHEMES: &[&str] = &["http", "https", "mailto", "xmpp"];
const SRC_SCHEMES: &[&str] = &["http", "https"];

/// The URL scheme of `url` (lowercase), `None` for relative URLs. Tabs and newlines inside
/// the scheme are ignored, as browsers do.
pub(super) fn url_scheme(url: &str) -> Option<String> {
    let cleaned: String = url
        .trim_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control())
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let end = cleaned.find([':', '/', '?', '#'])?;
    if cleaned.as_bytes()[end] != b':' {
        return None;
    }
    Some(cleaned[..end].to_ascii_lowercase())
}

/// Whether `url` is relative or uses one of `schemes`.
pub(super) fn url_allowed(url: &str, schemes: &[&str]) -> bool {
    url_scheme(url).is_none_or(|s| schemes.contains(&s.as_str()))
}

fn prefixed(value: &str) -> String {
    let lower = value.to_lowercase();
    if lower.starts_with("user-content-") {
        lower
    } else {
        format!("user-content-{lower}")
    }
}

/// Runs the sanitizer over the whole fragment. Returns the raw HTML elements that were kept
/// (text inside them, or directly under the root, came from raw HTML).
pub(super) fn run(doc: &Document, ctx: &Ctx) -> HashSet<NodeId> {
    let mut raw = HashSet::new();
    let tree_token = format!("{}-t", ctx.token);
    let marker_prefix = format!("{SOURCEPOS_COMMENT}{}:", ctx.token);
    let root = dom::root(doc);
    let mut stack: Vec<NodeRef<'_>> = root.children();
    stack.reverse();
    while let Some(n) = stack.pop() {
        enum Kind {
            Element,
            Text,
            KeepComment,
            Drop,
        }
        let kind = n.query_or(Kind::Drop, |t| match &t.data {
            NodeData::Element(_) => Kind::Element,
            NodeData::Text { .. } => Kind::Text,
            NodeData::Comment { contents } if contents.starts_with(marker_prefix.as_str()) => {
                Kind::KeepComment
            }
            _ => Kind::Drop,
        });
        match kind {
            Kind::Text | Kind::KeepComment => {}
            Kind::Drop => n.remove_from_parent(),
            Kind::Element => match leading_marker(&n) {
                Some(v) if v == ctx.token => {
                    n.remove_attr(MARK_ATTR);
                    push_children(&n, &mut stack);
                }
                Some(v) if v == tree_token => {
                    n.remove_attr(MARK_ATTR);
                    trusted_subtree(&n, ctx, &tree_token);
                }
                _ => {
                    if raw_element(&n, &mut stack) {
                        raw.insert(n.id);
                    }
                }
            },
        }
    }
    raw
}

fn push_children<'a>(n: &NodeRef<'a>, stack: &mut Vec<NodeRef<'a>>) {
    let mut children = n.children();
    children.reverse();
    stack.extend(children);
}

/// The marker's value when it is the element's **first** attribute, `None` otherwise.
///
/// The formatter writes the marker straight after the tag name. Raw markup cannot reach
/// that position: an unterminated raw start tag (`<details open ontoggle=…` followed by a
/// blank line) absorbs the next generated start tag as attributes, but the generated tag
/// name (`<p`) then always comes first, so a marker anywhere else means the element is raw.
/// html5ever keeps attributes in source order.
fn leading_marker(n: &NodeRef<'_>) -> Option<String> {
    n.query(|t| {
        let el = t.as_element()?;
        el.attrs
            .first()
            .filter(|a| a.name.ns.is_empty() && &*a.name.local == MARK_ATTR)
            .map(|a| a.value.to_string())
    })
    .flatten()
}

/// HTML formatting elements (HTML §13.2.4.3), the ones the tree builder clones on its own.
const FORMATTING: &[&str] = &[
    "a", "b", "big", "code", "em", "font", "i", "nobr", "s", "small", "strike", "strong", "tt", "u",
];

/// Clears the markers inside a trusted subtree (`TOKEN-t`: octicons, highlighted code, math)
/// and sanitizes what the HTML parser put there on its own. An unclosed raw formatting
/// element (`<b onclick=…>`, typically while a raw `<table>` is open) stays in the list of
/// active formatting elements, and the "reconstruct the active formatting elements" step
/// (or the adoption agency algorithm) clones it, attributes included, into later content,
/// a generated `<pre>` among others. Such a clone has no leading marker (the formatter
/// marks its own `<code>`, the only formatting element it writes in trusted subtrees) and
/// is filtered like the raw element it copies: github.com keeps a bare `<b>` inside the
/// `<pre>`. Its children stay trusted.
fn trusted_subtree(n: &NodeRef<'_>, ctx: &Ctx, tree_token: &str) {
    let mut stack: Vec<NodeRef<'_>> = n.children();
    stack.reverse();
    while let Some(c) = stack.pop() {
        if !c.is_element() {
            continue;
        }
        let generated = leading_marker(&c).is_some_and(|v| v == ctx.token || v == tree_token);
        let clone = !generated
            && c.qual_name_ref()
                .is_some_and(|q| &*q.ns == HTML_NS && FORMATTING.contains(&&*q.local));
        if clone {
            let local = dom::name(&c).map(|l| l.to_string()).unwrap_or_default();
            if KEPT.contains(&local.as_str()) {
                filter_attrs(&c, &local);
                push_children(&c, &mut stack);
            } else {
                unwrap_raw(&c, &mut stack);
            }
            continue;
        }
        if c.has_attr(MARK_ATTR) {
            c.remove_attr(MARK_ATTR);
        }
        push_children(&c, &mut stack);
    }
}

/// Sanitizes one raw element; returns whether it was kept.
fn raw_element<'a>(n: &NodeRef<'a>, stack: &mut Vec<NodeRef<'a>>) -> bool {
    let (local, html_ns) = match n.qual_name_ref() {
        Some(q) => (q.local.to_string(), &*q.ns == HTML_NS),
        None => return false,
    };
    if !html_ns || !KEPT.contains(&local.as_str()) {
        unwrap_raw(n, stack);
        return false;
    }
    filter_attrs(n, &local);
    push_children(n, stack);
    true
}

/// Applies the raw-HTML attribute allowlist of element `local` to `n`.
fn filter_attrs(n: &NodeRef<'_>, local: &str) {
    n.update(|t| {
        let Some(el) = t.as_element_mut() else { return };
        el.attrs.retain_mut(|a| {
            let name: &str = &a.name.local;
            if !a.name.ns.is_empty() {
                return false;
            }
            if !GENERIC.contains(&name) && !extra_allowed(local, name) {
                return false;
            }
            match name {
                "id" | "name" | "aria-describedby" | "aria-labelledby" => {
                    if a.value.is_empty() {
                        return true;
                    }
                    let v = prefixed(&a.value);
                    a.value = v.into();
                }
                "href" => return url_allowed(&a.value, HREF_SCHEMES),
                "src" | "longdesc" | "cite" => return url_allowed(&a.value, SRC_SCHEMES),
                "srcset" => {
                    return a
                        .value
                        .split(',')
                        .filter_map(|c| c.split_ascii_whitespace().next())
                        .all(|u| url_allowed(u, SRC_SCHEMES));
                }
                _ => {}
            }
            true
        });
    });
}

/// Unwraps a disallowed raw element (content kept). Text left directly inside table
/// structure (e.g. from `<caption>`) is foster-parented by the browser when the output is
/// parsed, exactly as github.com's own re-serialized HTML is (`S/probes/attrs` CASE-caption).
fn unwrap_raw<'a>(n: &NodeRef<'a>, stack: &mut Vec<NodeRef<'a>>) {
    let template = n.element_ref().and_then(|e| e.template_contents);
    let mut children = Vec::new();
    if let Some(t) = template {
        children.extend(NodeRef::new(t, n.tree).children());
    }
    children.extend(n.children());
    for c in &children {
        n.insert_before(c);
    }
    n.remove_from_parent();
    children.reverse();
    stack.extend(children);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(html: &str) -> String {
        let ctx = Ctx::for_tests(false);
        let doc = Document::fragment(html);
        run(&doc, &ctx);
        dom::root(&doc).inner_html().to_string()
    }

    #[test]
    fn unknown_elements_are_unwrapped() {
        assert_eq!(
            clean("<p>T3 <abbr>inner</abbr> end</p>"),
            "<p>T3 inner end</p>"
        );
        assert_eq!(clean("<form><button>Go</button></form>"), "Go");
        assert_eq!(
            clean("<p><svg><path d=\"x\"></path>inner</svg> end</p>"),
            "<p>inner end</p>"
        );
        assert_eq!(clean("<template>t<b>u</b></template>"), "t<b>u</b>");
        assert_eq!(clean("<input type=\"checkbox\" checked> plain"), " plain");
        assert_eq!(clean("<p>a<!-- c -->b</p>"), "<p>ab</p>");
    }

    #[test]
    fn attributes_follow_the_allowlist() {
        assert_eq!(
            clean(
                r#"<div id="Main" name="n1" border="1" role="note" class="x" data-x="1" itemscope itemtype="http://schema.org/Thing" hidden>d</div>"#
            ),
            r#"<div id="user-content-main" name="user-content-n1" itemscope="" itemtype="http://schema.org/Thing">d</div>"#
        );
        assert_eq!(
            clean(
                r#"<a href="https://e.com" target="_blank" rel="x" aria-label="l" aria-hidden="true" onclick="y">a</a>"#
            ),
            r#"<a href="https://e.com" aria-label="l">a</a>"#
        );
        assert_eq!(
            clean(r#"<span aria-label="l" style="c">x</span>"#),
            "<span>x</span>"
        );
        assert_eq!(
            clean(r#"<blockquote cite="https://c">q</blockquote>"#),
            r#"<blockquote cite="https://c">q</blockquote>"#
        );
        assert_eq!(
            clean(r#"<h3 id="user-content-x">h</h3>"#),
            r#"<h3 id="user-content-x">h</h3>"#
        );
    }

    #[test]
    fn dangerous_urls_are_dropped() {
        assert_eq!(
            clean(r#"<a href="javascript:alert(1)">js</a>"#),
            "<a>js</a>"
        );
        assert_eq!(
            clean(r#"<a href="java&#9;script:alert(1)">js</a>"#),
            "<a>js</a>"
        );
        assert_eq!(clean(r#"<a href=" JAVASCRIPT:x">js</a>"#), "<a>js</a>");
        assert_eq!(
            clean(r#"<a href="mailto:a@b.c">m</a>"#),
            r#"<a href="mailto:a@b.c">m</a>"#
        );
        assert_eq!(clean(r#"<img src="data:image/png;base64,x">"#), "<img>");
        assert_eq!(
            clean(r#"<img src="/abs.png" onerror="x">"#),
            r#"<img src="/abs.png">"#
        );
        assert_eq!(
            clean(r#"<video src="javascript:x" controls autoplay></video>"#),
            r#"<video controls=""></video>"#
        );
    }

    #[test]
    fn generated_markup_is_trusted() {
        let ctx = Ctx::for_tests(false);
        let html = format!(
            r#"<div{} class="markdown-alert"><svg{} class="octicon"><path d="m"></path></svg><span class="raw">x</span></div>"#,
            ctx.mark, ctx.mark_tree
        );
        let doc = Document::fragment(html);
        run(&doc, &ctx);
        assert_eq!(
            dom::root(&doc).inner_html().to_string(),
            r#"<div class="markdown-alert"><svg class="octicon"><path d="m"></path></svg><span>x</span></div>"#
        );
    }

    /// GH-SEC-1: an unterminated raw start tag absorbs the next generated start tag (and its
    /// marker) as attributes; a marker that is not the first attribute is not trusted.
    #[test]
    fn absorbed_marker_is_not_trusted() {
        let ctx = Ctx::for_tests(false);
        let doc = Document::fragment(format!(
            "<details open ontoggle=\"x\"\n<p{} dir=\"auto\">para</p>",
            ctx.mark
        ));
        run(&doc, &ctx);
        assert_eq!(
            dom::root(&doc).inner_html().to_string(),
            r#"<details open="" dir="auto">para<p></p></details>"#
        );
        for (tag, out) in [
            (
                "div onclick=\"x\" style=\"s\"",
                r#"<div dir="auto">para<p></p></div>"#,
            ),
            ("base href=\"https://e.com/\"", "para<p></p>"),
            ("form action=\"https://e.com/\"", "para<p></p>"),
            (
                "link rel=\"stylesheet\" href=\"https://e.com/s.css\"",
                "para<p></p>",
            ),
            ("input type=\"text\"", "para<p></p>"),
        ] {
            let doc = Document::fragment(format!("<{tag}\n<p{} dir=\"auto\">para</p>", ctx.mark));
            run(&doc, &ctx);
            assert_eq!(dom::root(&doc).inner_html().to_string(), out, "{tag}");
        }
        // The same holds for an absorbed trusted-subtree marker.
        let doc = Document::fragment(format!(
            "<div onclick=\"x\"\n<div{} class=\"highlight\"><pre>c</pre></div>",
            ctx.mark_tree
        ));
        run(&doc, &ctx);
        assert_eq!(
            dom::root(&doc).inner_html().to_string(),
            "<div><pre>c</pre></div>"
        );
    }

    /// GH-SEC-2: the parser clones an unclosed raw formatting element into a generated
    /// trusted subtree (code block); the clone is filtered like raw HTML.
    #[test]
    fn cloned_formatting_elements_in_trusted_subtrees_are_filtered() {
        let ctx = Ctx::for_tests(true);
        for (tag, kept) in [
            ("a", true),
            ("b", true),
            ("big", false),
            ("code", true),
            ("em", true),
            ("font", false),
            ("i", true),
            ("nobr", false),
            ("s", true),
            ("small", false),
            ("strike", true),
            ("strong", true),
            ("tt", true),
            ("u", false),
        ] {
            let html = format!(
                "<table>\n<p{m}>x <{tag} onclick=\"x\" style=\"s\" href=\"javascript:x\">y</p>\n<div{mt} class=\"c\"><pre class=\"notranslate\"><code{m} class=\"notranslate\">code\n</code></pre></div>\n",
                m = ctx.mark,
                mt = ctx.mark_tree
            );
            let doc = Document::fragment(html);
            run(&doc, &ctx);
            let out = dom::root(&doc).inner_html().to_string();
            let pre = if kept {
                format!(
                    "<pre class=\"notranslate\"><{tag}><code class=\"notranslate\">code\n</code></{tag}></pre>"
                )
            } else {
                "<pre class=\"notranslate\"><code class=\"notranslate\">code\n</code></pre>"
                    .to_owned()
            };
            assert!(out.contains(&pre), "{tag}: {out}");
            for bad in ["onclick", "style", "javascript", MARK_ATTR] {
                assert!(!out.contains(bad), "{tag}: {out}");
            }
        }
        // Generated elements of a trusted subtree keep their attributes.
        let doc = Document::fragment(format!(
            "<div{} class=\"highlight\"><pre><span class=\"pl-k\">fn</span></pre></div>",
            ctx.mark_tree
        ));
        run(&doc, &ctx);
        assert_eq!(
            dom::root(&doc).inner_html().to_string(),
            r#"<div class="highlight"><pre><span class="pl-k">fn</span></pre></div>"#
        );
    }

    #[test]
    fn caption_is_unwrapped() {
        assert_eq!(
            clean("<table><caption>cap</caption><tbody><tr><td>x</td></tr></tbody></table>"),
            "<table>cap<tbody><tr><td>x</td></tr></tbody></table>"
        );
    }

    #[test]
    fn schemes() {
        assert_eq!(url_scheme("https://x"), Some("https".to_owned()));
        assert_eq!(url_scheme("rel/x:y"), None);
        assert_eq!(url_scheme("#frag"), None);
        assert_eq!(url_scheme("//x.com/p"), None);
        assert_eq!(url_scheme("x?a:b"), None);
        assert!(url_allowed("/abs", SRC_SCHEMES));
        assert!(!url_allowed("ftp://x", HREF_SCHEMES));
    }
}
