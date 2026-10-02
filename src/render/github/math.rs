//! Math (GH §8.6): `<math-renderer>` markup, and dollar math found in the HTML tree.
//!
//! github.com finds `$…$` / `$$…$$` on the HTML tree after alerts are built (API probes with
//! context, 2026-10-01):
//! * text inside raw HTML counts too (`<p>$x$</p>`, `<td>`, `<li>`, `<span>`, `<summary>`), but
//!   only when the nearest block ancestor is a paragraph, list item, table cell, heading,
//!   `summary` or `details` (not `div`, `section`, `blockquote`, `dt`, `dd`, top level);
//! * text inside `a`, `code`, `pre`, `em` or `b` never converts (`_$x$_`, `[$x$](u)`,
//!   `***$x$***`), `strong`, `i`, `del`, `kbd`, `span`, … do;
//! * the start of a text node counts as a boundary (`**b**$x$` converts);
//! * a paragraph made only of `$$…$$` spans is display math, except inside list items
//!   (`S/pages/atapas_*`), and also when it is the body of an alert (`> [!TIP]\n> $$e$$`).
//!
//! Markdown backslash escapes are already resolved in the tree (`\$x\$` is math, as on
//! github.com). GitHub's double escaping of `<`/`&` in math is not copied.

use dom_query::{Document, NodeRef};

use super::{Ctx, dom};
use crate::hash::mdr_key;
use crate::html::push_text;

/// `<math-renderer>` markup as a string; `delim` is `$` or `$$`, `display` picks the class.
pub(super) fn math_html(ctx: &Ctx, tex: &str, delim: &str, display: bool) -> String {
    let (class, style) = classes(display);
    let key = mdr_key(&["math", class, tex]);
    let mut s = format!(
        r#"<math-renderer{} class="{class}" style="display: {style}" data-run-id="{}" data-mdr-key="{key}">"#,
        ctx.mark_tree, ctx.run_id
    );
    s.push_str(delim);
    push_text(&mut s, tex);
    s.push_str(delim);
    s.push_str("</math-renderer>");
    s
}

fn classes(display: bool) -> (&'static str, &'static str) {
    if display {
        ("js-display-math", "block")
    } else {
        ("js-inline-math", "inline-block")
    }
}

fn math_element<'a>(
    doc: &'a Document,
    ctx: &Ctx,
    tex: &str,
    delim: &str,
    display: bool,
) -> NodeRef<'a> {
    let (class, style) = classes(display);
    let el = doc.tree.new_element("math-renderer");
    el.set_attr("class", class);
    el.set_attr("style", &format!("display: {style}"));
    el.set_attr("data-run-id", &ctx.run_id);
    el.set_attr("data-mdr-key", &mdr_key(&["math", class, tex]));
    el.append_child(&doc.tree.new_text(format!("{delim}{tex}{delim}")));
    el
}

/// Text inside these never becomes math.
const EXCLUDED: &[&str] = &[
    "a",
    "code",
    "pre",
    "em",
    "b",
    "math-renderer",
    "svg",
    "script",
    "style",
    "textarea",
    "g-emoji",
];

/// Block containers; the nearest one decides whether its text is scanned ([`MATH_BLOCKS`]).
const BLOCKS: &[&str] = &[
    "p",
    "li",
    "td",
    "th",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "summary",
    "details",
    "div",
    "section",
    "blockquote",
    "dt",
    "dd",
    "dl",
    "ul",
    "ol",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "markdown-accessiblity-table",
    "picture",
    "video",
];

const MATH_BLOCKS: &[&str] = &[
    "p", "li", "td", "th", "h1", "h2", "h3", "h4", "h5", "h6", "summary", "details",
];

/// Whether text directly inside `n` (an element or text node) may contain math.
fn eligible(n: &NodeRef<'_>) -> bool {
    let mut decided = None;
    let mut cur = n.parent();
    while let Some(p) = cur {
        if let Some(name) = dom::name(&p) {
            // Footnote definitions are not scanned (github.com appends them after its math
            // filter; API probe 2026-10-01). Raw HTML cannot carry `data-footnotes`.
            if EXCLUDED.contains(&&*name) || (&*name == "section" && p.has_attr("data-footnotes")) {
                return false;
            }
            if decided.is_none() && BLOCKS.contains(&&*name) {
                decided = Some(MATH_BLOCKS.contains(&&*name));
            }
        }
        cur = p.parent();
    }
    decided.unwrap_or(false)
}

