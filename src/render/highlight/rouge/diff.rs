//! Exact port of Rouge 5.1.0's `Rouge::Lexers::Diff` (`lib/rouge/lexers/diff.rb`, MIT), the
//! GL §8.4 "diff" special case: whole lines become `gi`/`gd`/`gh`/`gs`/`p`.
//!
//! ```ruby
//! state :root do
//!   rule(/^ .*$\n?/, Text)
//!   rule(/^---$\n?/, Punctuation)
//!   rule %r((^\++.*$\n?)|(^>+[ \t]+.*$\n?)|(^>+$\n?))x, Generic::Inserted
//!   rule %r((^-+.*$\n?)|(^<+[ \t]+.*$\n?)|(^<+$\n?))x, Generic::Deleted
//!   rule(/^!.*$\n?/, Generic::Strong)
//!   rule(/^([Ii]ndex|diff).*$\n?/, Generic::Heading)
//!   rule(/^(@@[^@]*@@)([^\n]*\n)/) { groups Punctuation, Text }
//!   rule(/^\w.*$\n?/, Punctuation)
//!   rule(/^=.*$\n?/, Generic::Heading)
//!   rule(/.+$\n?/, Text)
//! end
//! ```
//! Ruby's `.` excludes `\n`, `$` is the end of a line and `\w` is ASCII-only. Every rule starts
//! at a line start except the last, which only ever runs there too (all rules consume through a
//! line end). A position where nothing matches (an empty line) yields Rouge's one-character
//! `Error` token, which is invisible here because it is the newline itself.

/// Tokenize `code` as Rouge's diff lexer does: `(class, text)` pairs, `None` = plain text.
pub(super) fn tokens(code: &str) -> Vec<(Option<&'static str>, &str)> {
    let mut out = Vec::new();
    let mut rest = code;
    while !rest.is_empty() {
        let (line, content) = match rest.find('\n') {
            Some(i) => (&rest[..=i], &rest[..i]),
            None => (rest, rest),
        };
        let first = content.as_bytes().first().copied();
        let class: Option<&'static str> = match first {
            Some(b' ') => None,
            _ if content == "---" => Some("p"),
            Some(b'+') => Some("gi"),
            Some(b'>') if repeated_then_blank(content, b'>') => Some("gi"),
            Some(b'-') => Some("gd"),
            Some(b'<') if repeated_then_blank(content, b'<') => Some("gd"),
            Some(b'!') => Some("gs"),
            _ if content.starts_with("index")
                || content.starts_with("Index")
                || content.starts_with("diff") =>
            {
                Some("gh")
            }
            _ => {
                if let Some((range, text)) = hunk_header(rest) {
                    out.push((Some("p"), range));
                    out.push((None, text));
                    rest = &rest[range.len() + text.len()..];
                    continue;
                }
                match first {
                    Some(b) if b.is_ascii_alphanumeric() || b == b'_' => Some("p"),
                    Some(b'=') => Some("gh"),
                    // `.+$\n?` (Text) or, for an empty line, Rouge's `Error` token for the
                    // newline: neither produces a span.
                    _ => None,
                }
            }
        };
        out.push((class, line));
        rest = &rest[line.len()..];
    }
    out
}

/// `^>+[ \t]+.*$` or `^>+$` (likewise for `<`).
fn repeated_then_blank(content: &str, marker: u8) -> bool {
    let after = content.trim_start_matches(char::from(marker));
    after.is_empty() || after.starts_with([' ', '\t'])
}

/// `^(@@[^@]*@@)([^\n]*\n)`: the `@@ … @@` part (which may span lines) and the rest of its
/// line including the newline (which must exist).
fn hunk_header(rest: &str) -> Option<(&str, &str)> {
    let body = rest.strip_prefix("@@")?;
    let at = body.find('@')?;
    if !body[at..].starts_with("@@") {
        return None;
    }
    let range_len = 2 + at + 2;
    let tail = &rest[range_len..];
    let nl = tail.find('\n')?;
    Some((&rest[..range_len], &tail[..=nl]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rouge_sample() {
        // R/rouge_samples/code.diff and diff.tokens.tsv.
        let code = "diff --git a/f.txt b/f.txt\nindex 83db48f..bf269f4 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n context\n-removed line\n+added line\n";
        let toks = tokens(code);
        let want = [
            (Some("gh"), "diff --git a/f.txt b/f.txt\n"),
            (Some("gh"), "index 83db48f..bf269f4 100644\n"),
            (Some("gd"), "--- a/f.txt\n"),
            (Some("gi"), "+++ b/f.txt\n"),
            (Some("p"), "@@ -1,3 +1,3 @@"),
            (None, "\n"),
            (None, " context\n"),
            (Some("gd"), "-removed line\n"),
            (Some("gi"), "+added line\n"),
        ];
        assert_eq!(toks, want);
    }

    #[test]
    fn other_rules() {
        let t = |c: &str| tokens(c).into_iter().map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(t("---\n"), [Some("p")]);
        assert_eq!(t("> new\n>\n>x\n"), [Some("gi"), Some("gi"), None]);
        assert_eq!(t("< old\n<<\n"), [Some("gd"), Some("gd")]);
        assert_eq!(t("! changed\n"), [Some("gs")]);
        assert_eq!(t("Index: x\n"), [Some("gh")]);
        assert_eq!(t("Only in a: b\n"), [Some("p")]);
        assert_eq!(t("==== x\n"), [Some("gh")]);
        assert_eq!(t("\n\n"), [None, None]);
        assert_eq!(t("é\n"), [None], "\\w is ASCII-only in Ruby");
        // No newline after the hunk header: the rule does not match; `.+` (Text) does.
        assert_eq!(t("@@ -1 +1 @@"), [None]);
        assert_eq!(tokens("@@ a\nb @@ c\nd\n")[0], (Some("p"), "@@ a\nb @@"));
    }
}
