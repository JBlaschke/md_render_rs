//! Escaping helpers for building HTML by hand.
//!
//! Everything the page module writes that is not trusted HTML goes through one of these:
//! text content, attribute values, JSON inside `<script type="application/json">`, and
//! JavaScript / CSS source inlined into `<script>` / `<style>` elements.

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::Regex;

/// Escape text content (`&`, `<`, `>`).
pub fn text(s: &str) -> Cow<'_, str> {
    escape(s, false)
}

/// Escape an attribute value for use inside double (or single) quotes.
pub fn attr(s: &str) -> Cow<'_, str> {
    escape(s, true)
}

fn escape(s: &str, quotes: bool) -> Cow<'_, str> {
    let needs = |c: char| matches!(c, '&' | '<' | '>') || (quotes && matches!(c, '"' | '\''));
    if !s.chars().any(needs) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quotes => out.push_str("&quot;"),
            '\'' if quotes => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Serialize a JSON value so it can be placed verbatim inside a `<script>` element.
///
/// `<`, `>` and `&` only ever occur inside JSON strings, where `<` etc. are equivalent
/// escapes, so `</script>` and `<!--` can never appear in the output. U+2028/U+2029 are escaped
/// too, so the text is also a valid JavaScript literal.
pub fn json_for_script(value: &serde_json::Value) -> String {
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
pub fn inline_script(js: &str) -> Cow<'_, str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)<(/script|!--)").expect("valid regex"));
    re.replace_all(js, "\\x3C$1")
}

/// Make CSS source safe to inline in a `<style>` element (`</style` cannot occur).
///
/// In CSS `\/` is an escape for `/` both in strings and in comments' surroundings, and
/// `</style` can only legitimately occur inside strings or comments.
pub fn inline_style(css: &str) -> Cow<'_, str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)</(style)").expect("valid regex"));
    re.replace_all(css, "<\\/$1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_attr_escaping() {
        assert_eq!(text("a<b>&c\"'"), "a&lt;b&gt;&amp;c\"'");
        assert_eq!(attr("a<b>&c\"'"), "a&lt;b&gt;&amp;c&quot;&#39;");
        assert!(matches!(text("plain"), Cow::Borrowed(_)));
        assert_eq!(text("üñí €"), "üñí €");
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
