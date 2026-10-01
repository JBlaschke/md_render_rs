//! comrak options and the AST passes that run before formatting (CQ §3.1 step 2).
//!
//! * tagfilter on raw HTML literals (GH §8.11; comrak's own `tagfilter` is deprecated),
//! * task-list fixes (GH §8.1: `- [ ]` alone is not a task),
//! * alert detection (GH §8.2, document-level blockquotes only),
//! * dollar math (GH §8.6 boundary rules) on `Text` nodes, plus display-math paragraphs.

use std::borrow::Cow;
use std::collections::HashSet;

use comrak::nodes::{AlertType, AstNode, NodeAlert, NodeValue};
use comrak::{Arena, Options};

use super::Ctx;
use super::util::{mdr_key, push_text};

/// comrak options for the GitHub flavor (GH §10): GFM extensions without comrak's alerts,
/// dollar math, shortcodes and heading ids, which we implement ourselves.
pub(super) fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.footnotes = true;
    o.extension.math_code = true;
    o.render.r#unsafe = true;
    o
}

/// Facts collected by the AST passes for the formatter.
#[derive(Debug, Default)]
pub(super) struct AstInfo {
    /// Alert nodes (by address) whose marker paragraph keeps more lines: the title `<p>` and
    /// the body `<p>` are then written without a newline between them (GH §8.2).
    pub alert_inline: HashSet<usize>,
    /// Whether any `<math-renderer>` was produced.
    pub math: bool,
}

pub(super) fn addr(node: &AstNode<'_>) -> usize {
    std::ptr::from_ref(node) as usize
}

/// Runs all AST passes.
pub(super) fn transform<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>, ctx: &Ctx) -> AstInfo {
    let mut info = AstInfo::default();
    let nodes: Vec<&'a AstNode<'a>> = root.descendants().collect();

    for &node in &nodes {
        let mut ast = node.data_mut();
        match &mut ast.value {
            NodeValue::HtmlBlock(b) => {
                if let Some(f) = tagfilter(&b.literal) {
                    b.literal = f;
                }
            }
            NodeValue::HtmlInline(lit) => {
                if let Some(f) = tagfilter(lit) {
                    *lit = f;
                }
            }
            _ => {}
        }
    }

    for &node in &nodes {
        if matches!(node.data().value, NodeValue::TaskItem(_)) {
            fix_task_item(arena, node);
        }
    }
    for &node in &nodes {
        let is_list = matches!(node.data().value, NodeValue::List(_));
        if is_list {
            let any_task =
                node.children().any(|c| matches!(c.data().value, NodeValue::TaskItem(_)));
            if let NodeValue::List(l) = &mut node.data_mut().value {
                l.is_task_list = any_task;
            }
        }
    }

    for child in root.children().collect::<Vec<_>>() {
        if matches!(child.data().value, NodeValue::BlockQuote) {
            detect_alert(child, &mut info);
        }
    }

    for &node in &nodes {
        if node.parent().is_none() && !std::ptr::eq(node, root) {
            continue; // detached by an earlier pass
        }
        let kind = match node.data().value {
            NodeValue::Paragraph => 1,
            NodeValue::Text(ref t) if t.contains('$') => 2,
            _ => 0,
        };
        match kind {
            1 => display_math(arena, node, ctx, &mut info),
            2 => inline_math(arena, node, ctx, &mut info),
            _ => {}
        }
    }
    info
}

// ------------------------------------------------------------------------------------------
// tagfilter (GH §8.11)

/// Raw tags renamed before HTML parsing: github.com's HTML parser predates the rule that
/// `<dialog>` / `<search>` close an open `<p>`; both are removed by the sanitizer anyway, so
/// an unknown name reproduces GitHub's tree (`S/probes/tags.gfm.html` T30, T101).
const LEGACY_INLINE: [&str; 2] = ["dialog", "search"];

const TAGFILTER: [&str; 9] =
    ["title", "textarea", "style", "xmp", "iframe", "noembed", "noframes", "script", "plaintext"];