/// Splits `text` into `$$…$$` spans separated only by whitespace; `None` if anything else
/// occurs. A blank span counts only when its delimiters are on separate lines (`$$\n$$` is
/// display math on github.com, GH-MATH-1 / `cases2/math_display_variants.gh.html`; `$$ $$`
/// and `$$$$` are not).
pub(super) fn display_spans(text: &str) -> Option<Vec<(usize, usize)>> {
    let mut spans = Vec::new();
    let mut pos = 0;
    loop {
        pos += text[pos..].len() - text[pos..].trim_start().len();
        if pos == text.len() {
            break;
        }
        if !text[pos..].starts_with("$$") {
            return None;
        }
        let close = pos + 2 + text[pos + 2..].find("$$")?;
        let content = &text[pos + 2..close];
        if content.trim().is_empty() && !content.contains('\n') {
            return None;
        }
        spans.push((pos, close + 2));
        pos = close + 2;
    }
    (!spans.is_empty()).then_some(spans)
}

fn is_math_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n')
}

/// Inline math spans in `s` as `(start, end, delimiter length)` (GH §8.6 boundary rules; the
/// start of `s` counts as a boundary).
pub(super) fn find_math(s: &str) -> Vec<(usize, usize, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' {
            i += 1;
            continue;
        }
        let mut run = 1;
        while i + run < b.len() && b[i + run] == b'$' {
            run += 1;
        }
        let prev_ok = i == 0 || matches!(b[i - 1], b' ' | b'\t' | b'\n' | b'(');
        if !prev_ok || run > 2 {
            i += run;
            continue;
        }
        let open = i + run;
        let close = if run == 1 {
            s[open..].find('$').map(|k| open + k)
        } else {
            s[open..].find("$$").map(|k| open + k)
        };
        let Some(close) = close else {
            i += run;
            continue;
        };
        let content = &b[open..close];
        let after = b.get(close + run).copied();
        let valid = !content.is_empty()
            && !is_math_space(content[0])
            && !is_math_space(content[content.len() - 1])
            && !content.contains(&b'\n')
            && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_');
        if valid {
            out.push((i, close + run, run));
            i = close + run;
        } else {
            i += run;
        }
    }
    out
}

/// Converts dollar math in the tree; returns whether any was found.
pub(super) fn run(doc: &Document, ctx: &Ctx) -> bool {
    let mut found = false;
    for p in dom::elements_named(doc, "p") {
        if display_paragraph(doc, ctx, &p) {
            found = true;
        }
    }
    let texts: Vec<NodeRef<'_>> = dom::root(doc)
        .descendants()
        .into_iter()
        .filter(NodeRef::is_text)
        .collect();
    for t in texts {
        let Some(s) = dom::text_of(&t) else { continue };
        if !s.contains('$') || !eligible(&t) {
            continue;
        }
        let spans = find_math(&s);
        if spans.is_empty() {
            continue;
        }
        let mut last = 0;
        for (start, end, run) in spans {
            if start > last {
                t.insert_before(&doc.tree.new_text(&s[last..start]));
            }
            let delim = &s[start..start + run];
            t.insert_before(&math_element(
                doc,
                ctx,
                &s[start + run..end - run],
                delim,
                false,
            ));
            last = end;
        }
        if last < s.len() {
            t.insert_before(&doc.tree.new_text(&s[last..]));
        }
        t.remove_from_parent();
        found = true;
    }
    found
}

