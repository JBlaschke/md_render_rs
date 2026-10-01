//! Evaluation harness for syntax highlighting against real site output (owner: highlight).
//!
//! Shared by the integration tests (`tests/highlight_golden.rs`, public API) and the in-crate
//! variant comparison (`src/render/highlight/eval.rs`, which includes this file by path). The
//! golden data is not committed (D18): it is read from `$MD_RENDER_GOLDEN_DIR` (default
//! `<CARGO_MANIFEST_DIR>/.dev/research`), and every caller skips loudly when it is missing.
//!
//! * GitHub: every `<div class="highlight highlight-SCOPE …"><pre…>TOKENS</pre></div>` in the
//!   GitHub API outputs under `github/`. The code is recovered from TOKENS; the comparison is per
//!   non-whitespace character: the full `pl-*` class path, the innermost class, and the color
//!   the PrettyLights CSS (GH §7.3) gives it.
//! * GitLab: `gitlab/rouge_samples/` (36 languages; `code.<lang>` + Rouge 5.1.0 tokens in
//!   `<lang>.tokens.tsv`), compared per non-whitespace character exactly like
//!   `R/tools/scope_map.py`: exact class and class family (first letter).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The golden data directory, if present.
pub fn golden_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("MD_RENDER_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".dev/research"));
    (dir.join("github").is_dir() && dir.join("gitlab").is_dir()).then_some(dir)
}

/// Print the loud skip line required by D18.
pub fn skipped(what: &str) {
    eprintln!("SKIPPED (set MD_RENDER_GOLDEN_DIR): {what}");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// ------------------------------------------------------------------------------------------
// GitHub

/// The GitHub API outputs used for tuning (the files the token tables in `S/` were made from).
pub const GITHUB_TUNING: &[&str] = &["github/api_gfm.html", "github/probes/langs2.gfm.html"];

/// Further GitHub API outputs (real READMEs, issue comments), used as a held-out check.
pub fn github_heldout(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for sub in ["github/ctx", "github/pages", "github/comments"] {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else { continue };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".api_gfm.html"))
            .collect();
        names.sort();
        out.extend(names.into_iter().map(|n| format!("{sub}/{n}")));
    }
    out
}

/// One highlighted block of GitHub output.
#[derive(Clone, Debug)]
pub struct GhBlock {
    /// The `highlight-` suffix, e.g. `source-rust`.
    pub class: String,
    /// Inner HTML of the `<pre>`.
    pub html: String,
    /// The info-string word of the fenced block in the Markdown source with the same code
    /// (`None`: not found, e.g. a raw-HTML `<pre lang>` block).
    pub lang: Option<String>,
}

/// The Markdown source of a GitHub API output file.
fn markdown_source(html_file: &str) -> String {
    match html_file {
        "github/api_gfm.html" => "github/test.md".to_owned(),
        "github/probes/langs2.gfm.html" => "github/probes/langs2.md".to_owned(),
        other => format!("{}.md", other.trim_end_matches(".api_gfm.html")),
    }
}

/// Fenced code blocks of a Markdown document: (first info word, code without final newline).
fn fenced_blocks(markdown: &str) -> Vec<(String, String)> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let mut out = Vec::new();
    for node in root.descendants() {
        if let comrak::nodes::NodeValue::CodeBlock(cb) = &node.data().value {
            let word = cb.info.split_whitespace().next().unwrap_or("").to_owned();
            let code = cb.literal.replace("\r\n", "\n");
            out.push((word, code.strip_suffix('\n').unwrap_or(&code).to_owned()));
        }
    }
    out
}

/// All highlighted blocks in a GitHub HTML document, with info words from `markdown`.
pub fn github_blocks(doc: &str, markdown: &str) -> Vec<GhBlock> {
    let re = regex::Regex::new(
        r#"(?s)<div class="highlight highlight-([^" ]+)[^"]*"[^>]*><pre[^>]*>(.*?)</pre></div>"#,
    )
    .expect("valid regex");
    let fenced = fenced_blocks(markdown);
    re.captures_iter(doc)
        .map(|c| {
            let html = c[2].to_owned();
            let code = code_of(&html);
            let lang = fenced.iter().find(|(_, lit)| *lit == code).map(|(w, _)| w.clone());
            GhBlock { class: c[1].to_owned(), html, lang }
        })
        .collect()
}

/// Read every block of the given files (paths relative to the golden dir).
pub fn github_blocks_in(dir: &Path, files: &[impl AsRef<str>]) -> Vec<GhBlock> {
    files
        .iter()
        .flat_map(|f| {
            let f = f.as_ref();
            let md = std::fs::read_to_string(dir.join(markdown_source(f))).unwrap_or_default();
            github_blocks(&read(&dir.join(f)), &md)
        })
        .collect()
}

