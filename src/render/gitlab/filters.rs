//! Banzai HTML filters that run after comrak (GL §5), ported from `R/tools/glpost.py` and the
//! GitLab sources it mirrors (`lib/banzai/filter/*.rb`). Each function is one filter; `run`
//! applies them in GitLab's order (GL §2 table, `GfmPipeline.filters`).

use std::collections::HashSet;
use std::sync::LazyLock;

use dom_query::NodeRef;
use regex::Regex;

use super::{dom, emoji, highlight, sanitize};

/// gitlab.com's `FILTER_ITEM_LIMIT`.
pub(super) const FILTER_ITEM_LIMIT: usize = 1000;

/// Context shared by the filters.
#[derive(Clone, Debug)]
pub(super) struct FilterContext {
    /// Hosts treated as internal by ExternalLinkFilter: gitlab.com's `Gitlab.config.gitlab.url`
    /// host (GL §5.12). Local hosts are deliberately *not* added: gitlab.com gives
    /// `http://localhost:3000` links `rel="nofollow ..."` and `target="_blank"` too (verified
    /// against the live `doc/user/markdown.md` render).
    pub internal_hosts: Vec<String>,
}

impl Default for FilterContext {
    fn default() -> Self {
        FilterContext { internal_hosts: vec!["gitlab.com".to_owned()] }
    }
}

/// Runs the GitLab filter chain on the parsed fragment `root` (GL §2 rows 4-34).
pub(super) fn run(root: &NodeRef<'_>, ctx: &FilterContext) {
    code_language(root);
    // JsonTableFilter, PlantumlFilter, SpacedLinkFilter: optional, not implemented (GL §2).
    sanitize::iframe_to_img(root);
    sanitize::sanitize(root);
    if !sanitize::sanitize_links(root) {
        // SanitizeLinkFilter::TIMEOUT_MARKDOWN_MESSAGE
        while let Some(c) = root.first_child() {
            c.remove_from_parent();
        }
        root.append_html(
            "<p>Timeout while sanitizing links - rendering aborted. Please reduce the number of \
             links if possible.</p>",
        );
        return;
    }
    wiki_links(root);
    attributes(root);
    math(root);
    color(root);
    mermaid(root);
    media(root, "video");
    media(root, "audio");
    table_of_contents(root);
    footnotes(root);
    inline_diff(root);
    image_link(root);
    external_links(root, ctx);
    emoji::filter(root);
    task_list(root);
    set_direction(root);
    highlight::syntax_highlight(root);
}

// ------------------------------------------------------------------------------------------------
// CodeLanguageFilter (GL §5.1)

/// Whether `code` is the only element child of a `pre` (CSS `pre > code:only-child`).
pub(super) fn is_only_code_child(code: &NodeRef<'_>) -> bool {
    dom::is(code, "code")
        && code.parent().is_some_and(|p| dom::is(&p, "pre"))
        && code.prev_element_sibling().is_none()
        && code.next_element_sibling().is_none()
}

pub(super) fn code_language(root: &NodeRef<'_>) {
    // wrap_bare_pre_content: `pre[lang]:not(:has(code))`
    for id in dom::elements_named(root, &["pre"]) {
        let pre = root.tree.get_unchecked(&id);
        if !pre.has_attr("lang") {
            continue;
        }
        let has_code = dom::descendants(&pre).iter().any(|d| dom::is(&root.tree.get_unchecked(d), "code"));
        if has_code {
            continue;
        }
        let code = root.tree.new_element("code");
        while let Some(c) = pre.first_child() {
            code.append_child(&c);
        }
        pre.append_child(&code);
    }
    for id in dom::elements_named(root, &["code"]) {
        let code = root.tree.get_unchecked(&id);
        if !is_only_code_child(&code) {
            continue;
        }
        let Some(pre) = code.parent() else { continue };
        if pre.parent().is_none() {
            continue;
        }
        let lang_class = dom::classes(&code).into_iter().find(|c| c.starts_with("language-"));
        let language = dom::attr(&pre, "data-lang")
            .or_else(|| dom::attr(&code, "data-lang"))
            .or_else(|| lang_class.as_ref().map(|c| c["language-".len()..].to_owned()))
            .or_else(|| dom::attr(&pre, "lang"))
            .or_else(|| dom::attr(&code, "lang"));
        let meta = dom::attr(&pre, "data-meta").or_else(|| dom::attr(&code, "data-meta"));
        let (lang, mut params) = match language.as_deref() {
            None => (None, None),
            Some(l) => match l.split_once(':') {
                None => (Some(l.to_owned()), None),
                Some((a, b)) => {
                    let b = dom::ruby_strip(b);
                    (Some(a.to_owned()), (!dom::is_blank(b)).then(|| b.to_owned()))
                }
            },
        };
        if params.is_none() {
            if let Some(m) = meta.filter(|m| !dom::is_blank(m)) {
                params = Some(m);
            }
        }
        code.remove_attr("lang");
        code.remove_attr("data-lang");
        pre.remove_attr("lang");
        pre.remove_attr("data-meta");
        code.remove_attr("data-meta");
        if let Some(lc) = &lang_class {
            let others: Vec<String> = dom::classes(&code).into_iter().filter(|c| c != lc).collect();
            if others.is_empty() {
                code.remove_attr("class");
            } else {
                code.set_attr("class", &others.join(" "));
            }
        }
        if let Some(lang) = lang.filter(|l| !dom::is_blank(l)) {
            pre.set_attr("data-lang", &lang);
            pre.set_attr("data-canonical-lang", &lang);
            if let Some(p) = params {
                pre.set_attr("data-lang-params", &p);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// WikiLinkGollumFilter (GL §6.4, offline variant)

/// Offline version of `WikiLinkGollumFilter`: links keep their relative `href` (the local server
/// resolves them), page links get `gfm gfm-gollum-wiki-page` so GitLab's CSS draws the icon.
pub(super) fn wiki_links(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["a"]) {
        let a = root.tree.get_unchecked(&id);
        let (Some(href), true) = (dom::attr(&a, "href"), a.has_attr("data-wikilink")) else {
            continue;
        };
        if href.eq_ignore_ascii_case("_toc_") && dom::text(&a).eq_ignore_ascii_case("_toc_") {
            continue;
        }
        if href.starts_with("http") {
            dom::add_class(&a, "gfm");
        } else {
            dom::add_class(&a, "gfm");
            dom::add_class(&a, "gfm-gollum-wiki-page");
        }
    }
}

// ------------------------------------------------------------------------------------------------
// AttributesFilter (GL §5.3)

static ATTRIBUTES_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A(?P<matched>\{(?P<attributes>.{1,100})\})").expect("regex"));
static VALUE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\A(?P<name>[A-Za-z0-9_-]+)="?(?P<value>[^"\t\n\x0B\x0C\r ]+)"?\z"#).expect("regex"));
static VALID_SIZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A[0-9]{1,4}(?:%|px)?\z").expect("regex"));
static CLASS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A\.(?P<class_name>[A-Za-z0-9_-]+)\z").expect("regex"));

pub(super) fn attributes(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["img"]) {
        let img = root.tree.get_unchecked(&id);
        let Some(sib) = img.next_sibling() else { continue };
        let Some(content) = dom::text_node_contents(&sib) else { continue };
        if !content.starts_with('{') {
            continue;
        }
        let Some(m) = ATTRIBUTES_PATTERN.captures(&content) else { continue };
        let matched = m.name("matched").map_or("", |x| x.as_str()).to_owned();
        let attrs = m.name("attributes").map_or("", |x| x.as_str());
        for attribute in attrs.split_ascii_whitespace() {
            if let Some(v) = VALUE_REGEX.captures(attribute) {
                let name = &v["name"];
                let value = &v["value"];
                if matches!(name, "width" | "height") && VALID_SIZE.is_match(value) {
                    img.set_attr(name, value);
                }
            } else if let Some(c) = CLASS_REGEX.captures(attribute) {
                let mapped = match &c["class_name"] {
                    "float-right" => "glfm-float-right",
                    "float-left" => "glfm-float-left",
                    "float-none" => "glfm-float-none",
                    _ => continue,
                };
                let class = match dom::attr(&img, "class") {
                    Some(old) => format!("{old} {mapped}"),
                    None => mapped.to_owned(),
                };
                img.set_attr("class", &class);
            }
        }
        dom::set_text_node_contents(&sib, content.replacen(&matched, "", 1).as_str());
    }
}

