//! Golden tests for the GitLab flavor (DESIGN D18): our offline render against live gitlab.com
//! server HTML, compared with a normalizing DOM diff (the rules of
//! `.dev/research/gitlab/tools/compare.py`, extended).
//!
//! Fixtures are not committed. They are read from `$MD_RENDER_GOLDEN_DIR` (default
//! `<repo>/.dev/research`): `gitlab/samples/<name>.{md,html}` (repository blob renders) and
//! `gitlab/issue_samples/issue_<iid>.{md,html}` (issue descriptions, comment pipeline).
//!
//! Normalization:
//! * oracle (server, pre-JS) -> our post-JS offline contract: strip the blob-viewer wrapper,
//!   un-lazy images (`data-src` -> `src`, drop class `lazy`), resolve `data-canonical-src`
//!   (repository links, uploads, asset proxy) back to the written URL, drop GitLab's random
//!   footnote suffix, drop `data-proxied-urls`, turn reference links back into the text or link
//!   they were made from (`data-original`, `data-original-href`), undo the wiki-link rewrite;
//! * ours -> server shape: drop `data-mdr-key`, `code-syntax-highlight-theme`, `loading`, the
//!   client-side table wrapper;
//! * both: inside `pre.code` only the text and the `span.line` structure are compared unless
//!   `GOLDEN_STRICT_HIGHLIGHT=1` (token classes come from Rouge on gitlab.com); attribute order,
//!   entity spelling and `/>` vs `>` vanish by comparing DOMs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dom_query::{Document, NodeData, NodeRef};
use md_render::render::{RenderOptions, render};
use md_render::{Flavor, Mode};

fn golden_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("MD_RENDER_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".dev/research"));
    dir.join("gitlab/samples").is_dir().then_some(dir)
}

fn strict_highlight() -> bool {
    std::env::var("GOLDEN_STRICT_HIGHLIGHT").is_ok_and(|v| v == "1")
}

/// Whether the highlighter knows Rouge tags (otherwise `language-*`/`data-lang` are not
/// compared outside strict mode).
fn highlighter_has_registry() -> bool {
    md_render::render::highlight::gitlab("x", "js").tag == "javascript"
}

// ------------------------------------------------------------------------------------------------
// DOM helpers

fn attr(n: &NodeRef<'_>, name: &str) -> Option<String> {
    n.attr(name).map(|v| v.to_string())
}

fn name(n: &NodeRef<'_>) -> Option<String> {
    n.element_ref().map(|e| e.name.local.to_string())
}

fn classes(n: &NodeRef<'_>) -> Vec<String> {
    attr(n, "class").map(|c| c.split_ascii_whitespace().map(String::from).collect()).unwrap_or_default()
}

fn remove_classes(n: &NodeRef<'_>, remove: &[&str]) {
    if !n.has_attr("class") {
        return;
    }
    let kept: Vec<String> = classes(n).into_iter().filter(|c| !remove.contains(&c.as_str())).collect();
    if kept.is_empty() {
        n.remove_attr("class");
    } else {
        n.set_attr("class", &kept.join(" "));
    }
}

fn all_elements<'a>(root: &NodeRef<'a>) -> Vec<NodeRef<'a>> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeRef<'a>> = root.children().into_iter().rev().collect();
    while let Some(n) = stack.pop() {
        if n.is_element() {
            out.push(n.clone());
            stack.extend(n.children().into_iter().rev());
        }
    }
    out
}

fn unwrap(n: &NodeRef<'_>) {
    while let Some(c) = n.first_child() {
        n.insert_before(&c);
    }
    n.remove_from_parent();
}

fn has_ancestor(n: &NodeRef<'_>, pred: impl Fn(&NodeRef<'_>) -> bool) -> bool {
    let mut cur = n.parent();
    while let Some(p) = cur {
        if pred(&p) {
            return true;
        }
        cur = p.parent();
    }
    false
}

// ------------------------------------------------------------------------------------------------
// Normalization

