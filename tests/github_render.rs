//! Integration tests for the GitHub flavor offline renderer (`md_render::render::github`):
//! committed hand-written fixtures, the output contract (GH §9), sanitizer guarantees,
//! determinism, robustness on hostile input, and a speed check.
//!
//! Fixtures: `tests/fixtures/github/<name>.md` with expected `<name>.html` (README context) and
//! optional `<name>.comment.html` (comment context). Regenerate after a reviewed change with
//! `UPDATE_FIXTURES=1 cargo test --test github_render`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use md_render::flavor::{Flavor, Mode};
use md_render::render::{RenderOptions, Rendered, github};

fn opts(mode: Mode, sourcepos: bool) -> RenderOptions {
    let mut o = RenderOptions::new(Flavor::Github);
    o.mode = mode;
    o.sourcepos = sourcepos;
    o.bundle_url = "/__/md-render/bundle".into();
    o
}

fn readme(md: &str) -> Rendered {
    github::render(md, &opts(Mode::Readme, false))
}

fn comment(md: &str) -> Rendered {
    github::render(md, &opts(Mode::Comment, false))
}

// ------------------------------------------------------------------------------------------
// fixtures

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/github")
}

#[test]
fn fixtures() {
    let update = std::env::var_os("UPDATE_FIXTURES").is_some();
    let mut inputs: Vec<PathBuf> = std::fs::read_dir(fixture_dir())
        .expect("tests/fixtures/github exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    inputs.sort();
    assert!(!inputs.is_empty());
    let mut failures = Vec::new();
    for input in inputs {
        let src = std::fs::read_to_string(&input).unwrap();
        for (mode, suffix) in [(Mode::Readme, "html"), (Mode::Comment, "comment.html")] {
            let expected_path = input.with_extension(suffix);
            if mode == Mode::Comment && !expected_path.exists() && !update {
                continue;
            }
            let got = github::render(&src, &opts(mode, false)).html;
            if update {
                std::fs::write(&expected_path, &got).unwrap();
                continue;
            }
            let want = std::fs::read_to_string(&expected_path)
                .unwrap_or_else(|_| panic!("missing {}", expected_path.display()));
            if got != want {
                let at = got
                    .bytes()
                    .zip(want.bytes())
                    .take_while(|(a, b)| a == b)
                    .count();
                let ctx = |s: &str| {
                    s.get(at.saturating_sub(80)..(at + 120).min(s.len()))
                        .unwrap_or("")
                        .to_owned()
                };
                failures.push(format!(
                    "{} differs at byte {at}:\n--- got ---\n{}\n--- want ---\n{}",
                    expected_path.display(),
                    ctx(&got),
                    ctx(&want)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

// ------------------------------------------------------------------------------------------
// contract (GH §9.A / §9.B)

#[test]
fn readme_context_basics() {
    let r = readme("# Hello *World*\n\npara\nsoft break\n\n## Hello *World*\n");
    assert_eq!(r.title.as_deref(), Some("Hello World"));
    assert!(r.html.contains(r##"<div class="markdown-heading" dir="auto"><h1 class="heading-element" dir="auto">Hello <em>World</em></h1><a id="user-content-hello-world" class="anchor" aria-label="Permalink: Hello World" href="#hello-world"><svg data-component="Octicon" class="octicon octicon-link""##), "{}", r.html);
    assert!(r.html.contains(r##"href="#hello-world-1""##));
    assert!(
        r.html.contains("<p dir=\"auto\">para\nsoft break</p>"),
        "soft breaks stay newlines"
    );
    assert_eq!(r.features, Default::default());
    assert!(r.warnings.is_empty());
}

#[test]
fn comment_context_differences() {
    let r = comment("## Head\n\na\nb `#00ff00` `x`\n\n| a |\n|---|\n| 1 |\n");
    assert!(
        r.html.contains("<h2 dir=\"auto\">Head</h2>"),
        "no anchors in comments: {}",
        r.html
    );
    assert!(r.html.contains("a<br>\nb"), "hard breaks: {}", r.html);
    assert!(r.html.contains(r#"<code class="notranslate">#00ff00<span class="ml-1 d-inline-block border circle color-border-subtle" style="background-color: #00ff00; height: 8px; width: 8px;"></span></code>"#));
    assert!(r.html.contains(r#"<code class="notranslate">x</code>"#));
    assert!(r.html.contains(r#"<table role="table">"#));
}

#[test]
fn front_matter() {
    let md = "---\ntitle: Usage\ndate: 2021-07-02T00:00:00Z\n---\n# Doc\n";
    let r = readme(md);
    assert!(r.html.starts_with("<markdown-accessiblity-table><table>\n  <tbody>\n  <tr>\n    <th>title</th>\n    <td>Usage</td>\n  </tr>\n  <tr>\n    <th>date</th>\n    <td>2021-07-02 00:00:00 UTC</td>\n  </tr>\n  </tbody>\n</table></markdown-accessiblity-table>\n\n"), "{}", r.html);
    assert_eq!(r.title.as_deref(), Some("Doc"));
    let with_sp = github::render(md, &opts(Mode::Readme, true));
    assert!(
        with_sp
            .html
            .starts_with(r#"<markdown-accessiblity-table data-sourcepos="1:1-4:3">"#)
    );
    assert!(
        with_sp.html.contains(r#"data-sourcepos="5:1-5:5""#),
        "line numbers are those of the file"
    );

    // Comment context: no table (hr + setext heading, as on github.com).
    let c = comment(md);
    assert!(c.html.starts_with("<hr>"), "{}", c.html);

    // Invalid YAML: rendered as Markdown, with a warning.
    let bad = readme("---\na: [\n---\nbody\n");
    assert_eq!(bad.warnings.len(), 1);
    assert!(bad.warnings[0].contains("front matter"));
    assert!(bad.html.starts_with("<hr>"));
}

#[test]
fn features_and_keys() {
    let r = readme("$x$\n\n```mermaid\ngraph TD\n```\n\n```rust\nfn x() {}\n```\n");
    assert!(r.features.math && r.features.mermaid);
    assert_eq!(r.html.matches("data-mdr-key=").count(), 3, "{}", r.html);
    let k1 = readme("a $x$ b").html;
    let k2 = readme("a $y$ b").html;
    assert_ne!(k1, k2);
    assert!(!readme("plain").features.math);
}

#[test]
fn sourcepos_only_on_top_level_blocks() {
    let md = "# T\n\n> quote *em*\n\n- a\n- b\n\n<div>\n\nraw\n\n</div>\n\n| x |\n|---|\n| 1 |\n";
    let r = github::render(md, &opts(Mode::Readme, true));
    let html = &r.html;
    assert!(
        html.contains(r#"<div class="markdown-heading" dir="auto" data-sourcepos="1:1-1:3">"#)
            || html.contains(r#"data-sourcepos="1:1-1:3""#)
    );
    assert!(
        html.contains(r#"<blockquote data-sourcepos="3:1-3:12">"#),
        "{html}"
    );
    assert!(!html.contains("<em data-sourcepos"));
    assert!(!html.contains("<li data-sourcepos"));
    assert!(
        html.contains(r#"<markdown-accessiblity-table data-sourcepos="14:1-16:5">"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<div data-sourcepos="8:1-8:5" dir="auto">"#)
            || html.contains("<div dir=\"auto\" data-sourcepos=\"8:1-8:5\">"),
        "raw HTML blocks get theirs: {html}"
    );
    assert!(!html.contains("mdr-sp:"), "marker comments are removed");
    let no_sp = readme(md).html;
    assert!(!no_sp.contains("data-sourcepos"));
}

#[test]
fn deterministic_and_run_ids() {
    let md = "x[^1] $y$\n\n[^1]: note\n";
    let a = readme(md);
    let b = readme(md);
    assert_eq!(a, b);
    let mut o = opts(Mode::Readme, false);
    o.source_path = Some("docs/a.md".into());
    let p1 = github::render(md, &o).html;
    let p2 = github::render(&format!("{md}\nmore\n"), &o).html;
    let rid = |h: &str| h.split("data-run-id=\"").nth(1).map(|s| s[..32].to_owned());
    assert_eq!(
        rid(&p1),
        rid(&p2),
        "run ids stay stable across edits of the same file"
    );
    assert!(p1.contains("user-content-fn-1-") && p1.contains("data-footnote-backref"));
}

// ------------------------------------------------------------------------------------------
// sanitizer guarantees (DESIGN §9)

#[test]
fn sanitizer_never_lets_scripts_through() {
    let nasty = [
        "<script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "<a href=\"javascript:alert(1)\">x</a>",
        "<a href=\"jav&#x09;ascript:alert(1)\">x</a>",
        "[x](javascript:alert(1))",
        "![x](javascript:alert(1))",
        "<svg><script>alert(1)</script></svg>",
        "<iframe src=\"https://evil\"></iframe>",
        "<div style=\"background:url(javascript:alert(1))\" onclick=\"x()\">d</div>",
        "<math><mi xlink:href=\"javascript:alert(1)\">x</mi></math>",
        "<object data=\"x\"></object><embed src=\"x\">",
        "<details open ontoggle=alert(1)>x</details>",
        "<form action=\"javascript:alert(1)\"><button formaction=\"javascript:x\">b</button></form>",
        "<a href=\"data:text/html,<script>alert(1)</script>\">d</a>",
        "<style>*{}</style><xmp><script></xmp><noscript><script>x</script></noscript>",
        "<template><script>alert(1)</script></template>",
        "<div data-mdr-g=\"0000000000000000\" class=\"markdown-alert\" onclick=\"x\">forged</div>",
        "<<script>script>alert(1)<</script>/script>",
        "<input type=checkbox checked onfocus=x autofocus>",
    ];
    for mode in [Mode::Readme, Mode::Comment] {
        for n in nasty {
            let html = github::render(n, &opts(mode, true))
                .html
                .to_ascii_lowercase();
            assert!(!html.contains("<script"), "{n:?} -> {html}");
            assert!(
                !html.contains("javascript:alert") || !html.contains("href=\"javascript"),
                "{n:?} -> {html}"
            );
            assert!(!html.contains("href=\"javascript"), "{n:?} -> {html}");
            assert!(!html.contains("src=\"javascript"), "{n:?} -> {html}");
            for attr in [
                " onerror",
                " onclick",
                " ontoggle",
                " onfocus",
                " style=\"background",
                " formaction",
                " xlink:href",
            ] {
                assert!(!html.contains(attr), "{n:?} -> {html}");
            }
            assert!(
                !html.contains("<iframe") && !html.contains("<object") && !html.contains("<embed"),
                "{html}"
            );
            assert!(!html.contains("data-mdr-g"), "{html}");
            assert!(
                !html.contains("class=\"markdown-alert\""),
                "raw lookalikes lose their class: {html}"
            );
            assert!(!html.contains("<input"), "raw inputs are removed: {html}");
        }
    }
}

// ------------------------------------------------------------------------------------------
// robustness

fn nasty_corpus() -> Vec<String> {
    let mut v: Vec<String> = vec![
        String::new(),
        " ".into(),
        "\n\n\n".into(),
        "\t \r\n \r".into(),
        "\u{feff}".into(),
        "\0\0\0".into(),
        "\u{fffd}\u{fffd} \u{fffe}\u{ffff}".into(),
        "```".into(),
        "```rust\nfn main() {".into(),
        "~~~math\n\\frac{".into(),
        "```mermaid".into(),
        "$".into(),
        "$$".into(),
        "$$$$".into(),
        "$`".into(),
        "[^".into(),
        "[^1]".into(),
        "[^1]:".into(),
        "> [!NOTE]".into(),
        "> [!NOTE]\n>".into(),
        "- [ ]".into(),
        "- [x]\n  - [ ]\n    - [X]".into(),
        "---".into(),
        "---\n".into(),
        "---\n---".into(),
        "---\n: : :\n---\n".into(),
        "---\n- a\n- b\n---\n".into(),
        "---\n!!binary aGVsbG8=\n---\n".into(),
        "---\n&a [*a]\n---\n".into(),
        "<".into(),
        "<a".into(),
        "</".into(),
        "<!--".into(),
        "<![CDATA[".into(),
        "<?php".into(),
        "<div><p><table><tr><td><h1><pre><code>".into(),
        "<table><caption><table><caption>x".into(),
        "<svg><foreignObject><div><svg><math><mtext><table>".into(),
        "<select><option><optgroup><keygen><textarea>".into(),
        "<frameset><frame><noframes>".into(),
        "<pre lang=\"mermaid\">a</pre><pre lang=\"math\">b</pre><pre lang=\"\">c</pre>".into(),
        "<h1></h1><h2> </h2><h3><img></h3>".into(),
        "<details><details><summary>".into(),
        "<picture><picture><source><img>".into(),
        "&#0; &#xD800; &#1114112; &bogus; &amp".into(),
        ":smile::::::+1::-1::\u{fe0f}\u{fe0f}".into(),
        "\u{200d}\u{fe0f}\u{20e3}".into(),
        "| a |\n|---|".into(),
        "|\n|-|\n|".into(),
        "* * *\n- - -\n___".into(),
        "[a]: <\n[b]: \n".into(),
        "![](<>)[](<>)".into(),
        "www. http:// https://. mailto: a@b".into(),
        "\\".into(),
        "`".into(),
        "``````````".into(),
    ];
    v.push(">".repeat(5000));
    v.push("> ".repeat(2000) + "x");
    v.push(
        (0..2000)
            .map(|i| format!("{}- item\n", "  ".repeat(i)))
            .collect(),
    );
    v.push("- ".repeat(3000) + "x");
    v.push("1. ".repeat(2000) + "x");
    v.push("[".repeat(10_000));
    v.push("*".repeat(10_000));
    v.push("_a ".repeat(5000));
    v.push("<b>".repeat(3000));
    v.push("<div>".repeat(3000));
    v.push("<table>".repeat(1000));
    v.push("$".repeat(10_000));
    v.push("$a$ ".repeat(3000));
    v.push(":smile: ".repeat(3000));
    v.push("x".repeat(10_000));
    v.push(format!("# {}", "h".repeat(10_000)));
    v.push(format!("```\n{}\n```", "c".repeat(10_000)));
    v.push(format!("|{}\n|{}\n", "a|".repeat(500), "-|".repeat(500)));
    v.push(format!(
        "| a | b |\n|---|---|\n{}",
        "| 1 | 2 |\n".repeat(3000)
    ));
    v.push("[^x]: y\n".repeat(1000) + &"[^x] ".repeat(1000));
    v.push(
        (0..500).map(|i| format!("[^{i}] ")).collect::<String>()
            + &(0..500)
                .map(|i| format!("\n[^{i}]: n{i}\n"))
                .collect::<String>(),
    );
    v.push(format!(
        "---\na: {}{}\n---\n",
        "[".repeat(300),
        "]".repeat(300)
    ));
    v.push("# dup\n".repeat(2000));
    v.push("https://x.com/".repeat(1000));
    v.push(format!("<p>{}</p>", "https://e.com/a ".repeat(500)));
    v
}

#[test]
fn hostile_inputs_never_panic() {
    for (i, input) in nasty_corpus().iter().enumerate() {
        for mode in [Mode::Readme, Mode::Comment] {
            for sp in [false, true] {
                let r = std::panic::catch_unwind(|| github::render(input, &opts(mode, sp)));
                let r = r.unwrap_or_else(|_| {
                    panic!("panic on corpus entry {i} ({mode:?}, sourcepos {sp})")
                });
                assert!(
                    !r.html.contains("data-mdr-g"),
                    "marker leaked for entry {i}"
                );
                assert!(
                    !r.html.contains("mdr-octicon"),
                    "placeholder leaked for entry {i}"
                );
            }
        }
    }
}

#[test]
fn hostile_inputs_are_fast_enough() {
    // Catch quadratic blow-ups (a debug build is ~10x slower than release).
    for (i, input) in nasty_corpus().iter().enumerate() {
        let t = Instant::now();
        let _ = github::render(input, &opts(Mode::Readme, true));
        let dt = t.elapsed();
        if dt > Duration::from_millis(100) {
            println!("corpus entry {i} ({} bytes) took {dt:?}", input.len());
        }
        assert!(dt < Duration::from_secs(10), "corpus entry {i} took {dt:?}");
    }
}

// ------------------------------------------------------------------------------------------
// speed (DESIGN §3.2: < 10 ms for a 20 KB README in release)

/// A ~20 KB README exercising every construct.
fn sample_readme() -> String {
    let unit = r#"## Section heading with `code` and :rocket:

Some **bold**, *italic*, ~~strike~~ and a [link](https://example.com) plus an
autolink https://github.com/owner/repo and inline math $E = mc^2$.

> [!NOTE]
> An alert with a list:
> - one
> - two

- [x] done task
- [ ] open task

| Left | Center | Right |
|:-----|:------:|------:|
| a    | b      | c     |
| d    | e      | f     |

```rust
fn main() {
    println!("Hello, world!");
}
```

![logo](images/logo.png "Logo") and a footnote[^n].

<details>
<summary>More</summary>

Hidden *markdown* with <kbd>Ctrl</kbd>+<kbd>C</kbd>.

</details>

"#;
    let mut s = String::from("# Project title\n\nIntro paragraph.\n\n");
    while s.len() < 20_000 {
        s.push_str(unit);
    }
    s.push_str("\n[^n]: The footnote text.\n");
    s
}

#[test]
fn readme_of_20kb_renders_quickly() {
    let md = sample_readme();
    let o = opts(Mode::Readme, true);
    let _ = github::render(&md, &o); // warm up lazy tables
    let runs = 5;
    let t = Instant::now();
    for _ in 0..runs {
        let r = github::render(&md, &o);
        assert!(r.html.len() > md.len());
    }
    let per = t.elapsed() / runs;
    println!(
        "github::render of a {} byte README: {per:?} per render",
        md.len()
    );
    let limit = if cfg!(debug_assertions) {
        Duration::from_millis(500)
    } else {
        Duration::from_millis(10)
    };
    assert!(per < limit, "render took {per:?} (limit {limit:?})");
}

/// Rendering time grows linearly with the document (no quadratic passes). Run in release:
/// `cargo test --release --test github_render -- --ignored --nocapture`.
#[test]
#[ignore = "timing; run in release"]
fn large_documents_scale_linearly() {
    let unit = sample_readme();
    let o = opts(Mode::Readme, true);
    let time = |copies: usize| {
        let md = unit.repeat(copies);
        let t = Instant::now();
        let r = github::render(&md, &o);
        assert!(!r.html.is_empty());
        (md.len(), t.elapsed())
    };
    let (small_len, small) = time(10);
    let (big_len, big) = time(100);
    println!("{small_len} bytes: {small:?}; {big_len} bytes: {big:?}");
    assert!(big < small * 30, "{small:?} -> {big:?}");
}
