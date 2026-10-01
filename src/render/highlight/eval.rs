//! Class-scheme variant comparison against real site output (GH §7.6/§7.7, GL §8.4).
//!
//! Golden data is read from `$MD_RENDER_GOLDEN_DIR` (D18); without it every test here prints
//! `SKIPPED (set MD_RENDER_GOLDEN_DIR)` and passes. Run with `--nocapture` to see the tables:
//!
//! ```text
//! MD_RENDER_GOLDEN_DIR=…/.dev/research cargo test --lib highlight::eval -- --nocapture
//! ```

#[path = "../../../tests/highlight_eval/mod.rs"]
mod harness;

use syntect::parsing::Scope;

use super::{pl, rouge};
use harness::*;

#[test]
fn github_variants() {
    let Some(dir) = golden_dir() else {
        skipped("highlight::eval::github_variants");
        return;
    };
    let tuning = github_blocks_in(&dir, GITHUB_TUNING);
    let heldout = github_blocks_in(&dir, &github_heldout(&dir));
    for variant in pl::Variant::ALL {
        let run = |blocks: &[GhBlock]| {
            evaluate_github(blocks, |code, lang| {
                let r = pl::highlight_with(code, lang, variant);
                (r.scope, r.html)
            })
        };
        let t = run(&tuning);
        let h = run(&heldout);
        println!("{}", github_table(&format!("== GitHub {variant:?}: tuning set"), &t.scores));
        println!("{}", github_table(&format!("== GitHub {variant:?}: held-out set"), &h.scores));
        for (lang, ours, theirs) in t.scope_mismatches.iter().chain(&h.scope_mismatches) {
            println!("scope mismatch: {lang:?} -> ours {ours:?}, GitHub {theirs}");
        }
        println!("unmatched blocks: {} tuning, {} held-out\n", t.unmatched, h.unmatched);
    }
}

/// Port check: the baseline scheme applied to the exact syntect dumps `scope_map.py` used
/// (`rouge_samples/syntect/*.scopes.tsv`) must reproduce `MAPPING_EVAL.txt`.
///
/// `MAPPING_EVAL.txt` says 50.5% / 75.8%; the port gives 50.8% / 76.1% because `scope_map.py`
/// decodes Rouge's `"\#{"` (Ruby `inspect` of `#{`) with Python's `eval`, which keeps the
/// backslash: one extra character shifts every later Ruby character (ruby: 60% → 68% exact once
/// fixed; every other language is identical). With that one fix applied to the Python
/// evaluator it prints 50.8% / 76.1% as well.
#[test]
fn gitlab_baseline_port_reproduces_scope_map_py() {
    let Some(dir) = golden_dir() else {
        skipped("highlight::eval::gitlab_baseline_port_reproduces_scope_map_py");
        return;
    };
    let base = dir.join("gitlab/rouge_samples");
    let scheme = rouge::classes_for_eval(rouge::Variant::Baseline);
    let mut total = RougeScore::default();
    for lang in rouge_sample_langs(&dir) {
        let Ok(dump) = std::fs::read_to_string(base.join(format!("syntect/{lang}.scopes.tsv")))
        else {
            continue;
        };
        let mut ours = Vec::new();
        for line in dump.lines() {
            let Some((stack, text)) = line.split_once('\t') else { continue };
            let stack: Vec<Scope> = stack.split(' ').filter_map(|s| Scope::new(s).ok()).collect();
            let class = scheme.classify(&stack).unwrap_or("");
            for _ in ruby_unescape(text).chars() {
                ours.push(class.to_owned());
            }
        }
        let code = std::fs::read_to_string(base.join(format!("code.{lang}"))).unwrap();
        let tsv = std::fs::read_to_string(base.join(format!("{lang}.tokens.tsv"))).unwrap();
        let (s, _) = score_rouge(&code, &ours, &rouge_char_classes(&tsv));
        println!(
            "{lang:<12} chars={:>5} exact={:>3.0}% family={:>3.0}%",
            s.chars,
            s.exact_pct(),
            s.family_pct()
        );
        total.add(s);
    }
    println!(
        "scope_map.py port on the original dumps: exact={:.1}% family={:.1}%",
        total.exact_pct(),
        total.family_pct()
    );
    assert_eq!(format!("{:.1}", total.exact_pct()), "50.8");
    assert_eq!(format!("{:.1}", total.family_pct()), "76.1");
}

