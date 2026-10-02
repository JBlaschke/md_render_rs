//! GitLab's sanitization boundary (GL §5.2): `IframeToImgFilter`, `SanitizationFilter`
//! (html-pipeline 2.14.3 `ALLOWLIST` + `BaseSanitizationFilter` + `SanitizationFilter`, run by
//! Sanitize 7.0.0) and `SanitizeLinkFilter`.
//!
//! The traversal reproduces `Sanitize#traverse` (top-down, children re-scanned after a node is
//! unlinked or reparented) and the transformer order: GitLab's custom transformers first (the
//! namespaced-attribute remover is unshifted to the front), then `CleanElement`, then
//! `CleanComment`.

use std::fmt::Write as _;
use std::sync::LazyLock;

use dom_query::{NodeData, NodeRef};
use regex::Regex;

use super::dom;

/// `ALLOWLIST[:elements]` + GitLab additions (`span details summary abbr section input`).
const ELEMENTS: &[&str] = &[
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "h7",
    "h8",
    "br",
    "b",
    "i",
    "strong",
    "em",
    "a",
    "pre",
    "code",
    "img",
    "tt",
    "div",
    "ins",
    "del",
    "sup",
    "sub",
    "p",
    "ol",
    "ul",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "blockquote",
    "dl",
    "dt",
    "dd",
    "kbd",
    "q",
    "samp",
    "var",
    "hr",
    "ruby",
    "rt",
    "rp",
    "li",
    "tr",
    "td",
    "th",
    "s",
    "strike",
    "summary",
    "details",
    "caption",
    "figure",
    "figcaption",
    "abbr",
    "bdo",
    "cite",
    "dfn",
    "mark",
    "small",
    "span",
    "time",
    "wbr",
    "section",
    "input",
];

/// `ALLOWLIST[:attributes][:all]` minus `name`, plus `data-sourcepos` and `data-escaped-char`.
const ALL_ATTRS: &[&str] = &[
    "abbr",
    "accept",
    "accept-charset",
    "accesskey",
    "action",
    "align",
    "alt",
    "aria-describedby",
    "aria-hidden",
    "aria-label",
    "aria-labelledby",
    "axis",
    "border",
    "cellpadding",
    "cellspacing",
    "char",
    "charoff",
    "charset",
    "checked",
    "clear",
    "cols",
    "colspan",
    "color",
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
    "ismap",
    "label",
    "lang",
    "maxlength",
    "media",
    "method",
    "multiple",
    "nohref",
    "noshade",
    "nowrap",
    "open",
    "progress",
    "prompt",
    "readonly",
    "rel",
    "rev",
    "role",
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
    "target",
    "title",
    "type",
    "usemap",
    "valign",
    "value",
    "vspace",
    "width",
    "itemprop",
    "data-sourcepos",
    "data-escaped-char",
];

/// Per-element attribute lists after all of GitLab's customisations. Note that
/// `allow_class_attributes` *assigns* `div`/`p`/`ul`/`ol`/`input` (dropping `itemscope` and
/// `itemtype` from `div`), and `allow_id_attributes` assigns `li` and `h1`-`h6`.
fn element_attrs(name: &str) -> &'static [&'static str] {
    match name {
        "a" => &[
            "href",
            "name",
            "data-wikilink",
            "data-placeholder",
            "id",
            "class",
            "data-footnote-ref",
            "data-footnote-backref",
            "data-footnote-backref-idx",
            "data-heading-content",
        ],
        "img" => &[
            "src",
            "longdesc",
            "data-diagram",
            "data-diagram-src",
            "data-placeholder",
            "class",
        ],
        "div" | "p" | "ul" | "ol" => &["class"],
        "blockquote" | "del" | "ins" | "q" => &["cite"],
        "span" => &["data-math-style", "data-placeholder", "class"],
        "code" => &["data-math-style", "class"],
        "pre" => &[
            "data-canonical-lang",
            "data-lang-params",
            "data-math-style",
            "data-mermaid-style",
            "data-kroki-style",
        ],
        "abbr" => &["title"],
        "th" | "td" => &["style", "class"],
        "table" => &[
            "data-table-fields",
            "data-table-filter",
            "data-table-markdown",
        ],
        "li" => &["id", "class"],
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => &["id"],
        "input" => &[
            "class",
            "data-inapplicable",
            "data-indeterminate",
            "data-checkbox-sourcepos",
        ],
        "section" => &["data-footnotes"],
        _ => &[],
    }
}

/// Protocol allowlists (`:relative` is allowed for all of them). `a[href]` is unrestricted here;
/// SanitizeLinkFilter handles it.
fn protocols(element: &str, attr: &str) -> Option<&'static [&'static str]> {
    match (element, attr) {
        ("blockquote" | "del" | "ins" | "q", "cite") | ("img", "src" | "longdesc") => {
            Some(&["http", "https"])
        }
        _ => None,
    }
}