// ------------------------------------------------------------------------------------------------
// MathFilter (GL §5.4), ColorFilter (GL §5.5), MermaidFilter (GL §5.6)

pub(super) fn math(root: &NodeRef<'_>) {
    let mut count = 0usize;
    for id in dom::elements_named(root, &["pre", "code", "span"]) {
        let n = root.tree.get_unchecked(&id);
        let is_pre = dom::is(&n, "pre");
        let styled = n.has_attr("data-math-style");
        let pre_math = is_pre && dom::attr_is(&n, "data-canonical-lang", "math");
        if !(styled || pre_math) {
            continue;
        }
        if count >= FILTER_ITEM_LIMIT {
            break;
        }
        if pre_math && !styled {
            n.set_attr("data-math-style", "display");
        }
        n.set_attr("class", "js-render-math");
        count += 1;
    }
}

/// `Banzai::ColorParser::COLOR_FORMAT` (Ruby `\d`/`\s` are ASCII-only).
static COLOR_FORMAT: LazyLock<Regex> = LazyLock::new(|| {
    let alpha = r"(?:0(?:\.[0-9]+)?|\.[0-9]+|1(?:\.0+)?)";
    let pct = r"(?:(?:[0-9]{1,2}|100)%)";
    let ach = format!(r"(?:,[ \t\r\n\x0B\x0C]*(?:{alpha}|{pct}))?");
    let bits = r"(?:[0-9]{1,2}|1[0-9][0-9]|2(?:[0-4][0-9]|5[0-5]))";
    let s = r"[ \t\r\n\x0B\x0C]*";
    let hex = r"\#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})";
    let rgb = format!(r"(?:rgba?\((?:(?:{bits},{s}){{2}}{bits}|(?:{pct},{s}){{2}}{pct}){ach}\))");
    let degs = r"(?:-?[0-9]+(?:deg)?)";
    let rads = r"(?:-?(?:[0-9]+(?:\.[0-9]+)?|\.[0-9]+)rad)";
    let hsl = format!(r"(?:hsla?\((?:{degs}|{rads}),{s}{pct},{s}{pct}{ach}\))");
    Regex::new(&format!(r"(?i)\A(?:{hex}|{rgb}|{hsl})\z")).expect("color regex")
});

/// `ColorParser.parse`.
pub(super) fn parse_color(text: &str) -> Option<&str> {
    COLOR_FORMAT.is_match(text).then_some(text)
}

pub(super) fn color(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["code"]) {
        let code = root.tree.get_unchecked(&id);
        let content = dom::text(&code);
        let unescaped = content.strip_prefix('\\').unwrap_or(&content);
        let Some(color) = parse_color(unescaped) else { continue };
        if content.starts_with('\\') {
            let unescaped = unescaped.to_owned();
            code.remove_children();
            code.append_child(&dom::new_text(root.tree, &unescaped));
        } else {
            let style = format!("background-color: {color};");
            let chip = dom::new_element(root.tree, "span", &[("class", "gfm-color_chip")]);
            chip.append_child(&dom::new_element(root.tree, "span", &[("style", &style)]));
            code.append_child(&chip);
        }
    }
}

pub(super) fn mermaid(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["code"]) {
        let code = root.tree.get_unchecked(&id);
        if code.parent().is_some_and(|p| dom::is(&p, "pre") && dom::attr_is(&p, "data-canonical-lang", "mermaid")) {
            dom::add_class(&code, "js-render-mermaid");
        }
    }
}

// ------------------------------------------------------------------------------------------------
// VideoLinkFilter / AudioLinkFilter (GL §5.7)

