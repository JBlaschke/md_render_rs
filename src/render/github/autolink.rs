//! Autolinking of URLs in raw-HTML text (GH §8.9).
//!
//! In the README/file context github.com also autolinks bare URLs that end up as text inside
//! raw HTML (for example the text of a tag-filtered `<iframe>` block, `S/ctx/naskio_*`); the
//! comment context does not (`S/api_gfm_ctx.html`). Markdown text is autolinked by comrak
//! already; this pass covers text nodes whose parent is raw HTML. The rules follow cmark-gfm's
//! `autolink.c` (`www.` and `scheme://` matches, domain check, trailing punctuation trimming),
//! additionally ending a link at `"` and `>` since the text is markup-like.

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

/// cmark-gfm `autolink_delim`: extend to whitespace / `<`, then trim trailing punctuation,
/// unbalanced `)` and entity-like `&name;`.
fn link_end(s: &str, start: usize, from: usize) -> usize {
    let b = s.as_bytes();
    let mut end = s[from..]
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
        .map_or(s.len(), |k| from + k);
    while end > start {
        let last = b[end - 1];
        if b"?!.,:*_~'\"".contains(&last) {
            end -= 1;
        } else if last == b';' {
            let mut k = end - 1;
            while k > start && b[k - 1].is_ascii_alphabetic() {
                k -= 1;
            }
            if k < end - 1 && k > start && b[k - 1] == b'&' { end = k - 1 } else { end -= 1 }
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
                || s[..i].chars().next_back().is_some_and(|c| c.is_whitespace() || "*_~(".contains(c));
            if boundary {
                if let Some(d) = check_domain(s, i, false) {
                    let end = link_end(s, i, d);
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
                    let end = link_end(s, st, d);
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

/// Autolinks text nodes that come from raw HTML: direct children of the root or of a kept
/// raw element (`raw`, from the sanitizer).
pub(super) fn run(doc: &Document, raw: &HashSet<NodeId>) {
    let root = dom::root(doc);
    let texts: Vec<NodeRef<'_>> = root.descendants().into_iter().filter(NodeRef::is_text).collect();
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
        assert_eq!(links("http://localhost:8080/x"), ["http://localhost:8080/x"]);
        assert_eq!(links("https://a_b.c_d/x"), Vec::<&str>::new());
        assert_eq!(links("no links here"), Vec::<&str>::new());
    }

    #[test]
    fn only_raw_text_is_linked() {
        let doc = Document::fragment("x https://e.com/a y<p>https://m.com</p><span>https://r.com</span>");
        let raw: HashSet<NodeId> = dom::elements_named(&doc, "span").iter().map(|n| n.id).collect();
        run(&doc, &raw);
        assert_eq!(
            dom::root(&doc).inner_html().to_string(),
            r#"x <a href="https://e.com/a">https://e.com/a</a> y<p>https://m.com</p><span><a href="https://r.com">https://r.com</a></span>"#
        );
    }
}