/// Sanitize's default `whitespace_elements`.
const WHITESPACE_ELEMENTS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "dd",
    "div",
    "dl",
    "dt",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "ul",
];

/// `IframeToImgFilter` (gitlab.com has `iframe_rendering_enabled`): `<iframe src>` becomes a bare
/// `<img src [width] [height]>` before sanitization.
pub(super) fn iframe_to_img(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["iframe"]) {
        let iframe = root.tree.get_unchecked(&id);
        let Some(src) = dom::attr(&iframe, "src") else {
            continue;
        };
        let img = dom::new_element(root.tree, "img", &[("src", &src)]);
        for a in ["width", "height"] {
            if let Some(v) = dom::attr(&iframe, a) {
                img.set_attr(a, &v);
            }
        }
        iframe.insert_before(&img);
        iframe.remove_from_parent();
    }
}

/// `SanitizationFilter` (GL §5.2).
pub(super) fn sanitize(root: &NodeRef<'_>) {
    lift_template_contents(root);
    // Iterative version of `Sanitize#traverse`.
    struct Frame {
        parent: dom_query::NodeId,
        child: Option<dom_query::NodeId>,
        prev: Option<dom_query::NodeId>,
    }
    let tree = root.tree;
    let mut stack: Vec<Frame> = Vec::new();
    transform(&tree.get_unchecked(&root.id), root);
    stack.push(Frame {
        parent: root.id,
        child: root.first_child().map(|c| c.id),
        prev: None,
    });
    while let Some(frame) = stack.last_mut() {
        let Some(child_id) = frame.child else {
            stack.pop();
            // Advance the parent frame past the child we just finished.
            advance(&mut stack, tree);
            continue;
        };
        let child = tree.get_unchecked(&child_id);
        frame.prev = child.prev_sibling().map(|p| p.id);
        transform(&child, root);
        // Descend into the (possibly modified) child.
        stack.push(Frame {
            parent: child_id,
            child: child.first_child().map(|c| c.id),
            prev: None,
        });
    }

    fn advance(stack: &mut [Frame], tree: &dom_query::Tree) {
        let Some(frame) = stack.last_mut() else {
            return;
        };
        let Some(child_id) = frame.child else { return };
        let child = tree.get_unchecked(&child_id);
        let still_child = child.parent().is_some_and(|p| p.id == frame.parent);
        frame.child = if still_child {
            child.next_sibling().map(|n| n.id)
        } else {
            match frame.prev {
                Some(prev) => tree.get_unchecked(&prev).next_sibling().map(|n| n.id),
                None => tree
                    .get_unchecked(&frame.parent)
                    .first_child()
                    .map(|n| n.id),
            }
        };
    }
}

/// Nokogiri keeps `<template>` contents as ordinary children; html5ever stores them in a separate
/// fragment. Move them back so they are sanitized and kept like on GitLab.
fn lift_template_contents(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["template"]) {
        let t = root.tree.get_unchecked(&id);
        let contents = t.element_ref().and_then(|el| el.template_contents);
        if let Some(cid) = contents {
            let frag = root.tree.get_unchecked(&cid);
            while let Some(c) = frag.first_child() {
                t.append_child(&c);
            }
        }
    }
}

/// Runs every transformer on one node, in Sanitize's order.
fn transform(node: &NodeRef<'_>, root: &NodeRef<'_>) {
    if node.id == root.id {
        return;
    }
    if node.is_comment() {
        // CleanComment (`allow_comments` is nil for repository blobs and issues).
        node.remove_from_parent();
        return;
    }
    if node.is_doctype()
        || node.query_or(false, |n| {
            matches!(n.data, NodeData::ProcessingInstruction { .. })
        })
    {
        node.remove_from_parent();
        return;
    }
    let Some(name) = dom::tag(node) else { return };
    let name = name.to_ascii_lowercase();
    remove_namespaced_attributes(node);
    // html-pipeline: top-level <li> and table parts outside a <table> are unwrapped.
    if name == "li" && !dom::has_ancestor(node, &["ul", "ol"], root) {
        dom::unwrap(node);
    }
    if matches!(
        name.as_str(),
        "thead" | "tbody" | "tfoot" | "tr" | "td" | "th"
    ) && !dom::has_ancestor(node, &["table"], root)
    {
        dom::unwrap(node);
    }
    if name == "a" {
        // remove_rel
        if !dom::attr_is(node, "rel", "license") {
            node.remove_attr("rel");
        }
        // unwrap_nested_a
        if dom::has_ancestor(node, &["a"], root) {
            dom::unwrap(node);
        }
    }
    remove_unsafe_table_style(node, &name);
    filter_json_table_fields(node);
    remove_id_attributes(node, &name);
    remove_unsafe_classes(node, &name);
    if name == "input" && !is_task_list_input(node) {
        node.remove_from_parent();
    }
    clean_element(node, &name);
}