/// Oracle (gitlab.com server HTML) -> offline contract.
fn normalize_oracle(root: &NodeRef<'_>) {
    // Reference links (need GitLab's database): back to what the author wrote.
    for a in all_elements(root) {
        if name(&a).as_deref() != Some("a") || !a.has_attr("data-reference-type") {
            continue;
        }
        if a.has_attr("data-gollum") {
            continue; // wiki link, handled below
        }
        // `data-original` holds the original inner HTML.
        let original = attr(&a, "data-original").unwrap_or_else(|| a.text().to_string());
        let original_text = Document::fragment(original.as_str()).html_root().text().to_string();
        // Labels are wrapped in `span.gl-label`.
        let target = match a.parent() {
            Some(p) if name(&p).as_deref() == Some("span") && classes(&p).iter().any(|c| c == "gl-label") => p,
            _ => a.clone(),
        };
        let href = attr(&a, "data-original-href").or_else(|| {
            (attr(&a, "data-link-reference").as_deref() == Some("true")).then(|| original_text.clone())
        });
        match href {
            Some(href) => {
                // The original link (an autolink or `[text](url)`): its sourcepos is lost.
                let link = root.tree.new_element("a");
                link.set_attr("data-sourcepos", "*");
                link.set_attr("href", &href);
                link.append_html(original.as_str());
                target.insert_before(&link);
            }
            None => target.insert_before(&root.tree.new_text(original_text.as_str())),
        }
        target.remove_from_parent();
    }
    // Footnote suffix: every `li[id^=fn-]` of the section shares one `-NNNN`.
    let suffix = all_elements(root)
        .into_iter()
        .filter(|n| name(n).as_deref() == Some("li"))
        .filter_map(|n| attr(&n, "id"))
        .find(|id| id.starts_with("fn-"))
        .and_then(|id| id.rfind('-').map(|i| id[i..].to_owned()))
        .filter(|s| s.len() > 1 && s[1..].bytes().all(|b| b.is_ascii_digit()));
    for n in all_elements(root) {
        let lazy = classes(&n).iter().any(|c| c == "lazy");
        if lazy {
            match attr(&n, "data-src") {
                Some(s) if !s.is_empty() => n.set_attr("src", &s),
                _ => n.remove_attr("src"),
            }
            n.remove_attr("data-src");
            remove_classes(&n, &["lazy"]);
        }
        if n.has_attr("data-gollum") {
            if let Some(c) = attr(&n, "data-canonical-src") {
                n.set_attr("href", &c);
            }
            for a in ["data-canonical-src", "data-link", "data-gollum", "data-reference-type", "data-project", "data-group"] {
                n.remove_attr(a);
            }
        } else if let Some(c) = attr(&n, "data-canonical-src") {
            let target = if name(&n).as_deref() == Some("a") { "href" } else { "src" };
            n.set_attr(target, &c);
            n.remove_attr("data-canonical-src");
            n.remove_attr("data-link");
            remove_classes(&n, &["gfm"]);
        }
        n.remove_attr("data-proxied-urls");
        // HeadingLocalizationFilter translated the anchor label for the viewer's locale.
        if let (Some(content), Some(label)) = (attr(&n, "data-heading-content"), attr(&n, "aria-label")) {
            if !label.starts_with("Link to heading '") {
                n.set_attr("aria-label", &format!("Link to heading '{content}'"));
            }
        }
        if let Some(sfx) = &suffix {
            for a in ["id", "href"] {
                if let Some(v) = attr(&n, a) {
                    let is_fn = v.starts_with("fn-") || v.starts_with("fnref-") || v.starts_with("#fn-") || v.starts_with("#fnref-");
                    if is_fn {
                        if let Some(stripped) = v.strip_suffix(sfx.as_str()) {
                            n.set_attr(a, stripped);
                        }
                    }
                }
            }
        }
    }
}

/// Ours (post-JS DOM) -> server shape.
fn normalize_ours(root: &NodeRef<'_>) {
    for n in all_elements(root) {
        n.remove_attr("data-mdr-key");
        n.remove_attr("loading");
        remove_classes(&n, &["code-syntax-highlight-theme"]);
        if name(&n).as_deref() == Some("table") {
            remove_classes(&n, &["gl-min-w-full", "gl-overflow-y-hidden"]);
        }
    }
    for n in all_elements(root) {
        if name(&n).as_deref() == Some("div") && classes(&n).iter().any(|c| c == "gl-table-shadow-overlay") {
            if let Some(inner) = n.first_element_child() {
                unwrap(&inner);
            }
            unwrap(&n);
        }
    }
}

