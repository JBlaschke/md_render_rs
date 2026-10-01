//! Linguist language lookup for GitHub code blocks (GH §7.2).
//!
//! Data: `data/github/linguist.json`, compacted from Linguist's `languages.yml` (name,
//! `tm_scope`, aliases, extensions; `languages.yml` order, which is sorted by name).
//!
//! Lookup of the info-string word, as github.com does it (GH §7.2, verified against
//! `S/probes/langs.gfm.html` and extra API probes):
//!
//! 1. `Linguist::Language[word]`: case-insensitive match against every language name, its
//!    *default alias* (the name lowercased with whitespace replaced by `-`, e.g. `common-lisp`,
//!    `protocol-buffer`, `json-with-comments`) and its explicit aliases; if that fails, the same
//!    lookup with the part before the first `,` (`rust,ignore` → `rust`).
//! 2. `Linguist::Language.find_by_extension("." + word)`: Linguist's `FileBlob#extensions` of
//!    that name, longest first (`sh.in` → `.sh.in`, `CMakeLists.txt` → `.cmakelists.txt`,
//!    `.txt`), the first one that any language claims, then the first such language in
//!    `languages.yml` order (`txt` → Adblock Filter List, `pl` → Perl, `h` → C).
//!
//! Language file *names* (`Makefile`, `Gemfile`, `.bashrc`) are not consulted: GitHub leaves
//! `GNUmakefile`/`Gemfile`/`.bashrc` blocks unhighlighted.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// One Linguist language.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Language {
    pub name: String,
    /// `None` for `tm_scope: none` (e.g. Text): known language, never highlighted.
    pub tm_scope: Option<String>,
    pub aliases: Vec<String>,
    pub extensions: Vec<String>,
}

/// One row of the data file: name, `tm_scope` (`null` = none), aliases, extensions.
type Row = (String, Option<String>, Vec<String>, Vec<String>);

#[derive(Deserialize)]
struct Data {
    languages: Vec<Row>,
}

struct Table {
    languages: Vec<Language>,
    /// Lowercased name, default alias and aliases → language index.
    index: HashMap<String, usize>,
    /// Lowercased extension (with leading `.`) → first language (in table order) claiming it.
    by_extension: HashMap<String, usize>,
}

const DATA: &str = include_str!("../../../../data/github/linguist.json");

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        // The data file is embedded at compile time and covered by unit tests; an unparsable
        // file degrades to "no language known" instead of panicking.
        let data: Data = serde_json::from_str(DATA).unwrap_or_else(|_| Data {
            languages: Vec::new(),
        });
        let mut index = HashMap::new();
        let mut by_extension = HashMap::new();
        let mut languages = Vec::with_capacity(data.languages.len());
        for (i, (name, tm_scope, aliases, extensions)) in data.languages.into_iter().enumerate() {
            let lower = name.to_lowercase();
            let default_alias: String = lower
                .chars()
                .map(|c| if c.is_whitespace() { '-' } else { c })
                .collect();
            index.entry(lower).or_insert(i);
            index.entry(default_alias).or_insert(i);
            for alias in &aliases {
                index.entry(alias.to_lowercase()).or_insert(i);
            }
            for ext in &extensions {
                by_extension.entry(ext.to_lowercase()).or_insert(i);
            }
            languages.push(Language {
                name,
                tm_scope,
                aliases,
                extensions,
            });
        }
        Table {
            languages,
            index,
            by_extension,
        }
    })
}

/// `FileBlob#extensions` of a file name `base.b.c` (non-empty `base`): `.b.c`, `.c` (longest
/// first).
fn extensions(name: &str) -> impl Iterator<Item = &str> {
    // Every '.' after the first character of the name starts one candidate.
    name.char_indices()
        .filter(|&(i, c)| c == '.' && i > 0)
        .map(move |(i, _)| &name[i..])
}

/// Find the Linguist language for an info-string word (case-insensitive). `None` when nothing
/// matches.
pub(crate) fn find(word: &str) -> Option<&'static Language> {
    if word.is_empty() {
        return None;
    }
    let table = table();
    let lower = word.to_lowercase();
    let by_name = |key: &str| table.index.get(key).copied();
    let idx = by_name(&lower)
        .or_else(|| lower.split_once(',').and_then(|(first, _)| by_name(first)))
        .or_else(|| {
            // find_by_extension("." + word): FileBlob splits "." + word at '.', dropping the
            // (empty) base name, so every '.'-separated suffix of the word is a candidate. A
            // placeholder base name makes `extensions` see the same segments.
            let file = format!("x.{lower}");
            extensions(&file).find_map(|ext| table.by_extension.get(ext).copied())
        })?;
    table.languages.get(idx)
}