fn remove_namespaced_attributes(node: &NodeRef<'_>) {
    node.update(|tn| {
        if let NodeData::Element(el) = &mut tn.data {
            el.attrs.retain(|a| a.name.ns.is_empty());
        }
    });
}

fn remove_unsafe_table_style(node: &NodeRef<'_>, name: &str) {
    if !matches!(name, "th" | "td") {
        return;
    }
    let Some(style) = dom::attr(node, "style") else {
        return;
    };
    // TABLE_ALIGNMENT_PATTERN = /text-align: (?<alignment>center|left|right)/ (unanchored)
    let found = ["center", "left", "right"]
        .iter()
        .filter_map(|a| style.find(&format!("text-align: {a}")).map(|i| (i, *a)))
        .min_by_key(|(i, _)| *i);
    match found {
        Some((_, a)) => node.set_attr("style", &format!("text-align: {a}")),
        None => node.remove_attr("style"),
    }
}

fn filter_json_table_fields(node: &NodeRef<'_>) {
    let Some(raw) = dom::attr(node, "data-table-fields") else {
        return;
    };
    let parsed: Option<Vec<serde_json::Map<String, serde_json::Value>>> =
        serde_json::from_str(&raw).ok();
    match parsed {
        Some(fields) => {
            let sliced: Vec<serde_json::Map<String, serde_json::Value>> = fields
                .into_iter()
                .map(|f| {
                    f.into_iter()
                        .filter(|(k, _)| matches!(k.as_str(), "key" | "label" | "sortable"))
                        .collect()
                })
                .collect();
            let json = serde_json::to_string(&sliced).unwrap_or_default();
            node.set_attr("data-table-fields", &json);
        }
        None => node.remove_attr("data-table-fields"),
    }
}

fn remove_id_attributes(node: &NodeRef<'_>, name: &str) {
    let Some(id) = dom::attr(node, "id") else {
        return;
    };
    let keep = match name {
        "a" => id.starts_with("fnref-"),
        "li" => id.starts_with("fn-"),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => id.starts_with("user-content-"),
        _ => true,
    };
    if !keep {
        node.remove_attr("id");
    }
}

const ALERT_CLASSES: &[&str] = &[
    "markdown-alert markdown-alert-note",
    "markdown-alert markdown-alert-tip",
    "markdown-alert markdown-alert-important",
    "markdown-alert markdown-alert-warning",
    "markdown-alert markdown-alert-caution",
];

fn remove_unsafe_classes(node: &NodeRef<'_>, name: &str) {
    let Some(class) = dom::attr(node, "class") else {
        return;
    };
    let only = |allowed: &[&str]| class.split_ascii_whitespace().all(|c| allowed.contains(&c));
    let remove = match name {
        "a" => class != "anchor",
        "div" => !ALERT_CLASSES.contains(&class.as_str()),
        "p" => class != "markdown-alert-title",
        "span" => {
            !class.contains("idiff") || !only(&["idiff", "left", "right", "deletion", "addition"])
        }
        "code" => class != "idiff",
        "ul" | "ol" => class != "task-list",
        "li" => class != "task-list-item" && class != "inapplicable task-list-item",
        "th" | "td" => class != "task-table-item",
        "img" => !only(&["glfm-float-right", "glfm-float-left", "glfm-float-none"]),
        "input" => class != "task-list-item-checkbox",
        _ => false,
    };
    if remove {
        node.remove_attr("class");
    }
}

fn is_task_list_input(node: &NodeRef<'_>) -> bool {
    dom::attr_is(node, "type", "checkbox")
        && dom::attr_is(node, "class", "task-list-item-checkbox")
        && node
            .parent()
            .is_some_and(|p| dom::is_any(&p, &["li", "td", "th"]))
}

