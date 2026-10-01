//! Rouge lexer registry and GitLab's language resolution (GL §8.1).
//!
//! Data: `data/gitlab/rouge-registry.json`, compacted from Rouge 5.1.0: `registry` is
//! `Rouge::Lexer.registry` (every tag and alias → lexer tag, 411 entries) and `lexers` holds each
//! lexer's title and plain `*.ext` filename globs (used to pick a syntect syntax).
//!
//! GitLab's `SyntaxHighlightFilter`:
//! ```text
//! lang = pre[data-canonical-lang]                       # case-sensitive, may be absent
//! if lang in {math, suggestion} ∪ Kroki diagram names:  tag = lang, plain text
//! else: lexer = Rouge::Lexer.find(lang) || PlainText;   tag = lexer.tag
//! ```
//! `Rouge::Lexer.find` is an exact, case-sensitive registry lookup (`Python` and
//! `console?comments=true` are not found, so they render as plaintext).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// Languages GitLab never hands to Rouge: `math`, `suggestion` and
/// `AsciidoctorExtensions::Kroki::SUPPORTED_DIAGRAM_NAMES` (GL §8.1, `R/tools/rouge_gitlab.rb`).
const NO_ROUGE: &[&str] = &[
    "math",
    "suggestion",
    "actdiag",
    "blockdiag",
    "bpmn",
    "bytefield",
    "c4plantuml",
    "d2",
    "dbml",
    "ditaa",
    "erd",
    "excalidraw",
    "graphviz",
    "mermaid",
    "nomnoml",
    "nwdiag",
    "packetdiag",
    "pikchr",
    "plantuml",
    "rackdiag",
    "seqdiag",
    "svgbob",
    "symbolator",
    "umlet",
    "vega",
    "vegalite",
    "wavedrom",
    "structurizr",
    "diagramsnet",
    "wireviz",
];

/// The Rouge tag of the plain-text lexer.
pub(crate) const PLAINTEXT: &str = "plaintext";

#[derive(Deserialize)]
struct LexerData {
    #[allow(dead_code)]
    title: String,
    ext: Vec<String>,
}

#[derive(Deserialize)]
struct Data {
    registry: HashMap<String, String>,
    lexers: HashMap<String, LexerData>,
}

const DATA: &str = include_str!("../../../../data/gitlab/rouge-registry.json");

fn data() -> &'static Data {
    static DATA_CELL: OnceLock<Data> = OnceLock::new();
    DATA_CELL.get_or_init(|| {
        // Embedded at compile time and covered by unit tests; an unparsable file degrades to
        // "every language is plaintext" instead of panicking.
        serde_json::from_str(DATA).unwrap_or_else(|_| Data {
            registry: HashMap::new(),
            lexers: HashMap::new(),
        })
    })
}

/// How GitLab treats a block's language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolved<'a> {
    /// Not handed to Rouge (`math`, `suggestion`, Kroki diagrams): `tag` is the language as
    /// written, the text is not tokenized.
    Verbatim(&'a str),
    /// A Rouge lexer tag (`plaintext` for unknown or missing languages).
    Rouge(&'static str),
}

impl Resolved<'_> {
    /// The tag for `language-<tag>` / `data-lang`.
    pub(crate) fn tag(&self) -> &str {
        match self {
            Resolved::Verbatim(tag) => tag,
            Resolved::Rouge(tag) => tag,
        }
    }
}

/// Resolve the info-string language exactly as GitLab does (GL §8.1).
pub(crate) fn resolve(lang: &str) -> Resolved<'_> {
    if NO_ROUGE.contains(&lang) {
        return Resolved::Verbatim(lang);
    }
    Resolved::Rouge(find(lang).unwrap_or(PLAINTEXT))
}

/// `Rouge::Lexer.find(name)&.tag`.
pub(crate) fn find(name: &str) -> Option<&'static str> {
    data().registry.get(name).map(String::as_str)
}

/// File extensions (without `*.`) of a lexer's filename globs.
pub(crate) fn extensions(tag: &str) -> &'static [String] {
    data().lexers.get(tag).map_or(&[], |l| l.ext.as_slice())
}

/// All lexer tags (tests and evaluation only).
#[cfg(test)]
pub(crate) fn tags() -> Vec<&'static str> {
    let mut v: Vec<&str> = data().lexers.keys().map(String::as_str).collect();
    v.sort_unstable();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(lang: &str) -> String {
        resolve(lang).tag().to_owned()
    }

    #[test]
    fn data_loads() {
        assert_eq!(data().registry.len(), 411);
        assert_eq!(data().lexers.len(), 234);
        assert!(extensions("rust").iter().any(|e| e == "rs"));
    }

    #[test]
    fn registry_examples() {
        // GL §8.1 examples.
        for (lang, want) in [
            ("js", "javascript"),
            ("py", "python"),
            ("sh", "shell"),
            ("bash", "shell"),
            ("zsh", "shell"),
            ("ksh", "shell"),
            ("console", "console"),
            ("terminal", "console"),
            ("shell_session", "console"),
            ("shell-session", "console"),
            ("yml", "yaml"),
            ("rs", "rust"),
            ("rust", "rust"),
            ("ts", "typescript"),
            ("md", "markdown"),
            ("mkd", "markdown"),
            ("text", "plaintext"),
            ("c++", "cpp"),
            ("cpp", "cpp"),
            ("golang", "go"),
            ("dockerfile", "docker"),
            ("Dockerfile", "docker"),
            ("make", "make"),
            ("makefile", "make"),
            ("patch", "diff"),
            ("udiff", "diff"),
            ("tex", "tex"),
            ("latex", "tex"),
            ("objc", "objective_c"),
            ("objective-c", "objective_c"),
            ("cs", "csharp"),
            ("ex", "viml"),
            ("exs", "elixir"),
            ("jsonc", "json-doc"),
        ] {
            assert_eq!(tag(lang), want, "{lang}");
        }
    }

    #[test]
    fn unknown_case_sensitive_and_options() {
        for lang in [
            "",
            "txt",
            "gql",
            "ps1",
            "rst",
            "Python",
            "RUST",
            "console?comments=true",
        ] {
            assert_eq!(tag(lang), "plaintext", "{lang}");
        }
        assert_eq!(resolve("python"), Resolved::Rouge("python"));
    }

    #[test]
    fn verbatim_languages() {
        for lang in ["math", "mermaid", "plantuml", "suggestion", "d2"] {
            assert_eq!(resolve(lang), Resolved::Verbatim(lang));
            assert_eq!(tag(lang), lang);
        }
    }
}