fn tag_at(b: &[u8], j: usize, names: &[&str]) -> bool {
    names.iter().any(|t| {
        let end = j + t.len();
        end <= b.len()
            && b[j..end].eq_ignore_ascii_case(t.as_bytes())
            && match b.get(end) {
                None => false,
                Some(&c) => {
                    c.is_ascii_whitespace() || c == b'>' || (c == b'/' && b.get(end + 1) == Some(&b'>'))
                }
            }
    })
}

/// Escapes the `<` of every filtered tag (any case) and renames [`LEGACY_INLINE`] tags;
/// `None` when nothing matched.
fn tagfilter(literal: &str) -> Option<String> {
    let b = literal.as_bytes();
    let mut out: Option<String> = None;
    let mut last = 0;
    for (i, _) in literal.match_indices('<') {
        let mut j = i + 1;
        if b.get(j) == Some(&b'/') {
            j += 1;
        }
        let replacement = if tag_at(b, j, &TAGFILTER) {
            "&lt;"
        } else if tag_at(b, j, &LEGACY_INLINE) {
            if j == i + 1 { "<mdr-legacy-" } else { "</mdr-legacy-" }
        } else {
            continue;
        };
        let s = out.get_or_insert_with(|| String::with_capacity(literal.len() + 16));
        s.push_str(&literal[last..i]);
        s.push_str(replacement);
        last = j;
        if replacement == "&lt;" {
            last = i + 1;
        }
    }
    out.map(|mut s| {
        s.push_str(&literal[last..]);
        s
    })
}

// ------------------------------------------------------------------------------------------
// task lists (GH §8.1)

/// A `TaskItem` without text (`- [ ]` alone) is a plain item showing the brackets.
fn fix_task_item<'a>(arena: &'a Arena<'a>, node: &'a AstNode<'a>) {
    let first_is_para =
        node.first_child().is_some_and(|c| matches!(c.data().value, NodeValue::Paragraph));
    if first_is_para {
        return;
    }
    let symbol = match node.data().value {
        NodeValue::TaskItem(ref t) => t.symbol,
        _ => return,
    };
    let list = node.parent().and_then(|p| match p.data().value {
        NodeValue::List(ref l) => Some(*l),
        _ => None,
    });
    let Some(list) = list else { return };
    let start = node.data().sourcepos.start;
    node.data_mut().value = NodeValue::Item(list);
    let para = arena.alloc(AstNode::from(NodeValue::Paragraph));
    para.data_mut().sourcepos.start = start;
    let text = format!("[{}]", symbol.unwrap_or(' '));
    para.append(arena.alloc(AstNode::from(NodeValue::Text(Cow::Owned(text)))));
    node.prepend(para);
}

// ------------------------------------------------------------------------------------------
// alerts (GH §8.2)

fn alert_type(marker: &str) -> Option<AlertType> {
    let inner = marker.trim_end().strip_prefix("[!")?.strip_suffix(']')?;
    Some(match inner.to_ascii_lowercase().as_str() {
        "note" => AlertType::Note,
        "tip" => AlertType::Tip,
        "important" => AlertType::Important,
        "warning" => AlertType::Warning,
        "caution" => AlertType::Caution,
        _ => return None,
    })
}