/// `Sanitize::Transformers::CleanElement`.
fn clean_element(node: &NodeRef<'_>, name: &str) {
    if node.parent().is_none() {
        return;
    }
    if !ELEMENTS.contains(&name) {
        let tree = node.tree;
        let has_children = node.first_child().is_some();
        if WHITESPACE_ELEMENTS.contains(&name) {
            node.insert_before(&dom::new_text(tree, " "));
            if has_children {
                node.insert_after(&dom::new_text(tree, " "));
            }
        }
        if has_children && name != "script" {
            while let Some(c) = node.first_child() {
                node.insert_before(&c);
            }
        }
        node.remove_from_parent();
        return;
    }
    let per_element = element_attrs(name);
    let protocol_rules = |attr: &str| protocols(name, attr);
    node.update(|tn| {
        let NodeData::Element(el) = &mut tn.data else {
            return;
        };
        el.attrs.retain_mut(|a| {
            let attr_name: String = a.name.local.as_ref().to_ascii_lowercase();
            if !ALL_ATTRS.contains(&attr_name.as_str())
                && !per_element.contains(&attr_name.as_str())
            {
                return false;
            }
            if let Some(allowed) = protocol_rules(&attr_name) {
                if let Some(proto) = protocol_prefix(&a.value)
                    && !allowed.contains(&proto.to_lowercase().as_str())
                {
                    return false;
                }
                let stripped = a.value.trim_matches(is_ruby_space).to_owned();
                if stripped.len() != a.value.len() {
                    a.value.clear();
                    a.value.push_slice(&stripped);
                }
            }
            // Sanitize's libxml2 workaround: escape ' ' and '"' in href/src/action and a[name].
            let libxml_attr = matches!(attr_name.as_str(), "href" | "src" | "action")
                || (name == "a" && attr_name == "name");
            if libxml_attr && a.value.contains([' ', '"']) {
                let escaped = a.value.replace(' ', "%20").replace('"', "%22");
                a.value.clear();
                a.value.push_slice(&escaped);
            }
            true
        });
    });
}

fn is_ruby_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r' | '\0')
}

/// Sanitize's `REGEX_PROTOCOL` = `/\A\s*([^\/#]*?)(?::|&#0*58|&#x0*3a)/i`; returns group 1.
fn protocol_prefix(value: &str) -> Option<&str> {
    let start = value.len()
        - value
            .trim_start_matches([' ', '\t', '\n', '\x0B', '\x0C', '\r'])
            .len();
    let rest = &value[start..];
    let lower = rest.to_ascii_lowercase();
    for (i, c) in rest.char_indices() {
        if c == '/' || c == '#' {
            return None;
        }
        let tail = &lower[i..];
        if tail.starts_with(':') || entity_colon(tail) {
            return Some(&rest[..i]);
        }
    }
    None
}

/// `&#0*58` or `&#x0*3a` at the start of `s` (already lower-cased).
fn entity_colon(s: &str) -> bool {
    if let Some(r) = s.strip_prefix("&#x") {
        r.trim_start_matches('0').starts_with("3a")
    } else if let Some(r) = s.strip_prefix("&#") {
        r.trim_start_matches('0').starts_with("58")
    } else {
        false
    }
}

/// `SanitizeLinkFilter` (GL §5.2): strip `href`/`src`/`data-src`/`data-canonical-src` and remove
/// them when the URL is unparsable or uses `data:`, `javascript:` or `vbscript:`.
pub(super) fn sanitize_links(root: &NodeRef<'_>) -> bool {
    const ATTRS: [&str; 4] = ["href", "src", "data-src", "data-canonical-src"];
    let ids: Vec<_> = dom::descendants(root)
        .into_iter()
        .filter(|id| {
            let n = root.tree.get_unchecked(id);
            n.is_element() && ATTRS.iter().any(|a| n.has_attr(a))
        })
        .collect();
    if ids.len() > 10_000 {
        // MAX_LINK_ATTRIBUTES: GitLab aborts with TIMEOUT_MARKDOWN_MESSAGE.
        return false;
    }
    for id in ids {
        let n = root.tree.get_unchecked(&id);
        for a in ATTRS {
            let Some(v) = dom::attr(&n, a) else { continue };
            let v = dom::ruby_strip(&v).to_owned();
            n.set_attr(a, &v);
            if !permit_url(&v) {
                n.remove_attr(a);
            }
        }
    }
    true
}

/// `SanitizeNodeLink#permit_url?`: Addressable parse + validation, `normalize`, then the
/// unsafe-scheme check.
pub(super) fn permit_url(url: &str) -> bool {
    match parse_uri(url) {
        Err(()) => false,
        Ok(parts) if !normalize_succeeds(&parts) => false,
        Ok(parts) => match parts.scheme {
            None => true,
            Some(scheme) => {
                let scheme: String = scheme
                    .trim()
                    .to_ascii_lowercase()
                    .chars()
                    .filter(|c| c.is_ascii_alphabetic() || matches!(c, '+' | '.' | '-'))
                    .collect();
                !matches!(scheme.as_str(), "data" | "javascript" | "vbscript")
            }
        },
    }
}

/// The parts of a URI that matter for GitLab's checks.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct UriParts<'a> {
    pub scheme: Option<&'a str>,
    pub authority: Option<&'a str>,
    pub host: Option<&'a str>,
    pub port: Option<&'a str>,
    pub path: &'a str,
}