/// All languages, in table order (tests and evaluation only).
#[cfg(test)]
pub(crate) fn languages() -> &'static [Language] {
    &table().languages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(word: &str) -> Option<&'static str> {
        find(word).and_then(|l| l.tm_scope.as_deref())
    }

    fn name(word: &str) -> Option<&'static str> {
        find(word).map(|l| l.name.as_str())
    }

    #[test]
    fn data_loads() {
        assert!(languages().len() > 800, "{}", languages().len());
        assert_eq!(name("rust"), Some("Rust"));
    }

    #[test]
    fn names_aliases_case_insensitive() {
        for w in ["rs", "Rust", "RUST", "rust"] {
            assert_eq!(scope(w), Some("source.rust"), "{w}");
        }
        assert_eq!(scope("js"), Some("source.js"));
        assert_eq!(scope("javascript"), Some("source.js"));
        assert_eq!(scope("node"), Some("source.js"));
        assert_eq!(scope("jsx"), Some("source.js"));
        assert_eq!(scope("sh"), Some("source.shell"));
        assert_eq!(scope("shell"), Some("source.shell"));
        assert_eq!(scope("zsh"), Some("source.shell"));
        assert_eq!(scope("console"), Some("text.shell-session"));
        assert_eq!(scope("shellsession"), Some("text.shell-session"));
        assert_eq!(scope("c++"), Some("source.c++"));
        assert_eq!(scope("cpp"), Some("source.c++"));
        assert_eq!(scope("objective-c"), Some("source.objc"));
        assert_eq!(scope("OBJECTIVE-C"), Some("source.objc"));
        assert_eq!(scope("c#"), Some("source.cs"));
        assert_eq!(scope("golang"), Some("source.go"));
        assert_eq!(scope("Dockerfile"), Some("source.dockerfile"));
        assert_eq!(scope("containerfile"), Some("source.dockerfile"));
    }

    #[test]
    fn default_alias_and_comma() {
        assert_eq!(scope("common-lisp"), Some("source.commonlisp"));
        assert_eq!(scope("protocol-buffer"), Some("source.proto"));
        assert_eq!(scope("json-with-comments"), Some("source.json.comments"));
        assert_eq!(scope("git-config"), Some("source.gitconfig"));
        assert_eq!(scope("rust,ignore"), Some("source.rust"));
        assert_eq!(scope("shell,bash"), Some("source.shell"));
        assert_eq!(find("commonlisp"), None);
    }

    #[test]
    fn extensions_first_language_wins() {
        assert_eq!(name("txt"), Some("Adblock Filter List"));
        assert_eq!(scope("txt"), Some("text.adblock"));
        assert_eq!(name("pl"), Some("Perl"));
        assert_eq!(name("h"), Some("C"));
        assert_eq!(name("mk"), Some("Makefile"));
        assert_eq!(name("ex"), Some("Elixir"));
        assert_eq!(scope("cfg"), Some("source.haproxy-config"));
        assert_eq!(scope("py"), Some("source.python"));
        assert_eq!(scope(".py"), Some("source.python"));
        assert_eq!(scope("sh.in"), Some("source.shell"));
        assert_eq!(scope("CMakeLists.txt"), Some("text.adblock"));
        assert_eq!(scope("hpp"), Some("source.c++"));
    }

    #[test]
    fn known_without_scope_and_unknown() {
        // tm_scope: none → known language, no highlighting.
        assert_eq!(name("text"), Some("Text"));
        assert_eq!(scope("text"), None);
        for w in [
            "shell-session",
            "docker",
            "plaintext",
            "GNUmakefile",
            "Gemfile",
            ".bashrc",
        ] {
            assert_eq!(scope(w), None, "{w}");
        }
        for w in ["", "{.python}", "no-such-language", ",", "."] {
            assert_eq!(find(w), None, "{w}");
        }
    }

    #[test]
    fn file_blob_extensions() {
        let v: Vec<_> = extensions("x.sh.in").collect();
        assert_eq!(v, [".sh.in", ".in"]);
        let v: Vec<_> = extensions("x..py").collect();
        assert_eq!(v, ["..py", ".py"]);
        assert_eq!(extensions("x").count(), 0);
    }
}