/// Both sides: code-block contents.
fn normalize_code(root: &NodeRef<'_>, strict: bool, registry: bool) {
    for n in all_elements(root) {
        let is_code_pre = name(&n).as_deref() == Some("pre") && classes(&n).iter().any(|c| c == "code");
        if is_code_pre && !strict && !registry {
            let kept: Vec<String> = classes(&n).into_iter().filter(|c| !c.starts_with("language-") || c == "language-plaintext").collect();
            n.set_attr("class", &kept.join(" "));
            remove_classes(&n, &["language-plaintext"]);
        }
        if name(&n).as_deref() != Some("span") || strict {
            continue;
        }
        if !has_ancestor(&n, |p| name(p).as_deref() == Some("pre") && classes(p).iter().any(|c| c == "code")) {
            continue;
        }
        if classes(&n).iter().any(|c| c == "line") {
            if !registry {
                n.remove_attr("data-lang");
            }
        } else {
            unwrap(&n);
        }
    }
}

/// Flattens a DOM into comparable lines (tag + sorted attributes; text with exact content).
fn flatten(root: &NodeRef<'_>) -> Vec<String> {
    let mut out = Vec::new();
    // (node, depth)
    let mut stack: Vec<(NodeRef<'_>, usize)> = root.children().into_iter().rev().map(|c| (c, 0)).collect();
    let mut pending_text = String::new();
    let mut pending_depth = 0;
    let flush = |out: &mut Vec<String>, text: &mut String, depth: usize| {
        if !text.is_empty() {
            out.push(format!("{}{:?}", "  ".repeat(depth), text));
            text.clear();
        }
    };
    while let Some((n, depth)) = stack.pop() {
        let data = n.query(|t| match &t.data {
            NodeData::Text { contents } => Some(contents.to_string()),
            _ => None,
        });
        match data.flatten() {
            Some(t) => {
                if pending_text.is_empty() {
                    pending_depth = depth;
                }
                pending_text.push_str(&t);
                continue;
            }
            None => flush(&mut out, &mut pending_text, pending_depth),
        }
        let Some(el) = n.element_ref() else { continue };
        let mut attrs: BTreeMap<String, String> = BTreeMap::new();
        for a in &el.attrs {
            attrs.insert(a.name.local.to_string(), a.value.to_string());
        }
        let tag = el.name.local.to_string();
        drop(el);
        let attrs: Vec<String> = attrs.into_iter().map(|(k, v)| format!("{k}={v:?}")).collect();
        out.push(format!("{}<{tag} {}>", "  ".repeat(depth), attrs.join(" ")));
        stack.extend(n.children().into_iter().rev().map(|c| (c, depth + 1)));
    }
    flush(&mut out, &mut pending_text, pending_depth);
    out
}

/// Line equality with `"*"` attribute wildcards on the oracle side.
fn line_eq(ours: &str, oracle: &str) -> bool {
    if ours == oracle {
        return true;
    }
    if !oracle.contains("=\"*\"") {
        return false;
    }
    let mut pattern = String::from("^");
    for (i, part) in oracle.split("=\"*\"").enumerate() {
        if i > 0 {
            pattern.push_str("=\"[^\"]*\"");
        }
        pattern.push_str(&regex::escape(part));
    }
    pattern.push('$');
    regex::Regex::new(&pattern).is_ok_and(|r| r.is_match(ours))
}

/// Myers diff; returns (ours-only, oracle-only) line indices, or None when the edit distance is
/// larger than `max_d`.
fn diff(a: &[String], b: &[String], max_d: usize) -> Option<Vec<(char, String)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (n + m) as usize;
    let offset = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    for d in 0..=max.min(max_d) as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = (k + offset) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) { v[idx + 1] } else { v[idx - 1] + 1 };
            let mut y = x - k;
            while x < n && y < m && line_eq(&a[x as usize], &b[y as usize]) {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                found = Some(d);
                break;
            }
            k += 2;
        }
        if found.is_some() {
            break;
        }
    }
    let d_final = found?;
    // Backtrack.
    let mut edits = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=d_final).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[(k - 1 + offset) as usize] < v[(k + 1 + offset) as usize]) { k + 1 } else { k - 1 };
        let prev_x = v[(prev_k + offset) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            edits.push((' ', a[x as usize].clone()));
        }
        if x == prev_x {
            y -= 1;
            edits.push(('+', b[y as usize].clone()));
        } else {
            x -= 1;
            edits.push(('-', a[x as usize].clone()));
        }
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        edits.push((' ', a[x as usize].clone()));
    }
    edits.reverse();
    Some(edits)
}

