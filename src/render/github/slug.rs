//! GitHub heading slugs (GH §6.2, verified 473/473 in CQ §1.3).
//!
//! github-slugger semantics on top of Ruby's per-character `downcase`: keep only
//! `[\p{Alphabetic}\p{M}\p{Nd}\p{Pc}\p{Join_Control} -]`, space becomes `-`, duplicates get
//! `-1`, `-2`, ... with github-slugger's occurrence bookkeeping.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

/// Whether `c` (already lowercased) survives in a slug, besides ` ` and `-`.
fn is_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^[\p{Alphabetic}\p{M}\p{Nd}\p{Pc}\p{Join_Control}]$")
            .expect("static slug regex")
    });
    let mut buf = [0u8; 4];
    re.is_match(c.encode_utf8(&mut buf))
}

/// The slug of `text` before de-duplication (GH §6.2 `slug_base`).
pub(super) fn slug_base(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for c in text.chars() {
        // Per-character full lowercase mapping: `İ` -> `i̇`, and no final-sigma rule
        // (`ΣΑΣ` -> `σασ`), unlike `str::to_lowercase`.
        for l in c.to_lowercase() {
            if l == ' ' {
                s.push('-');
            } else if l == '-' || is_word(l) {
                s.push(l);
            }
        }
    }
    s
}

/// Per-document slug de-duplication (github-slugger).
#[derive(Debug, Default)]
pub(super) struct Slugger {
    occurrences: HashMap<String, usize>,
}

impl Slugger {
    /// The next unique slug for a heading with text `text`.
    pub(super) fn slug(&mut self, text: &str) -> String {
        let base = slug_base(text);
        let mut result = base.clone();
        while self.occurrences.contains_key(&result) {
            let n = self.occurrences.get(&base).copied().unwrap_or(0) + 1;
            self.occurrences.insert(base.clone(), n);
            result = format!("{base}-{n}");
        }
        self.occurrences.insert(result.clone(), 0);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verified examples from GH §6.2.
    #[test]
    fn base_examples() {
        let cases = [
            ("Hello, World! What's up?", "hello-world-whats-up"),
            (
                "Ünïcödé Hëädïng — Ελληνικά 日本語",
                "ünïcödé-hëädïng--ελληνικά-日本語",
            ),
            (
                r#"C++ & C# / Rust's "quotes" (parens) [brackets] {braces}"#,
                "c--c--rusts-quotes-parens-brackets-braces",
            ),
            ("Spaces   around", "spaces---around"),
            ("under_score-dash.dot", "under_score-dashdot"),
            ("Tab\tinside", "tabinside"),
            ("-leading-dash", "-leading-dash"),
            ("- dash -", "--dash--"),
            ("İstanbul ΣΑΣ", "i\u{307}stanbul-σασ"),
            ("ǅungla ǈ", "ǆungla-ǉ"),
            ("ẞ", "ß"),
            ("Ⅻ", "ⅻ"),
            ("Ⓐ", "ⓐ"),
            ("𝐀", "𝐀"),
            ("ＡＢＣ", "ａｂｃ"),
            ("x² ½ Ⅻ ① ٣ ๓", "x--ⅻ--٣-๓"),
            ("a‿b ﹏c _d", "a‿b-﹏c-_d"),
            ("e\u{301}", "e\u{301}"),
            (
                "Zero\u{200c}width\u{200d}joiner",
                "zero\u{200c}width\u{200d}joiner",
            ),
            ("a\u{200b}b\u{ad}c\u{a0}d\u{2003}e", "abcde"),
            ("Emoji :rocket: heading 🎉", "emoji-rocket-heading-"),
            ("Emoji 😀 :smile: :octocat: end", "emoji--smile-octocat-end"),
            (":+1: thumbs", "1-thumbs"),
            (" Image heading", "-image-heading"),
            ("A & B © C", "a--b--c"),
            ("Footnote1 heading", "footnote1-heading"),
            ("Math $x^2$ heading", "math-x2-heading"),
            ("???", ""),
        ];
        for (input, want) in cases {
            assert_eq!(slug_base(input), want, "input {input:?}");
        }
    }

    /// github-slugger de-duplication (GH §6.2 table).
    #[test]
    fn dedupe() {
        let mut s = Slugger::default();
        assert_eq!(s.slug("Duplicate"), "duplicate");
        assert_eq!(s.slug("Duplicate"), "duplicate-1");
        assert_eq!(s.slug("Duplicate"), "duplicate-2");
        assert_eq!(s.slug("Duplicate-1"), "duplicate-1-1");

        let mut s = Slugger::default();
        assert_eq!(s.slug("Foo-1"), "foo-1");
        assert_eq!(s.slug("Foo"), "foo");
        assert_eq!(s.slug("Foo"), "foo-2");
        assert_eq!(s.slug("Foo"), "foo-3");

        let mut s = Slugger::default();
        assert_eq!(s.slug("???"), "");
        assert_eq!(s.slug("!!!"), "-1");
    }
}