/// Decode the entities GitHub/our output use in code.
pub fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

/// Every character of a span-annotated HTML fragment with its class path (outermost first).
pub fn class_paths(html: &str) -> Vec<(char, Vec<String>)> {
    let mut out = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("<span class=\"") {
            let end = r.find("\">").expect("well-formed span");
            stack.push(r[..end].to_owned());
            rest = &r[end + 2..];
        } else if let Some(r) = rest.strip_prefix("<span") {
            // Spans without a class (none expected): keep the nesting balanced.
            let end = r.find('>').expect("well-formed span");
            stack.push(String::new());
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix("</span>") {
            stack.pop();
            rest = r;
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            let path: Vec<String> = stack.iter().filter(|c| !c.is_empty()).cloned().collect();
            for c in decode_entities(&rest[..end]).chars() {
                out.push((c, path.clone()));
            }
            rest = &rest[end..];
        }
    }
    out
}

/// The plain code of a span-annotated fragment.
pub fn code_of(html: &str) -> String {
    class_paths(html).into_iter().map(|(c, _)| c).collect()
}

/// The PrettyLights CSS (GH §7.3) as (selector classes, color), in stylesheet order.
const PL_CSS: &[(&[&str], &str)] = &[
    (&["pl-c"], "comment"),
    (&["pl-c1"], "constant"),
    (&["pl-s", "pl-v"], "constant"),
    (&["pl-e"], "entity"),
    (&["pl-en"], "entity"),
    // storage-modifier-import is the default foreground color in every theme.
    (&["pl-smi"], "fg"),
    (&["pl-s", "pl-s1"], "fg"),
    (&["pl-ent"], "entity-tag"),
    (&["pl-k"], "keyword"),
    (&["pl-s"], "string"),
    (&["pl-pds"], "string"),
    (&["pl-s", "pl-pse", "pl-s1"], "string"),
    (&["pl-sr"], "string"),
    (&["pl-sr", "pl-cce"], "string"),
    (&["pl-sr", "pl-sre"], "string"),
    (&["pl-sr", "pl-sra"], "string"),
    (&["pl-v"], "variable"),
    (&["pl-smw"], "variable"),
    (&["pl-bu"], "unmatched"),
    (&["pl-ii"], "invalid"),
    (&["pl-c2"], "carriage-return"),
    (&["pl-sr", "pl-cce"], "string-regexp"),
    (&["pl-ml"], "markup-list"),
    (&["pl-mh"], "markup-heading"),
    (&["pl-mh", "pl-en"], "markup-heading"),
    (&["pl-ms"], "markup-heading"),
    (&["pl-mi"], "markup-italic"),
    (&["pl-mb"], "markup-bold"),
    (&["pl-md"], "markup-deleted"),
    (&["pl-mi1"], "markup-inserted"),
    (&["pl-mc"], "markup-changed"),
    (&["pl-mi2"], "markup-ignored"),
    (&["pl-mdr"], "meta-diff-range"),
    (&["pl-ba"], "brackethighlighter-angle"),
    (&["pl-sg"], "sublimelinter-gutter-mark"),
    (&["pl-corl"], "constant-other-reference-link"),
];

/// Whether `needle` is a subsequence of `hay` (descendant combinators).
fn is_subsequence(needle: &[&str], hay: &[String]) -> bool {
    let mut it = hay.iter();
    needle.iter().all(|n| it.any(|h| h == n))
}

/// The color a character with class `path` gets (CSS cascade + inheritance).
pub fn color(path: &[String]) -> &'static str {
    let mut inherited = "fg";
    for (i, class) in path.iter().enumerate() {
        let ancestors = &path[..i];
        let mut best: Option<(usize, &'static str)> = None;
        for (selector, color) in PL_CSS {
            let (last, outer) = selector.split_last().expect("non-empty selector");
            if last != class || !is_subsequence(outer, ancestors) {
                continue;
            }
            // Higher specificity wins; equal specificity: later rule wins.
            if best.is_none_or(|(spec, _)| selector.len() >= spec) {
                best = Some((selector.len(), color));
            }
        }
        if let Some((_, color)) = best {
            inherited = color;
        }
    }
    inherited
}

/// Character agreement counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Score {
    /// Non-whitespace characters compared.
    pub chars: usize,
    /// Same full class path.
    pub path: usize,
    /// Same innermost class.
    pub inner: usize,
    /// Same resulting color.
    pub color: usize,
    /// Blocks whose recovered code differs from the input (should be 0).
    pub text_mismatch: usize,
}

