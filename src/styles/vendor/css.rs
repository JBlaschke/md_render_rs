//! Finding and rewriting the external references of a stylesheet: every `url(...)` (quoted or
//! unquoted, any case), every `@import` (string or `url()` form) and every bare string argument
//! of `image-set()` / `-webkit-image-set()`, which CSS Images 4 also treats as a URL. CR §7,
//! GH §4.3, GL §11.2.
//!
//! A small tokenizer is used instead of a regex so that references inside comments and strings
//! (e.g. `content: "url(x)"`) are not mistaken for real ones and escapes are handled. Only the
//! *value* of a reference is replaced; the surrounding `url(`, quotes and modifiers are kept.

use std::ops::Range;

/// What kind of reference was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefKind {
    /// `url(...)` anywhere except right after `@import`, or a string argument of `image-set()`:
    /// an asset (image, font, ...).
    Url,
    /// `@import "x"` / `@import url(x)`: another stylesheet.
    Import,
}

/// One reference in a stylesheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CssRef {
    pub kind: RefKind,
    /// Byte range of the raw value (inside the quotes, or the unquoted token).
    pub range: Range<usize>,
    /// The value with CSS escapes resolved.
    pub value: String,
    /// The quote character (`"` or `'`) for quoted values.
    pub quote: Option<u8>,
}

/// Bytes that may continue an identifier (`url(` preceded by one is not a `url()` function).
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

fn starts_with_ci(b: &[u8], at: usize, pat: &[u8]) -> bool {
    b.get(at..at + pat.len())
        .is_some_and(|s| s.eq_ignore_ascii_case(pat))
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_ws(b[i]) {
        i += 1;
    }
    i
}

/// Skip whitespace and comments.
fn skip_ws_comments(b: &[u8], mut i: usize) -> usize {
    loop {
        i = skip_ws(b, i);
        if b.get(i) == Some(&b'/') && b.get(i + 1) == Some(&b'*') {
            i = find(b, i + 2, b"*/").map_or(b.len(), |j| j + 2);
        } else {
            return i;
        }
    }
}

/// Index after the escape starting with the `\` at `i`: 1-6 hex digits plus one optional
/// whitespace, or a single other character.
fn skip_escape(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    if b.get(j).is_some_and(u8::is_ascii_hexdigit) {
        while j < b.len() && j < i + 7 && b[j].is_ascii_hexdigit() {
            j += 1;
        }
        if b.get(j).copied().is_some_and(is_ws) {
            j += 1;
        }
        j
    } else {
        j + 1
    }
}

fn find(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    b.get(from..)?
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|p| p + from)
}

/// A string token starting at `start` (which holds the quote).
struct StrTok {
    /// Range between the quotes.
    inner: Range<usize>,
    /// Index after the closing quote (or where the bad/unterminated string stopped).
    end: usize,
    closed: bool,
}

fn parse_string(b: &[u8], start: usize, quote: u8) -> StrTok {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            c if c == quote => {
                return StrTok {
                    inner: start + 1..i,
                    end: i + 1,
                    closed: true,
                };
            }
            b'\\' => i += 2,
            // An unescaped newline ends a string as a "bad string" (CSS Syntax §4.3.5).
            b'\n' | b'\r' | b'\x0c' => {
                return StrTok {
                    inner: start + 1..i,
                    end: i,
                    closed: false,
                };
            }
            _ => i += 1,
        }
    }
    let end = b.len();
    StrTok {
        inner: start + 1..end.min(b.len()),
        end,
        closed: false,
    }
}

/// Parse `url(...)` whose `u` is at `start`. Returns the reference and the index after `)`, or
/// `None` for a malformed ("bad url") token.
fn parse_url(css: &str, start: usize, kind: RefKind) -> Option<(CssRef, usize)> {
    let b = css.as_bytes();
    let mut i = skip_ws(b, start + 4);
    match b.get(i) {
        Some(&q @ (b'"' | b'\'')) => {
            let s = parse_string(b, i, q);
            if !s.closed {
                return None;
            }
            // CSS Values 4 allows modifiers after the string; tolerate anything up to `)`.
            let close = find(b, s.end, b")")?;
            let r = CssRef {
                kind,
                value: unescape(&css[s.inner.clone()]),
                range: s.inner,
                quote: Some(q),
            };
            Some((r, close + 1))
        }
        _ => {
            let begin = i;
            while i < b.len() {
                match b[i] {
                    b')' => {
                        let r = CssRef {
                            kind,
                            value: unescape(&css[begin..i]),
                            range: begin..i,
                            quote: None,
                        };
                        return Some((r, i + 1));
                    }
                    c if is_ws(c) => {
                        let j = skip_ws(b, i);
                        if b.get(j) == Some(&b')') {
                            let r = CssRef {
                                kind,
                                value: unescape(&css[begin..i]),
                                range: begin..i,
                                quote: None,
                            };
                            return Some((r, j + 1));
                        }
                        return None;
                    }
                    b'"' | b'\'' | b'(' => return None,
                    b'\\' => i = skip_escape(b, i),
                    _ => i += 1,
                }
            }
            None
        }
    }
}