/// A paragraph of only `$$…$$` spans (text and `<br>` children) becomes display math.
fn display_paragraph(doc: &Document, ctx: &Ctx, p: &NodeRef<'_>) -> bool {
    let mut text = String::new();
    for c in p.children() {
        if let Some(t) = dom::text_of(&c) {
            text.push_str(&t);
        } else if !c.has_name("br") {
            return false;
        }
    }
    if !text.trim_start().starts_with("$$")
        || dom::has_ancestor(p, |a| a.has_name("li"))
        || !eligible(&p.first_child().unwrap_or(*p))
    {
        return false;
    }
    let Some(spans) = display_spans(&text) else {
        return false;
    };
    p.remove_children();
    let mut last = 0;
    for (start, end) in spans {
        if start > last {
            p.append_child(&doc.tree.new_text(&text[last..start]));
        }
        p.append_child(&math_element(
            doc,
            ctx,
            &text[start + 2..end - 2],
            "$$",
            true,
        ));
        last = end;
    }
    if last < text.len() {
        p.append_child(&doc.tree.new_text(&text[last..]));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(s: &str) -> Vec<&str> {
        find_math(s).into_iter().map(|(a, b, _)| &s[a..b]).collect()
    }

    fn run_on(html: &str) -> String {
        let doc = Document::fragment(html);
        run(&doc, &Ctx::for_tests(false));
        dom::root(&doc)
            .inner_html()
            .to_string()
            .replace(
                r#" style="display: inline-block" data-run-id="00112233445566778899aabbccddeeff""#,
                "",
            )
            .replace(
                r#" style="display: block" data-run-id="00112233445566778899aabbccddeeff""#,
                "",
            )
            .split(" data-mdr-key=\"")
            .enumerate()
            .map(|(i, part)| {
                if i == 0 {
                    part.to_owned()
                } else {
                    part[17..].to_owned()
                }
            })
            .collect()
    }

    /// GH §8.6 and `S/probes/math{,2,3,4}.gfm.html`.
    #[test]
    fn inline_boundaries() {
        let none = Vec::<&str>::new();
        assert_eq!(spans("$x$ at start"), ["$x$"]);
        assert_eq!(spans("a2 ($x$)"), ["$x$"]);
        assert_eq!(spans("a4 a $x$b"), none);
        assert_eq!(spans("a5 $1 and $2"), none);
        assert_eq!(spans("a6 $x $y$"), ["$y$"]);
        assert_eq!(spans("a7 $$a$$b"), none);
        assert_eq!(spans("a8 $x$-y"), ["$x$"]);
        assert_eq!(spans("a9 x-$y$"), none);
        assert_eq!(spans("b1 $x$_y"), none);
        assert_eq!(spans("b3 $$$"), none);
        assert_eq!(spans("c1 $x$$y$"), ["$x$"]);
        assert_eq!(spans("c4 $x$y$"), none);
        assert_eq!(spans("C: inline $$x^2$$ in"), ["$$x^2$$"]);
        assert_eq!(
            spans("B: costs $5 and $10, $ x $ spaced, $x$y, a$x$ b"),
            none
        );
        assert_eq!(spans("x $x$é y"), ["$x$"]);
        assert_eq!(spans("x ((\"$x$ y"), none);
        assert_eq!(spans("x\t$y$ tab"), ["$y$"]);
        assert_eq!(spans("x\u{a0}$y$"), none);
        assert_eq!(spans("multi $a +\nb$"), none);
        assert_eq!(spans("$x<y$ and $a & b$"), ["$x<y$", "$a & b$"]);
    }

    #[test]
    fn display_spans_cases() {
        assert_eq!(display_spans("$$x$$"), Some(vec![(0, 5)]));
        assert_eq!(display_spans("$$\na\nb\n$$"), Some(vec![(0, 9)]));
        assert_eq!(display_spans("$$a$$ $$b$$"), Some(vec![(0, 5), (6, 11)]));
        assert_eq!(display_spans("$$a$$b"), None);
        assert_eq!(display_spans("$$a$$ text"), None);
        assert_eq!(display_spans("$$ $$"), None);
        assert_eq!(display_spans("$$a"), None);
        // GH-MATH-1: a blank span counts when the delimiters are on separate lines.
        assert_eq!(display_spans("$$\n$$"), Some(vec![(0, 5)]));
        assert_eq!(display_spans("$$  \n $$"), Some(vec![(0, 8)]));
        assert_eq!(display_spans("$$$$"), None);
    }

    /// API probes with context (2026-10-01).
    #[test]
    fn where_math_applies() {
        assert_eq!(
            run_on(
                "<p>a $x$ <em>$e$</em> <b>$b$</b> <strong>$s$</strong> <a>$l$</a> <code>$c$</code></p>"
            ),
            r#"<p>a <math-renderer class="js-inline-math">$x$</math-renderer> <em>$e$</em> <b>$b$</b> <strong><math-renderer class="js-inline-math">$s$</math-renderer></strong> <a>$l$</a> <code>$c$</code></p>"#
        );
        assert_eq!(
            run_on("<div>$d$ x</div><blockquote>$q$ x</blockquote>"),
            "<div>$d$ x</div><blockquote>$q$ x</blockquote>"
        );
        assert_eq!(
            run_on("<p><strong>b</strong>$x$ end</p>"),
            r#"<p><strong>b</strong><math-renderer class="js-inline-math">$x$</math-renderer> end</p>"#
        );
        assert_eq!(
            run_on("<p>$$a$$ $$b$$</p><ul><li><p>$$c$$</p></li></ul>"),
            r#"<p><math-renderer class="js-display-math">$$a$$</math-renderer> <math-renderer class="js-display-math">$$b$$</math-renderer></p><ul><li><p><math-renderer class="js-inline-math">$$c$$</math-renderer></p></li></ul>"#
        );
        // Comment context: hard breaks inside a multi-line display block disappear.
        assert_eq!(
            run_on("<p>$$<br>\na<br>\n$$</p>"),
            "<p><math-renderer class=\"js-display-math\">$$\na\n$$</math-renderer></p>"
        );
    }
}
