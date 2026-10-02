//! Autolinking of bare URLs (GH §8.9).
//!
//! Two passes share the matcher:
//!
//! * [`find_markdown`], used by the AST pass in [`super::ast`], links what comrak leaves
//!   unlinked in Markdown text although cmark-gfm (and therefore github.com) links it, in both
//!   contexts (GH-AUTO-1). comrak's `url_match` / `www_match` call `check_domain` with
//!   `allow_short = relaxed_autolinks`, so without `relaxed_autolinks` (which GitHub does not
//!   have: it allows every scheme and bracketed links) a host needs a dot after `scheme://` or
//!   after `www.`. cmark-gfm's `url_match` passes `allow_short = 1`, so
//!   `http://localhost:3000`, `https://intranet/x` and `https://my-host:9000/x` are links on
//!   github.com, and its `www_match` checks the domain from the start of `www.`, so the dot
//!   after `www` counts (`www.localhost`). cmark-gfm also compares the scheme
//!   case-insensitively (`strncasecmp` in `sd_autolink_issafe`), comrak exactly.
//! * [`run`]: in the README/file context github.com also autolinks bare URLs that end up as
//!   text inside raw HTML (for example the text of a tag-filtered `<iframe>` block,
//!   `S/ctx/naskio_*`); the comment context does not (`S/api_gfm_ctx.html`). This pass covers
//!   text nodes whose parent is raw HTML, additionally ending a link at `"` and `>` since the
//!   text is markup-like.
//!
//! The rules follow cmark-gfm's `autolink.c` (`www.` and `scheme://` matches, domain check,
//! trailing punctuation trimming).

use std::collections::HashSet;

use dom_query::{Document, NodeId, NodeRef};

use super::dom;

/// Elements whose text is never autolinked.
const SKIP: &[&str] = &["a", "code", "pre", "math-renderer", "svg", "g-emoji"];

fn is_domain_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// End (byte offset) of a valid domain starting at `start`.
fn check_domain(s: &str, start: usize, allow_short: bool) -> Option<usize> {
    let mut periods = 0;
    let mut end = start;
    let mut seg_underscore = [false, false]; // last two segments
    for (off, c) in s[start..].char_indices() {
        if c == '.' {
            if off == 0 || s[start..start + off].ends_with('.') {
                break;
            }
            periods += 1;
            seg_underscore = [seg_underscore[1], false];
        } else if is_domain_char(c) {
            if c == '_' {
                seg_underscore[1] = true;
            }
        } else {
            break;
        }
        end = start + off + c.len_utf8();
    }
    let domain = &s[start..end];
    let domain = domain.trim_end_matches('.');
    let end = start + domain.len();
    if domain.is_empty() || (!allow_short && periods == 0) || seg_underscore.contains(&true) {
        return None;
    }
    Some(end)
}

/// cmark-gfm `autolink_delim`: extend to whitespace / `<` (in `markup` text also to `>` and
/// `"`), then trim trailing punctuation, unbalanced `)` and entity-like `&name;`.
fn link_end(s: &str, start: usize, from: usize, markup: bool) -> usize {
    let b = s.as_bytes();
    let stop = |c: char| {
        if markup {
            c.is_whitespace() || matches!(c, '<' | '>' | '"')
        } else {
            // cmark-gfm stops at `cmark_isspace` (ASCII) and `<`.
            c.is_ascii_whitespace() || c == '<'
        }
    };
    let mut end = s[from..].find(stop).map_or(s.len(), |k| from + k);
    while end > start {
        let last = b[end - 1];
        if b"?!.,:*_~'\"".contains(&last) {
            end -= 1;
        } else if last == b';' {
            let mut k = end - 1;
            while k > start && b[k - 1].is_ascii_alphabetic() {
                k -= 1;
            }
            if k < end - 1 && k > start && b[k - 1] == b'&' {
                end = k - 1
            } else {
                end -= 1
            }
        } else if last == b')' {
            let open = s[start..end].matches('(').count();
            let close = s[start..end].matches(')').count();
            if close > open { end -= 1 } else { break }
        } else {
            break;
        }
    }
    end
}