/// Ruby `File.extname(path).sub('.', '').downcase`.
fn ruby_extname(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let trimmed = base.trim_start_matches('.');
    match trimmed.rfind('.') {
        Some(i) if i + 1 < trimmed.len() => trimmed[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

pub(super) fn media(root: &NodeRef<'_>, kind: &str) {
    let exts: &[&str] = if kind == "video" {
        &["mp4", "m4v", "mov", "webm", "ogv"]
    } else {
        &["mp3", "oga", "ogg", "spx", "wav"]
    };
    for id in dom::elements_named(root, &["img"]) {
        let img = root.tree.get_unchecked(&id);
        if !dom::is_attached(&img, root) || dom::has_ancestor(&img, &["a"], root) {
            continue;
        }
        let src = dom::attr(&img, "data-canonical-src")
            .filter(|s| !dom::is_blank(s))
            .or_else(|| dom::attr(&img, "src"));
        let Some(src) = src.filter(|s| !dom::is_blank(s)) else { continue };
        if !exts.contains(&ruby_extname(&src).as_str()) {
            continue;
        }
        let title = dom::attr(&img, "title").or_else(|| dom::attr(&img, "alt")).unwrap_or_default();
        let el = dom::new_element(root.tree, kind, &[]);
        el.set_attr("src", &dom::attr(&img, "src").unwrap_or_default());
        el.set_attr("controls", "true");
        el.set_attr("data-setup", "{}");
        el.set_attr("data-title", &title);
        if kind == "video" {
            el.set_attr("preload", "metadata");
            el.set_attr("class", "gl-rounded-lg");
            for a in ["height", "width"] {
                if let Some(v) = dom::attr(&img, a) {
                    el.set_attr(a, &v);
                }
            }
        }
        if let Some(c) = dom::attr(&img, "data-canonical-src") {
            el.set_attr("data-canonical-src", &c);
        }
        let class = format!("media-container {kind}-container");
        let span = dom::new_element(root.tree, "span", &[("class", &class)]);
        span.append_child(&el);
        img.insert_before(&span);
        img.remove_from_parent();
    }
}

// ------------------------------------------------------------------------------------------------
// TableOfContentsTagFilter (GL §5.8)

struct HeaderNode {
    level: usize,
    href: String,
    text: String,
    parent: Option<usize>,
    children: Vec<usize>,
}

/// `build_toc`: the nested `ul.section-nav`, or `None` without headings.
fn build_toc<'a>(root: &NodeRef<'a>) -> Option<NodeRef<'a>> {
    // Index 0 is the level-0 root.
    let mut nodes = vec![HeaderNode { level: 0, href: String::new(), text: String::new(), parent: None, children: vec![] }];
    let mut current = 0usize;
    for id in dom::elements_named(root, &["h1", "h2", "h3", "h4", "h5", "h6"]) {
        let h = root.tree.get_unchecked(&id);
        let anchor = dom::descendants(&h).into_iter().map(|d| root.tree.get_unchecked(&d)).find(|d| {
            dom::is(d, "a") && dom::classes(d).iter().any(|c| c == "anchor")
        });
        let Some(anchor) = anchor else { continue };
        let href = dom::attr(&anchor, "href").unwrap_or_default();
        let href: String = href.chars().skip(1).collect();
        let level = dom::tag(&h).and_then(|t| t[1..].parse().ok()).unwrap_or(1);
        // HeaderNode#find_parent
        let parent = if level == nodes[current].level {
            nodes[current].parent
        } else if level > nodes[current].level {
            Some(current)
        } else {
            let mut p = Some(current);
            while let Some(pi) = p {
                if nodes[pi].level >= level {
                    p = nodes[pi].parent;
                } else {
                    break;
                }
            }
            p
        };
        let idx = nodes.len();
        nodes.push(HeaderNode {
            level,
            href,
            text: dom::ruby_strip(&dom::text(&h)).to_owned(),
            parent,
            children: vec![],
        });
        if let Some(p) = parent {
            nodes[p].children.push(idx);
        }
        current = idx;
    }
    fn list<'a>(root: &NodeRef<'a>, nodes: &[HeaderNode], children: &[usize], is_root: bool) -> Option<NodeRef<'a>> {
        if children.is_empty() {
            return None;
        }
        let ul = root.tree.new_element("ul");
        if is_root {
            ul.set_attr("class", "section-nav");
        }
        // Iterative construction (the heading nesting depth is at most 6).
        for &c in children {
            let li = root.tree.new_element("li");
            let a = dom::new_element(root.tree, "a", &[("href", &format!("#{}", nodes[c].href))]);
            a.append_child(&dom::new_text(root.tree, &nodes[c].text));
            li.append_child(&a);
            if let Some(sub) = list(root, nodes, &nodes[c].children, false) {
                li.append_child(&sub);
            }
            ul.append_child(&li);
        }
        Some(ul)
    }
    let root_children = nodes[0].children.clone();
    list(root, &nodes, &root_children, true)
}

pub(super) fn table_of_contents(root: &NodeRef<'_>) {
    let mut targets = Vec::new();
    // GOLLUM_TOC_QUERY: text under a[data-wikilink="true"] starting with `_toc_` (only T, O, C
    // are case-folded by the XPath translate()), whose parent's parent is a <p> with text `_toc_`.
    for id in dom::text_nodes(root) {
        let t = root.tree.get_unchecked(&id);
        let content = dom::text_node_contents(&t).unwrap_or_default();
        if !translate_toc(&content).starts_with("_toc_") {
            continue;
        }
        let in_wikilink = {
            let mut cur = t.parent();
            let mut found = false;
            while let Some(p) = cur {
                if p.id == root.id {
                    break;
                }
                if dom::is(&p, "a") && dom::attr_is(&p, "data-wikilink", "true") {
                    found = true;
                    break;
                }
                cur = p.parent();
            }
            found
        };
        if !in_wikilink {
            continue;
        }
        let Some(a) = t.parent() else { continue };
        let Some(p) = a.parent() else { continue };
        if dom::is(&p, "p") && dom::text(&p).eq_ignore_ascii_case("_toc_") {
            targets.push(p.id);
        }
    }
    // TOC_QUERY: a text node that is the only child of a <p> and is `[toc]`.
    for id in dom::text_nodes(root) {
        let t = root.tree.get_unchecked(&id);
        let Some(p) = t.parent() else { continue };
        if !dom::is(&p, "p") {
            continue;
        }
        let content = dom::text_node_contents(&t).unwrap_or_default();
        if !translate_toc(&content).starts_with("[toc]") {
            continue;
        }
        if p.children_it(false).count() == 1 && dom::ruby_strip(&content).eq_ignore_ascii_case("[toc]") {
            targets.push(p.id);
        }
    }
    if targets.is_empty() {
        return;
    }
    let toc = build_toc(root);
    for id in targets {
        let p = root.tree.get_unchecked(&id);
        if p.parent().is_none() {
            continue;
        }
        // GitLab memoizes one <ul> and `replace`s every tag with it, so the node moves to the
        // last tag (libxml2 unlinks it from the previous position).
        if let Some(toc) = &toc {
            p.insert_before(toc);
        }
        p.remove_from_parent();
    }
}

