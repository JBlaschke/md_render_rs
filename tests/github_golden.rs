//! Golden tests for the GitHub flavor against real github.com output (DESIGN D18).
//!
//! The oracles are third-party files and are not committed; set `MD_RENDER_GOLDEN_DIR` to
//! the research directory (layout `github/{ctx,pages,comments,probes}`), e.g.
//!
//! ```sh
//! MD_RENDER_GOLDEN_DIR=/path/to/.dev/research cargo test --test github_golden -- --nocapture
//! ```
//!
//! Comparison is a normalizing DOM diff (CQ §3.7): attribute order, entity spelling, void
//! syntax, run ids, `data-identity`, camo URLs / `data-canonical-src`, `tabindex="-1"`, our
//! `data-sourcepos` / `data-mdr-key`, and whitespace-only text between blocks are ignored.
//! Inside code blocks only the text is compared unless `GOLDEN_STRICT_HIGHLIGHT=1` (syntax
//! highlighting is another module's job). Things an offline renderer cannot know (issue
//! labels, `#123` references, `@mentions`) are reduced to what we emit offline.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use dom_query::{Document, NodeData, NodeRef};
use md_render::flavor::{Flavor, Mode};
use md_render::render::{RenderOptions, github};
use regex::Regex;

const BUNDLE: &str = "/__/md-render/bundle";

fn golden_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("MD_RENDER_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".dev/research"));
    dir.join("github").is_dir().then_some(dir)
}

fn strict_highlight() -> bool {
    std::env::var("GOLDEN_STRICT_HIGHLIGHT").is_ok_and(|v| v == "1")
}

// ------------------------------------------------------------------------------------------
// canonical DOM

#[derive(Debug, Clone, PartialEq)]
enum C {
    E { name: String, attrs: BTreeMap<String, String>, kids: Vec<C> },
    T(String),
}

#[derive(Clone, Copy)]
struct Norm {
    /// Drop `dir="auto"` (API `mode=gfm` without context never emits it).
    drop_dir_auto: bool,
    strict_code: bool,
    /// Comment context: `#123` references become plain text (we never link them offline).
    refs_as_text: bool,
    /// API probes: octicons were replaced by a bare `<SVG>` placeholder.
    probe: bool,
}

const BLOCKS: &[&str] = &[
    "article", "blockquote", "dd", "details", "div", "dl", "dt", "h1", "h2", "h3", "h4", "h5",
    "h6", "hr", "li", "markdown-accessiblity-table", "mdr-attachment", "mdr-code", "ol", "p", "pre", "section",
    "summary", "table", "tbody", "td", "tfoot", "th", "thead", "tr", "ul", "themed-picture",
    "picture", "source", "video", "math-renderer",
];

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).unwrap())
}

fn local(n: &NodeRef<'_>) -> Option<String> {
    n.qual_name_ref().map(|q| q.local.to_string())
}