impl Score {
    pub fn add(&mut self, o: Score) {
        self.chars += o.chars;
        self.path += o.path;
        self.inner += o.inner;
        self.color += o.color;
        self.text_mismatch += o.text_mismatch;
    }

    fn pct(n: usize, d: usize) -> f64 {
        if d == 0 { 100.0 } else { 100.0 * n as f64 / d as f64 }
    }

    pub fn path_pct(&self) -> f64 {
        Self::pct(self.path, self.chars)
    }

    pub fn inner_pct(&self) -> f64 {
        Self::pct(self.inner, self.chars)
    }

    pub fn color_pct(&self) -> f64 {
        Self::pct(self.color, self.chars)
    }
}

/// Compare our HTML for a block with GitHub's.
pub fn score_github(expected: &str, ours: &str) -> Score {
    let a = class_paths(expected);
    let b = class_paths(ours);
    let mut s = Score::default();
    if a.iter().map(|x| x.0).ne(b.iter().map(|x| x.0)) {
        s.text_mismatch = 1;
    }
    for (i, (c, pa)) in a.iter().enumerate() {
        if c.is_whitespace() {
            continue;
        }
        s.chars += 1;
        let Some((_, pb)) = b.get(i) else { continue };
        if pa == pb {
            s.path += 1;
        }
        if pa.last() == pb.last() {
            s.inner += 1;
        }
        if color(pa) == color(pb) {
            s.color += 1;
        }
    }
    s
}

/// Result of a GitHub evaluation.
#[derive(Debug, Default)]
pub struct GithubEval {
    /// Scores per `highlight-` class.
    pub scores: BTreeMap<String, Score>,
    /// Blocks whose info word resolved to another scope than GitHub's: (word, ours, GitHub's).
    pub scope_mismatches: Vec<(String, Option<String>, String)>,
    /// Blocks without a fenced source block (not evaluated).
    pub unmatched: usize,
}

/// Score `highlight` (code, info word) → (scope, html) over `blocks`, per `highlight-` class.
pub fn evaluate_github(
    blocks: &[GhBlock],
    highlight: impl Fn(&str, &str) -> (Option<String>, Option<String>),
) -> GithubEval {
    let mut eval = GithubEval::default();
    for block in blocks {
        let Some(lang) = &block.lang else {
            eval.unmatched += 1;
            continue;
        };
        // The input as the Markdown parser hands it over: with its final newline.
        let code = format!("{}\n", code_of(&block.html));
        let (scope, html) = highlight(&code, lang);
        if scope.as_ref().map(|s| s.replace('.', "-")).as_deref() != Some(block.class.as_str()) {
            eval.scope_mismatches.push((lang.clone(), scope, block.class.clone()));
        }
        let ours = html.unwrap_or_else(|| {
            code.trim_end_matches('\n').replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        });
        eval.scores.entry(block.class.clone()).or_default().add(score_github(&block.html, &ours));
    }
    eval
}

/// Total over all languages.
pub fn total(scores: &BTreeMap<String, Score>) -> Score {
    let mut t = Score::default();
    for s in scores.values() {
        t.add(*s);
    }
    t
}

/// Render a per-language table.
pub fn github_table(title: &str, scores: &BTreeMap<String, Score>) -> String {
    let mut s = format!("{title}\n{:<28} {:>6} {:>7} {:>7} {:>7}\n", "highlight-", "chars", "path%", "inner%", "color%");
    for (lang, sc) in scores {
        s += &format!(
            "{lang:<28} {:>6} {:>7.1} {:>7.1} {:>7.1}{}\n",
            sc.chars,
            sc.path_pct(),
            sc.inner_pct(),
            sc.color_pct(),
            if sc.text_mismatch > 0 { "  TEXT MISMATCH" } else { "" }
        );
    }
    let t = total(scores);
    s += &format!(
        "{:<28} {:>6} {:>7.1} {:>7.1} {:>7.1}\n",
        "TOTAL",
        t.chars,
        t.path_pct(),
        t.inner_pct(),
        t.color_pct()
    );
    s
}

// ------------------------------------------------------------------------------------------
// GitLab