/// Whether the function whose `(` is at `open` is `image-set()` or `-webkit-image-set()`, whose
/// direct string arguments are image URLs (CSS Images 4 §2.2: `image-set("a.png" 1x)`).
fn is_image_set_call(b: &[u8], open: usize) -> bool {
    let start = b[..open]
        .iter()
        .rposition(|&c| !is_ident_byte(c))
        .map_or(0, |k| k + 1);
    let name = &b[start..open];
    name.eq_ignore_ascii_case(b"image-set") || name.eq_ignore_ascii_case(b"-webkit-image-set")
}

/// Find every `url()` and `@import` reference, and every string argument of `image-set()` /
/// `-webkit-image-set()` (reported as [`RefKind::Url`]), in source order. Never panics.
pub(crate) fn scan(css: &str) -> Vec<CssRef> {
    let b = css.as_bytes();
    let mut out = Vec::new();
    // One entry per open parenthesis: whether it opened an image-set() call. `url()` tokens
    // are consumed whole and never pushed.
    let mut parens: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = find(b, i + 2, b"*/").map_or(b.len(), |j| j + 2);
            }
            q @ (b'"' | b'\'') => {
                let s = parse_string(b, i, q);
                if s.closed && parens.last() == Some(&true) {
                    out.push(CssRef {
                        kind: RefKind::Url,
                        value: unescape(&css[s.inner.clone()]),
                        range: s.inner,
                        quote: Some(q),
                    });
                }
                i = s.end.max(i + 1);
            }
            b'(' => {
                parens.push(is_image_set_call(b, i));
                i += 1;
            }
            b')' => {
                parens.pop();
                i += 1;
            }
            // Function arguments never hold braces: an unclosed `(` ends with its rule, so a
            // malformed `image-set(` cannot turn later `content: "..."` strings into URLs.
            b'{' | b'}' => {
                parens.clear();
                i += 1;
            }
            b'\\' => i += 2,
            b'@' if starts_with_ci(b, i + 1, b"import")
                && !b.get(i + 7).copied().is_some_and(is_ident_byte) =>
            {
                let j = skip_ws_comments(b, i + 7);
                match b.get(j) {
                    Some(&q @ (b'"' | b'\'')) => {
                        let s = parse_string(b, j, q);
                        if s.closed {
                            out.push(CssRef {
                                kind: RefKind::Import,
                                value: unescape(&css[s.inner.clone()]),
                                range: s.inner,
                                quote: Some(q),
                            });
                        }
                        i = s.end.max(j + 1);
                    }
                    _ if starts_with_ci(b, j, b"url(") => {
                        match parse_url(css, j, RefKind::Import) {
                            Some((r, end)) => {
                                out.push(r);
                                i = end;
                            }
                            None => {
                                parens.push(false); // the bad url's `(`, closed later
                                i = j + 4;
                            }
                        }
                    }
                    _ => i = j.max(i + 1),
                }
            }
            b'u' | b'U'
                if starts_with_ci(b, i, b"url(") && (i == 0 || !is_ident_byte(b[i - 1])) =>
            {
                match parse_url(css, i, RefKind::Url) {
                    Some((r, end)) => {
                        out.push(r);
                        i = end;
                    }
                    None => {
                        parens.push(false); // the bad url's `(`, closed later
                        i += 4;
                    }
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Resolve CSS escapes (CSS Syntax §4.3.7): `\` + 1-6 hex digits (+ one optional whitespace),
/// `\` + newline (line continuation, dropped), `\` + any other char (that char).
pub(crate) fn unescape(raw: &str) -> String {
    if !raw.contains('\\') {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            None => {}
            Some('\n') => {
                chars.next();
            }
            Some(h) if h.is_ascii_hexdigit() => {
                let mut code = 0u32;
                let mut n = 0;
                while n < 6 {
                    match chars.peek().and_then(|c| c.to_digit(16)) {
                        Some(d) => {
                            code = code * 16 + d;
                            chars.next();
                            n += 1;
                        }
                        None => break,
                    }
                }
                if chars.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                    chars.next();
                }
                out.push(match code {
                    0 => '\u{FFFD}',
                    c => char::from_u32(c).unwrap_or('\u{FFFD}'),
                });
            }
            Some(other) => {
                out.push(other);
                chars.next();
            }
        }
    }
    out
}

/// Escape `value` so it can stand in for the raw value of `r` (inside its quotes, or unquoted).
fn escape_for(value: &str, quote: Option<u8>) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        let needs = match quote {
            Some(q) => c == q as char || c == '\\' || c == '\n' || c == '\r',
            None => {
                c == '\\'
                    || c == '"'
                    || c == '\''
                    || c == '('
                    || c == ')'
                    || c.is_ascii_whitespace()
            }
        };
        if needs {
            if c.is_ascii_whitespace() {
                // Hex escape plus a terminating space (a literal newline cannot be escaped).
                out.push_str(&format!("\\{:x} ", c as u32));
            } else {
                out.push('\\');
                out.push(c);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Replace the value of each reference for which `replace` returns `Some(new_value)` (unescaped;
/// it is escaped here as needed). `refs` must come from [`scan`] on the same `css`.
pub(crate) fn rewrite(
    css: &str,
    refs: &[CssRef],
    mut replace: impl FnMut(&CssRef) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(css.len() + refs.len() * 8);
    let mut last = 0;
    for r in refs {
        if r.range.start < last || r.range.end > css.len() {
            continue; // defensive: overlapping or foreign ranges are ignored
        }
        if let Some(new) = replace(r) {
            out.push_str(&css[last..r.range.start]);
            out.push_str(&escape_for(&new, r.quote));
            last = r.range.end;
        }
    }
    out.push_str(&css[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(css: &str) -> Vec<(RefKind, String, Option<char>)> {
        scan(css)
            .into_iter()
            .map(|r| (r.kind, r.value, r.quote.map(char::from)))
            .collect()
    }

    #[test]
    fn finds_quoted_and_unquoted_urls() {
        let css = r#"a{background:url(/images/a.png)}b{src:url("x/y.woff2") format("woff2"),url('z.ttf')}c{b:URL( spaced.gif )}"#;
        assert_eq!(
            values(css),
            vec![
                (RefKind::Url, "/images/a.png".into(), None),
                (RefKind::Url, "x/y.woff2".into(), Some('"')),
                (RefKind::Url, "z.ttf".into(), Some('\'')),
                (RefKind::Url, "spaced.gif".into(), None),
            ]
        );
    }

    #[test]
    fn finds_imports() {
        let css = r#"@import "a.css";@import url(b.css) screen;@IMPORT 'c.css';@import url("d.css");x{y:url(e.png)}"#;
        let v = values(css);
        assert_eq!(
            v,
            vec![
                (RefKind::Import, "a.css".into(), Some('"')),
                (RefKind::Import, "b.css".into(), None),
                (RefKind::Import, "c.css".into(), Some('\'')),
                (RefKind::Import, "d.css".into(), Some('"')),
                (RefKind::Url, "e.png".into(), None),
            ]
        );
    }

    #[test]
    fn skips_comments_strings_and_identifiers() {
        let css = r#"/* url(comment.png) */a{content:"url(string.png)";b:my-url(no.png);c:xurl(no.png)}d{e:url(yes.png)}"#;
        assert_eq!(values(css), vec![(RefKind::Url, "yes.png".into(), None)]);
    }

    #[test]
    fn data_uris_with_parens_and_quotes() {
        let css = r#"a{b:url("data:image/svg+xml;charset=utf-8,<svg xmlns='http://www.w3.org/2000/svg'><path d='M0 0)'/></svg>")}c{d:url(after.png)}"#;
        let v = values(css);
        assert_eq!(v.len(), 2);
        assert!(v[0].1.starts_with("data:image/svg+xml"));
        assert_eq!(v[1].1, "after.png");
    }

    #[test]
    fn bad_urls_are_skipped_without_panicking() {
        for css in [
            "a{b:url(bad url.png)}",
            "a{b:url(un(paren)}",
            "a{b:url(\"unterminated",
            "a{b:url(",
            "url(",
            "@import",
            "@import ",
            "\\",
            "\"",
            "/*",
            "a{b:url(x\\",
        ] {
            let refs = scan(css);
            // Rewriting with whatever was found must not panic either.
            let _ = rewrite(css, &refs, |_| Some("X".into()));
        }
        assert!(values("a{b:url(bad url.png)}").is_empty());
        // Recovery: a later good url() is still found.
        assert_eq!(values("a{b:url(bad url.png)}c{d:url(ok.png)}").len(), 1);
    }

    #[test]
    fn image_set_strings_are_urls() {
        let css = r#"e{background-image:image-set("img/hi.png" 1x, 'img/hi@2x.png' 2x)}f{background:-WEBKIT-image-set("w.png" 1x,url(u.png) 2x)}g{b:image-set("a.avif" type("image/avif"), "a.png")}"#;
        assert_eq!(
            values(css),
            vec![
                (RefKind::Url, "img/hi.png".into(), Some('"')),
                (RefKind::Url, "img/hi@2x.png".into(), Some('\'')),
                (RefKind::Url, "w.png".into(), Some('"')),
                (RefKind::Url, "u.png".into(), None),
                // type("image/avif") is a MIME type, not a URL.
                (RefKind::Url, "a.avif".into(), Some('"')),
                (RefKind::Url, "a.png".into(), Some('"')),
            ]
        );
        let out = rewrite(css, &scan(css), |r| Some(format!("../{}", r.value)));
        assert!(
            out.starts_with(
                r#"e{background-image:image-set("../img/hi.png" 1x, '../img/hi@2x.png' 2x)}"#
            ),
            "{out}"
        );
        assert!(out.contains(r#"type("image/avif")"#), "{out}");
    }

    #[test]
    fn strings_outside_image_set_are_not_urls() {
        for css in [
            r#"a{content:"x.png"}"#,
            r#"a{font-family:"img.png"}b{c:my-image-set("x.png")}d{e:imageset("y.png")}"#,
            r#"a{b:image-set(linear-gradient(red, blue) 1x)}c{content:"after.png"}"#,
            r#"@font-face{src:local("Name"),url(f.woff2) format("woff2")}"#,
            // A bad url() inside image-set() closes properly; an unclosed image-set( ends with
            // its rule.
            r#"a{b:image-set(url(bad x) 1x)}c{content:"z.png"}"#,
            r#"a{b:image-set(}c{content:"z.png"}"#,
            r#"a{b:image-set(/* "comment.png" */ url(ok.png) 1x)}"#,
        ] {
            let v = values(css);
            let strings: Vec<&str> = v
                .iter()
                .filter(|(_, _, quote)| quote.is_some())
                .map(|(_, value, _)| value.as_str())
                .collect();
            assert!(strings.is_empty(), "{css}: {v:?}");
        }
        assert_eq!(
            values(r#"a{b:image-set(/* "comment.png" */ url(ok.png) 1x)}"#),
            vec![(RefKind::Url, "ok.png".into(), None)]
        );
        assert_eq!(
            values(r#"a{b:image-set(url(bad x) 1x, "good.png" 2x)}"#),
            vec![(RefKind::Url, "good.png".into(), Some('"'))]
        );
    }

    #[test]
    fn unescapes() {
        assert_eq!(unescape(r"a\(b\)"), "a(b)");
        assert_eq!(unescape(r"\31 23"), "123");
        assert_eq!(unescape(r"\0041"), "A");
        assert_eq!(unescape("a\\\nb"), "ab");
        assert_eq!(unescape(r"\0"), "\u{FFFD}");
        assert_eq!(unescape(r"\110000"), "\u{FFFD}");
        assert_eq!(unescape("x\\"), "x");
        assert_eq!(values(r"a{b:url(we\ ird.png)}")[0].1, "we ird.png");
    }

    #[test]
    fn rewrite_keeps_quotes_and_surroundings() {
        let css = r#"a{b:url(/i/a.png)}c{d:url("../b.png#frag")}e{f:url('c.png')}@import "g.css";"#;
        let refs = scan(css);
        let out = rewrite(css, &refs, |r| match r.value.as_str() {
            "/i/a.png" => Some("../images/a.png".into()),
            "../b.png#frag" => Some("b.png#frag".into()),
            "g.css" => Some("lib/g.css".into()),
            _ => None,
        });
        assert_eq!(
            out,
            r#"a{b:url(../images/a.png)}c{d:url("b.png#frag")}e{f:url('c.png')}@import "lib/g.css";"#
        );
    }

    #[test]
    fn rewrite_escapes_special_characters() {
        let css = r#"a{b:url(x)}c{d:url("y")}"#;
        let refs = scan(css);
        let out = rewrite(css, &refs, |r| {
            Some(if r.quote.is_some() {
                "q\"uote".into()
            } else {
                "p(a) r".into()
            })
        });
        assert_eq!(out, r#"a{b:url(p\(a\)\20 r)}c{d:url("q\"uote")}"#);
        // And the result scans back to the intended values.
        let back: Vec<String> = scan(&out).into_iter().map(|r| r.value).collect();
        assert_eq!(back, vec!["p(a) r", "q\"uote"]);
    }

    #[test]
    fn non_ascii_text_is_preserved() {
        let css = "a{content:\"é中\"}b{c:url(ü.png)}/* 😀 */";
        let refs = scan(css);
        assert_eq!(refs[0].value, "ü.png");
        let out = rewrite(css, &refs, |_| Some("u.png".into()));
        assert_eq!(out, "a{content:\"é中\"}b{c:url(u.png)}/* 😀 */");
    }
}
