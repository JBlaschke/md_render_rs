//! Robustness of `highlight::github` / `highlight::gitlab` (owner: highlight): no panics on
//! nasty input, deterministic output, well-formed HTML whose text is exactly the code.

use md_render::render::highlight::{GithubCode, github, github_plain, gitlab};

/// Languages covering every special path (shell-session, diff, console, plaintext, verbatim,
/// no-grammar) and the most common grammars.
const LANGS: &[&str] = &[
    "",
    "unknown-lang",
    "text",
    "rust",
    "python",
    "js",
    "ts",
    "tsx",
    "jsx",
    "c",
    "cpp",
    "go",
    "java",
    "ruby",
    "php",
    "html",
    "css",
    "scss",
    "xml",
    "json",
    "yaml",
    "toml",
    "sh",
    "bash",
    "console",
    "shell-session",
    "diff",
    "markdown",
    "sql",
    "dockerfile",
    "makefile",
    "lua",
    "perl",
    "haskell",
    "kotlin",
    "swift",
    "latex",
    "graphql",
    "nix",
    "regex",
    "powershell",
    "mermaid",
    "math",
    "cs",
];

fn nasty_inputs() -> Vec<(String, String)> {
    let mut v: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("space", " ".into()),
        ("newline", "\n".into()),
        ("newlines", "\n\n\n".into()),
        ("whitespace", " \t \n\t\n   \n".into()),
        ("crlf", "a = 1\r\nb = \"x\"\r\n".into()),
        ("lone cr", "a\rb\r".into()),
        ("nul and controls", "\0\u{1}\u{7}\u{1b}[31m\u{7f}x\n".into()),
        ("replacement chars", "\u{FFFD}".repeat(200) + "\n"),
        ("bidi", "let s = \"\u{202E}abc\u{202C}\"; // \u{2066}x\u{2069}\n".into()),
        ("nbsp", "a\u{00A0}=\u{3000}1\n".into()),
        ("unterminated string", "x = \"abc\nnext line\n".into()),
        ("unterminated comment", "/* never closed\nstill comment\n".into()),
        ("unterminated triple", "'''\n\"\"\"\n```\n".into()),
        ("unterminated html comment", "<!-- open\n<div>\n".into()),
        ("raw html soup", "<div><span <a href=\"x\" onclick='y'></b></div><<>>&amp;&&lt;<script>alert(1)</script>\n".into()),
        ("deep parens", "(".repeat(3000) + &")".repeat(3000) + "\n"),
        ("deep brackets", "[".repeat(2000) + "\n" + &"{".repeat(2000) + "\n"),
        ("deep tags", "<a>".repeat(1500) + "\n"),
        ("deep quotes", "> ".repeat(1000) + "x\n"),
        ("deep lists", (0..120).map(|i| format!("{}- item\n", "  ".repeat(i))).collect()),
        ("huge table", "| a | b | c |\n|---|---|---|\n".to_owned() + &"| 1 | 2 | 3 |\n".repeat(200)),
        ("long line 10k", "x".repeat(10_000) + "\n"),
        ("long quotes 10k", "\"".repeat(10_000) + "\n"),
        ("long tokens", "a = 1; ".repeat(1400) + "\n"),
        ("many lines", "x\n".repeat(30_000)),
        ("backtracking bait", "a".repeat(5000) + "!\n" + &"\\".repeat(3000) + "\n"),
        ("diff-ish", "@@ -1 +1 @@\n@@\n-\n+\n!\n<\n>\n---\n+++\n".into()),
        ("prompts", "$\n$ \n#\n> x\n user@host$ ls\n...\n<...>\n".into()),
        ("no final newline", "fn main() { let x = 1; }".into()),
        ("unicode identifiers", "let größe = 1; // コメント\nдлина = 2\n".into()),
        ("emoji", "print(\"🦀🎉\") # 👍\n".into()),
    ];
    // > 512 KiB: refused by the work limits, plain output.
    v.push(("oversized", "let a = 1;\n".repeat(60_000)));
    v.into_iter().map(|(n, s)| (n.to_owned(), s)).collect()
}