/// XPath `translate(., '[TOC]', '[toc]')` / `translate(., '_TOC_', '_toc_')`.
fn translate_toc(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'T' => 't',
            'O' => 'o',
            'C' => 'c',
            c => c,
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// FootnoteFilter (GL §5.9; offline: no random suffix)

pub(super) fn footnotes(root: &NodeRef<'_>) {
    let refs: Vec<_> = dom::elements_named(root, &["a"])
        .into_iter()
        .filter(|id| {
            let a = root.tree.get_unchecked(id);
            a.has_attr("data-footnote-ref") && a.parent().is_some_and(|p| dom::is(&p, "sup"))
        })
        .collect();
    if refs.len() > FILTER_ITEM_LIMIT {
        return;
    }
    let Some(section) = dom::elements_named(root, &["section"])
        .into_iter()
        .map(|id| root.tree.get_unchecked(&id))
        .find(|s| s.has_attr("data-footnotes"))
    else {
        return;
    };
    append_class(&section, "footnotes");
    let section_items: Vec<_> = dom::elements_named(&section, &["li"]);
    let mut modified: HashSet<String> = HashSet::new();
    for id in refs {
        let link = root.tree.get_unchecked(&id);
        let Some(link_id) = dom::attr(&link, "id") else { continue };
        let ref_num = link_id.strip_prefix("fnref-").unwrap_or(&link_id).to_owned();
        let fn_id = format!("fn-{ref_num}");
        let footnote = section_items
            .iter()
            .map(|i| root.tree.get_unchecked(i))
            .find(|li| dom::attr_is(li, "id", &fn_id));
        if footnote.is_none() && !modified.contains(&ref_num) {
            continue;
        }
        if let Some(sup) = link.parent() {
            append_class(&sup, "footnote-ref");
        }
        if modified.contains(&ref_num) {
            continue;
        }
        let Some(footnote) = footnote else { continue };
        let backref_href = format!("#fnref-{ref_num}");
        let backref = dom::descendants(&footnote)
            .into_iter()
            .map(|d| root.tree.get_unchecked(&d))
            .find(|a| dom::is(a, "a") && dom::attr_is(a, "href", &backref_href));
        if let Some(backref) = backref {
            let idx = dom::attr(&backref, "data-footnote-backref-idx").unwrap_or_default();
            let title = format!("Back to reference {idx}");
            backref.set_attr("title", &title);
            backref.set_attr("aria-label", &title);
            append_class(&backref, "footnote-backref");
        }
        modified.insert(ref_num);
    }
}

/// Nokogiri `append_class` (keeps duplicates).
fn append_class(n: &NodeRef<'_>, class: &str) {
    let value = match dom::attr(n, "class") {
        Some(old) if !old.trim().is_empty() => format!("{} {class}", old.trim()),
        _ => class.to_owned(),
    };
    n.set_attr("class", &value);
}

// ------------------------------------------------------------------------------------------------
// InlineDiffFilter (GL §5.10)

static INLINE_DIFF_DELETION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\[\-(.*?)\-\]|\{\-(.*?)\-\})").expect("regex"));
static INLINE_DIFF_ADDITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\[\+(.*?)\+\]|\{\+(.*?)\+\})").expect("regex"));

/// The regex rewrite of `inline_diff_filter` on (escaped) HTML text.
pub(super) fn inline_diff_html(text: &str) -> String {
    let replace = |re: &Regex, input: &str, kind: &str| -> String {
        let mut out = String::with_capacity(input.len() + 64);
        let mut last = 0;
        for c in re.captures_iter(input).take(FILTER_ITEM_LIMIT) {
            let m = c.get(0).expect("group 0");
            out.push_str(&input[last..m.start()]);
            out.push_str("<span class=\"idiff left right ");
            out.push_str(kind);
            out.push_str("\">");
            out.push_str(c.get(1).map_or("", |g| g.as_str()));
            out.push_str(c.get(2).map_or("", |g| g.as_str()));
            out.push_str("</span>");
            last = m.end();
        }
        out.push_str(&input[last..]);
        out
    };
    let deleted = replace(&INLINE_DIFF_DELETION, text, "deletion");
    replace(&INLINE_DIFF_ADDITION, &deleted, "addition")
}

pub(super) fn inline_diff(root: &NodeRef<'_>) {
    for id in dom::text_nodes(root) {
        let t = root.tree.get_unchecked(&id);
        let content = dom::text_node_contents(&t).unwrap_or_default();
        if !(content.contains("[-") || content.contains("{-") || content.contains("[+") || content.contains("{+")) {
            continue;
        }
        if dom::has_ancestor(&t, &["pre", "code", "tt"], root) {
            continue;
        }
        let escaped = escape_text(&content);
        let html = inline_diff_html(&escaped);
        if html == escaped {
            continue;
        }
        t.replace_with_html(html);
    }
}

/// Nokogiri's text-node `to_html` escaping (`&`, `<`, `>`).
pub(super) fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------
// ImageLinkFilter (GL §5.11)

pub(super) fn image_link(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["img"]) {
        let img = root.tree.get_unchecked(&id);
        if !dom::is_attached(&img, root) || dom::has_ancestor(&img, &["a"], root) {
            continue;
        }
        // ImageLazyLoadFilter copies `src` to `data-src`; an empty one is skipped here.
        let Some(src) = dom::attr(&img, "src").filter(|s| !s.is_empty()) else { continue };
        let link = dom::new_element(
            root.tree,
            "a",
            &[("class", "no-attachment-icon"), ("href", &src), ("target", "_blank"), ("rel", "noopener noreferrer")],
        );
        if let Some(c) = dom::attr(&img, "data-canonical-src") {
            link.set_attr("data-canonical-src", &c);
        }
        if let (Some(d), Some(ds)) = (dom::attr(&img, "data-diagram"), dom::attr(&img, "data-diagram-src")) {
            link.set_attr("data-diagram", &d);
            link.set_attr("data-diagram-src", &ds);
            img.remove_attr("data-diagram");
            img.remove_attr("data-diagram-src");
        }
        img.insert_before(&link);
        link.append_child(&img);
    }
}