fn detect_alert(bq: &AstNode<'_>, info: &mut AstInfo) {
    let Some(para) = bq.first_child() else { return };
    if !matches!(para.data().value, NodeValue::Paragraph) {
        return;
    }
    let mut marker = String::new();
    let mut marker_nodes = Vec::new();
    let mut brk = None;
    for child in para.children() {
        match child.data().value {
            NodeValue::Text(ref t) => {
                marker.push_str(t);
                marker_nodes.push(child);
            }
            NodeValue::SoftBreak | NodeValue::LineBreak => {
                brk = Some(child);
                break;
            }
            _ => return,
        }
    }
    let Some(kind) = alert_type(&marker) else { return };
    let more_in_para = brk.is_some_and(|b| b.next_sibling().is_some());
    if !more_in_para && para.next_sibling().is_none() {
        return; // `> [!NOTE]` alone stays a blockquote
    }
    bq.data_mut().value = NodeValue::Alert(Box::new(NodeAlert {
        alert_type: kind,
        title: None,
        multiline: false,
        fence_length: 0,
        fence_offset: 0,
    }));
    for n in marker_nodes {
        n.detach();
    }
    if let Some(b) = brk {
        b.detach();
    }
    if para.first_child().is_none() {
        para.detach();
    } else {
        info.alert_inline.insert(addr(bq));
    }
}

// ------------------------------------------------------------------------------------------
// dollar math (GH §8.6)

/// `<math-renderer>` markup; `delim` is `$` or `$$`, `display` picks the class.
pub(super) fn math_html(ctx: &Ctx, tex: &str, delim: &str, display: bool) -> String {
    let (class, style) =
        if display { ("js-display-math", "block") } else { ("js-inline-math", "inline-block") };
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

/// A paragraph consisting only of `$$…$$` (single or multi-line) is display math — except
/// inside list items, where github.com applies the inline rules (`S/pages/atapas_*`).
fn display_math<'a>(arena: &'a Arena<'a>, para: &'a AstNode<'a>, ctx: &Ctx, info: &mut AstInfo) {
    if para
        .ancestors()
        .any(|a| matches!(a.data().value, NodeValue::Item(_) | NodeValue::TaskItem(_)))
    {
        return;
    }
    let mut text = String::new();
    for child in para.children() {
        match child.data().value {
            NodeValue::Text(ref t) => text.push_str(t),
            NodeValue::SoftBreak => text.push('\n'),
            _ => return,
        }
    }
    if text.len() < 5 || !text.starts_with("$$") || !text.ends_with("$$") {
        return;
    }
    let inner = &text[2..text.len() - 2];
    if inner.trim().is_empty() {
        return;
    }
    let html = math_html(ctx, inner, "$$", true);
    for child in para.children().collect::<Vec<_>>() {
        child.detach();
    }
    para.append(arena.alloc(AstNode::from(NodeValue::Raw(html))));
    info.math = true;
}

fn is_math_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n')
}

