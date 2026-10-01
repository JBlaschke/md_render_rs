//! Integration tests for the GitLab offline renderer: hand-written fixtures
//! (`tests/fixtures/gitlab/<name>.md` -> `<name>.html`), the `Rendered` contract, robustness on
//! hostile inputs, determinism and speed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use md_render::render::{RenderOptions, Rendered, render};
use md_render::{Flavor, Mode};

fn opts() -> RenderOptions {
    RenderOptions::new(Flavor::Gitlab)
}

fn gl(md: &str) -> Rendered {
    render(md, &opts())
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gitlab")
}

/// Every `tests/fixtures/gitlab/*.md` renders exactly to its `.html` (with source positions; the
/// file is rendered as if served from the fixture directory). `UPDATE_FIXTURES=1` rewrites them.
#[test]
fn fixtures() {
    let dir = fixtures_dir();
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixture dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "md") && !p.file_stem().unwrap().to_string_lossy().starts_with('_'))
        .collect();
    names.sort();
    assert!(!names.is_empty());
    let update = std::env::var("UPDATE_FIXTURES").is_ok_and(|v| v == "1");
    let mut failures = Vec::new();
    for md_path in names {
        let md = std::fs::read_to_string(&md_path).unwrap();
        let mut o = opts();
        o.source_path = Some(md_path.clone());
        o.root = Some(dir.clone());
        let html = render(&md, &o).html + "\n";
        let expected_path = md_path.with_extension("html");
        if update {
            std::fs::write(&expected_path, &html).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&expected_path).unwrap_or_default();
        if highlight_neutral(&html) != highlight_neutral(&expected) {
            failures.push(format!("{}:\n--- expected\n{expected}\n--- got\n{html}", md_path.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Token spans and Rouge tags belong to the highlighter (another module); fixtures pin the
/// GitLab markup around them. `FIXTURES_STRICT=1` compares them too.
fn highlight_neutral(html: &str) -> String {
    if std::env::var("FIXTURES_STRICT").is_ok_and(|v| v == "1") {
        return html.to_owned();
    }
    let doc = dom_query::Document::fragment(html);
    for line in doc.select("pre.code span.line").nodes() {
        line.set_attr("data-lang", "*");
    }
    for span in doc.select("pre.code span.line span").nodes().iter().rev() {
        while let Some(c) = span.first_child() {
            span.insert_before(&c);
        }
        span.remove_from_parent();
    }
    for pre in doc.select("pre.code").nodes() {
        let class = pre.attr("class").unwrap_or_default().to_string();
        let class: Vec<&str> = class
            .split_ascii_whitespace()
            .map(|c| if c.starts_with("language-") { "language-*" } else { c })
            .collect();
        pre.set_attr("class", &class.join(" "));
    }
    doc.html_root().inner_html().to_string()
}

#[test]
fn rendered_contract() {
    let r = gl("# Title *here* :smile:\n\nInline $`a^2`$ and\n\n```mermaid\ngraph TD\n```\n");
    assert_eq!(r.title.as_deref(), Some("Title here 😄"));
    assert!(r.features.math);
    assert!(r.features.mermaid);
    assert!(r.warnings.is_empty());

    let plain = gl("no title\n\n## h2 only\n");
    assert_eq!(plain.title, None);
    assert!(!plain.features.math && !plain.features.mermaid);

    let empty = gl("");
    assert_eq!(empty, Rendered::default());
    assert_eq!(gl("\u{FEFF}").html, "");
}

#[test]
fn sourcepos_toggle() {
    let md = "# A\n\n- [x] done\n\n```js\nx\n```\n";
    let with = gl(md).html;
    assert!(with.contains("data-sourcepos=\"1:1-1:3\""));
    assert!(with.contains("data-checkbox-sourcepos="));
    let mut o = opts();
    o.sourcepos = false;
    let without = render(md, &o).html;
    assert!(!without.contains("sourcepos"), "{without}");
    assert!(without.contains("id=\"user-content-a\""));
}

#[test]
fn includes_in_readme_mode_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("part.md"), "**included**\n").unwrap();
    std::fs::write(dir.path().join("main.md"), "").unwrap();
    let md = "before\n\n::include{file=part.md}\n\n::include{file=missing.md}\n";
    let mut o = opts();
    o.source_path = Some(dir.path().join("main.md"));
    o.root = Some(dir.path().to_path_buf());
    let r = render(md, &o);
    assert!(r.html.contains("<strong data-sourcepos=\"3:1-3:12\">included</strong>"), "{}", r.html);
    assert!(r.html.contains("Error including '"), "{}", r.html);
    assert_eq!(r.warnings, vec!["::include{file=missing.md}: not found".to_owned()]);
    let files = md_render::render::gitlab::included_files(md, &o);
    assert_eq!(files.len(), 1);

    o.mode = Mode::Comment;
    let c = render(md, &o);
    assert!(c.html.contains("::include{file=part.md}"));
    assert!(c.warnings.is_empty());
    assert!(md_render::render::gitlab::included_files(md, &o).is_empty());
}

#[test]
fn includes_cannot_escape_the_root() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(outer.path().join("secret.md"), "SECRET").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outer.path().join("secret.md"), root.join("link.md")).unwrap();
    let mut o = opts();
    o.source_path = Some(root.join("README.md"));
    o.root = Some(root.clone());
    for md in ["::include{file=../secret.md}", "::include{file=/../secret.md}", "::include{file=link.md}"] {
        let r = render(md, &o);
        assert!(!r.html.contains("SECRET"), "{md}: {}", r.html);
    }
    // Absolute (repository-root relative) paths stay inside the root.
    std::fs::write(root.join("top.md"), "TOP").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    o.source_path = Some(root.join("sub/README.md"));
    assert!(render("::include{file=/top.md}", &o).html.contains("TOP"));
}