/// Debug aid: `MDR_HL_DUMP=source-rust` prints GitHub's and our HTML line by line for every
/// block of that class (differing lines are marked).
#[test]
fn github_dump() {
    let (Some(dir), Ok(want)) = (golden_dir(), std::env::var("MDR_HL_DUMP")) else { return };
    let mut files: Vec<String> = GITHUB_TUNING.iter().map(|s| s.to_string()).collect();
    files.extend(github_heldout(&dir));
    for block in github_blocks_in(&dir, &files) {
        if block.class != want {
            continue;
        }
        let Some(lang) = &block.lang else { continue };
        let code = format!("{}\n", code_of(&block.html));
        let ours = pl::highlight_with(&code, lang, pl::DEFAULT_VARIANT).html.unwrap_or_default();
        println!("=== {} ({lang})", block.class);
        for (a, b) in block.html.split('\n').zip(ours.split('\n')) {
            if a == b {
                println!("  = {a}");
            } else {
                println!("  G {a}\n  O {b}");
            }
        }
    }
}

/// Debug aid: `MDR_HL_SCOPES=source-rust` prints, for every syntect token of every block of
/// that class whose class path differs from GitHub's, the token, both paths and the scopes.
#[test]
fn github_scopes() {
    let (Some(dir), Ok(want)) = (golden_dir(), std::env::var("MDR_HL_SCOPES")) else { return };
    let mut files: Vec<String> = GITHUB_TUNING.iter().map(|s| s.to_string()).collect();
    files.extend(github_heldout(&dir));
    for block in github_blocks_in(&dir, &files) {
        if block.class != want {
            continue;
        }
        let Some(lang) = &block.lang else { continue };
        let code = format!("{}\n", code_of(&block.html));
        let ours = pl::highlight_with(&code, lang, pl::DEFAULT_VARIANT).html.unwrap_or_default();
        let theirs = class_paths(&block.html);
        let mine = class_paths(&ours);
        let Some(pl::Grammar::Syntect(syntax)) = pl::linguist::find(lang)
            .and_then(|l| pl::grammar(l.tm_scope.as_deref()?, &l.name))
        else {
            continue;
        };
        println!("=== {} ({lang}) syntax {}", block.class, syntax.name);
        let mut pos = 0usize; // in chars
        let _ = super::engine::tokenize(&code, syntax, super::engine::Limits::DEFAULT, |e, stack| {
            if let super::engine::Event::Text(t) = e {
                let n = t.chars().count();
                let differs = (pos..pos + n).any(|i| match (theirs.get(i), mine.get(i)) {
                    (Some(a), Some(b)) => !a.0.is_whitespace() && a.1 != b.1,
                    _ => false,
                });
                if differs && !t.trim().is_empty() {
                    let g = theirs.get(pos).map(|x| x.1.join(">")).unwrap_or_default();
                    let o = mine.get(pos).map(|x| x.1.join(">")).unwrap_or_default();
                    let scopes: Vec<String> = stack.iter().skip(1).map(|s| s.build_string()).collect();
                    println!("TOK\t{t:?}\tG={g}\tO={o}\tS={}", scopes.join(" "));
                }
                pos += n;
            }
        });
    }
}

/// Debug aid: `MDR_HL_DUMP_GL=rust` prints Rouge's and our classes per token of a sample.
#[test]
fn gitlab_dump() {
    let (Some(dir), Ok(lang)) = (golden_dir(), std::env::var("MDR_HL_DUMP_GL")) else { return };
    let base = dir.join("gitlab/rouge_samples");
    let code = std::fs::read_to_string(base.join(format!("code.{lang}"))).unwrap();
    let tsv = std::fs::read_to_string(base.join(format!("{lang}.tokens.tsv"))).unwrap();
    let rouge = rouge_char_classes(&tsv);
    let ours = our_char_classes(&rouge::highlight_with(&code, &lang, rouge::DEFAULT_VARIANT).lines);
    let chars: Vec<char> = code.chars().collect();
    // Print runs of (rouge, ours) classes.
    let mut i = 0;
    while i < chars.len().min(rouge.len()).min(ours.len()) {
        let mut j = i;
        while j < chars.len().min(rouge.len()).min(ours.len())
            && rouge[j] == rouge[i]
            && ours[j] == ours[i]
            && chars[j] != '\n'
        {
            j += 1;
        }
        let j = j.max(i + 1);
        let text: String = chars[i..j].iter().collect();
        if !text.trim().is_empty() {
            let mark = if rouge[i] == ours[i] { " " } else { "X" };
            println!("{mark} rouge={:<4} ours={:<4} {text:?}", rouge[i], ours[i]);
        }
        i = j;
    }
}

#[test]
fn gitlab_variants() {
    let Some(dir) = golden_dir() else {
        skipped("highlight::eval::gitlab_variants");
        return;
    };
    for variant in rouge::Variant::ALL {
        let (scores, conf) =
            evaluate_gitlab(&dir, |code, lang| rouge::highlight_with(code, lang, variant).lines);
        println!("{}", gitlab_table(&format!("== GitLab {variant:?}"), &scores, &conf));
    }
}