/// Approximation of `Addressable::URI.parse` + `#validate` (RFC 3986 appendix B split, scheme
/// format, port digits, invalid host characters, missing hierarchical segment for IP-based
/// schemes). `Err` means Addressable would raise `InvalidURIError`.
pub(super) fn parse_uri(url: &str) -> Result<UriParts<'_>, ()> {
    // URIREGEX = /^(([^:\/?#]+):)?(\/\/([^\/?#]*))?([^?#]*)(\?([^#]*))?(#(.*))?$/. The negated
    // classes of the scheme, authority, path and query groups match newlines too; only the
    // fragment's `.` stops at one (and `$` then matches there), and the fragment is not
    // validated. So the whole string is split: `jav\nascript:x` has the scheme `jav\nascript`,
    // which fails the scheme format check, like Addressable's `InvalidURIError`.
    let mut rest = url;
    let mut parts = UriParts::default();
    if let Some(i) = rest.find([':', '/', '?', '#'])
        && rest.as_bytes()[i] == b':'
        && i > 0
    {
        parts.scheme = Some(&rest[..i]);
        rest = &rest[i + 1..];
    }
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        let authority = &after[..end];
        parts.authority = Some(authority);
        rest = &after[end..];
        // host = authority without userinfo and port
        let hostport = match authority.rfind('@') {
            Some(i) if !authority[..i].contains(['[', ']']) => &authority[i + 1..],
            _ => authority,
        };
        let (host, port) = match hostport.rfind(':') {
            Some(i) if !hostport[i + 1..].contains([':', '@', '[', ']']) => {
                (&hostport[..i], Some(&hostport[i + 1..]))
            }
            _ => (hostport, None),
        };
        parts.host = Some(host);
        parts.port = port.filter(|p| !p.is_empty());
    }
    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    parts.path = &rest[..path_end];

    if let Some(s) = parts.scheme {
        let mut chars = s.chars();
        let ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'));
        if !ok {
            return Err(());
        }
    }
    if let Some(p) = parts.port
        && !p.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(());
    }
    if let Some(h) = parts.host
        && (h.contains(['<', '>', '{', '}', '/', '\\', '?', '#', '@', '"'])
            || h.chars().any(char::is_whitespace))
    {
        return Err(());
    }
    if parts.host.is_none() && parts.path.starts_with("//") {
        return Err(());
    }
    const IP_BASED: &[&str] = &[
        "http", "https", "ftp", "tftp", "sftp", "ssh", "svn+ssh", "telnet", "nntp", "gopher",
        "wais", "ldap", "prospero",
    ];
    if let Some(s) = parts.scheme
        && IP_BASED.contains(&s.to_ascii_lowercase().as_str())
        && parts.host.is_none_or(str::is_empty)
        && parts.path.is_empty()
    {
        return Err(());
    }
    // `URI#to_s`, which `URI.new` calls: "Cannot assemble URI string with ambiguous path".
    if parts.scheme.is_none() && ambiguous_path(parts.path) {
        return Err(());
    }
    Ok(parts)
}

/// Ruby `path =~ Addressable::URI::NORMPATH` (`/^(?!\/)[^\/:]*:.*$/`): a scheme-less path whose
/// first segment holds a `:` (`:x`) would read as a scheme. `^` matches at every line start and
/// `[^\/:]` also matches line breaks, so it is enough that some line not starting with `/`
/// reaches a `:` before any `/` (`a/\n:b`).
fn ambiguous_path(path: &str) -> bool {
    let b = path.as_bytes();
    let mut next_special = None;
    for i in (0..b.len()).rev() {
        if matches!(b[i], b'/' | b':') {
            next_special = Some(b[i]);
        }
        let line_start = i == 0 || b[i - 1] == b'\n';
        if line_start && b[i] != b'/' && next_special == Some(b':') {
            return true;
        }
    }
    false
}

/// Longest path [`normalize_succeeds`] normalizes. Addressable's dot-segment removal is
/// quadratic in the worst case; a longer (hostile) path is kept as Addressable parsed it.
const MAX_NORMALIZED_PATH: usize = 4096;