/// Decode a Ruby `String#inspect` literal (as written by `rouge_gitlab.rb --tokens`).
pub fn ruby_unescape(lit: &str) -> String {
    let inner = lit.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(lit);
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('e') => out.push('\u{1b}'),
            Some('s') => out.push(' '),
            Some('0') => out.push('\0'),
            Some('u') => {
                let rest: String = chars.by_ref().take_while(|&c| c != '}').collect();
                let hex = rest.trim_start_matches('{');
                if let Some(ch) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Per-character Rouge classes of a `.tokens.tsv` file (`""` for `Text` and `w`, as
/// `scope_map.py` does).
pub fn rouge_char_classes(tsv: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in tsv.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(short), Some(_qual), Some(val)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let class = if short == "(text)" || short == "w" { "" } else { short };
        for _ in ruby_unescape(val).chars() {
            out.push(class.to_owned());
        }
    }
    out
}

/// Per-character classes of our `lines` (flat Rouge spans; the bidi wrapper is ignored).
pub fn our_char_classes(lines: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push(String::new());
        }
        for (_, path) in class_paths(line) {
            let class = path.iter().rev().find(|c| !c.starts_with("unicode-bidi")).cloned();
            out.push(class.unwrap_or_default());
        }
    }
    out.push(String::new()); // the final newline
    out
}

/// The 36 Rouge sample languages present in `dir/gitlab/rouge_samples`.
pub fn rouge_sample_langs(dir: &Path) -> Vec<String> {
    let mut langs: Vec<String> = std::fs::read_dir(dir.join("gitlab/rouge_samples"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".tokens.tsv").map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    langs.sort();
    langs
}

/// Exact / family agreement for one sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RougeScore {
    pub chars: usize,
    pub exact: usize,
    pub family: usize,
}

impl RougeScore {
    pub fn add(&mut self, o: RougeScore) {
        self.chars += o.chars;
        self.exact += o.exact;
        self.family += o.family;
    }

    pub fn exact_pct(&self) -> f64 {
        100.0 * self.exact as f64 / self.chars.max(1) as f64
    }

    pub fn family_pct(&self) -> f64 {
        100.0 * self.family as f64 / self.chars.max(1) as f64
    }
}

/// `scope_map.py`'s comparison: non-whitespace characters of `code`, up to the shorter of the
/// two class sequences.
pub fn score_rouge(code: &str, ours: &[String], rouge: &[String]) -> (RougeScore, Vec<(String, String)>) {
    let code: Vec<char> = code.chars().collect();
    let n = ours.len().min(rouge.len());
    let mut s = RougeScore::default();
    let mut confusions = Vec::new();
    for i in 0..n {
        if i >= code.len() || code[i].is_whitespace() {
            continue;
        }
        s.chars += 1;
        if ours[i] == rouge[i] {
            s.exact += 1;
        } else {
            confusions.push((ours[i].clone(), rouge[i].clone()));
        }
        if ours[i].chars().next() == rouge[i].chars().next() {
            s.family += 1;
        }
    }
    (s, confusions)
}

/// Evaluate `highlight` (code, lang) → lines over all Rouge samples.
pub fn evaluate_gitlab(
    dir: &Path,
    highlight: impl Fn(&str, &str) -> Vec<String>,
) -> (BTreeMap<String, RougeScore>, BTreeMap<(String, String), usize>) {
    let mut scores = BTreeMap::new();
    let mut confusions: BTreeMap<(String, String), usize> = BTreeMap::new();
    let base = dir.join("gitlab/rouge_samples");
    for lang in rouge_sample_langs(dir) {
        let code = read(&base.join(format!("code.{lang}")));
        let rouge = rouge_char_classes(&read(&base.join(format!("{lang}.tokens.tsv"))));
        let ours = our_char_classes(&highlight(&code, &lang));
        let (s, conf) = score_rouge(&code, &ours, &rouge);
        for c in conf {
            *confusions.entry(c).or_default() += 1;
        }
        scores.insert(lang, s);
    }
    (scores, confusions)
}

/// Render the per-language GitLab table (`MAPPING_EVAL.txt` format).
pub fn gitlab_table(
    title: &str,
    scores: &BTreeMap<String, RougeScore>,
    confusions: &BTreeMap<(String, String), usize>,
) -> String {
    let mut s = format!("{title}\n");
    let mut t = RougeScore::default();
    for (lang, sc) in scores {
        s += &format!(
            "{lang:<12} chars={:>5} exact={:>3.0}% family={:>3.0}%\n",
            sc.chars,
            sc.exact_pct(),
            sc.family_pct()
        );
        t.add(*sc);
    }
    s += &format!("TOTAL exact={:.1}% family={:.1}%\n", t.exact_pct(), t.family_pct());
    let mut conf: Vec<_> = confusions.iter().collect();
    conf.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    s += "top confusions (ours->rouge):";
    for ((a, b), n) in conf.into_iter().take(25) {
        s += &format!(" ({a:?},{b:?})={n}");
    }
    s.push('\n');
    s
}

/// Total of a GitLab score table.
pub fn gitlab_total(scores: &BTreeMap<String, RougeScore>) -> RougeScore {
    let mut t = RougeScore::default();
    for s in scores.values() {
        t.add(*s);
    }
    t
}