fn trim_root_whitespace(root: &NodeRef<'_>) {
    for c in root.children() {
        let ws = c.query(|t| matches!(&t.data, NodeData::Text { contents } if contents.trim().is_empty())).unwrap_or(false);
        if ws {
            c.remove_from_parent();
        }
    }
}

struct Outcome {
    name: String,
    differing: usize,
    lines: usize,
    report: String,
}

fn compare(name: &str, ours_html: &str, oracle_html: &str, blob: bool) -> Outcome {
    let strict = strict_highlight();
    let registry = highlighter_has_registry() || strict;

    let ours_doc = Document::fragment(ours_html);
    let ours = ours_doc.html_root();
    normalize_ours(&ours);
    normalize_code(&ours, strict, registry);
    trim_root_whitespace(&ours);

    let oracle_doc = Document::fragment(oracle_html);
    let mut oracle = oracle_doc.html_root();
    if blob {
        if let Some(fc) = all_elements(&oracle).into_iter().find(|n| classes(n).iter().any(|c| c == "file-content")) {
            oracle = fc;
        }
    }
    normalize_oracle(&oracle);
    normalize_code(&oracle, strict, registry);
    trim_root_whitespace(&oracle);

    let a = flatten(&ours);
    let b = flatten(&oracle);
    let lines = a.len().max(b.len());
    match diff(&a, &b, 3000) {
        None => Outcome { name: name.to_owned(), differing: usize::MAX, lines, report: "  (more than 3000 differences)\n".into() },
        Some(edits) => {
            let differing = edits.iter().filter(|(c, _)| *c != ' ').count();
            let mut report = String::new();
            let mut shown = 0;
            for (i, (c, l)) in edits.iter().enumerate() {
                if *c == ' ' {
                    continue;
                }
                if shown >= 60 {
                    report.push_str("  ...\n");
                    break;
                }
                if i > 0 && edits[i - 1].0 == ' ' {
                    report.push_str(&format!("   @ {}\n", truncate(&edits[i - 1].1)));
                }
                report.push_str(&format!("  {c} {}\n", truncate(l)));
                shown += 1;
            }
            Outcome { name: name.to_owned(), differing, lines, report }
        }
    }
}

fn truncate(s: &str) -> String {
    if s.chars().count() > 220 {
        format!("{}…", s.chars().take(220).collect::<String>())
    } else {
        s.to_owned()
    }
}

/// Known residuals: (file, reason). These may differ; everything else must match exactly.
const BLOB_RESIDUALS: &[(&str, &str)] = &[];
const ISSUE_RESIDUALS: &[(&str, &str)] = &[
    ("issue_374563", "stale cached render: older comrak list end sourcepos (prototype differs too)"),
    ("issue_388801", "stale cached render: pre-17.0 heading markup and sourcepos (GL §14)"),
    ("issue_424022", "stale cached render: pre-17.0 heading markup and sourcepos (GL §14)"),
    ("issue_630440", "needs GitLab services: PlantUML image, group custom emoji, `+` reference"),
];