/// Whether `Addressable::URI#normalize` succeeds (GL-SAN-1). It rebuilds the URI from the
/// normalized parts, and the normalized path (percent-decoded where PCHAR allows, dot segments
/// removed) may fail the checks the raw one passed: a scheme-less path can become ambiguous
/// (`javascript%3Aalert(1)` -> `javascript:alert(1)`, `./a:b` -> `a:b`), and one without an
/// authority can start with `//` ("Cannot have a path with two leading slashes": `/.//x`,
/// `.///`). Checked against Addressable 2.8.7 (GitLab locks 2.9.0, which changes none of
/// this). Not modelled: normalisation failures of the host (`//%3A%2E`).
fn normalize_succeeds(parts: &UriParts<'_>) -> bool {
    let path = parts.path;
    let lower = path.to_ascii_lowercase();
    let may_be_ambiguous = parts.scheme.is_none() && (path.contains(':') || lower.contains("%3a"));
    // Only dot segments and `strip` change a path's shape: `./` + `//x` and ` //x` (after a
    // scheme) become `//x`.
    let may_double_slash = parts.authority.is_none()
        && path.contains('/')
        && (path.contains('.') || lower.contains("%2e") || dom::ruby_strip(path) != path);
    if !(may_be_ambiguous || may_double_slash) || path.len() > MAX_NORMALIZED_PATH {
        return true;
    }
    let normalized = normalized_path(path);
    !(may_be_ambiguous && ambiguous_path(&normalized)
        || may_double_slash && normalized.starts_with("//"))
}

/// Addressable `normalized_path`, up to the empty-path default (`/` for http and friends,
/// which cannot matter here): `strip`, each segment through `normalize_component(PCHAR)`,
/// then `URI.normalize_path`.
fn normalized_path(path: &str) -> String {
    let segments: Vec<String> = dom::ruby_strip(path)
        .split('/')
        .map(|segment| {
            // `normalize_component`: decode every `%XX`, re-encode what is not PCHAR.
            let mut out = String::with_capacity(segment.len());
            for b in percent_encoding::percent_decode_str(segment) {
                if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&b) {
                    out.push(char::from(b));
                } else {
                    let _ = write!(out, "%{b:02X}");
                }
            }
            out
        })
        .collect();
    normalize_dot_segments(&segments.join("/"))
}

