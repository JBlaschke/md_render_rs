//! HTML escaping for every module that builds HTML by hand.
//!
//! Three escaping sets, each matching what the reproduced sites (or the page chrome) emit, so
//! that output bytes stay identical to GitHub's and GitLab's:
//!
//! | function | escapes | used for |
//! |---|---|---|
//! | [`text`], [`push_text`] | `&` `<` `>` | text content: GitHub (GH §7.1 "GitHub escapes only `<`, `>`, `&` in text"), Nokogiri's text-node `to_html`, Rouge's `escape_special_html_chars` |
//! | [`attr`], [`push_attr`] | `&` `"` `<` `>` | double-quoted attribute values written by the renderers (GitHub's serializer, Nokogiri) |
//! | [`attr_any_quote`] | `&` `"` `'` `<` `>` (`'` as `&#39;`) | the page chrome's attribute values (safe inside single quotes too) |
//!
//! Plus the helpers for content placed inside raw-text elements: JSON inside
//! `<script type="application/json">`, and JavaScript / CSS source inlined into `<script>` /
//! `<style>` elements.

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::Regex;

/// Which characters an escaping function replaces.
#[derive(Clone, Copy)]
enum Set {
    /// `&`, `<`, `>`.
    Text,
    /// `&`, `"`, `<`, `>`.
    Attr,
    /// `&`, `"`, `'`, `<`, `>`.
    AttrAnyQuote,
}

#[inline]
fn replacement(b: u8, set: Set) -> Option<&'static str> {
    match b {
        b'&' => Some("&amp;"),
        b'<' => Some("&lt;"),
        b'>' => Some("&gt;"),
        b'"' if !matches!(set, Set::Text) => Some("&quot;"),
        b'\'' if matches!(set, Set::AttrAnyQuote) => Some("&#39;"),
        _ => None,
    }
}

/// Appends `s` to `out` with the characters of `set` replaced. All of them are ASCII, so byte
/// positions are always character boundaries.
fn push(out: &mut String, s: &str, set: Set) {
    let mut last = 0;
    for (i, b) in s.bytes().enumerate() {
        if let Some(rep) = replacement(b, set) {
            out.push_str(&s[last..i]);
            out.push_str(rep);
            last = i + 1;
        }
    }
    out.push_str(&s[last..]);
}

fn escape(s: &str, set: Set) -> Cow<'_, str> {
    match s.bytes().position(|b| replacement(b, set).is_some()) {
        None => Cow::Borrowed(s),
        Some(first) => {
            let mut out = String::with_capacity(s.len() + 16);
            out.push_str(&s[..first]);
            push(&mut out, &s[first..], set);
            Cow::Owned(out)
        }
    }
}

/// Escapes text content (`&`, `<`, `>`).
pub(crate) fn text(s: &str) -> Cow<'_, str> {
    escape(s, Set::Text)
}

/// Appends `s` escaped as text content (`&`, `<`, `>`).
pub(crate) fn push_text(out: &mut String, s: &str) {
    push(out, s, Set::Text);
}

/// Escapes a double-quoted attribute value (`&`, `"`, `<`, `>`).
pub(crate) fn attr(s: &str) -> Cow<'_, str> {
    escape(s, Set::Attr)
}

/// Appends `s` escaped for a double-quoted attribute value (`&`, `"`, `<`, `>`).
pub(crate) fn push_attr(out: &mut String, s: &str) {
    push(out, s, Set::Attr);
}

/// Escapes an attribute value for use inside double or single quotes (`&`, `"`, `'`, `<`, `>`;
/// `'` becomes `&#39;`). The page chrome uses this for all its attributes.
pub(crate) fn attr_any_quote(s: &str) -> Cow<'_, str> {
    escape(s, Set::AttrAnyQuote)
}

/// Serialize a JSON value so it can be placed verbatim inside a `<script>` element.
///
/// `<`, `>` and `&` only ever occur inside JSON strings, where `<` etc. are equivalent
/// escapes, so `</script>` and `<!--` can never appear in the output. U+2028/U+2029 are escaped
/// too, so the text is also a valid JavaScript literal.
pub(crate) fn json_for_script(value: &serde_json::Value) -> String {
    // Serializing a `serde_json::Value` cannot fail (all keys are strings).
    let raw = value.to_string();
    let mut out = String::with_capacity(raw.len() + 16);
    for c in raw.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// Make JavaScript source safe to inline in a `<script>` element.
///
/// The HTML tokenizer ends a script at `</script` (any case) and treats `<!--` specially, so the
/// `<` of those sequences is written as `\x3C`. That is an equivalent escape inside string,
/// template and regular-expression literals (the only places where these sequences occur in
/// real code) and harmless inside comments.
pub(crate) fn inline_script(js: &str) -> Cow<'_, str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)<(/script|!--)").expect("valid regex"));
    re.replace_all(js, "\\x3C$1")
}

/// Make CSS source safe to inline in a `<style>` element (`</style` cannot occur).
///
/// In CSS `\/` is an escape for `/` both in strings and in comments' surroundings, and
/// `</style` can only legitimately occur inside strings or comments.
pub(crate) fn inline_style(css: &str) -> Cow<'_, str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)</(style)").expect("valid regex"));
    re.replace_all(css, "<\\/$1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_attr_escaping() {
        let s = "a<b>&c\"'ü";
        assert_eq!(text(s), "a&lt;b&gt;&amp;c\"'ü");
        assert_eq!(attr(s), "a&lt;b&gt;&amp;c&quot;'ü");
        assert_eq!(attr_any_quote(s), "a&lt;b&gt;&amp;c&quot;&#39;ü");
        let (mut t, mut a) = (String::from(">"), String::new());
        push_text(&mut t, s);
        push_attr(&mut a, s);
        assert_eq!(t, ">a&lt;b&gt;&amp;c\"'ü");
        assert_eq!(a, attr(s));
        assert!(matches!(text("plain"), Cow::Borrowed(_)));
        assert!(matches!(attr("it's"), Cow::Borrowed(_)));
        assert!(matches!(text("\"q\""), Cow::Borrowed(_)));
        assert_eq!(text("üñí €"), "üñí €");
        assert_eq!(attr_any_quote("€'€"), "€&#39;€");
    }

    #[test]
    fn json_cannot_close_script() {
        let v = serde_json::json!({"a": "</script><!-- & \u{2028}", "b": [1, 2]});
        let s = json_for_script(&v);
        assert!(!s.contains('<') && !s.contains('>') && !s.contains('&'));
        assert!(!s.contains('\u{2028}'));
        let back: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn inline_script_escaping() {
        let js = r#"var a = "</script>"; var b = '</SCRIPT x'; /<!--/.test(c);"#;
        let out = inline_script(js);
        assert!(!out.to_ascii_lowercase().contains("</script"));
        assert!(!out.contains("<!--"));
        assert_eq!(
            out,
            r#"var a = "\x3C/script>"; var b = '\x3C/SCRIPT x'; /\x3C!--/.test(c);"#
        );
        assert!(matches!(inline_script("let x = 1 < 2;"), Cow::Borrowed(_)));
    }

    #[test]
    fn inline_style_escaping() {
        let out = inline_style("a::after{content:\"</style>\"}/* </STYLE */");
        assert!(!out.to_ascii_lowercase().contains("</style"));
        assert_eq!(out, "a::after{content:\"<\\/style>\"}/* <\\/STYLE */");
    }
}
