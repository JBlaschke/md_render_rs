//! Language lookup tables of `highlight::github` (Linguist, GH §7.2) and `highlight::gitlab`
//! (Rouge registry, GL §8.1) through the public API (owner: highlight).

mod highlight_eval;

use md_render::render::highlight::{github, gitlab};

fn gh_scope(lang: &str) -> Option<String> {
    github("x = 1\n", lang).scope
}

fn gl_tag(lang: &str) -> String {
    gitlab("x = 1\n", lang).tag
}

#[test]
fn github_lookup() {
    let cases: &[(&str, Option<&str>)] = &[
        ("rs", Some("source.rust")),
        ("Rust", Some("source.rust")),
        ("RUST", Some("source.rust")),
        ("rust,ignore", Some("source.rust")),
        ("js", Some("source.js")),
        ("javascript", Some("source.js")),
        ("jsx", Some("source.js")),
        ("ts", Some("source.ts")),
        ("tsx", Some("source.tsx")),
        ("console", Some("text.shell-session")),
        ("shellsession", Some("text.shell-session")),
        ("sh-session", Some("text.shell-session")),
        ("shell-session", None),
        ("sh", Some("source.shell")),
        ("bash", Some("source.shell")),
        ("zsh", Some("source.shell")),
        ("shell", Some("source.shell")),
        ("c++", Some("source.c++")),
        ("cpp", Some("source.c++")),
        ("h", Some("source.c")),
        ("objective-c", Some("source.objc")),
        ("objc", Some("source.objc")),
        ("c#", Some("source.cs")),
        ("csharp", Some("source.cs")),
        ("golang", Some("source.go")),
        ("py", Some("source.python")),
        ("python3", Some("source.python")),
        ("yml", Some("source.yaml")),
        ("md", Some("text.md")),
        ("Dockerfile", Some("source.dockerfile")),
        ("docker", None),
        ("makefile", Some("source.makefile")),
        ("diff", Some("source.diff")),
        ("patch", Some("source.diff")),
        ("html", Some("text.html.basic")),
        ("json", Some("source.json")),
        ("jsonc", Some("source.json.comments")),
        ("txt", Some("text.adblock")),
        ("text", None),
        ("plaintext", None),
        ("", None),
        ("no-such-language", None),
        ("{.python}", None),
    ];
    for &(lang, want) in cases {
        assert_eq!(gh_scope(lang).as_deref(), want, "{lang:?}");
    }
}

#[test]
fn github_grammar_presence() {
    // Known scope with a grammar: highlighted HTML.
    for lang in [
        "rust", "python", "js", "console", "diff", "md", "json", "yaml", "toml", "html",
    ] {
        assert!(github("x\n", lang).html.is_some(), "{lang}");
    }
    // Known scope without a two-face grammar: wrapper only (html None).
    for lang in ["powershell", "mermaid"] {
        let r = github("x\n", lang);
        assert!(r.scope.is_some() && r.html.is_none(), "{lang}: {r:?}");
    }
}

#[test]
fn gitlab_lookup() {
    let cases: &[(&str, &str)] = &[
        ("rs", "rust"),
        ("rust", "rust"),
        ("Rust", "plaintext"),
        ("RUST", "plaintext"),
        ("rust,ignore", "rust"),
        ("js", "javascript"),
        ("ts", "typescript"),
        ("console", "console"),
        ("shell-session", "console"),
        ("terminal", "console"),
        ("sh", "shell"),
        ("bash", "shell"),
        ("shell", "shell"),
        ("c++", "cpp"),
        ("cpp", "cpp"),
        ("objective-c", "objective_c"),
        ("objc", "objective_c"),
        ("yml", "yaml"),
        ("md", "markdown"),
        ("Dockerfile", "docker"),
        ("patch", "diff"),
        ("jsonc", "json-doc"),
        ("ex", "viml"),
        ("txt", "plaintext"),
        ("text", "plaintext"),
        ("", "plaintext"),
        ("no-such-language", "plaintext"),
        ("console?comments=true", "plaintext"),
        ("mermaid", "mermaid"),
        ("math", "math"),
        ("plantuml", "plantuml"),
        ("suggestion", "suggestion"),
    ];
    for &(lang, want) in cases {
        assert_eq!(gl_tag(lang), want, "{lang:?}");
    }
}

#[test]
fn gitlab_verbatim_and_plain_lines() {
    let r = gitlab("graph TD;\n  A-->B\n", "mermaid");
    assert_eq!(r.lines, ["graph TD;", "  A--&gt;B"]);
    let r = gitlab("<b>&</b>\n\n", "txt");
    assert_eq!(r.tag, "plaintext");
    assert_eq!(r.lines, ["&lt;b&gt;&amp;&lt;/b&gt;", ""]);
    assert!(gitlab("", "rust").lines.is_empty());
}

/// Every `LANG:x` probe of `S/probes/langs.gfm.html` (~190 info words posted to github.com):
/// our Linguist lookup must give the same `highlight-` class, or none where GitHub renders a
/// plain block.
#[test]
fn github_lookup_matches_github_probe() {
    let Some(dir) = highlight_eval::golden_dir() else {
        highlight_eval::skipped("highlight_lookup::github_lookup_matches_github_probe");
        return;
    };
    let html = std::fs::read_to_string(dir.join("github/probes/langs.gfm.html")).unwrap();
    let re = regex::Regex::new(
        r#"<p>LANG:(.*?)</p>\s*(?:<div class="highlight highlight-([^" ]+)|<pre)"#,
    )
    .unwrap();
    let mut total = 0;
    let mut wrong = Vec::new();
    for c in re.captures_iter(&html) {
        total += 1;
        let lang = highlight_eval::decode_entities(&c[1]);
        let want = c.get(2).map(|m| m.as_str().to_owned());
        let ours = gh_scope(&lang).map(|s| s.replace('.', "-"));
        if ours != want {
            wrong.push(format!("{lang}: ours {ours:?}, GitHub {want:?}"));
        }
    }
    println!(
        "Linguist lookup: {}/{total} probes agree",
        total - wrong.len()
    );
    assert!(total > 150, "probe file parsed: {total}");
    assert!(wrong.is_empty(), "{wrong:#?}");
}