/// `Addressable::URI.normalize_path`: RFC 3986 §5.2.4 the Addressable way. Each round applies
/// the first rule that changes something (`mod ||= gsub!(...)`), until none does.
fn normalize_dot_segments(path: &str) -> String {
    static RULE_2A: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/\./|/\.$").unwrap());
    static RULE_2B_2C: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"/([^/]*)/\.\./|/([^/]*)/\.\.$").unwrap());
    static RULE_2D: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\.\.?/?").unwrap());
    static RULE_PREFIXED_PARENT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^/\.\.?/|^(/\.\.?)+/?$").unwrap());
    let gsub = |re: &Regex, p: &str, with: &str| {
        re.is_match(p).then(|| re.replace_all(p, with).into_owned())
    };
    let mut p = path.to_owned();
    loop {
        if let Some(next) = gsub(&RULE_2A, &p, "/") {
            p = next;
            continue;
        }
        if let Some(pair) = RULE_2B_2C.captures(&p) {
            // gsub!(/\/#{parent}\/\.\.\/|(\/#{current}\/\.\.$)/, '/'), the missing one being ''.
            let parent = format!("/{}/../", pair.get(1).map_or("", |m| m.as_str()));
            let current = format!("/{}/..", pair.get(2).map_or("", |m| m.as_str()));
            let mut next = String::with_capacity(p.len());
            let mut rest = p.as_str();
            while let Some(c) = rest.chars().next() {
                if rest.starts_with(&parent) {
                    next.push('/');
                    rest = &rest[parent.len()..];
                } else if rest == current {
                    next.push('/');
                    rest = "";
                } else {
                    next.push(c);
                    rest = &rest[c.len_utf8()..];
                }
            }
            if next != p {
                p = next;
                continue;
            }
        }
        if let Some(next) = gsub(&RULE_2D, &p, "") {
            p = next;
            continue;
        }
        if let Some(next) = gsub(&RULE_PREFIXED_PARENT, &p, "/") {
            p = next;
            continue;
        }
        return p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dom_query::Document;

    fn run(html: &str) -> String {
        let doc = Document::fragment(html);
        let root = doc.html_root();
        iframe_to_img(&root);
        sanitize(&root);
        sanitize_links(&root);
        root.inner_html().to_string()
    }

    #[test]
    fn script_removed_with_contents_style_text_kept() {
        assert_eq!(run("<p>a<script>alert(1)</script>b</p>"), "<p>ab</p>");
        assert_eq!(run("<style>a > b</style>"), "a &gt; b");
    }

    #[test]
    fn unknown_elements_unwrapped_with_whitespace() {
        assert_eq!(run("<u>x</u><center>c</center>"), "xc");
        assert_eq!(run("a<article>b</article>c"), "a b c");
        assert_eq!(run("a<nav></nav>c"), "a c");
        assert_eq!(run("<form><button>Go</button></form>"), "Go");
    }

    #[test]
    fn comments_removed() {
        assert_eq!(run("<p>a<!-- x -->b</p>"), "<p>ab</p>");
    }

    #[test]
    fn attributes_filtered() {
        assert_eq!(
            run("<p style=\"color:red\" onclick=\"x()\" align=\"right\" class=\"x\">t</p>"),
            "<p align=\"right\">t</p>"
        );
        assert_eq!(
            run("<div class=\"markdown-alert markdown-alert-note\" itemscope=\"\">t</div>"),
            "<div class=\"markdown-alert markdown-alert-note\">t</div>"
        );
        assert_eq!(
            run("<a name=\"x y\" href=\"a b\">t</a>"),
            "<a name=\"x%20y\" href=\"a%20b\">t</a>"
        );
        assert_eq!(
            run("<span class=\"idiff left addition\">t</span>"),
            "<span class=\"idiff left addition\">t</span>"
        );
        assert_eq!(run("<span class=\"idiff foo\">t</span>"), "<span>t</span>");
    }

    #[test]
    fn ids_and_rel() {
        assert_eq!(run("<h2 id=\"x\">t</h2>"), "<h2>t</h2>");
        assert_eq!(
            run("<h2 id=\"user-content-x\">t</h2>"),
            "<h2 id=\"user-content-x\">t</h2>"
        );
        assert_eq!(
            run("<a id=\"fnref-1\" rel=\"nofollow\" href=\"#x\">t</a>"),
            "<a id=\"fnref-1\" href=\"#x\">t</a>"
        );
        assert_eq!(
            run("<a rel=\"license\" href=\"/l\">t</a>"),
            "<a rel=\"license\" href=\"/l\">t</a>"
        );
    }

    #[test]
    fn table_style_reduced() {
        assert_eq!(
            run(
                "<table><tr><td style=\"color: red; text-align: center\">x</td><td style=\"color:red\">y</td></tr></table>"
            ),
            "<table><tbody><tr><td style=\"text-align: center\">x</td><td>y</td></tr></tbody></table>"
        );
    }

    #[test]
    fn inputs_only_task_checkboxes() {
        assert_eq!(run("<p><input type=\"text\">x</p>"), "<p>x</p>");
        assert_eq!(
            run(
                "<ul><li><input type=\"checkbox\" class=\"task-list-item-checkbox\" disabled=\"\"> x</li></ul>"
            ),
            "<ul><li><input type=\"checkbox\" class=\"task-list-item-checkbox\" disabled=\"\"> x</li></ul>"
        );
        assert_eq!(
            run("<p><input type=\"checkbox\" class=\"task-list-item-checkbox\">x</p>"),
            "<p>x</p>"
        );
    }

    #[test]
    fn protocols() {
        assert_eq!(
            run("<img src=\"data:image/png;base64,AA\" alt=\"a\">"),
            "<img alt=\"a\">"
        );
        assert_eq!(
            run("<img src=\" https://x/y.png \">"),
            "<img src=\"https://x/y.png\">"
        );
        assert_eq!(run("<a href=\"javascript:alert(1)\">x</a>"), "<a>x</a>");
        assert_eq!(
            run("<a href=\"JaVa&#x09;script:alert(1)\">x</a>"),
            "<a>x</a>"
        );
        assert_eq!(run("<a href=\"vbscript:x\">x</a>"), "<a>x</a>");
        assert_eq!(
            run("<a href=\"mailto:a@b.c\">x</a>"),
            "<a href=\"mailto:a@b.c\">x</a>"
        );
        assert_eq!(run("<a href=\"http://h:x/\">x</a>"), "<a>x</a>");
        assert_eq!(run("<a href=\"http:\">x</a>"), "<a>x</a>");
        assert_eq!(
            run("<blockquote cite=\"ftp://x\">q</blockquote>"),
            "<blockquote>q</blockquote>"
        );
    }

    #[test]
    fn nested_links_and_stray_parts() {
        assert_eq!(
            run("<a href=\"/a\">x<svg><a href=\"/b\">y</a></svg></a>"),
            "<a href=\"/a\">xy</a>"
        );
        assert_eq!(run("<li>x</li>"), "x");
        assert_eq!(run("<div><td>x</td></div>"), "<div>x</div>");
    }

    #[test]
    fn namespaced_attributes_removed() {
        assert_eq!(
            run("<svg><a xlink:href=\"javascript:x\">t</a></svg>"),
            "<a>t</a>"
        );
    }

    #[test]
    fn iframes_become_images() {
        assert_eq!(
            run("<iframe src=\"https://e.com/x\" width=\"5\" height=\"6\">x</iframe>"),
            "<img src=\"https://e.com/x\" width=\"5\" height=\"6\">"
        );
        assert_eq!(run("<iframe>x</iframe>"), "x");
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let html = "<div>".repeat(4_000) + "x" + &"</div>".repeat(4_000);
        let out = run(&html);
        assert!(out.contains('x'));
        let html = "<font>".repeat(20_000) + "x";
        assert_eq!(run(&html), "x");
    }

    #[test]
    fn uri_parsing() {
        assert!(parse_uri("http://example.com/x").is_ok());
        assert!(parse_uri("relative/path").is_ok());
        assert!(parse_uri("#frag").is_ok());
        assert!(parse_uri("a b:c").is_err());
        assert!(parse_uri("https://").is_err());
        assert!(parse_uri("http://x:80/").is_ok());
        assert!(parse_uri("http://x:8a/").is_err());
        assert!(permit_url("mailto:x@y"));
        assert!(!permit_url("data:text/html,x"));
    }

    /// GL-SAN-1: `Addressable::URI.parse(url).normalize` raises for scheme-less references
    /// whose (normalized) first segment holds a `:`, and for paths that normalize to `//...`
    /// without an authority. Expected verdicts from Addressable 2.8.7.
    #[test]
    fn ambiguous_paths_are_removed_like_addressable() {
        let drop = [
            ":x",
            ":",
            "::",
            ":/x",
            "javascript%3Aalert(1)",
            "JaVaScRiPt%3Ax",
            "a%3Ab",
            "a%3ab",
            "x%3A",
            "%3A",
            "%3A/x",
            "./a%3Ab",
            "./a:b",
            "../a:b",
            "../x/../a:b",
            "..a%3Ab",
            "...a%3Ab",
            ".a%3Ab",
            "%2E%2E/a%3Ab",
            "%2e/a:b",
            "a%2Fb%3Ac",
            ".%3A",
            "/.//x",
            "/..//x",
            "/a/..//x",
            ".///",
            "..///x:",
            "%2E///%41",
            "http:/.//x",
            "http: //x",
            "x:\t//y",
            "a/\n:b",
            "/x\nb:c",
            "//h/x\nb:c",
            "a/\n\nb:c",
            "a%3Ab.md",
            "%41%3Ax",
        ];
        for url in drop {
            assert!(!permit_url(url), "{url:?} must be removed");
        }
        let keep = [
            "a/b%3Ac",
            "sub/../a%3Ab",
            "x/..%3A",
            "./",
            "././/x",
            "mailto:./a:b",
            "/a%3Ab",
            "//h/a%3Ab",
            "?a:b",
            "#a:b",
            "a?b:c",
            "docs/a%3Ab.md",
            "a/./b:c",
            "./x/./y:z",
            "x/../../a:b",
            "a/b/../../c:d",
            "/a/../b:c",
            "relative/path.md",
        ];
        for url in keep {
            assert!(permit_url(url), "{url:?} must be kept");
        }
        assert_eq!(normalized_path("x/../../a:b"), "x/a:b");
        assert_eq!(normalized_path("a/b/../../c:d"), "a/../c:d");
        assert_eq!(normalized_path("../x/../a%3Ab"), "a:b");
        assert_eq!(normalized_path("/.//x"), "//x");
        assert_eq!(normalized_path("a%2fb%41%zz"), "a%2FbA%25zz");
        assert_eq!(
            run("<a href=\"javascript%3Aalert(1)\">z</a><a href=\":x\">y</a>"),
            "<a>z</a><a>y</a>"
        );
    }

    /// SEC-1: a newline inside the scheme is part of Addressable's scheme (which is then
    /// invalid), not the end of the URL; browsers strip it and would see `javascript:`.
    #[test]
    fn newline_split_schemes_are_removed() {
        assert!(!permit_url("jav\nascript:alert(1)"));
        assert!(!permit_url("javascript\n:alert(1)"));
        assert!(!permit_url("\njavascript:alert(1)"));
        assert!(!permit_url("ht\ntp://x"));
        assert!(!permit_url("http://x\ny/"));
        // A newline after the scheme, in the path or query is fine for Addressable.
        assert!(permit_url("http://x/a\nb"));
        assert!(permit_url("http://x/?q=\nr"));
        assert!(permit_url("rel\npath"));
        for html in [
            "<a href=\"jav&#10;ascript:alert(1)\">x</a>",
            "<a href=\"jav&NewLine;ascript:alert(1)\">x</a>",
            "<a href=\"javascript&#10;:alert(1)\">x</a>",
            "<a href=\"jav&#x0A;ascript&#x0A;:alert(1)\">x</a>",
        ] {
            assert_eq!(run(html), "<a>x</a>", "{html}");
        }
        assert_eq!(
            run("<img src=\"jav&#10;ascript:alert(1)\">"),
            "<img>",
            "src is checked the same way"
        );
    }
}