/// Hostile inputs: nothing panics, output is deterministic, and the sanitizer never lets
/// scripts, event handlers or `javascript:` URLs through.
#[test]
fn nasty_inputs() {
    let mut corpus: Vec<String> = vec![
        String::new(),
        " ".into(),
        "\n\n\n".into(),
        "\t \u{a0}\r\n".into(),
        "\u{FEFF}".into(),
        "\u{FFFD}\u{FFFD} \u{0}\u{1}\u{7f} \u{FFFE}".into(),
        "```".into(),
        "```js\nunterminated".into(),
        "~~~\n```\n".into(),
        "$$\nunterminated math".into(),
        "$`x".into(),
        "---\n".into(),
        "---\nfront: matter\n".into(),
        "+++\n+++\n".into(),
        "::include{file=}".into(),
        "::include{file=/}".into(),
        "::include{file=..}".into(),
        "::include{file=\0}".into(),
        "[[_TOC_]]\n[TOC]\n[[_TOC_]]".into(),
        "[^1]\n\n[^1]: [^1]".into(),
        "- [~] x\n- [?] y\n- [ ]\n|a|\n|-|\n|[x]|".into(),
        "<script>alert(1)</script><img src=x onerror=alert(1)><a href=\"javascript:alert(1)\">x</a>".into(),
        "<svg><script>alert(1)</script><a xlink:href=\"javascript:alert(1)\">x</a></svg>".into(),
        "<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>".into(),
        "<table><tr><td><table><tr><td>deep</td></tr></table></td></tr></table><li>x<td>y".into(),
        "<a href=\"x\"><a href=\"y\">nested</a></a><form><input type=checkbox class=task-list-item-checkbox>".into(),
        "<iframe src=\"javascript:alert(1)\"></iframe><iframe src=\"https://e.com\">x</iframe>".into(),
        "<div style=\"background:url(javascript:alert(1))\" onclick=\"x\">s</div>".into(),
        "<template><a href=\"javascript:x\">t</a></template><noscript><p>x</p></noscript>".into(),
        "<!-- unterminated comment".into(),
        "<div id=\"foo\"\n*hi*".into(),
        "<a href=\"foo".into(),
        "![x](javascript:alert(1)) [y](vbscript:x) [z](data:text/html,x) [w](JaVaScRiPt:x)".into(),
        "{+ a [- b -] +} [+ +] {- -}".into(),
        ":smile::+1::-1::1234: :smile:a :smile:".into(),
        "👨‍👩‍👧‍👦🏳️‍🌈🇺🇳🏴󠁧󠁢󠁳󠁣󠁴󠁿1️⃣#️⃣ ‍ \u{FE0F}\u{200D}".into(),
        "`#fff` `\\#fff` `rgb(1,2,3)` `hsl(1rad, 1%, 1%)`".into(),
        "![v](a.mp4) ![a](b.mp3){width=99} ![i](c.png){.float-left}".into(),
        "> [!NOTE]\n> x\n\n>>> [!warning] T\ny\n>>>".into(),
        "Term\n: Definition\n".into(),
        "[[Wiki Link]] [[a|b]] [[https://e.com|x]]".into(),
        "\u{202E}[rtl](https://e.com/\u{202E})".into(),
        "<p dir=rtl align=right>x</p><h7>h7</h7><bdo dir=rtl>y</bdo>".into(),
    ];
    corpus.push("> ".repeat(500) + "deep quote");
    corpus.push((0..400).map(|i| format!("{}- item\n", "  ".repeat(i))).collect());
    corpus.push("<div>".repeat(3000) + "x");
    corpus.push("[".repeat(5000) + &"]".repeat(5000));
    corpus.push("*".repeat(10_000));
    corpus.push("x".repeat(10_000));
    corpus.push(format!("{}\n", "a ".repeat(5000)));
    corpus.push("| a | b |\n|---|---|\n".to_owned() + &"| 1 | 2 |\n".repeat(3000));
    corpus.push(format!("| {} |\n|{}|\n", vec!["h"; 500].join(" | "), vec!["-"; 500].join("|")));
    corpus.push(":smile: ".repeat(1500));
    corpus.push("😄".repeat(1500));
    corpus.push("$x$ ".repeat(1500));
    corpus.push("```\n".repeat(1000));

    for md in &corpus {
        let a = gl(md);
        let b = gl(md);
        assert_eq!(a, b, "non-deterministic output for {:?}", truncate(md));
        let lower = a.html.to_ascii_lowercase();
        assert!(!lower.contains("<script"), "script survived: {:?}", truncate(md));
        assert!(!lower.contains("javascript:"), "javascript: survived: {:?} -> {}", truncate(md), truncate(&a.html));
        assert!(!lower.contains("vbscript:"), "vbscript: survived: {:?}", truncate(md));
        assert!(!has_event_handler(&lower), "event handler survived: {:?}", truncate(md));
        assert!(!lower.contains("<iframe"), "iframe survived: {:?}", truncate(md));
        assert!(!lower.contains("<style"), "style survived: {:?}", truncate(md));
        let mut o = opts();
        o.sourcepos = false;
        o.mode = Mode::Comment;
        let _ = render(md, &o);
    }
}