// ------------------------------------------------------------------------------------------------
// ExternalLinkFilter (GL §5.12)

const RTLO: char = '\u{202E}';
const ENCODED_RTLO: &str = "%E2%80%AE";

pub(super) fn external_links(root: &NodeRef<'_>, ctx: &FilterContext) {
    for id in dom::elements_named(root, &["a"]) {
        let a = root.tree.get_unchecked(&id);
        let Some(href) = dom::attr(&a, "href").filter(|h| !h.is_empty()) else { continue };
        let src_attr = if a.has_attr("data-canonical-src") { "data-canonical-src" } else { "href" };
        let src = if src_attr == "href" { href } else { dom::attr(&a, src_attr).unwrap_or_default() };
        if src.starts_with('#') {
            continue;
        }
        let parts = match ruby_uri_to_s(&src) {
            Some(normalized) => {
                if normalized != src {
                    a.set_attr(src_attr, &normalized);
                }
                sanitize::parse_uri(&normalized).ok().map(|p| (p.scheme.map(str::to_owned), p.host.map(hostname)))
            }
            None => None,
        };
        if let Some((scheme, host)) = &parts {
            let relative = host.is_none() && scheme.is_none();
            let internal_host = host.as_deref().is_some_and(|h| ctx.internal_hosts.iter().any(|i| i == h));
            if relative || internal_host {
                continue;
            }
        }
        // sanitize_link_text!
        for d in dom::descendants(&a) {
            let n = root.tree.get_unchecked(&d);
            if let Some(t) = dom::text_node_contents(&n) {
                if t.contains(RTLO) {
                    dom::set_text_node_contents(&n, &t.replace(RTLO, ENCODED_RTLO));
                }
            }
        }
        if let Some((_, host)) = &parts {
            let current = dom::attr(&a, src_attr).unwrap_or_default();
            let idn = host.as_deref().is_some_and(is_idn_host);
            if idn || current.contains(ENCODED_RTLO) {
                dom::add_class(&a, "has-tooltip");
                let title = url::Url::parse(&current).map(|u| u.to_string()).unwrap_or(current);
                a.set_attr("title", &title);
            }
        }
        // add_nofollow!: scheme http, https or none (also when the URI did not parse).
        let scheme_ok = match &parts {
            None => true,
            Some((None, _)) => true,
            Some((Some(s), _)) => s == "http" || s == "https",
        };
        if scheme_ok {
            let license = dom::attr_is(&a, "rel", "license");
            let rel = if license { "nofollow noreferrer noopener license" } else { "nofollow noreferrer noopener" };
            a.set_attr("rel", rel);
            a.set_attr("target", "_blank");
        }
    }
}

/// Addressable `hostname`: the host without IPv6 brackets.
fn hostname(host: &str) -> String {
    host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host).to_owned()
}

/// `normalized_host.start_with?('xn--')`.
fn is_idn_host(host: &str) -> bool {
    let first = host.split('.').next().unwrap_or("");
    !first.is_ascii() || host.to_ascii_lowercase().starts_with("xn--")
}

