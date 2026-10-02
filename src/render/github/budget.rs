//! Tree-building budget (GH-PERF-1).
//!
//! html5ever's tree builder scans the stack of open elements on (almost) every start tag
//! (`close_p_element_in_button_scope` → `in_scope_named`), so building the DOM costs about
//! the sum, over all start tags, of the nesting depth at that tag. Thousands of nested raw
//! `<div>`s or `>` blockquote markers make that quadratic: 40 000 levels took ~2 s and
//! 100 000 levels ~24 s per render in a release build, all of it in `Document::fragment`
//! (comrak and serialization stay in the milliseconds). Our own DOM passes walk ancestors
//! too. [`nesting_cost`] estimates the sum with one linear scan of the formatter output; the
//! renderer shows a document over [`MAX_NESTING_COST`] as plain source text with a warning,
//! as github.com shows the raw file when it cannot render one. Browsers flatten such trees
//! anyway (Blink stops nesting at 512 open elements), so nothing meaningful is lost.
//!
//! The estimate is a heuristic for runaway input, not a security boundary: the sanitizer
//! still runs on every document that is rendered.

use std::collections::HashMap;

/// About 50 ms of tree building in a release build; real documents stay several orders of
/// magnitude below it (a large README costs well under 100 000).
pub(super) const MAX_NESTING_COST: u64 = 20_000_000;

/// Elements that never stay open: void elements and those whose end tag the parser implies
/// (a `<p>` or `<li>` start tag closes an open one).
const NON_NESTING: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr", "p", "li", "dt", "dd", "tr", "td", "th", "option", "optgroup", "thead",
    "tbody", "tfoot", "colgroup", "caption", "rb", "rt", "rtc", "rp", "html", "head", "body",
];

/// Start tags whose "in body" handling scans the stack of open elements ("close a `p`
/// element" in button scope, the `li`/`dd`/`dt` loops, scope checks). Other start tags
/// (`<span>`, `<b>`, …) cost about the same at any depth.
const SCANNING: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "button",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "listing",
    "main",
    "menu",
    "nav",
    "nobr",
    "ol",
    "p",
    "plaintext",
    "pre",
    "search",
    "section",
    "summary",
    "table",
    "ul",
    "xmp",
];

fn tag_name(html: &str, start: usize) -> (String, usize) {
    let end = html[start..]
        .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
        .map_or(html.len(), |k| start + k);
    (html[start..end].to_ascii_lowercase(), end)
}

/// Estimated tree-building cost of `html`: the sum of the open-element depth at each
/// [`SCANNING`] start tag, plus one per other start tag. End tags close the innermost open
/// element of their name (and everything opened after it); end tags for elements that are
/// not open are ignored, as the parser does.
pub(super) fn nesting_cost(html: &str) -> u64 {
    let b = html.as_bytes();
    let mut stack: Vec<String> = Vec::new();
    let mut open: HashMap<String, usize> = HashMap::new();
    let mut cost: u64 = 0;
    let mut i = 0;
    while let Some(k) = html[i..].find('<') {
        i += k + 1;
        match b.get(i) {
            Some(b'!') if html[i..].starts_with("!--") => {
                // Comments end at `-->` (`<!-->` and `<!--->` at once).
                let body = i + 3;
                if html[body..].starts_with('>') {
                    i = body + 1;
                } else if html[body..].starts_with("->") {
                    i = body + 2;
                } else {
                    match html[body..].find("-->") {
                        Some(e) => i = body + e + 3,
                        None => break,
                    }
                }
            }
            Some(b'!' | b'?') => match html[i..].find('>') {
                // Doctype, CDATA and bogus comments end at the next `>`.
                Some(e) => i += e + 1,
                None => break,
            },
            Some(b'/') if b.get(i + 1).is_some_and(u8::is_ascii_alphabetic) => {
                let (name, end) = tag_name(html, i + 1);
                i = end;
                if open.get(&name).copied().unwrap_or(0) > 0 {
                    while let Some(top) = stack.pop() {
                        if let Some(n) = open.get_mut(&top) {
                            *n -= 1;
                        }
                        if top == name {
                            break;
                        }
                    }
                }
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let (name, end) = tag_name(html, i);
                i = end;
                cost += 1;
                if SCANNING.contains(&name.as_str()) {
                    cost += stack.len() as u64;
                }
                if !NON_NESTING.contains(&name.as_str()) {
                    *open.entry(name.clone()).or_insert(0) += 1;
                    stack.push(name);
                }
            }
            _ => {}
        }
    }
    cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs() {
        // Siblings are linear, nesting is quadratic.
        assert_eq!(nesting_cost(&"<div></div>".repeat(100)), 100);
        let n = 1000_u64;
        let deep = format!(
            "{}x{}",
            "<div>".repeat(n as usize),
            "</div>".repeat(n as usize)
        );
        assert_eq!(nesting_cost(&deep), n * (n + 1) / 2);
        // Inline start tags do not scan the stack (but still nest).
        assert_eq!(nesting_cost(&"<span>".repeat(1000)), 1000);
        assert_eq!(nesting_cost("<span><span><div>"), 1 + 1 + 3);
        // Implied end tags and void elements do not nest.
        assert_eq!(nesting_cost(&"<p>a<br><li>b<img src=x>".repeat(100)), 400);
        // Stray end tags do not hide nesting; end tags close what was opened after them.
        assert_eq!(nesting_cost("<div></span><div></span><div>"), 6);
        assert_eq!(nesting_cost("<div><b><i></div><div>"), 1 + 1 + 1 + 1);
        assert_eq!(nesting_cost("<div><b><i></div><div><div>"), 4 + 2);
        // Comments, doctypes and text are skipped.
        assert_eq!(
            nesting_cost("<!-- <div><div> --><!--><div><!DOCTYPE x><?x <div>?>"),
            1
        );
        assert_eq!(nesting_cost("a < b <3 </ x"), 0);
        // Upper-case names are the same elements.
        assert_eq!(nesting_cost("<DIV></div><div>"), 2);
    }
}