/// Inline math spans in `s` as `(start, end, delimiter length)`. `boundary` says whether a `$`
/// at offset 0 counts as preceded by a boundary (block start or line break).
fn find_math(s: &str, boundary: bool) -> Vec<(usize, usize, usize)> {
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
        let prev_ok = if i == 0 { boundary } else { matches!(b[i - 1], b' ' | b'\t' | b'\n' | b'(') };
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

fn inline_math<'a>(arena: &'a Arena<'a>, node: &'a AstNode<'a>, ctx: &Ctx, info: &mut AstInfo) {
    // Never inside image alt text (rendered as plain text) .
    if node.ancestors().any(|a| matches!(a.data().value, NodeValue::Image(_))) {
        return;
    }
    let boundary = match node.previous_sibling() {
        None => node.parent().is_some_and(|p| {
            matches!(
                p.data().value,
                NodeValue::Paragraph | NodeValue::TableCell | NodeValue::Heading(_)
            )
        }),
        Some(prev) => matches!(prev.data().value, NodeValue::SoftBreak | NodeValue::LineBreak),
    };
    let text = match node.data().value {
        NodeValue::Text(ref t) => t.to_string(),
        _ => return,
    };
    let spans = find_math(&text, boundary);
    if spans.is_empty() {
        return;
    }
    let mut last = 0;
    let mut anchor = node;
    let mut first = true;
    for (start, end, run) in spans {
        let before = &text[last..start];
        if first {
            node.data_mut().value = NodeValue::Text(Cow::Owned(before.to_owned()));
            first = false;
        } else if !before.is_empty() {
            let t = arena.alloc(AstNode::from(NodeValue::Text(Cow::Owned(before.to_owned()))));
            anchor.insert_after(t);
            anchor = t;
        }
        let delim = &text[start..start + run];
        let html = math_html(ctx, &text[start + run..end - run], delim, false);
        let raw = arena.alloc(AstNode::from(NodeValue::Raw(html)));
        anchor.insert_after(raw);
        anchor = raw;
        last = end;
    }
    if last < text.len() {
        let t = arena.alloc(AstNode::from(NodeValue::Text(Cow::Owned(text[last..].to_owned()))));
        anchor.insert_after(t);
    }
    info.math = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagfilter_cases() {
        assert_eq!(tagfilter("<script>x</script>").as_deref(), Some("&lt;script>x&lt;/script>"));
        assert_eq!(tagfilter("<IFRAME src=x>").as_deref(), Some("&lt;IFRAME src=x>"));
        assert_eq!(tagfilter("<style/>").as_deref(), Some("&lt;style/>"));
        assert_eq!(tagfilter("<scripts>"), None);
        assert_eq!(tagfilter("<div>ok</div>"), None);
        assert_eq!(tagfilter("<title"), None);
        assert_eq!(tagfilter("a <dialog>b</DIALOG>").as_deref(), Some("a <mdr-legacy-dialog>b</mdr-legacy-DIALOG>"));
    }

    #[test]
    fn alert_markers() {
        assert_eq!(alert_type("[!NOTE]"), Some(AlertType::Note));
        assert_eq!(alert_type("[!note]  "), Some(AlertType::Note));
        assert_eq!(alert_type("[!Caution]"), Some(AlertType::Caution));
        assert_eq!(alert_type("[!FOO]"), None);
        assert_eq!(alert_type("[!NOTE] Title"), None);
    }

    fn spans(s: &str, boundary: bool) -> Vec<&str> {
        find_math(s, boundary).into_iter().map(|(a, b, _)| &s[a..b]).collect()
    }

    /// GH §8.6 and `S/probes/math{,2,3,4}.gfm.html`.
    #[test]
    fn inline_math_boundaries() {
        assert_eq!(spans("$x$ at start", true), ["$x$"]);
        assert_eq!(spans("$x$ not at start", false), Vec::<&str>::new());
        assert_eq!(spans("a2 ($x$)", true), ["$x$"]);
        assert_eq!(spans("a4 a $x$b", true), Vec::<&str>::new());
        assert_eq!(spans("a5 $1 and $2", true), Vec::<&str>::new());
        assert_eq!(spans("a6 $x $y$", true), ["$y$"]);
        assert_eq!(spans("a7 $$a$$b", true), Vec::<&str>::new());
        assert_eq!(spans("a8 $x$-y", true), ["$x$"]);
        assert_eq!(spans("a9 x-$y$", true), Vec::<&str>::new());
        assert_eq!(spans("b1 $x$_y", true), Vec::<&str>::new());
        assert_eq!(spans("b3 $$$", true), Vec::<&str>::new());
        assert_eq!(spans("c1 $x$$y$", true), ["$x$"]);
        assert_eq!(spans("c4 $x$y$", true), Vec::<&str>::new());
        assert_eq!(spans("C: inline $$x^2$$ in", true), ["$$x^2$$"]);
        assert_eq!(spans("B: costs $5 and $10, $ x $ spaced, $x$y, a$x$ b", true), Vec::<&str>::new());
        assert_eq!(spans("x $x$é y", true), ["$x$"]);
        assert_eq!(spans("x ((\"$x$ y", true), Vec::<&str>::new());
        assert_eq!(spans("x\t$y$ tab", true), ["$y$"]);
        assert_eq!(spans("x\u{a0}$y$", true), Vec::<&str>::new());
        assert_eq!(spans("multi $a +\nb$", true), Vec::<&str>::new());
        assert_eq!(spans("$x<y$ and $a & b$", true), ["$x<y$", "$a & b$"]);
    }
}