/// Ruby `URI.parse(s).to_s` (RFC 3986 parser): `None` when Ruby raises `URI::InvalidURIError`.
/// `to_s` lower-cases the scheme and drops a default port.
pub(super) fn ruby_uri_to_s(s: &str) -> Option<String> {
    fn pchar(c: char, extra: &str) -> bool {
        c.is_ascii_alphanumeric() || "!$&'()*+,-.;=_~".contains(c) || extra.contains(c)
    }
    fn valid_pct(s: &str, extra: &str) -> bool {
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let c = b[i] as char;
            if c == '%' {
                if i + 2 >= b.len() {
                    return false;
                }
                if !(b[i + 1] as char).is_ascii_hexdigit() || !(b[i + 2] as char).is_ascii_hexdigit() {
                    return false;
                }
                i += 3;
                continue;
            }
            if !c.is_ascii() || !pchar(c, extra) {
                return false;
            }
            i += 1;
        }
        true
    }
    if !s.is_ascii() {
        return None;
    }
    let (before_frag, fragment) = match s.find('#') {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    if let Some(f) = fragment {
        if !valid_pct(f, ":@/?") {
            return None;
        }
    }
    // The query is `[^#]*+`: anything goes (non-ASCII was rejected above).
    let (before_query, query) = match before_frag.find('?') {
        Some(i) => (&before_frag[..i], Some(&before_frag[i + 1..])),
        None => (before_frag, None),
    };
    // scheme
    let (scheme, rest) = match before_query.find([':', '/']) {
        Some(i) if before_query.as_bytes()[i] == b':' => {
            let sch = &before_query[..i];
            let mut cs = sch.chars();
            let ok = cs.next().is_some_and(|c| c.is_ascii_alphabetic())
                && cs.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
            if !ok {
                return None;
            }
            (Some(sch.to_ascii_lowercase()), &before_query[i + 1..])
        }
        _ => (None, before_query),
    };
    let mut out = String::with_capacity(s.len());
    if let Some(sch) = &scheme {
        out.push_str(sch);
        out.push(':');
    }
    let path = if let Some(auth_rest) = rest.strip_prefix("//") {
        let end = auth_rest.find('/').unwrap_or(auth_rest.len());
        let authority = &auth_rest[..end];
        let (userinfo, hostport) = match authority.rfind('@') {
            Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
            None => (None, authority),
        };
        if let Some(u) = userinfo {
            if !valid_pct(u, ":") {
                return None;
            }
        }
        let (host, port) = if hostport.starts_with('[') {
            let close = hostport.find(']')?;
            let host = &hostport[..=close];
            let after = &hostport[close + 1..];
            let port = match after.strip_prefix(':') {
                Some(p) => Some(p),
                None if after.is_empty() => None,
                None => return None,
            };
            (host, port)
        } else {
            match hostport.rfind(':') {
                Some(i) => (&hostport[..i], Some(&hostport[i + 1..])),
                None => (hostport, None),
            }
        };
        if !host.starts_with('[') && !valid_pct(host, "") {
            return None;
        }
        if port.is_some_and(|p| !p.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
        out.push_str("//");
        if let Some(u) = userinfo {
            out.push_str(u);
            out.push('@');
        }
        out.push_str(host);
        let default = match scheme.as_deref() {
            Some("http") => Some("80"),
            Some("https") => Some("443"),
            Some("ftp") => Some("21"),
            _ => None,
        };
        if let Some(p) = port.filter(|p| !p.is_empty()) {
            if Some(p.trim_start_matches('0')) != default.map(|d| d.trim_start_matches('0')) {
                out.push(':');
                out.push_str(p);
            }
        }
        &auth_rest[end..]
    } else {
        // path-noscheme: no ':' in the first segment of a relative reference.
        if scheme.is_none() && rest.split('/').next().is_some_and(|seg| seg.contains(':')) {
            return None;
        }
        rest
    };
    if !valid_pct(path, ":@/") {
        return None;
    }
    out.push_str(path);
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    Some(out)
}

// ------------------------------------------------------------------------------------------------
// TaskListFilter (GL §5.14)

/// ActiveSupport `String#truncate(100, separator: ' ', omission: '…')`.
pub(super) fn truncate_100(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 100 {
        return s.to_owned();
    }
    let room = 99; // 100 - "…".length
    let stop = chars[..=room.min(chars.len() - 1)]
        .iter()
        .rposition(|&c| c == ' ')
        .unwrap_or(room);
    let mut out: String = chars[..stop].iter().collect();
    out.push('…');
    out
}

pub(super) fn task_list(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["input"]) {
        let input = root.tree.get_unchecked(&id);
        if !dom::classes(&input).iter().any(|c| c == "task-list-item-checkbox") {
            continue;
        }
        let Some(parent) = input.parent() else { continue };
        let is_table_item = dom::classes(&parent).iter().any(|c| c == "task-table-item");
        if is_table_item {
            if let Some(header) = column_header_text(&input, &parent, root) {
                if !dom::is_blank(&header) {
                    input.set_attr("aria-label", &format!("Check option in column \"{header}\""));
                }
            }
        } else {
            let mut text = String::new();
            let mut it = input.next_sibling();
            while let Some(n) = it {
                if dom::is_any(&n, &["ol", "ul"]) {
                    break;
                }
                text.push_str(&dom::text(&n));
                it = n.next_sibling();
            }
            let truncated = truncate_100(dom::ruby_strip(&text));
            if !dom::is_blank(&truncated) {
                input.set_attr("aria-label", &format!("Check option: {truncated}"));
            }
        }
        if !input.has_attr("aria-label") {
            input.set_attr("aria-label", "Check option");
        }
        if !input.has_attr("data-inapplicable") {
            continue;
        }
        let space = dom::new_text(root.tree, " ");
        input.insert_after(&space);
        // yield_text_nodes_without_descending_into(space.next_sibling, %w[p div ul ol])
        let mut stack: Vec<NodeRef<'_>> = space.next_sibling().into_iter().collect();
        while let Some(it) = stack.pop() {
            if let Some(next) = it.next_sibling() {
                stack.push(next);
            }
            if it.is_text() {
                if !dom::is_blank(&dom::text(&it)) {
                    let s = dom::new_element(root.tree, "s", &[("class", "inapplicable")]);
                    it.insert_before(&s);
                    s.append_child(&it);
                }
            } else if !dom::is_any(&it, &["p", "div", "ul", "ol"]) {
                if let Some(first) = it.first_child() {
                    stack.push(first);
                }
            }
        }
    }
}

fn column_header_text(input: &NodeRef<'_>, cell: &NodeRef<'_>, root: &NodeRef<'_>) -> Option<String> {
    let row = cell.parent()?;
    let index = dom::element_children(&row).iter().position(|c| c.id == cell.id)?;
    let table = dom::ancestor(input, "table", root)?;
    let first_row = dom::elements_named(&table, &["tr"]).into_iter().next()?;
    let first_row = root.tree.get_unchecked(&first_row);
    let header = dom::element_children(&first_row).into_iter().nth(index)?;
    Some(dom::ruby_strip(&dom::text(&header)).to_owned())
}

// ------------------------------------------------------------------------------------------------
// SetDirectionFilter (GL §5.15)

