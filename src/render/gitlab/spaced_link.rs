//! `SpacedLinkFilter` (GL §2 row 7): `[text](a b.md)` and `![alt](my image.png)` are not links
//! in CommonMark (spaces in the destination), but GitLab turns them into links by escaping the
//! spaces and rendering the snippet through the Markdown engine again. It runs before the
//! sanitizer, so the generated links are sanitized like everything else.

use std::sync::LazyLock;

use dom_query::{Document, NodeRef};
use regex::Regex;

use super::dom;

/// `LINK_OR_IMAGE_PATTERN` (RE2; `.` does not match `\n`).
static LINK_OR_IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?P<preview_operator>!)?\[(?P<text>.+?)\]\((?P<new_link>.+?)(?P<title> ".+?")?\)"#,
    )
    .expect("spaced link regex")
});

/// Text under these is never rewritten (`IGNORE_PARENTS`).
const IGNORE_PARENTS: &[&str] = &["a", "code", "kbd", "pre", "script", "style"];

/// `transform_markdown`: the matched snippet with spaces in the destination escaped.
fn transform_markdown(caps: &regex::Captures<'_>) -> String {
    format!(
        "{}[{}]({}{})",
        caps.name("preview_operator").map_or("", |m| m.as_str()),
        caps.name("text").map_or("", |m| m.as_str()),
        caps.name("new_link")
            .map_or("", |m| m.as_str())
            .replace(' ', "%20"),
        caps.name("title").map_or("", |m| m.as_str()),
    )
}

/// Inner HTML of the first `<p>` of the rendered snippet, or the whole HTML without one.
fn snippet_inline_html(html: &str) -> String {
    let doc = Document::fragment(html);
    match doc.select("p").nodes().first() {
        Some(p) => p.inner_html().to_string(),
        None => html.to_owned(),
    }
}

/// Rewrites one text node's content; `None` when nothing matches.
///
/// Like `Gitlab::StringRegexMarker`, matches are found in the raw text but the block sees the
/// corresponding slice of the *escaped* text (`node.to_html`), which is what gets re-rendered:
/// `[a](<b)` becomes the snippet `[a](&lt;b)`, i.e. `<a href="%3Cb">` (GLFM snapshot
/// `06_07_00__inlines__links__011`).
pub(super) fn rewrite(text: &str, render: &dyn Fn(&str) -> String) -> Option<String> {
    if !text.contains("](") || !LINK_OR_IMAGE.is_match(text) {
        return None;
    }
    let escape = super::filters::escape_text;
    let mut out = String::with_capacity(text.len() * 2);
    let mut last = 0;
    for m in LINK_OR_IMAGE.find_iter(text) {
        out.push_str(&escape(&text[last..m.start()]));
        let rich = escape(m.as_str());
        match LINK_OR_IMAGE.captures(&rich) {
            Some(caps) => out.push_str(&snippet_inline_html(&render(&transform_markdown(&caps)))),
            None => out.push_str(&rich),
        }
        last = m.end();
    }
    out.push_str(&escape(&text[last..]));
    Some(out)
}

/// The filter. `render` is GitLab's `MarkdownFilter` (comrak with GitLab's options).
pub(super) fn spaced_links(root: &NodeRef<'_>, render: &dyn Fn(&str) -> String) {
    for id in dom::text_nodes(root) {
        let t = root.tree.get_unchecked(&id);
        let content = dom::text_node_contents(&t).unwrap_or_default();
        if !content.contains("](") {
            continue;
        }
        if dom::has_ancestor(&t, IGNORE_PARENTS, root) {
            continue;
        }
        // `span[@data-math-style]` ancestors.
        let mut cur = t.parent();
        let mut in_math = false;
        while let Some(p) = cur {
            if p.id == root.id {
                break;
            }
            if dom::is(&p, "span") && p.has_attr("data-math-style") {
                in_math = true;
                break;
            }
            cur = p.parent();
        }
        if in_math {
            continue;
        }
        if let Some(html) = rewrite(&content, render) {
            t.replace_with_html(html);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_render(md: &str) -> String {
        // Minimal stand-in for comrak: enough to check the plumbing.
        format!(
            "<p data-sourcepos=\"1:1-1:{}\">RENDERED({})</p>",
            md.len(),
            dom::escape_html(md)
        )
    }

    #[test]
    fn escapes_spaces_and_keeps_title() {
        assert_eq!(
            rewrite("see [the doc](my doc.md \"T\") now", &fake_render).as_deref(),
            Some("see RENDERED([the doc](my%20doc.md \"T\")) now")
        );
        assert_eq!(
            rewrite("![alt](a b.png)", &fake_render).as_deref(),
            Some("RENDERED(![alt](a%20b.png))")
        );
        assert_eq!(rewrite("no link here", &fake_render), None);
        assert_eq!(rewrite("[a] (b c)", &fake_render), None);
    }

    #[test]
    fn real_render() {
        let render = |md: &str| super::super::render_snippet(md);
        let out = rewrite("x [doc](a b.md) y", &render).unwrap();
        assert_eq!(
            out,
            "x <a data-sourcepos=\"1:1-1:15\" href=\"a%20b.md\">doc</a> y"
        );
        // GLFM snapshot 06_07_00__inlines__links__011 (the escaped text is re-rendered).
        let out = rewrite("[a](<b)c", &render).unwrap();
        assert_eq!(out, "<a data-sourcepos=\"1:1-1:10\" href=\"%3Cb\">a</a>c");
        // GLFM snapshot 06_03_00__inlines__entity_and_numeric_character_references__017.
        let out = rewrite("[a](url \"tit\")", &render).unwrap();
        assert_eq!(
            out,
            "<a data-sourcepos=\"1:1-1:14\" href=\"url\" title=\"tit\">a</a>"
        );
    }
}