fn decode_camo(v: &str) -> Option<String> {
    let rest = v.strip_prefix("https://camo.githubusercontent.com/")?;
    let hexurl = rest.split('/').nth(1)?;
    let bytes: Option<Vec<u8>> = (0..hexurl.len() / 2)
        .map(|i| u8::from_str_radix(hexurl.get(2 * i..2 * i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

/// `private-user-images…/<n>-<uuid>.<ext>?jwt=…` (needs GitHub's lookup) back to the
/// `github.com/user-attachments/assets/<uuid>` URL that was written.
fn user_attachment(v: &str) -> Option<String> {
    static RE_PUI: OnceLock<Regex> = OnceLock::new();
    static RE_ASSETS: OnceLock<Regex> = OnceLock::new();
    let uuid = r"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})";
    if let Some(c) = re(&RE_ASSETS, &format!(r"^https://github\.com/[^/]+/[^/]+/assets/\d+/{uuid}$")).captures(v) {
        return Some(format!("https://github.com/user-attachments/assets/{}", &c[1]));
    }
    let c = re(&RE_PUI, r"^https://private-user-images\.githubusercontent\.com/\d+/\d+-([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.[A-Za-z0-9]+\?jwt=").captures(v)?;
    Some(format!("https://github.com/user-attachments/assets/{}", &c[1]))
}

fn norm_value(name: &str, v: &str) -> String {
    static RID: OnceLock<Regex> = OnceLock::new();
    static EMOJI: OnceLock<Regex> = OnceLock::new();
    let mut v = decode_camo(v).or_else(|| user_attachment(v)).unwrap_or_else(|| v.to_owned());
    // GitHub lowercases footnote *reference* ids (a bug we do not copy, GH §8.4).
    if name == "id" && v.starts_with("user-content-fnref-") {
        v = v.to_lowercase();
    }
    if name == "src" {
        let e = re(&EMOJI, r"^(?:https://github\.githubassets\.com/images/icons/emoji/|/__/md-render/bundle/github/emoji/)([a-z0-9_+-]+)\.png$");
        if let Some(c) = e.captures(&v) {
            v = format!("emoji:{}", &c[1]);
        }
    }
    if name == "class" {
        let mut parts: Vec<&str> = v.split_ascii_whitespace().collect();
        parts.sort_unstable();
        v = parts.join(" ");
    }
    re(&RID, "[0-9a-f]{32}").replace_all(&v, "RID").into_owned()
}

fn attrs_of(n: &NodeRef<'_>, norm: Norm) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for a in n.attrs() {
        let name = a.name.local.to_string();
        let value = a.value.to_string();
        match name.as_str() {
            "data-sourcepos" | "data-mdr-key" | "data-identity" | "data-canonical-src" => continue,
            // Issue hovercards need a lookup of the issue (GH §8.10); `data-animated-image`
            // needs GitHub's JS and is never emitted offline (GH §3.1).
            "data-hovercard-type" | "data-hovercard-url" | "data-animated-image" => continue,
            "rid" => {
                out.insert("data-run-id".to_owned(), "RID".to_owned());
                continue;
            }
            "tabindex" if value == "-1" => continue,
            "dir" if norm.drop_dir_auto && value == "auto" => continue,
            _ => {}
        }
        out.insert(name.clone(), norm_value(&name, &value));
    }
    out
}

/// Code text without one leading newline (dropped by the HTML parser after `<pre>` but not
/// after `<pre><code>`); plain blocks also lose their final newline (GH §7.1: highlighted
/// TOKENS already lack it, plain ESC keeps it).
fn code_text(n: &NodeRef<'_>, highlighted: bool) -> String {
    let t = n.text().to_string();
    let t = t.strip_prefix('\n').unwrap_or(&t);
    if highlighted { t.to_owned() } else { t.strip_suffix('\n').unwrap_or(t).to_owned() }
}

fn attachment_uuid(href: &str) -> Option<String> {
    href.strip_prefix("https://github.com/user-attachments/assets/").map(str::to_owned)
}

fn canon_children(n: &NodeRef<'_>, norm: Norm) -> Vec<C> {
    let mut kids: Vec<C> = Vec::new();
    for c in n.children() {
        for k in canon(&c, norm) {
            match (kids.last_mut(), k) {
                (Some(C::T(prev)), C::T(t)) => prev.push_str(&t),
                (_, k) => kids.push(k),
            }
        }
    }
    if norm.refs_as_text {
        // GitHub shortens same-repo `owner/repo#N` references to `#N` (needs the context).
        static REPO_REF: OnceLock<Regex> = OnceLock::new();
        static SHA: OnceLock<Regex> = OnceLock::new();
        let r = re(&REPO_REF, r"\b[\w.-]+/[\w.-]+(#\d+)");
        let sha = re(&SHA, r"\b(?:[\w.-]+/[\w.-]+@)?[0-9a-f]{7,40}\b");
        for k in &mut kids {
            if let C::T(t) = k {
                let shortened = r.replace_all(t, "$1");
                *t = sha.replace_all(&shortened, "COMMIT").into_owned();
            }
        }
    }
    let is_block = |c: Option<&C>| match c {
        None => true,
        Some(C::E { name, .. }) => BLOCKS.contains(&name.as_str()),
        Some(C::T(_)) => false,
    };
    let mut out = Vec::with_capacity(kids.len());
    for i in 0..kids.len() {
        if let C::T(t) = &kids[i] {
            if t.trim().is_empty()
                && is_block(if i == 0 { None } else { kids.get(i - 1) })
                && is_block(kids.get(i + 1))
            {
                continue;
            }
        }
        out.push(kids[i].clone());
    }
    out
}

fn canon(n: &NodeRef<'_>, norm: Norm) -> Vec<C> {
    if let Some(Some(t)) = n.query(|t| match &t.data {
        NodeData::Text { contents } => Some(contents.to_string()),
        _ => None,
    }) {
        return vec![C::T(t)];
    }
    let Some(name) = local(n) else { return Vec::new() };
    let class = n.attr("class").map(|c| c.to_string()).unwrap_or_default();
    let has = |c: &str| class.split_ascii_whitespace().any(|x| x == c);
    match name.as_str() {
        "tool-tip" => return Vec::new(),
        "a" if has("user-mention") => return vec![C::T(n.text().to_string())],
        "a" if has("commit-link") => return vec![C::T("COMMIT".to_owned())],
        // Without context the API keeps external videos; with one (our contexts) they go.
        "video" if norm.probe => return canon_children(n, norm),
        // Without context the API highlights mermaid/geojson/… instead of enriching them.
        "section" if norm.probe && !norm.strict_code && has("js-render-needs-enrichment") => {
            let pre = n.descendants().into_iter().find(|d| d.has_name("pre"));
            let text = pre.map(|p| code_text(&p, false)).unwrap_or_default();
            return vec![C::E { name: "mdr-code".into(), attrs: BTreeMap::new(), kids: vec![C::T(text)] }];
        }
        "svg" if norm.probe => {
            let texts = n.children().into_iter().filter_map(|c| {
                c.query(|t| match &t.data {
                    NodeData::Text { contents } => Some(C::T(contents.to_string())),
                    _ => None,
                })
                .flatten()
            });
            let mut out = vec![C::E { name, attrs: BTreeMap::new(), kids: Vec::new() }];
            out.extend(texts);
            return out;
        }
        // github.com turns a bare user-attachments video URL into a player (needs a lookup).
        "details" if has("details-reset") => {
            let src = n
                .descendants()
                .into_iter()
                .find(|d| d.has_name("video"))
                .and_then(|v| v.attr("src").map(|s| s.to_string()))
                .unwrap_or_default();
            if let Some(url) = user_attachment(&src) {
                let mut attrs = BTreeMap::new();
                attrs.insert("uuid".to_owned(), attachment_uuid(&url).unwrap_or_default());
                return vec![C::E { name: "mdr-attachment".into(), attrs, kids: Vec::new() }];
            }
        }
        "p" => {
            let els = n.element_children();
            if els.len() == 1 && els[0].has_name("a") && n.text().trim() == els[0].text().trim() {
                let href = els[0].attr("href").map(|s| s.to_string()).unwrap_or_default();
                if let Some(uuid) = attachment_uuid(&href).filter(|_| els[0].text().trim() == href) {
                    let mut attrs = BTreeMap::new();
                    attrs.insert("uuid".to_owned(), uuid);
                    return vec![C::E { name: "mdr-attachment".into(), attrs, kids: Vec::new() }];
                }
            }
        }
        // Comment context: a bare issue URL is shown as a `#N` reference by github.com.
        "a" if norm.refs_as_text && !has("issue-link") => {
            static ISSUE: OnceLock<Regex> = OnceLock::new();
            let href = n.attr("href").map(|h| h.to_string()).unwrap_or_default();
            let r = re(&ISSUE, r"^https://github\.com/[\w.-]+/[\w.-]+/(?:issues|pull)/(\d+)$");
            if let Some(c) = r.captures(&href).filter(|_| n.text().trim() == href) {
                return vec![C::T(format!("#{}", &c[1]))];
            }
        }
        "a" if has("IssueLabel") || has("issue-link") => {
            let text = n.text().trim().to_owned();
            if norm.refs_as_text && text.contains('#') && !text.contains("://") {
                return vec![C::T(text)];
            }
            let href = n.attr("href").map(|h| h.to_string()).unwrap_or_default();
            let mut attrs = BTreeMap::new();
            attrs.insert("href".to_owned(), href.clone());
            return vec![C::E { name: "a".into(), attrs, kids: vec![C::T(href)] }];
        }
        "div" if !norm.strict_code && (has("highlight") || has("snippet-clipboard-content")) => {
            let text = code_text(n, has("highlight"));
            return vec![C::E { name: "mdr-code".into(), attrs: BTreeMap::new(), kids: vec![C::T(text)] }];
        }
        "pre" if !norm.strict_code => {
            let els = n.element_children();
            let parent_wrapped = n.parent().is_some_and(|p| {
                p.has_class("highlight") || p.has_class("snippet-clipboard-content")
            });
            let code_like = (els.len() == 1 && els[0].has_name("code")) || n.has_attr("lang");
            if !parent_wrapped && code_like {
                return vec![C::E { name: "mdr-code".into(), attrs: BTreeMap::new(), kids: vec![C::T(code_text(n, false))] }];
            }
        }
        _ => {}
    }
    let mut kids = canon_children(n, norm);
    if name == "p" {
        // Leading/trailing whitespace inside a paragraph does not render.
        if let Some(C::T(t)) = kids.first_mut() {
            *t = t.trim_start().to_owned();
        }
        if let Some(C::T(t)) = kids.last_mut() {
            *t = t.trim_end().to_owned();
        }
        kids.retain(|k| !matches!(k, C::T(t) if t.is_empty()));
    }
    vec![C::E { name, attrs: attrs_of(n, norm), kids }]
}

fn canon_html(html: &str, norm: Norm) -> Vec<C> {
    let doc = Document::fragment(html);
    let mut kids = canon_children(&doc.html_root(), norm);
    let blank = |k: Option<&C>| matches!(k, Some(C::T(t)) if t.trim().is_empty());
    while blank(kids.last()) {
        kids.pop();
    }
    while blank(kids.first()) {
        kids.remove(0);
    }
    kids
}

// ------------------------------------------------------------------------------------------
// diff

fn show(c: &C) -> String {
    match c {
        C::T(t) => format!("text {:?}", trunc(t, 120)),
        C::E { name, attrs, .. } => {
            let mut s = format!("<{name}");
            for (k, v) in attrs {
                let _ = write!(s, " {k}=\"{}\"", trunc(v, 60));
            }
            s.push('>');
            s
        }
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_owned() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// First difference between two node lists, as a human-readable path + description.
fn diff(ours: &[C], theirs: &[C], path: &str) -> Option<String> {
    for (i, pair) in ours.iter().zip(theirs.iter()).enumerate() {
        let here = match pair.1 {
            C::E { name, .. } => format!("{path}/{name}[{i}]"),
            C::T(_) => format!("{path}/#text[{i}]"),
        };
        match pair {
            (C::T(a), C::T(b)) => {
                if a != b {
                    let p = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
                    let ctx_a: String = a.chars().skip(p.saturating_sub(20)).take(60).collect();
                    let ctx_b: String = b.chars().skip(p.saturating_sub(20)).take(60).collect();
                    return Some(format!("{here}: text differs at char {p}\n      ours:   {ctx_a:?}\n      theirs: {ctx_b:?}"));
                }
            }
            (C::E { name: na, attrs: aa, kids: ka }, C::E { name: nb, attrs: ab, kids: kb }) => {
                if na != nb || aa != ab {
                    return Some(format!("{here}:\n      ours:   {}\n      theirs: {}", show(pair.0), show(pair.1)));
                }
                if let Some(d) = diff(ka, kb, &here) {
                    return Some(d);
                }
            }
            (a, b) => {
                let prev = |v: &[C]| if i == 0 { "(start)".to_owned() } else { show(&v[i - 1]) };
                return Some(format!(
                    "{here}:\n      ours:   {}\n      theirs: {}\n      after:  {}",
                    show(a),
                    show(b),
                    prev(ours)
                ));
            }
        }
    }
    if ours.len() != theirs.len() {
        let n = ours.len().min(theirs.len());
        let extra = |v: &[C]| v.get(n).map(show).unwrap_or_else(|| "(nothing)".into());
        return Some(format!(
            "{path}: {} vs {} children; next:\n      ours:   {}\n      theirs: {}",
            ours.len(),
            theirs.len(),
            extra(ours),
            extra(theirs)
        ));
    }
    None
}

// ------------------------------------------------------------------------------------------
// cases

struct Case {
    name: String,
    input: PathBuf,
    oracle: PathBuf,
    mode: Mode,
    norm: Norm,
}

fn article(html: &str) -> &str {
    let start = html.find("<article").and_then(|i| html[i..].find('>').map(|j| i + j + 1));
    let end = html.rfind("</article>");
    match (start, end) {
        (Some(s), Some(e)) if s <= e => &html[s..e],
        _ => html,
    }
}

fn cases(dir: &Path) -> Vec<Case> {
    let gh = dir.join("github");
    let strict = strict_highlight();
    let readme = Norm { drop_dir_auto: false, strict_code: strict, refs_as_text: false, probe: false };
    let mut out = Vec::new();
    let mut add = |name: String, input: PathBuf, oracle: PathBuf, mode: Mode, norm: Norm| {
        if input.is_file() && oracle.is_file() {
            out.push(Case { name, input, oracle, mode, norm });
        }
    };
    let list = |sub: &str| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(gh.join(sub))
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        v.sort();
        v
    };
    for p in list("ctx") {
        let f = p.file_name().unwrap().to_string_lossy().to_string();
        if let Some(base) = f.strip_suffix(".md.md") {
            add(format!("ctx/{base}"), p.clone(), gh.join(format!("ctx/{base}.md.ctx.html")), Mode::Readme, readme);
        }
    }
    add("ctx/frontmatter".into(), gh.join("ctx/frontmatter_src.md"), gh.join("ctx/frontmatter.ctx.html"), Mode::Readme, readme);
    for p in list("pages") {
        let f = p.file_name().unwrap().to_string_lossy().to_string();
        if let Some(base) = f.strip_suffix(".README.md") {
            add(format!("pages/{base}"), p.clone(), gh.join(format!("pages/{base}.README.api_html.html")), Mode::Readme, readme);
        }
    }
    let with_ctx = Norm { drop_dir_auto: false, strict_code: strict, refs_as_text: true, probe: false };
    for p in list("comments") {
        let f = p.file_name().unwrap().to_string_lossy().to_string();
        if let Some(base) = f.strip_suffix(".md") {
            add(format!("comments/{base} (api_gfm)"), p.clone(), gh.join(format!("comments/{base}.api_gfm.html")), Mode::Comment, with_ctx);
            add(format!("comments/{base} (bodyHTML)"), p.clone(), gh.join(format!("comments/{base}.bodyHTML.html")), Mode::Comment, with_ctx);
        }
    }
    let no_ctx = Norm { drop_dir_auto: true, strict_code: strict, refs_as_text: true, probe: true };
    for p in list("probes") {
        let f = p.file_name().unwrap().to_string_lossy().to_string();
        if let Some(base) = f.strip_suffix(".md") {
            add(format!("probes/{base}"), p.clone(), gh.join(format!("probes/{base}.gfm.html")), Mode::Comment, no_ctx);
        }
    }
    // The research kitchen-sink document in comment context (API `mode=gfm` + context).
    add("test.md (api_gfm_ctx)".into(), gh.join("test.md"), gh.join("api_gfm_ctx.html"), Mode::Comment, with_ctx);
    add("test.md (api_gfm)".into(), gh.join("test.md"), gh.join("api_gfm.html"), Mode::Comment, no_ctx);
    out
}

/// Probes whose oracle is known to differ for documented reasons: highlight-only probes
/// need the highlight module (tree-sitter languages drop trailing blank lines from TOKENS).
const KNOWN_PROBE_RESIDUALS: &[&str] = &["probes/langs2"];

/// Oracles showing GitHub's double escaping around math (`$x<y$` -> `$x&amp;lt;y$`), which we
/// deliberately do not copy (GH §8.6); undone before comparing.
const DOUBLE_ESCAPED: &[&str] = &["probes/math", "probes/math3"];

#[test]
fn github_golden() {
    let Some(dir) = golden_dir() else {
        println!("github_golden: SKIPPED (set MD_RENDER_GOLDEN_DIR)");
        return;
    };
    let mut table = String::new();
    let mut failed_required = Vec::new();
    let (mut pass, mut total) = (0, 0);
    for case in cases(&dir) {
        let src = std::fs::read_to_string(&case.input).unwrap();
        let oracle = std::fs::read_to_string(&case.oracle).unwrap();
        let mut opts = RenderOptions::new(Flavor::Github);
        opts.mode = case.mode;
        opts.bundle_url = BUNDLE.into();
        opts.sourcepos = true;
        let ours = github::render(&src, &opts);
        let oracle = if DOUBLE_ESCAPED.contains(&case.name.as_str()) {
            oracle.replace("&amp;amp;", "&amp;").replace("&amp;lt;", "&lt;").replace("&amp;gt;", "&gt;")
        } else {
            oracle
        };
        let theirs = canon_html(article(&oracle), case.norm);
        let mine = canon_html(&ours.html, case.norm);
        total += 1;
        match diff(&mine, &theirs, "") {
            None => {
                pass += 1;
                let _ = writeln!(table, "  PASS  {}", case.name);
            }
            Some(d) => {
                let known = KNOWN_PROBE_RESIDUALS.contains(&case.name.as_str());
                let _ = writeln!(table, "  {}  {}\n    first difference {d}", if known { "KNOWN" } else { "FAIL" }, case.name);
                if !known {
                    failed_required.push(case.name.clone());
                }
            }
        }
    }
    println!("github_golden: {pass}/{total} passed (strict highlight: {})\n{table}", strict_highlight());
    assert!(failed_required.is_empty(), "golden mismatches: {failed_required:?}");
}

/// Prints our rendering of `$MD_RENDER_DUMP` (debugging aid; `--ignored --nocapture`).
#[test]
#[ignore = "debugging aid"]
fn github_dump() {
    let Some(path) = std::env::var_os("MD_RENDER_DUMP") else { return };
    let src = std::fs::read_to_string(path).unwrap();
    let mut opts = RenderOptions::new(Flavor::Github);
    if std::env::var("MD_RENDER_MODE").is_ok_and(|m| m == "comment") {
        opts.mode = Mode::Comment;
    }
    opts.bundle_url = BUNDLE.into();
    println!("{}", github::render(&src, &opts).html);
}