/// `(start, end, href)` of every autolink in `s`.
pub(super) fn find(s: &str) -> Vec<(usize, usize, String)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut floor = 0; // no match may start before the end of the previous one
    while i < b.len() {
        if b[i] == b'w' && s[i..].starts_with("www.") {
            let boundary = i == 0
                || s[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_whitespace() || "*_~(".contains(c));
            if boundary {
                if let Some(d) = check_domain(s, i, false) {
                    let end = link_end(s, i, d, true);
                    if end > i + 4 {
                        out.push((i, end, format!("http://{}", &s[i..end])));
                        i = end;
                        floor = end;
                        continue;
                    }
                }
            }
        } else if b[i] == b':' && s[i..].starts_with("://") {
            let mut st = i;
            while st > floor && b[st - 1].is_ascii_alphabetic() {
                st -= 1;
            }
            let scheme = s[st..i].to_ascii_lowercase();
            if matches!(scheme.as_str(), "http" | "https" | "ftp") {
                if let Some(d) = check_domain(s, i + 3, true) {
                    let end = link_end(s, st, d, true);
                    if end > i + 3 {
                        out.push((st, end, s[st..end].to_owned()));
                        i = end;
                        floor = end;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// `(start, end, href)` of every autolink cmark-gfm makes in the Markdown text `s` (one
/// comrak text node) that comrak itself leaves unlinked (GH-AUTO-1).
///
/// * `start_boundary`: whether a `www.` right at the start of `s` follows a character that
///   allows it (start of the inline content, whitespace, or one of `*_~(`).
/// * `brackets`: the number of `[` opened before `s` in the same block and not closed yet,
///   updated for the brackets in `s`. cmark-gfm makes no autolink while a bracket is open
///   (`cmark_inline_parser_in_bracket`), and neither does comrak (`within_brackets`).
///
/// A `scheme://` match needs `http` or `https` (any case) as the whole alphabetic run before
/// the `:`, an ASCII alphanumeric right after `://` (`sd_autolink_issafe`) and a domain that
/// may be short. `ftp://` is left alone: github.com drops its `href` (GH §8.9), so it renders
/// as text either way.
pub(super) fn find_markdown(
    s: &str,
    start_boundary: bool,
    brackets: &mut usize,
) -> Vec<(usize, usize, String)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut floor = 0; // no match may start before the end of the previous one
    while i < b.len() {
        match b[i] {
            b'[' => *brackets += 1,
            b']' => *brackets = brackets.saturating_sub(1),
            b'w' if *brackets == 0 && s[i..].starts_with("www.") => {
                let boundary = if i == 0 {
                    start_boundary
                } else {
                    b[i - 1].is_ascii_whitespace() || b"*_~(".contains(&b[i - 1])
                };
                if let Some(d) = boundary.then(|| check_domain(s, i, false)).flatten() {
                    let end = link_end(s, i, d, false);
                    if end > i + 4 {
                        out.push((i, end, format!("http://{}", &s[i..end])));
                        i = end;
                        floor = end;
                        continue;
                    }
                }
            }
            b':' if *brackets == 0 && s[i..].starts_with("://") => {
                let mut st = i;
                while st > floor && b[st - 1].is_ascii_alphabetic() {
                    st -= 1;
                }
                let safe = matches!(s[st..i].to_ascii_lowercase().as_str(), "http" | "https")
                    && b.get(i + 3).is_some_and(u8::is_ascii_alphanumeric);
                if let Some(d) = safe.then(|| check_domain(s, i + 3, true)).flatten() {
                    let end = link_end(s, st, d, false);
                    if end > i + 3 {
                        out.push((st, end, s[st..end].to_owned()));
                        i = end;
                        floor = end;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Re-checks a [`find_markdown`] match `url` against the raw source `line`, searched from
/// byte `from`: cmark-gfm matches the source, while comrak's text is unescaped. Returns the
/// offset in `line` just after the occurrence and the length of the link cmark-gfm makes
/// there, which is shorter when the raw text trims differently (`https://h/&amp;` ends before
/// the entity). `None` when `line` does not hold `url` verbatim: `http\://localhost` and
/// `http&#58;//localhost` are not links on github.com.
pub(super) fn in_source(line: &str, from: usize, url: &str) -> Option<(usize, usize)> {
    let occ = from + line.get(from..)?.find(url)?;
    let end = link_end(line, occ, occ + url.len(), false);
    let len = (end - occ).min(url.len());
    let min = url.find("://").map_or(4, |k| k + 3);
    (len > min).then_some((occ + url.len(), len))
}

/// Autolinks text nodes that come from raw HTML: direct children of the root or of a kept
/// raw element (`raw`, from the sanitizer).
pub(super) fn run(doc: &Document, raw: &HashSet<NodeId>) {
    let root = dom::root(doc);
    let texts: Vec<NodeRef<'_>> = root
        .descendants()
        .into_iter()
        .filter(NodeRef::is_text)
        .collect();
    for t in texts {
        let Some(parent) = t.parent() else { continue };
        if parent.id != root.id && !raw.contains(&parent.id) {
            continue;
        }
        let Some(s) = dom::text_of(&t) else { continue };
        if !s.contains("://") && !s.contains("www.") {
            continue;
        }
        if dom::name(&parent).is_some_and(|n| SKIP.contains(&&*n))
            || dom::has_ancestor(&t, |a| dom::name(a).is_some_and(|n| SKIP.contains(&&*n)))
        {
            continue;
        }
        let links = find(&s);
        if links.is_empty() {
            continue;
        }
        let mut last = 0;
        for (start, end, href) in links {
            if start > last {
                t.insert_before(&doc.tree.new_text(&s[last..start]));
            }
            let a = doc.tree.new_element("a");
            a.set_attr("href", &href);
            a.append_child(&doc.tree.new_text(&s[start..end]));
            t.insert_before(&a);
            last = end;
        }
        if last < s.len() {
            t.insert_before(&doc.tree.new_text(&s[last..]));
        }
        t.remove_from_parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(s: &str) -> Vec<&str> {
        find(s).into_iter().map(|(a, b, _)| &s[a..b]).collect()
    }

    #[test]
    fn matches() {
        assert_eq!(
            links(r#"src="https://www.youtube.com/embed/T-D1?si=V" title="x""#),
            ["https://www.youtube.com/embed/T-D1?si=V"]
        );
        assert_eq!(links("see www.example.com."), ["www.example.com"]);
        assert_eq!(find("www.example.com")[0].2, "http://www.example.com");
        assert_eq!(links("(https://e.com/a_(b))"), ["https://e.com/a_(b)"]);
        assert_eq!(links("https://e.com/a)"), ["https://e.com/a"]);
        assert_eq!(links("https://e.com/&amp;"), ["https://e.com/"]);
        assert_eq!(links("xwww.example.com"), Vec::<&str>::new());
        assert_eq!(links("javascript://x.com"), Vec::<&str>::new());
        assert_eq!(
            links("http://localhost:8080/x"),
            ["http://localhost:8080/x"]
        );
        assert_eq!(links("https://a_b.c_d/x"), Vec::<&str>::new());
        assert_eq!(links("no links here"), Vec::<&str>::new());
    }

    fn md_links(s: &str, start_boundary: bool) -> Vec<String> {
        let mut brackets = 0;
        find_markdown(s, start_boundary, &mut brackets)
            .into_iter()
            .map(|(_, _, h)| h)
            .collect()
    }

    /// GH-AUTO-1: cmark-gfm's `url_match` / `www_match` rules on Markdown text.
    #[test]
    fn markdown_matches() {
        assert_eq!(
            md_links("Open http://localhost:3000 now.", true),
            ["http://localhost:3000"]
        );
        assert_eq!(
            md_links("https://intranet/path", true),
            ["https://intranet/path"]
        );
        assert_eq!(
            md_links("https://my-host:9000/x", true),
            ["https://my-host:9000/x"]
        );
        assert_eq!(md_links("HTTPS://h/x.", true), ["HTTPS://h/x"]);
        assert_eq!(md_links("(http://h/a_(b))", true), ["http://h/a_(b)"]);
        assert_eq!(md_links("1http://h", true), ["http://h"]);
        // Markdown text is not markup: `"` and `>` do not end a link (only trimmed / kept).
        assert_eq!(md_links("http://h/a>b", true), ["http://h/a>b"]);
        assert_eq!(md_links("http://h/\"", true), ["http://h/"]);
        // www: the dot after `www` counts, the boundary rule applies.
        assert_eq!(md_links("www.localhost", true), ["http://www.localhost"]);
        assert_eq!(md_links("*www.localhost", true), ["http://www.localhost"]);
        assert!(md_links("www.localhost", false).is_empty());
        assert!(md_links("xwww.localhost", true).is_empty());
        assert!(md_links("www.", true).is_empty());
        // Negatives.
        for s in [
            "ftp://localhost",
            "see http:// alone",
            "http://-x",
            "http:// x",
            "xhttp://localhost",
            "javascript://x",
            "http://a_b/x",
            "www.a_b",
            "no links here",
        ] {
            assert!(md_links(s, true).is_empty(), "{s}");
        }
        // No autolink while a bracket is open, in this text node or an earlier one.
        assert!(md_links("[in http://h brackets]", true).is_empty());
        assert_eq!(md_links("[a] http://h", true), ["http://h"]);
        let mut brackets = 0;
        assert!(find_markdown("![alt ", true, &mut brackets).is_empty());
        assert_eq!(brackets, 1);
        assert!(find_markdown(" http://h www.x", true, &mut brackets).is_empty());
        assert_eq!(find_markdown("] http://h", true, &mut brackets).len(), 1);
        assert_eq!(brackets, 0);
    }

    #[test]
    fn source_recheck() {
        // Verbatim: the full match.
        assert_eq!(in_source("x http://h/a y", 0, "http://h/a"), Some((12, 10)));
        // An entity right after the URL ends it in the source (`&amp;` decoded to `&`).
        assert_eq!(
            in_source("https://h/&amp;", 0, "https://h/&"),
            Some((11, 10))
        );
        // Escaped or entity-encoded in the source: not a link.
        assert_eq!(in_source(r"http\://h", 0, "http://h"), None);
        assert_eq!(in_source("http&#58;//h", 0, "http://h"), None);
        // Searched from `from`.
        assert_eq!(in_source("http://h http://h", 1, "http://h"), Some((17, 8)));
    }

    #[test]
    fn only_raw_text_is_linked() {
        let doc =
            Document::fragment("x https://e.com/a y<p>https://m.com</p><span>https://r.com</span>");
        let raw: HashSet<NodeId> = dom::elements_named(&doc, "span")
            .iter()
            .map(|n| n.id)
            .collect();
        run(&doc, &raw);
        assert_eq!(
            dom::root(&doc).inner_html().to_string(),
            r#"x <a href="https://e.com/a">https://e.com/a</a> y<p>https://m.com</p><span><a href="https://r.com">https://r.com</a></span>"#
        );
    }
}
