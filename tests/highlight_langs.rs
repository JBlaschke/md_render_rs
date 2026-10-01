//! Per-language highlighting agreement with github.com on committed samples (owner: highlight).
//!
//! `tests/fixtures/github/highlight-langs/<name>.md` holds one fenced code block per language,
//! written for md-render's review of the GitHub highlighter; `<name>.gh.html` is github.com's
//! rendering of it (`POST /markdown`, mode gfm, recorded 2026-10-01). `jsxjs` is the `jsx`
//! sample in a ` ```js ` fence (github.com highlights both alike).
//!
//! Each sample has a floor for its per-character agreement (full class path and resulting
//! color, as in `tests/highlight_golden.rs`), set to the measured value: class-scheme tuning is
//! measured here and cannot silently regress. Raise a floor when tuning improves a language.

mod highlight_eval;

use std::path::Path;

use highlight_eval::*;
use md_render::render::highlight::github;

/// (sample, minimum path %, minimum color %).
const FLOORS: &[(&str, f64, f64)] = &[
    ("bash", 89.4, 92.8),
    ("c", 75.3, 88.6),
    ("console", 100.0, 100.0),
    ("cpp", 92.3, 93.1),
    ("csharp", 94.3, 95.0),
    ("css", 93.6, 96.3),
    ("diff", 100.0, 100.0),
    ("dockerfile", 99.5, 99.5),
    ("go", 91.5, 96.9),
    ("html", 88.7, 92.2),
    ("java", 84.2, 96.6),
    ("javascript", 90.6, 94.1),
    ("json", 100.0, 100.0),
    ("jsx", 94.6, 97.6),
    ("jsxjs", 94.6, 97.6),
    ("kotlin", 76.1, 76.1),
    ("lua", 98.5, 98.5),
    ("makefile", 72.0, 76.9),
    ("markdown", 94.6, 97.6),
    ("php", 68.5, 80.0),
    // No grammar (two-face's PowerShell needs Oniguruma): plain text.
    ("powershell", 18.0, 32.2),
    ("python", 92.3, 94.8),
    ("ruby", 79.0, 86.8),
    ("rust", 77.4, 87.7),
    ("shell", 89.4, 92.8),
    ("sql", 95.9, 96.3),
    ("swift", 56.3, 84.4),
    ("toml", 100.0, 100.0),
    ("tsx", 95.7, 98.3),
    ("typescript", 86.9, 87.6),
    ("yaml", 97.2, 97.2),
];

fn sample_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/github/highlight-langs")
}

/// Agreement of our output with github.com's for one sample.
fn score(name: &str) -> Score {
    let dir = sample_dir();
    let md = std::fs::read_to_string(dir.join(format!("{name}.md"))).unwrap();
    let html = std::fs::read_to_string(dir.join(format!("{name}.gh.html"))).unwrap();
    let blocks = github_blocks(&html, &md);
    assert_eq!(blocks.len(), 1, "{name}: one highlighted block");
    let eval = evaluate_github(&blocks, |code, lang| {
        let r = github(code, lang);
        (r.scope, r.html)
    });
    assert!(eval.unmatched.is_empty(), "{name}: fenced source found");
    assert!(
        eval.scope_mismatches.is_empty(),
        "{name}: {:?}",
        eval.scope_mismatches
    );
    total(&eval.scores)
}

#[test]
fn every_sample_has_a_floor() {
    let mut names: Vec<String> = std::fs::read_dir(sample_dir())
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".md").map(str::to_owned)
        })
        .collect();
    names.sort();
    let floors: Vec<&str> = FLOORS.iter().map(|f| f.0).collect();
    assert_eq!(names, floors);
}

#[test]
fn agreement_does_not_regress() {
    let mut report = format!(
        "{:<12} {:>6} {:>7} {:>7}\n",
        "sample", "chars", "path%", "color%"
    );
    let mut failures = Vec::new();
    // The measured values rounded down, in `FLOORS` syntax (for raising floors after tuning).
    let mut suggested = String::new();
    let down = |pct: f64| (pct * 10.0).floor() / 10.0;
    for &(name, path_floor, color_floor) in FLOORS {
        let s = score(name);
        assert_eq!(s.text_mismatch, 0, "{name}: recovered text differs");
        report += &format!(
            "{name:<12} {:>6} {:>7.1} {:>7.1}\n",
            s.chars,
            s.path_pct(),
            s.color_pct()
        );
        suggested += &format!(
            "    (\"{name}\", {:.1}, {:.1}),\n",
            down(s.path_pct()),
            down(s.color_pct())
        );
        if s.path_pct() < path_floor || s.color_pct() < color_floor {
            failures.push(format!(
                "{name}: path {:.1}% (floor {path_floor}), color {:.1}% (floor {color_floor})",
                s.path_pct(),
                s.color_pct()
            ));
        }
    }
    println!("{report}\n{suggested}");
    assert!(failures.is_empty(), "{failures:#?}");
}
