//! Task lists (GH §8.1).
//!
//! github.com turns list items into tasks on the HTML tree (its `task_list` filter), so raw
//! HTML `<ul><li>[ ] x</li></ul>` converts too (API probe with context, 2026-10-01). A `li`
//! whose content — or first `<p>`, for loose items — starts with `[ ]`, `[x]` or `[X]`
//! followed by whitespace gets the checkbox in place of the marker (the whitespace after it is
//! kept: `[ ]  two` → `<input …>  two`), `class="task-list-item"`, and its list
//! `class="contains-task-list"`. `[ ]` alone, `[ ]x`, `[~]` and markers inside other elements
//! are not tasks.

use dom_query::{Document, NodeRef};

use super::dom;

/// `(prefix length, marker length, checked)` when `text` starts like a task item
/// (`task_list` gem `ItemPattern`: `^(?:\s*[-+*]|(?:\d+\.))?\s*(\[\s\]|\[[xX]\])(?=\s)`).
fn marker(text: &str, followed_by_blank: bool) -> Option<(usize, bool)> {
    let b = text.as_bytes();
    let mut i = 0;
    let skip_ws = |i: &mut usize| {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    };
    let save = i;
    skip_ws(&mut i);
    if i < b.len() && matches!(b[i], b'-' | b'+' | b'*') {
        i += 1;
    } else {
        i = save;
        let digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && b.get(i + digits) == Some(&b'.') {
            i += digits + 1;
        }
    }
    skip_ws(&mut i);
    let m = b.get(i..i + 3)?;
    let checked = match m {
        [b'[', c, b']'] if c.is_ascii_whitespace() => false,
        [b'[', b'x' | b'X', b']'] => true,
        _ => return None,
    };
    match b.get(i + 3) {
        Some(c) if c.is_ascii_whitespace() => Some((i, checked)),
        // The marker ends the text node and the next text node starts with a blank.
        None if followed_by_blank => Some((i, checked)),
        _ => None,
    }
}

fn checkbox<'a>(doc: &'a Document, checked: bool) -> NodeRef<'a> {
    let input = doc.tree.new_element("input");
    input.set_attr("type", "checkbox");
    input.set_attr("id", "");
    input.set_attr("disabled", "");
    input.set_attr("class", "task-list-item-checkbox");
    if checked {
        input.set_attr("aria-label", "Completed task");
        input.set_attr("checked", "");
    } else {
        input.set_attr("aria-label", "Incomplete task");
    }
    input
}

/// Converts every qualifying `li`.
pub(super) fn run(doc: &Document) {
    for li in dom::elements_named(doc, "li") {
        convert(doc, &li);
    }
}

fn convert(doc: &Document, li: &NodeRef<'_>) {
    // Loose items carry the marker in their first paragraph.
    let first_non_blank = li
        .children()
        .into_iter()
        .find(|c| dom::text_of(c).is_none_or(|t| !t.trim().is_empty()));
    let target = match first_non_blank {
        Some(p) if p.has_name("p") => p,
        _ => *li,
    };
    let Some(first) = target.first_child() else {
        return;
    };
    let Some(text) = dom::text_of(&first) else {
        return;
    };
    let next_starts_blank = first
        .next_sibling()
        .and_then(|n| dom::text_of(&n))
        .is_some_and(|t| t.starts_with(|c: char| c.is_ascii_whitespace()));
    let Some((at, checked)) = marker(&text, next_starts_blank) else {
        return;
    };

    if at > 0 {
        first.insert_before(&doc.tree.new_text(&text[..at]));
    }
    first.insert_before(&checkbox(doc, checked));
    let rest = &text[at + 3..];
    if rest.is_empty() {
        first.remove_from_parent();
    } else {
        first.set_text(rest);
    }
    li.add_class("task-list-item");
    if let Some(list) = li.parent().filter(|p| p.has_name("ul") || p.has_name("ol")) {
        list.add_class("contains-task-list");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: &str = r#"<input type="checkbox" id="" disabled="" class="task-list-item-checkbox" aria-label="Incomplete task">"#;
    const DONE: &str = r#"<input type="checkbox" id="" disabled="" class="task-list-item-checkbox" aria-label="Completed task" checked="">"#;

    fn run_on(html: &str) -> String {
        let doc = Document::fragment(html);
        run(&doc);
        dom::root(&doc).inner_html().to_string()
    }

    #[test]
    fn tight_and_loose() {
        assert_eq!(
            run_on("<ul>\n<li>[ ] open</li>\n<li>[x] done</li>\n<li>plain</li>\n</ul>"),
            format!(
                "<ul class=\"contains-task-list\">\n<li class=\"task-list-item\">{OPEN} open</li>\n<li class=\"task-list-item\">{DONE} done</li>\n<li>plain</li>\n</ul>"
            )
        );
        assert_eq!(
            run_on("<ol><li>\n<p>[X] loose</p>\n</li></ol>"),
            format!(
                "<ol class=\"contains-task-list\"><li class=\"task-list-item\">\n<p>{DONE} loose</p>\n</li></ol>"
            )
        );
        assert_eq!(
            run_on("<ul><li>[ ]  two spaces</li></ul>"),
            format!(
                "<ul class=\"contains-task-list\"><li class=\"task-list-item\">{OPEN}  two spaces</li></ul>"
            )
        );
    }

    /// `S/probes/tasks.gfm.html`.
    #[test]
    fn non_tasks() {
        for html in [
            "<ul><li>[ ]</li></ul>",
            "<ul><li>[x]</li></ul>",
            "<ul><li>[ ]no space</li></ul>",
            "<ul><li>[~] tilde</li></ul>",
            "<ul><li><strong>[ ] bold</strong></li></ul>",
            "<ul><li>[ ]<b>x</b></li></ul>",
            "<ul><li>text [ ] later</li></ul>",
        ] {
            assert_eq!(run_on(html), html);
        }
    }
}