fn has_event_handler(html: &str) -> bool {
    // ` on<letters>=` inside a tag.
    let bytes = html.as_bytes();
    let mut in_tag = false;
    for i in 0..bytes.len() {
        match bytes[i] {
            b'<' => in_tag = true,
            b'>' => in_tag = false,
            b' ' | b'\n' | b'\t' if in_tag => {
                let rest = &html[i + 1..];
                if rest.starts_with("on") {
                    let name: String = rest[2..].chars().take_while(|c| c.is_ascii_alphabetic()).collect();
                    if !name.is_empty() && rest[2 + name.len()..].starts_with('=') {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn truncate(s: &str) -> String {
    s.chars().take(120).collect()
}

/// A ~20 KB README (synthesized from the committed fixtures) renders fast; the measured time
/// is printed (`--nocapture`). The bound is loose so slow CI machines and debug builds pass; the
/// release-mode steady state is far below it (see the report in the PR description).
#[test]
fn speed_20kb_readme() {
    let mut md = String::new();
    let dir = fixtures_dir();
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    sources.sort();
    while md.len() < 20_000 {
        for p in &sources {
            md.push_str(&std::fs::read_to_string(p).unwrap());
            md.push_str("\n\n");
        }
    }
    let o = opts();
    let _ = render(&md, &o); // warm-up (emoji DB, regexes)
    let runs = 20;
    let start = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(render(std::hint::black_box(&md), &o));
    }
    let per = start.elapsed() / runs;
    println!("gitlab render of {} bytes: {:?} per render (steady state)", md.len(), per);
    let bound = if cfg!(debug_assertions) { Duration::from_secs(5) } else { Duration::from_millis(100) };
    assert!(per < bound, "too slow: {per:?}");
}