/// Remove `<span …>` / `</span>` tags; panics on any other markup or unbalanced spans.
fn strip_spans(html: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("</span>") {
            assert!(depth > 0, "unbalanced </span> in {html:?}");
            depth -= 1;
            rest = r;
        } else if rest.starts_with("<span class=\"") {
            let end = rest.find('>').expect("tag end");
            depth += 1;
            rest = &rest[end + 1..];
        } else {
            panic!(
                "raw '<' in highlighted output: {:?}",
                &rest[..rest.len().min(80)]
            );
        }
    }
    out.push_str(rest);
    assert_eq!(depth, 0, "unclosed span in {html:?}");
    assert!(!out.contains('>'), "raw '>' in highlighted output");
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn check_github(name: &str, lang: &str, code: &str, r: &GithubCode) {
    let Some(html) = &r.html else { return };
    // Spans never cross a newline (GH §7.1).
    for line in html.split('\n') {
        strip_spans(line);
    }
    // The text is the code minus its final newline (GitHub's tree-sitter languages drop all
    // trailing newlines).
    let text = unescape(&strip_spans(html));
    let code = code.replace("\r\n", "\n");
    assert_eq!(
        text.trim_end_matches('\n'),
        code.trim_end_matches('\n'),
        "{name}/{lang}: text differs from the code"
    );
    assert!(
        text.len() < code.len() || !code.ends_with('\n'),
        "{name}/{lang}: final newline kept"
    );
}

