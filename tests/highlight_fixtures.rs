//! Committed highlight fixtures (owner: highlight): inputs authored for md-render
//! (`tests/fixtures/{github,gitlab}/highlight/NN-<lang>.code`) with the real outputs of
//! github.com (`POST /markdown`, mode gfm; `.html`: `highlight-` class, then the `<pre>` inner
//! HTML) and of Rouge 5.1.0 with GitLab's `HTMLGitlab` formatter (`.lines`: tag, then one line
//! per source line), recorded 2026-10-01.
//!
//! Blocks listed in `*_EXACT` must be reproduced byte for byte; for the others (tree-sitter
//! languages on github.com, Rouge lexers with only approximate scope mappings) the text must be
//! identical and the per-character agreement high.

mod highlight_eval;

use std::path::{Path, PathBuf};

use md_render::render::highlight::{github, gitlab};

const GITHUB_EXACT: &[&str] = &[
    "02-json",
    "03-yaml",
    "04-toml",
    "05-diff",
    "06-console",
    "11-go",
];
const GITLAB_EXACT: &[&str] = &[
    "01-python",
    "02-json",
    "05-diff",
    "06-console",
    "07-ts",
    "08-sh",
    "09-rust",
    "10-js",
    "11-go",
];

fn fixtures(flavor: &str, ext: &str) -> Vec<(String, String, String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(flavor)
        .join("highlight");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixture dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "code"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|p| {
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            let lang = stem.split_once('-').unwrap().1.to_owned();
            let code = std::fs::read_to_string(&p).unwrap();
            let expected = std::fs::read_to_string(p.with_extension(ext)).unwrap();
            (stem, lang, code, expected)
        })
        .collect()
}

#[test]
fn github_fixtures() {
    for (name, lang, code, expected) in fixtures("github", "html") {
        let (class, want) = expected.split_once('\n').unwrap();
        let ours = github(&code, &lang);
        let scope = ours
            .scope
            .as_deref()
            .map(|s| format!("highlight-{}", s.replace('.', "-")));
        assert_eq!(scope.as_deref(), Some(class), "{name}: scope");
        let html = ours.html.expect("grammar");
        let score = highlight_eval::score_github(want, &html);
        println!("{name}: exact={} {score:?}", html == want);
        assert_eq!(score.text_mismatch, 0, "{name}: text");
        if GITHUB_EXACT.contains(&name.as_str()) {
            assert_eq!(html, want, "{name}");
        } else {
            assert!(score.color_pct() >= 85.0, "{name}: {score:?}");
        }
    }
}

#[test]
fn gitlab_fixtures() {
    for (name, lang, code, expected) in fixtures("gitlab", "lines") {
        let (tag, want) = expected.split_once('\n').unwrap();
        let want: Vec<&str> = want.split('\n').collect();
        let ours = gitlab(&code, &lang);
        assert_eq!(ours.tag, tag, "{name}: tag");
        assert_eq!(ours.lines.len(), want.len(), "{name}: line count");
        let mine = highlight_eval::our_char_classes(&ours.lines);
        let theirs = highlight_eval::our_char_classes(
            &want.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        let (score, _) = highlight_eval::score_rouge(&code, &mine, &theirs);
        let exact = ours
            .lines
            .iter()
            .map(String::as_str)
            .eq(want.iter().copied());
        println!("{name}: exact={exact} {score:?}");
        for (a, b) in want.iter().zip(&ours.lines) {
            if a != b {
                println!("  R {a}\n  O {b}");
            }
        }
        if GITLAB_EXACT.contains(&name.as_str()) {
            assert_eq!(ours.lines, want, "{name}");
        } else {
            assert!(score.exact_pct() >= 80.0, "{name}: {score:?}");
        }
    }
}