fn run_set(dir: &Path, subdir: &str, mode: Mode, residuals: &[(&str, &str)]) -> (usize, usize, Vec<String>) {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir.join("gitlab").join(subdir))
        .expect("read fixture dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    names.sort();
    let mut pass = 0;
    let mut total = 0;
    let mut unexpected = Vec::new();
    println!("\n{:<44} {:>8} {:>8}  result", format!("gitlab/{subdir}"), "lines", "diffs");
    for md_path in names {
        let html_path = md_path.with_extension("html");
        let Ok(oracle) = std::fs::read_to_string(&html_path) else { continue };
        let md = std::fs::read_to_string(&md_path).expect("read md");
        let stem = md_path.file_stem().unwrap().to_string_lossy().to_string();
        let mut opts = RenderOptions::new(Flavor::Gitlab);
        opts.mode = mode;
        opts.source_path = Some(md_path.clone());
        let rendered = render(&md, &opts);
        let o = compare(&stem, &rendered.html, &oracle, subdir == "samples");
        total += 1;
        let residual = residuals.iter().find(|(n, _)| *n == stem);
        let status = match (o.differing == 0, residual) {
            (true, _) => {
                pass += 1;
                "PASS".to_owned()
            }
            (false, Some((_, why))) => format!("RESIDUAL ({why})"),
            (false, None) => {
                unexpected.push(stem.clone());
                "FAIL".to_owned()
            }
        };
        let diffs = if o.differing == usize::MAX { ">3000".to_owned() } else { o.differing.to_string() };
        println!("{:<44} {:>8} {:>8}  {status}", o.name, o.lines, diffs);
        if o.differing != 0 && std::env::var("GOLDEN_VERBOSE").is_ok_and(|v| v == "1") {
            print!("{}", o.report);
        }
    }
    println!("{pass}/{total} identical after normalization");
    (pass, total, unexpected)
}

#[test]
fn gitlab_golden_blobs() {
    let Some(dir) = golden_dir() else {
        println!("gitlab_golden_blobs: SKIPPED (set MD_RENDER_GOLDEN_DIR)");
        return;
    };
    let (_, _, unexpected) = run_set(&dir, "samples", Mode::Readme, BLOB_RESIDUALS);
    assert!(unexpected.is_empty(), "unexpected golden differences: {unexpected:?} (GOLDEN_VERBOSE=1 shows diffs)");
}

#[test]
fn gitlab_golden_issues() {
    let Some(dir) = golden_dir() else {
        println!("gitlab_golden_issues: SKIPPED (set MD_RENDER_GOLDEN_DIR)");
        return;
    };
    let (_, _, unexpected) = run_set(&dir, "issue_samples", Mode::Comment, ISSUE_RESIDUALS);
    assert!(unexpected.is_empty(), "unexpected golden differences: {unexpected:?} (GOLDEN_VERBOSE=1 shows diffs)");
}

#[test]
fn normalizer_self_check() {
    // A server-shaped snippet and its offline counterpart must normalize to the same lines.
    let oracle = "<div class=\"file-content js-markup-content md\">\n<p data-sourcepos=\"1:1-1:5\" dir=\"auto\"><a class=\"no-attachment-icon gfm\" href=\"/g/p/-/raw/main/i.png\" target=\"_blank\" rel=\"noopener noreferrer\" data-canonical-src=\"i.png\"><img data-sourcepos=\"1:1-1:5\" src=\"data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==\" alt=\"x\" decoding=\"async\" class=\"lazy gfm\" data-src=\"/g/p/-/raw/main/i.png\" data-canonical-src=\"i.png\"></a> <a href=\"/g/p/-/issues/1\" data-original=\"#1\" data-reference-type=\"issue\" class=\"gfm gfm-issue\">Title (#1)</a></p>\n</div>";
    let ours = "<p data-sourcepos=\"1:1-1:5\" dir=\"auto\"><a class=\"no-attachment-icon\" href=\"i.png\" target=\"_blank\" rel=\"noopener noreferrer\"><img data-sourcepos=\"1:1-1:5\" src=\"i.png\" alt=\"x\" decoding=\"async\" loading=\"lazy\"></a> #1</p>";
    let o = compare("self", ours, oracle, true);
    assert_eq!(o.differing, 0, "{}", o.report);
}
