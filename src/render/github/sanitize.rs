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

use dom_query::{Document, NodeData, NodeRef};

use super::{Ctx, MARK_ATTR, SOURCEPOS_COMMENT, dom};

const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Elements kept from raw HTML (GH §8.11, `S/probes/tags.gfm.html`).
const KEPT: &[&str] = &[
    "a", "b", "blockquote", "br", "code", "dd", "del", "details", "div", "dl", "dt", "em", "h1",
    "h2", "h3", "h4", "h5", "h6", "hr", "i", "img", "ins", "kbd", "li", "mark", "ol", "p",
    "picture", "pre", "q", "rp", "rt", "ruby", "s", "samp", "section", "source", "span",
    "strike", "strong", "sub", "summary", "sup", "table", "tbody", "td", "tfoot", "th", "thead",
    "tr", "tt", "ul", "var", "video",
];

/// Attributes allowed on every kept element (GH §8.11, `S/probes/attrs.gfm.html`).
const GENERIC: &[&str] = &[
    "abbr", "accept", "accept-charset", "accesskey", "action", "align", "alt", "axis",
    "cellpadding", "cellspacing", "char", "charoff", "charset", "checked", "clear", "color",
    "cols", "colspan", "compact", "coords", "datetime", "dir", "disabled", "enctype", "for",
    "frame", "headers", "height", "hreflang", "hspace", "id", "ismap", "itemprop", "label",
    "lang", "maxlength", "media", "method", "multiple", "name", "nohref", "noshade", "nowrap",
    "open", "prompt", "readonly", "rev", "rows", "rowspan", "rules", "scope", "selected",
    "shape", "size", "span", "start", "summary", "tabindex", "title", "type", "usemap",
    "valign", "value", "width",
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
    let cleaned: String =
        url.trim_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control())
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
    if lower.starts_with("user-content-") { lower } else { format!("user-content-{lower}") }
}

/// Runs the sanitizer over the whole fragment.
pub(super) fn run(doc: &Document, ctx: &Ctx) {
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
            Kind::Element => {
                let marker = n.attr(MARK_ATTR).map(|v| v.to_string());
                match marker {
                    Some(v) if v == ctx.token => {
                        n.remove_attr(MARK_ATTR);
                        push_children(&n, &mut stack);
                    }
                    Some(v) if v == tree_token => {
                        n.remove_attr(MARK_ATTR);
                        strip_markers(&n);
                    }
                    _ => raw_element(&n, &mut stack),
                }
            }
        }
    }
}

fn push_children<'a>(n: &NodeRef<'a>, stack: &mut Vec<NodeRef<'a>>) {
    let mut children = n.children();
    children.reverse();
    stack.extend(children);
}

/// Removes nested marker attributes inside a trusted subtree (math inside a table cell of a
/// trusted block cannot occur, but stay robust).
fn strip_markers(n: &NodeRef<'_>) {
    for d in n.descendants() {
        if d.is_element() && d.has_attr(MARK_ATTR) {
            d.remove_attr(MARK_ATTR);
        }
    }
}