pub(super) fn set_direction(root: &NodeRef<'_>) {
    for c in root.children() {
        if !dom::is_any(&c, &["p", "h1", "h2", "h3", "h4", "h5", "h6", "ol", "ul", "blockquote", "table"]) {
            continue;
        }
        if dom::is(&c, "ul") && dom::attr_is(&c, "class", "section-nav") {
            continue;
        }
        c.set_attr("dir", "auto");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dom_query::Document;

    fn apply(html: &str, f: impl Fn(&NodeRef<'_>)) -> String {
        let doc = Document::fragment(html);
        let root = doc.html_root();
        f(&root);
        root.inner_html().to_string()
    }

    #[test]
    fn code_language_moves_lang_to_pre() {
        assert_eq!(
            apply("<pre lang=\"ruby:red\" data-sourcepos=\"1:1-3:3\" data-meta=\"gem\"><code>x</code></pre>", code_language),
            "<pre data-sourcepos=\"1:1-3:3\" data-lang=\"ruby\" data-canonical-lang=\"ruby\" data-lang-params=\"red\"><code>x</code></pre>"
        );
        assert_eq!(
            apply("<pre lang=\"js\" data-meta=\"title x\"><code>x</code></pre>", code_language),
            "<pre data-lang=\"js\" data-canonical-lang=\"js\" data-lang-params=\"title x\"><code>x</code></pre>"
        );
        assert_eq!(
            apply("<pre><code class=\"language-py other\">x</code></pre>", code_language),
            "<pre data-lang=\"py\" data-canonical-lang=\"py\"><code class=\"other\">x</code></pre>"
        );
        assert_eq!(apply("<pre lang=\"c\">x</pre>", code_language), "<pre data-lang=\"c\" data-canonical-lang=\"c\"><code>x</code></pre>");
        // Two element children: not `code:only-child`.
        assert_eq!(apply("<pre lang=\"c\"><code>a</code><b>b</b></pre>", code_language), "<pre lang=\"c\"><code>a</code><b>b</b></pre>");
    }

    #[test]
    fn attributes_on_images() {
        assert_eq!(
            apply("<p><img src=\"a.png\" alt=\"\">{width=50% height=\"30px\" .float-right .bogus title=x} after</p>", attributes),
            "<p><img src=\"a.png\" alt=\"\" width=\"50%\" height=\"30px\" class=\"glfm-float-right\"> after</p>"
        );
        assert_eq!(apply("<p><img src=\"a\">{width=99999}</p>", attributes), "<p><img src=\"a\"></p>");
        assert_eq!(apply("<p><img src=\"a\"> {width=9}</p>", attributes), "<p><img src=\"a\"> {width=9}</p>");
    }

    #[test]
    fn math_marks_nodes() {
        assert_eq!(
            apply("<p><span data-math-style=\"inline\" class=\"x\">a</span><code data-math-style=\"inline\">b</code></p><pre data-canonical-lang=\"math\"><code>c</code></pre>", math),
            "<p><span data-math-style=\"inline\" class=\"js-render-math\">a</span><code data-math-style=\"inline\" class=\"js-render-math\">b</code></p><pre data-canonical-lang=\"math\" data-math-style=\"display\" class=\"js-render-math\"><code>c</code></pre>"
        );
    }

    #[test]
    fn color_chips() {
        for good in ["#123", "#1234", "#123456", "#12345678", "RGB(0,255,1)", "rgba(10%, 20%, 30%, 0.5)", "hsl(120deg, 10%, 20%)", "HSLA(-1.5rad, 0%, 100%, 1.0)", "rgb(1, 2, 3, 50%)"] {
            assert!(parse_color(good).is_some(), "{good}");
        }
        for bad in ["#12", "#12345", "rgb(256,0,0)", "rgb(1,2)", "hsl(1, 2, 3)", " #123", "#123 ", "red", "#１２３"] {
            assert!(parse_color(bad).is_none(), "{bad}");
        }
        assert_eq!(
            apply("<code>#abc</code><code>\\#abc</code><code>#ab</code>", color),
            "<code>#abc<span class=\"gfm-color_chip\"><span style=\"background-color: #abc;\"></span></span></code><code>#abc</code><code>#ab</code>"
        );
    }

    #[test]
    fn mermaid_class() {
        assert_eq!(
            apply("<pre data-canonical-lang=\"mermaid\"><code>graph</code></pre>", mermaid),
            "<pre data-canonical-lang=\"mermaid\"><code class=\"js-render-mermaid\">graph</code></pre>"
        );
    }

    #[test]
    fn media_links() {
        assert_eq!(
            apply("<p><img src=\"v/a.MP4\" alt=\"clip\" width=\"10\"></p>", |r| media(r, "video")),
            "<p><span class=\"media-container video-container\"><video src=\"v/a.MP4\" controls=\"true\" data-setup=\"{}\" data-title=\"clip\" preload=\"metadata\" class=\"gl-rounded-lg\" width=\"10\"></video></span></p>"
        );
        assert_eq!(
            apply("<p><img src=\"a.ogg\" alt=\"s\" title=\"T\"></p>", |r| media(r, "audio")),
            "<p><span class=\"media-container audio-container\"><audio src=\"a.ogg\" controls=\"true\" data-setup=\"{}\" data-title=\"T\"></audio></span></p>"
        );
        assert_eq!(apply("<a href=\"x\"><img src=\"a.mp4\"></a>", |r| media(r, "video")), "<a href=\"x\"><img src=\"a.mp4\"></a>");
        assert_eq!(ruby_extname("a/b.c/.hidden"), "");
        assert_eq!(ruby_extname("x.tar.GZ"), "gz");
        assert_eq!(ruby_extname("x."), "");
    }

    #[test]
    fn toc_nesting_and_last_tag_wins() {
        let html = "<p><a href=\"_TOC_\" data-wikilink=\"true\">_TOC_</a></p>\
                    <h2><a href=\"#a\" class=\"anchor\"></a>A</h2><h3>B<a href=\"#b\" class=\"anchor\"></a></h3>\
                    <h1>C<a href=\"#c\" class=\"anchor\"></a></h1><h4>skip</h4><p>[TOC]</p>";
        assert_eq!(
            apply(html, table_of_contents),
            "<h2><a href=\"#a\" class=\"anchor\"></a>A</h2><h3>B<a href=\"#b\" class=\"anchor\"></a></h3>\
             <h1>C<a href=\"#c\" class=\"anchor\"></a></h1><h4>skip</h4>\
             <ul class=\"section-nav\"><li><a href=\"#a\">A</a><ul><li><a href=\"#b\">B</a></li></ul></li><li><a href=\"#c\">C</a></li></ul>"
        );
        assert_eq!(apply("<p>[toc]</p><p>x</p>", table_of_contents), "<p>x</p>");
        assert_eq!(apply("<p>[TOC] x</p>", table_of_contents), "<p>[TOC] x</p>");
    }

    #[test]
    fn footnote_classes() {
        let html = "<p>a<sup><a href=\"#fn-1\" id=\"fnref-1\" data-footnote-ref=\"\">1</a></sup>\
                    <sup><a href=\"#fn-1\" id=\"fnref-1-2\" data-footnote-ref=\"\">1</a></sup></p>\
                    <section data-footnotes=\"\"><ol><li id=\"fn-1\"><p>x <a href=\"#fnref-1\" data-footnote-backref=\"\" data-footnote-backref-idx=\"1\" aria-label=\"Back to reference 1\">↩</a></p></li></ol></section>";
        assert_eq!(
            apply(html, footnotes),
            "<p>a<sup class=\"footnote-ref\"><a href=\"#fn-1\" id=\"fnref-1\" data-footnote-ref=\"\">1</a></sup>\
             <sup><a href=\"#fn-1\" id=\"fnref-1-2\" data-footnote-ref=\"\">1</a></sup></p>\
             <section data-footnotes=\"\" class=\"footnotes\"><ol><li id=\"fn-1\"><p>x <a href=\"#fnref-1\" data-footnote-backref=\"\" data-footnote-backref-idx=\"1\" aria-label=\"Back to reference 1\" title=\"Back to reference 1\" class=\"footnote-backref\">↩</a></p></li></ol></section>"
        );
    }

    #[test]
    fn inline_diffs() {
        assert_eq!(inline_diff_html("a {+ x +} [-y-]"), "a <span class=\"idiff left right addition\"> x </span> <span class=\"idiff left right deletion\">y</span>");
        assert_eq!(inline_diff_html("{+ x +]"), "{+ x +]");
        assert_eq!(
            apply("<p>{+ a [- b -] c +}</p><code>{+x+}</code>", inline_diff),
            "<p><span class=\"idiff left right addition\"> a <span class=\"idiff left right deletion\"> b </span> c </span></p><code>{+x+}</code>"
        );
        assert_eq!(apply("<p>{+ &lt;b&gt; +}</p>", inline_diff), "<p><span class=\"idiff left right addition\"> &lt;b&gt; </span></p>");
    }

    #[test]
    fn image_links() {
        assert_eq!(
            apply("<p><img src=\"i.png\" alt=\"x\"><a href=\"y\"><img src=\"j.png\"></a><img alt=\"no src\"></p>", image_link),
            "<p><a class=\"no-attachment-icon\" href=\"i.png\" target=\"_blank\" rel=\"noopener noreferrer\"><img src=\"i.png\" alt=\"x\"></a><a href=\"y\"><img src=\"j.png\"></a><img alt=\"no src\"></p>"
        );
    }

    #[test]
    fn external_link_rules() {
        let ctx = FilterContext::default();
        let run = |h: &str| apply(h, |r| external_links(r, &ctx));
        assert_eq!(run("<a href=\"https://example.com\">x</a>"), "<a href=\"https://example.com\" rel=\"nofollow noreferrer noopener\" target=\"_blank\">x</a>");
        assert_eq!(run("<a href=\"https://gitlab.com/x\">x</a>"), "<a href=\"https://gitlab.com/x\">x</a>");
        assert_eq!(run("<a href=\"docs/x.md\">x</a>"), "<a href=\"docs/x.md\">x</a>");
        assert_eq!(run("<a href=\"#x\">x</a>"), "<a href=\"#x\">x</a>");
        assert_eq!(run("<a href=\"mailto:a@b.c\">x</a>"), "<a href=\"mailto:a@b.c\">x</a>");
        assert_eq!(run("<a href=\"//cdn.example\">x</a>"), "<a href=\"//cdn.example\" rel=\"nofollow noreferrer noopener\" target=\"_blank\">x</a>");
        assert_eq!(run("<a href=\"HTTP://e.com:80/p\">x</a>"), "<a href=\"http://e.com/p\" rel=\"nofollow noreferrer noopener\" target=\"_blank\">x</a>");
        // Not parsable by Ruby's URI: GitLab still adds nofollow.
        assert_eq!(run("<a href=\"a|b\">x</a>"), "<a href=\"a|b\" rel=\"nofollow noreferrer noopener\" target=\"_blank\">x</a>");
        assert_eq!(run("<a href=\"https://e.com\" rel=\"license\">x</a>"), "<a href=\"https://e.com\" rel=\"nofollow noreferrer noopener license\" target=\"_blank\">x</a>");
        assert_eq!(
            run("<a href=\"https://e.com/%E2%80%AE\">a\u{202E}b</a>"),
            "<a href=\"https://e.com/%E2%80%AE\" class=\"has-tooltip\" title=\"https://e.com/%E2%80%AE\" rel=\"nofollow noreferrer noopener\" target=\"_blank\">a%E2%80%AEb</a>"
        );
    }

    #[test]
    fn ruby_uri() {
        assert_eq!(ruby_uri_to_s("https://x.y/a?b=c#d").as_deref(), Some("https://x.y/a?b=c#d"));
        assert_eq!(ruby_uri_to_s("a#b#c"), None);
        assert_eq!(ruby_uri_to_s("ünï"), None);
        assert_eq!(ruby_uri_to_s("a:b/c").as_deref(), Some("a:b/c"));
        assert_eq!(ruby_uri_to_s("x%zz"), None);
        assert_eq!(ruby_uri_to_s("http://[::1]:8080/").as_deref(), Some("http://[::1]:8080/"));
    }

    #[test]
    fn task_list_labels() {
        let html = "<ul><li><input type=\"checkbox\" class=\"task-list-item-checkbox\" data-inapplicable=\"\" disabled=\"\"> skip <em>it</em>\n<ul><li>n</li></ul></li></ul>";
        assert_eq!(
            apply(html, task_list),
            "<ul><li><input type=\"checkbox\" class=\"task-list-item-checkbox\" data-inapplicable=\"\" disabled=\"\" aria-label=\"Check option: skip it\"> <s class=\"inapplicable\"> skip </s><em><s class=\"inapplicable\">it</s></em>\n<ul><li>n</li></ul></li></ul>"
        );
        let table = "<table><thead><tr><th>Col</th><th>B</th></tr></thead><tbody><tr><td>x</td>\
                     <td class=\"task-table-item\"><input type=\"checkbox\" class=\"task-list-item-checkbox\"> </td></tr></tbody></table>";
        assert!(apply(table, task_list).contains("aria-label=\"Check option in column &quot;B&quot;\""));
        assert!(apply("<li><input class=\"task-list-item-checkbox\"> </li>", task_list).contains("aria-label=\"Check option\""));
    }

    #[test]
    fn truncation() {
        let s = "word ".repeat(30);
        let t = truncate_100(s.trim());
        assert!(t.ends_with("word…"), "{t}");
        assert!(t.chars().count() <= 100);
        assert_eq!(truncate_100("short"), "short");
        let long: String = "x".repeat(150);
        assert_eq!(truncate_100(&long).chars().count(), 100);
    }

    #[test]
    fn direction_top_level_only() {
        assert_eq!(
            apply("<p>a</p><div><p>b</p></div><ul class=\"section-nav\"></ul><ul></ul><hr>", set_direction),
            "<p dir=\"auto\">a</p><div><p>b</p></div><ul class=\"section-nav\"></ul><ul dir=\"auto\"></ul><hr>"
        );
    }
}