fn check_gitlab(name: &str, lang: &str, code: &str, lines: &[String]) {
    let code = code.replace("\r\n", "\n");
    let mut expected: Vec<&str> = code.split('\n').collect();
    if code.is_empty() || code.ends_with('\n') {
        expected.pop();
    }
    assert_eq!(lines.len(), expected.len(), "{name}/{lang}: line count");
    for (line, want) in lines.iter().zip(expected) {
        // The bidi warning wrapper is the only nested span and the only attribute besides class.
        let line = line.replace(
            " has-tooltip\" data-toggle=\"tooltip\" title=\"Potentially unwanted character detected: Unicode BiDi Control",
            "",
        );
        let text = unescape(&strip_spans(&line));
        let want: String = want
            .chars()
            .filter(|&c| c != '\r')
            .map(|c| {
                if matches!(
                    c,
                    '\u{00A0}' | '\u{1680}' | '\u{2000}'
                        ..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
                ) {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        assert_eq!(text, want, "{name}/{lang}: line text");
    }
}

#[test]
fn nasty_inputs_never_panic_and_stay_well_formed() {
    let inputs = nasty_inputs();
    // One thread per language (also exercises concurrent first use of the grammars).
    std::thread::scope(|s| {
        for &lang in LANGS {
            let inputs = &inputs;
            s.spawn(move || {
                for (name, code) in inputs {
                    let gh = github(code, lang);
                    check_github(name, lang, code, &gh);
                    let gl = gitlab(code, lang);
                    check_gitlab(name, lang, code, &gl.lines);
                    assert!(!gl.tag.is_empty());
                }
            });
        }
    });
}

#[test]
fn output_is_deterministic() {
    let code = "fn main() {\n    println!(\"{}\", 1 + 2); // c\n}\n<div class=\"x\">&</div>\n";
    for &lang in LANGS {
        assert_eq!(github(code, lang), github(code, lang), "{lang}");
        assert_eq!(gitlab(code, lang), gitlab(code, lang), "{lang}");
    }
}

#[test]
fn oversized_input_falls_back_to_plain_text() {
    let code = "let a = \"<x>\";\n".repeat(40_000);
    let gh = github(&code, "rust");
    let html = gh.html.expect("known grammar");
    assert!(
        !html.contains("<span"),
        "no token spans beyond the work limits"
    );
    assert!(html.starts_with("let a = \"&lt;x&gt;\";\n"));
    let gl = gitlab(&code, "rust");
    assert_eq!(gl.tag, "rust");
    assert_eq!(gl.lines.len(), 40_000);
    assert_eq!(gl.lines[0], "let a = \"&lt;x&gt;\";");
    assert_eq!(github_plain(&code).len(), html.len());
}

#[test]
fn concurrent_use() {
    let handles: Vec<_> = (0..8)
        .map(|i| {
            std::thread::spawn(move || {
                let lang = LANGS[i % LANGS.len()];
                let code = format!("x{i} = \"{i}\" # {i}\n");
                (github(&code, lang), gitlab(&code, lang))
            })
        })
        .collect();
    for h in handles {
        h.join().expect("no panic in worker");
    }
}

/// The slowest (input, language) pairs of the nasty corpus. Run with
/// `cargo test --release --test highlight_robustness -- --ignored --nocapture nasty_timings`.
#[test]
#[ignore = "timing; run explicitly in release mode"]
fn nasty_timings() {
    use std::io::Write;
    let mut times = Vec::new();
    for (name, code) in nasty_inputs() {
        for &lang in LANGS {
            eprint!("{name} / {lang} ... ");
            let _ = std::io::stderr().flush();
            let t = std::time::Instant::now();
            let _ = github(&code, lang);
            let gh = t.elapsed();
            let t = std::time::Instant::now();
            let _ = gitlab(&code, lang);
            let gl = t.elapsed();
            eprintln!("{gh:?} {gl:?}");
            times.push((gh, gl, name.clone(), lang));
        }
    }
    times.sort_by_key(|t| std::cmp::Reverse(t.0 + t.1));
    for (gh, gl, name, lang) in times.iter().take(25) {
        println!("{:>10.1?} {:>10.1?}  {name} / {lang}", gh, gl);
    }
}

/// Steady-state throughput on a ~20 KB README's worth of code blocks. Run with
/// `cargo test --release --test highlight_robustness -- --ignored --nocapture`.
#[test]
#[ignore = "timing; run explicitly in release mode"]
fn perf_20kb_readme_code() {
    let blocks: Vec<(&str, String)> = vec![
        ("rust", "use std::collections::HashMap;\n\n/// Doc.\n#[derive(Debug, Clone)]\npub struct Point<T> { x: T, y: T }\n\nimpl<T: std::fmt::Display> Point<T> {\n    pub fn new(x: T, y: T) -> Self { Self { x, y } }\n}\n\nfn main() {\n    let mut map: HashMap<&str, i32> = HashMap::new();\n    map.insert(\"one\", 1);\n    println!(\"{:?}\", map.get(\"one\"));\n}\n".repeat(6)),
        ("python", "import os\nfrom dataclasses import dataclass\n\n@dataclass\nclass Settings:\n    \"\"\"Doc.\"\"\"\n    name: str = \"x\"\n    def label(self) -> str:\n        return f\"{self.name!r} {os.sep}\"\n".repeat(8)),
        ("js", "import { x } from './x.js';\nconst re = /ab+c/gi;\nclass A extends B {\n  constructor() { super(); this.n = `${x} y`; }\n}\nfetch('/api').then(r => r.json()).then(console.log);\n".repeat(8)),
        ("console", "$ cargo build --release\n   Compiling foo v0.1.0\n$ ls -la | grep md > out.txt\ntotal 0\n".repeat(10)),
        ("json", "{\n  \"name\": \"md-render\",\n  \"version\": 1.5,\n  \"list\": [1, \"two\", {\"three\": 3}, null, true]\n}\n".repeat(10)),
        ("yaml", "# c\nname: md-render\nlist:\n  - one\n  - \"two\"\nmulti: |\n  line\n".repeat(10)),
        ("diff", "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n ctx\n-old\n+new\n".repeat(10)),
        ("html", "<!DOCTYPE html>\n<html lang=\"en\"><head><style>body { color: red; }</style></head>\n<body><a href=\"x\">&amp;</a><script>console.log(\"hi\");</script></body></html>\n".repeat(6)),
    ];
    let total: usize = blocks.iter().map(|(_, c)| c.len()).sum();
    // Warm up: lazy syntax-set load and per-grammar regex compilation.
    for (lang, code) in &blocks {
        let _ = (github(code, lang), gitlab(code, lang));
    }
    let runs = 20;
    let t = std::time::Instant::now();
    for _ in 0..runs {
        for (lang, code) in &blocks {
            std::hint::black_box(github(code, lang));
        }
    }
    let gh = t.elapsed() / runs;
    let t = std::time::Instant::now();
    for _ in 0..runs {
        for (lang, code) in &blocks {
            std::hint::black_box(gitlab(code, lang));
        }
    }
    let gl = t.elapsed() / runs;
    println!(
        "highlight {total} bytes of code, unchanged blocks (memoized): github {gh:?}, gitlab {gl:?} per pass"
    );
    // Defeat the memo with a unique trailing blank-ish line per pass (grammars warmed up).
    let fresh = |i: u32, code: &str| format!("{code}{i}\n");
    let t = std::time::Instant::now();
    for i in 0..runs {
        for (lang, code) in &blocks {
            std::hint::black_box(github(&fresh(i + 100, code), lang));
        }
    }
    let gh = t.elapsed() / runs;
    let t = std::time::Instant::now();
    for i in 0..runs {
        for (lang, code) in &blocks {
            std::hint::black_box(gitlab(&fresh(i + 200, code), lang));
        }
    }
    let gl = t.elapsed() / runs;
    println!(
        "highlight {total} bytes of code, changed blocks: github {gh:?}, gitlab {gl:?} per pass"
    );
    for (lang, code) in &blocks {
        let t = std::time::Instant::now();
        for i in 0..runs {
            std::hint::black_box(github(&fresh(i + 300, code), lang));
        }
        println!(
            "  {lang:<8} {:>6} bytes: {:?}",
            code.len(),
            t.elapsed() / runs
        );
    }
}