fn raw_element<'a>(n: &NodeRef<'a>, stack: &mut Vec<NodeRef<'a>>) {
    let (local, html_ns) = match n.qual_name_ref() {
        Some(q) => (q.local.to_string(), &*q.ns == HTML_NS),
        None => return,
    };
    if !html_ns || !KEPT.contains(&local.as_str()) {
        unwrap_raw(n, stack);
        return;
    }
    n.update(|t| {
        let Some(el) = t.as_element_mut() else { return };
        el.attrs.retain_mut(|a| {
            let name: &str = &a.name.local;
            if !a.name.ns.is_empty() {
                return false;
            }
            if !GENERIC.contains(&name) && !extra_allowed(&local, name) {
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
    push_children(n, stack);
}

const TABLE_PARTS: &[&str] = &["table", "thead", "tbody", "tfoot", "tr"];

/// Unwraps a disallowed raw element. Inside table structure, non-table content is moved in
/// front of the table, mirroring the HTML parser's foster parenting (GitHub hoists
/// `<caption>` text this way, `S/probes/attrs.gfm.html` CASE-caption).
fn unwrap_raw<'a>(n: &NodeRef<'a>, stack: &mut Vec<NodeRef<'a>>) {
    let in_table = n
        .parent()
        .and_then(|p| dom::name(&p))
        .is_some_and(|p| TABLE_PARTS.contains(&&*p));
    let template = n.element_ref().and_then(|e| e.template_contents);
    let mut children = Vec::new();
    if let Some(t) = template {
        children.extend(NodeRef::new(t, n.tree).children());
    }
    children.extend(n.children());
    let table = if in_table {
        let mut cur = n.parent();
        while let Some(p) = cur {
            if p.has_name("table") {
                break;
            }
            cur = p.parent();
        }
        cur
    } else {
        None
    };
    for c in &children {
        let is_table_part = dom::name(c).is_some_and(|nm| {
            matches!(&*nm, "tr" | "td" | "th" | "thead" | "tbody" | "tfoot" | "table")
        });
        match &table {
            Some(t) if !is_table_part => t.insert_before(c),
            _ => n.insert_before(c),
        }
    }
    n.remove_from_parent();
    let mut rev = children;
    rev.reverse();
    stack.extend(rev);
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
        assert_eq!(clean("<p>T3 <abbr>inner</abbr> end</p>"), "<p>T3 inner end</p>");
        assert_eq!(clean("<form><button>Go</button></form>"), "Go");
        assert_eq!(clean("<p><svg><path d=\"x\"></path>inner</svg> end</p>"), "<p>inner end</p>");
        assert_eq!(clean("<template>t<b>u</b></template>"), "t<b>u</b>");
        assert_eq!(clean("<input type=\"checkbox\" checked> plain"), " plain");
        assert_eq!(clean("<p>a<!-- c -->b</p>"), "<p>ab</p>");
    }

    #[test]
    fn attributes_follow_the_allowlist() {
        assert_eq!(
            clean(r#"<div id="Main" name="n1" border="1" role="note" class="x" data-x="1" itemscope itemtype="http://schema.org/Thing" hidden>d</div>"#),
            r#"<div id="user-content-main" name="user-content-n1" itemscope="" itemtype="http://schema.org/Thing">d</div>"#
        );
        assert_eq!(
            clean(r#"<a href="https://e.com" target="_blank" rel="x" aria-label="l" aria-hidden="true" onclick="y">a</a>"#),
            r#"<a href="https://e.com" aria-label="l">a</a>"#
        );
        assert_eq!(clean(r#"<span aria-label="l" style="c">x</span>"#), "<span>x</span>");
        assert_eq!(clean(r#"<blockquote cite="https://c">q</blockquote>"#), r#"<blockquote cite="https://c">q</blockquote>"#);
        assert_eq!(clean(r#"<h3 id="user-content-x">h</h3>"#), r#"<h3 id="user-content-x">h</h3>"#);
    }

    #[test]
    fn dangerous_urls_are_dropped() {
        assert_eq!(clean(r#"<a href="javascript:alert(1)">js</a>"#), "<a>js</a>");
        assert_eq!(clean(r#"<a href="java&#9;script:alert(1)">js</a>"#), "<a>js</a>");
        assert_eq!(clean(r#"<a href=" JAVASCRIPT:x">js</a>"#), "<a>js</a>");
        assert_eq!(clean(r#"<a href="mailto:a@b.c">m</a>"#), r#"<a href="mailto:a@b.c">m</a>"#);
        assert_eq!(clean(r#"<img src="data:image/png;base64,x">"#), "<img>");
        assert_eq!(clean(r#"<img src="/abs.png" onerror="x">"#), r#"<img src="/abs.png">"#);
        assert_eq!(clean(r#"<video src="javascript:x" controls autoplay></video>"#), r#"<video controls=""></video>"#);
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

    #[test]
    fn caption_text_is_hoisted() {
        assert_eq!(
            clean("<table><caption>cap</caption><tbody><tr><td>x</td></tr></tbody></table>"),
            "cap<table><tbody><tr><td>x</td></tr></tbody></table>"
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
