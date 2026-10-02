//! Alerts (GH §8.2): a `blockquote` that is a direct child of the content root and whose first
//! paragraph starts with `[!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]` or `[!CAUTION]`
//! (any case) on a line of its own becomes
//! `<div class="markdown-alert markdown-alert-TYPE"><p class="markdown-alert-title">ICON Title</p>…`.
//!
//! github.com detects alerts on the HTML tree, not on the Markdown AST: raw HTML
//! `<blockquote><p>[!TIP]</p>…` at the top level converts too, while a Markdown blockquote that
//! ends up inside a raw `<div>` or `<details>` does not (API probes with context, 2026-10-01).
//! A marker without any content (`> [!NOTE]` alone) stays a blockquote.

use dom_query::{Document, NodeRef};

use super::{dom, octicons};

/// `(class suffix, octicon, title)` for a marker such as `[!note]`.
fn alert_kind(marker: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match marker.to_ascii_lowercase().as_str() {
        "note" => ("note", "info", "Note"),
        "tip" => ("tip", "light-bulb", "Tip"),
        "important" => ("important", "report", "Important"),
        "warning" => ("warning", "alert", "Warning"),
        "caution" => ("caution", "stop", "Caution"),
        _ => return None,
    })
}

/// Parses `[!TYPE]` at the start of `text` (after optional blanks). Returns the kind and the
/// remainder after the marker and its trailing blanks; the remainder must be empty or start
/// with a newline.
fn parse_marker(text: &str) -> Option<((&'static str, &'static str, &'static str), &str)> {
    let t = text.trim_start_matches([' ', '\t']);
    let inner = t.strip_prefix("[!")?;
    let close = inner.find(']')?;
    let kind = alert_kind(&inner[..close])?;
    let rest = inner[close + 1..].trim_start_matches([' ', '\t']);
    (rest.is_empty() || rest.starts_with('\n')).then_some((kind, rest))
}

fn is_blank_text(n: &NodeRef<'_>) -> bool {
    dom::text_of(n).is_some_and(|t| t.trim().is_empty())
}

/// Converts every qualifying top-level blockquote.
pub(super) fn run(doc: &Document) {
    let root = dom::root(doc);
    for bq in root.children() {
        if bq.has_name("blockquote") {
            convert(doc, &bq);
        }
    }
}

fn convert(doc: &Document, bq: &NodeRef<'_>) {
    // The first non-blank child must be a paragraph starting with the marker text.
    let Some(para) = bq.children().into_iter().find(|c| !is_blank_text(c)) else {
        return;
    };
    if !para.has_name("p") {
        return;
    }
    let Some(first) = para.first_child() else {
        return;
    };
    let Some(text) = dom::text_of(&first) else {
        return;
    };
    let Some((kind, rest)) = parse_marker(&text) else {
        return;
    };

    // What follows the marker on its line: nothing (then a `<br>` or the paragraph end) or a
    // newline that starts the next line of the same paragraph.
    let rest = rest.strip_prefix('\n').map(str::to_owned);
    let br = if rest.is_none() {
        first.next_sibling().filter(|n| n.has_name("br"))
    } else {
        None
    };
    let para_has_more = match &rest {
        Some(r) => !r.is_empty() || first.next_sibling().is_some(),
        None => match &br {
            Some(b) => b.next_sibling().is_some(),
            None => first.next_sibling().is_some(),
        },
    };
    if rest.is_none() && br.is_none() && first.next_sibling().is_some() {
        return; // marker followed by inline content on the same line: `[!NOTE] **x**`
    }
    let blocks_after = {
        let mut cur = para.next_sibling();
        let mut any = false;
        while let Some(n) = cur {
            if !is_blank_text(&n) {
                any = true;
                break;
            }
            cur = n.next_sibling();
        }
        any
    };
    if !para_has_more && !blocks_after {
        return; // `> [!NOTE]` alone stays a blockquote
    }

    // Strip the marker line from the paragraph.
    match rest {
        Some(r) if !r.is_empty() => first.set_text(r.as_str()),
        _ => first.remove_from_parent(),
    }
    if let Some(b) = br {
        if let Some(next) = b.next_sibling()
            && let Some(t) = dom::text_of(&next)
        {
            let stripped = t.strip_prefix('\n').unwrap_or(&t).to_owned();
            if stripped.is_empty() {
                next.remove_from_parent();
            } else {
                next.set_text(stripped.as_str());
            }
        }
        b.remove_from_parent();
    }

    // Build `div.markdown-alert` around the blockquote's content.
    let (class, icon, title) = kind;
    let div = doc.tree.new_element("div");
    div.set_attr("class", &format!("markdown-alert markdown-alert-{class}"));
    if let Some(sp) = bq.attr("data-sourcepos") {
        div.set_attr("data-sourcepos", &sp);
    }
    let title_p = doc.tree.new_element("p");
    title_p.set_attr("class", "markdown-alert-title");
    title_p.append_child(&doc.tree.new_element(&octicons::placeholder(icon)));
    title_p.append_child(&doc.tree.new_text(title));
    div.append_child(&title_p);

    let mut leading = true;
    for c in bq.children() {
        if leading && is_blank_text(&c) {
            c.remove_from_parent();
            continue;
        }
        leading = false;
        if c.id == para.id && para.first_child().is_none() {
            // The marker was the whole paragraph.
            para.remove_from_parent();
            continue;
        }
        div.append_child(&c);
    }
    bq.insert_before(&div);
    bq.remove_from_parent();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_on(html: &str) -> String {
        let doc = Document::fragment(html);
        run(&doc);
        dom::root(&doc).inner_html().to_string()
    }

    const ICON_NOTE: &str = "<mdr-octicon-info></mdr-octicon-info>";

    #[test]
    fn markers() {
        assert!(parse_marker("[!NOTE]").is_some());
        assert!(parse_marker("[!note]\nbody").is_some());
        assert!(parse_marker("  [!Caution]  ").is_some());
        assert!(parse_marker("[!NOTE] Title").is_none());
        assert!(parse_marker("[!FOO]").is_none());
        assert!(parse_marker("x [!NOTE]").is_none());
    }

    #[test]
    fn shapes() {
        // Marker with more lines in the same paragraph: title and body without a newline.
        assert_eq!(
            run_on("<blockquote>\n<p>[!NOTE]\nbody</p>\n</blockquote>"),
            format!(
                r#"<div class="markdown-alert markdown-alert-note"><p class="markdown-alert-title">{ICON_NOTE}Note</p><p>body</p>
</div>"#
            )
        );
        // Marker alone in its paragraph.
        assert_eq!(
            run_on("<blockquote>\n<p>[!TIP]</p>\n<p>one</p>\n</blockquote>"),
            r#"<div class="markdown-alert markdown-alert-tip"><p class="markdown-alert-title"><mdr-octicon-light-bulb></mdr-octicon-light-bulb>Tip</p>
<p>one</p>
</div>"#
        );
        // Comment context: the marker line ends in `<br>`.
        assert_eq!(
            run_on("<blockquote>\n<p>[!WARNING]<br>\nbody</p>\n</blockquote>"),
            r#"<div class="markdown-alert markdown-alert-warning"><p class="markdown-alert-title"><mdr-octicon-alert></mdr-octicon-alert>Warning</p><p>body</p>
</div>"#
        );
    }

    #[test]
    fn non_alerts() {
        for html in [
            "<blockquote>\n<p>[!NOTE]</p>\n</blockquote>",
            "<blockquote>\n<p>[!NOTE] title\nbody</p>\n</blockquote>",
            "<blockquote>\n<p><strong>[!NOTE]</strong>\nbody</p>\n</blockquote>",
            "<blockquote>\n<p>[!NOTE]<em>x</em></p>\n</blockquote>",
            "<div><blockquote>\n<p>[!NOTE]\nnested</p>\n</blockquote></div>",
            "<ul><li><blockquote><p>[!NOTE]\nin list</p></blockquote></li></ul>",
        ] {
            assert_eq!(
                run_on(html),
                Document::fragment(html)
                    .html_root()
                    .inner_html()
                    .to_string(),
                "{html}"
            );
        }
    }

    #[test]
    fn sourcepos_moves_to_the_div() {
        let out = run_on(
            r#"<blockquote data-sourcepos="1:1-2:6"><p>[!NOTE]
x</p></blockquote>"#,
        );
        assert!(
            out.starts_with(
                r#"<div class="markdown-alert markdown-alert-note" data-sourcepos="1:1-2:6">"#
            ),
            "{out}"
        );
    }
}
