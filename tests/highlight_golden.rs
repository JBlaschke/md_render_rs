//! Highlighting fidelity against real site output (owner: highlight; D18 golden data from
//! `$MD_RENDER_GOLDEN_DIR`, skipped loudly when absent).
//!
//! * GitHub: per-character agreement with github.com's `pl-*` spans on the API outputs under
//!   `github/` (tuning set: the files the token tables were made from; held-out: real READMEs
//!   and comments).
//! * GitLab: per-character agreement with Rouge 5.1.0 on `gitlab/rouge_samples` (36 languages),
//!   counted exactly like `R/tools/scope_map.py`; must beat its baseline (50.5% exact, 75.8%
//!   family in `MAPPING_EVAL.txt`).

mod highlight_eval;

use highlight_eval::*;
use md_render::render::highlight::{github, gitlab};

#[test]
fn github_agreement() {
    let Some(dir) = golden_dir() else {
        skipped("highlight_golden::github_agreement");
        return;
    };
    let run = |files: &[String]| {
        let blocks = github_blocks_in(&dir, files);
        evaluate_github(&blocks, |code, lang| {
            let r = github(code, lang);
            (r.scope, r.html)
        })
    };
    let tuning = run(&GITHUB_TUNING.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let heldout = run(&github_heldout(&dir));
    println!("{}", github_table("GitHub tuning set", &tuning.scores));
    println!("{}", github_table("GitHub held-out set", &heldout.scores));
    // Every info word resolves to GitHub's highlight-<scope> class.
    for e in [&tuning, &heldout] {
        assert!(e.scope_mismatches.is_empty(), "{:?}", e.scope_mismatches);
        assert!(e.scores.values().all(|s| s.text_mismatch == 0), "recovered text differs");
    }
    let (t, h) = (total(&tuning.scores), total(&heldout.scores));
    // Measured 2026-10-01: tuning 90.1% path / 93.3% color, held-out 91.8% / 92.2%.
    assert!(t.path_pct() >= 88.0 && t.color_pct() >= 91.0, "tuning: {t:?}");
    assert!(h.path_pct() >= 88.0 && h.color_pct() >= 90.0, "held-out: {h:?}");
}

#[test]
fn gitlab_agreement_beats_scope_map_baseline() {
    let Some(dir) = golden_dir() else {
        skipped("highlight_golden::gitlab_agreement_beats_scope_map_baseline");
        return;
    };
    let (scores, confusions) = evaluate_gitlab(&dir, |code, lang| gitlab(code, lang).lines);
    println!("{}", gitlab_table("GitLab vs Rouge 5.1.0", &scores, &confusions));
    assert_eq!(scores.len(), 36, "all Rouge samples evaluated");
    let t = gitlab_total(&scores);
    // The Python prototype's baseline (MAPPING_EVAL.txt).
    assert!(t.exact_pct() >= 50.5, "{t:?}");
    assert!(t.family_pct() >= 75.8, "{t:?}");
    // Measured 2026-10-01: 82.9% exact / 90.2% family.
    assert!(t.exact_pct() >= 80.0 && t.family_pct() >= 88.0, "{t:?}");
    // The exact ports are exact.
    for lang in ["diff", "console", "json", "plaintext"] {
        let s = scores[lang];
        assert_eq!(s.exact, s.chars, "{lang}: {s:?}");
    }
}
