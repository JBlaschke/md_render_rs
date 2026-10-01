//! The neutral HTML formatter (CQ §3.1 step 3, CR §1.6 `create_formatter!`).
//!
//! Emits comrak-like markup in GitHub's shapes where the construct is purely generated
//! (footnotes GH §8.4, task lists GH §8.1, alerts GH §8.2, code blocks GH §7.1, math GH §8.6,
//! enrichment GH §8.7). Every element it writes carries the sanitizer marker
//! ([`Ctx::mark`] / [`Ctx::mark_tree`]) so the raw-HTML allowlist (GH §8.11) applies to raw
//! HTML only. Only top-level blocks get `data-sourcepos` (DESIGN §5).

use std::collections::HashSet;
use std::fmt::{self, Write as _};

use comrak::html::{ChildRendering, Context, format_node_default};
use comrak::nodes::{AlertType, AstNode, ListType, NodeValue, TableAlignment};
use comrak::{Options, create_formatter};

use super::Ctx;
use super::ast::{addr, math_html};
use super::code;
use super::octicons;

/// Formatter state (comrak hands it back after formatting).
#[derive(Default)]
pub(super) struct FmtState {
    pub comment: bool,
    pub sourcepos: bool,
    pub mark: String,
    pub mark_tree: String,
    pub run_id: String,
    pub token: String,
    pub bundle_url: String,
    pub alert_inline: HashSet<usize>,
    /// Feature flags for [`crate::render::Features`].
    pub math: bool,
    pub mermaid: bool,
    /// Number of enrichment sections so far (for distinct `data-identity`s).
    pub enrichments: usize,
    suppress_cr: bool,
    pending_checkbox: Option<&'static str>,
    footnote_ix: u32,
}

impl FmtState {
    pub(super) fn new(ctx: &Ctx, alert_inline: HashSet<usize>) -> Self {
        FmtState {
            comment: ctx.comment,
            sourcepos: ctx.sourcepos,
            mark: ctx.mark.clone(),
            mark_tree: ctx.mark_tree.clone(),
            run_id: ctx.run_id.clone(),
            token: ctx.token.clone(),
            bundle_url: ctx.bundle_url.clone(),
            alert_inline,
            ..FmtState::default()
        }
    }

    /// A [`Ctx`] view of the state, for the shared markup builders.
    fn ctx(&self) -> Ctx {
        Ctx {
            comment: self.comment,
            sourcepos: self.sourcepos,
            run_id: self.run_id.clone(),
            token: self.token.clone(),
            mark: self.mark.clone(),
            mark_tree: self.mark_tree.clone(),
            bundle_url: self.bundle_url.clone(),
        }
    }
}

type Ctxt<'o, 'c> = Context<'o, 'c, FmtState>;
type R = Result<ChildRendering, fmt::Error>;

create_formatter!(GhFormatter<FmtState>, {
    _ => |context, node, entering| {
        return format_node(context, node, entering);
    },
});

/// Formats the AST rooted at `root`.
pub(super) fn format<'a>(
    root: &'a AstNode<'a>,
    options: &Options,
    state: FmtState,
) -> (String, FmtState) {
    let mut out = String::with_capacity(4096);
    let plugins = comrak::options::Plugins::default();
    // `format_document_with_plugins` skips comrak's debug-build AST validation (our passes
    // keep the tree well-formed, and rendering must never panic).
    match GhFormatter::format_document_with_plugins(root, options, &mut out, &plugins, state) {
        Ok(state) => (out, state),
        // Writing into a String cannot fail; keep whatever was produced.
        Err(_) => (out, FmtState::default()),
    }
}

fn is_top_level(node: &AstNode<'_>) -> bool {
    node.parent()
        .is_some_and(|p| matches!(p.data().value, NodeValue::Document))
}

/// ` data-sourcepos="…"` for top-level blocks when enabled (DESIGN §5).
pub(super) fn sourcepos_attr(state: &FmtState, node: &AstNode<'_>) -> String {
    if state.sourcepos && is_top_level(node) {
        let sp = node.data().sourcepos;
        if sp.start.line > 0 {
            return format!(" data-sourcepos=\"{sp}\"");
        }
    }
    String::new()
}

fn tight_paragraph(node: &AstNode<'_>) -> bool {
    node.parent()
        .and_then(|p| p.parent())
        .is_some_and(|gp| match gp.data().value {
            NodeValue::List(ref l) => l.tight,
            _ => false,
        })
}

const CHECKBOX_OPEN: &str = r#"type="checkbox" id="" disabled="" class="task-list-item-checkbox" aria-label="Incomplete task"> "#;
const CHECKBOX_DONE: &str = r#"type="checkbox" id="" disabled="" class="task-list-item-checkbox" aria-label="Completed task" checked=""> "#;

fn write_checkbox(context: &mut Ctxt<'_, '_>, body: &str) -> fmt::Result {
    let mark = context.user.mark.clone();
    write!(context, "<input{mark} {body}")
}

fn alert_meta(t: AlertType) -> (&'static str, &'static str, &'static str) {
    match t {
        AlertType::Note => ("note", "info", "Note"),
        AlertType::Tip => ("tip", "light-bulb", "Tip"),
        AlertType::Important => ("important", "report", "Important"),
        AlertType::Warning => ("warning", "alert", "Warning"),
        AlertType::Caution => ("caution", "stop", "Caution"),
    }
}

fn footnote_label(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    // cmark-gfm `houdini_escape_href` (GH §8.4: labels are percent-encoded UTF-8).
    let _ = comrak::html::escape_href(&mut s, name, false);
    s
}

#[allow(clippy::too_many_lines)]
fn format_node<'a>(context: &mut Ctxt<'_, '_>, node: &'a AstNode<'a>, entering: bool) -> R {
    let mark = context.user.mark.clone();
    let ast = node.data();
    match &ast.value {
        NodeValue::Document => {
            if !entering && context.user.footnote_ix > 0 {
                context.cr()?;
                context.write_str("</ol>\n</section>\n")?;
            }
        }
        NodeValue::BlockQuote => {
            if entering {
                context.cr()?;
                let sp = sourcepos_attr(&context.user, node);
                writeln!(context, "<blockquote{mark}{sp}>")?;
            } else {
                context.cr()?;
                context.write_str("</blockquote>\n")?;
            }
        }
        NodeValue::Alert(a) => {
            if entering {
                let (kind, icon, title) = alert_meta(a.alert_type);
                context.cr()?;
                let sp = sourcepos_attr(&context.user, node);
                let svg = octicons::alert_svg(icon, &context.user.mark_tree);
                write!(
                    context,
                    r#"<div{mark} class="markdown-alert markdown-alert-{kind}"{sp}><p{mark} class="markdown-alert-title">{svg}{title}</p>"#
                )?;
                if context.user.alert_inline.contains(&addr(node)) {
                    context.user.suppress_cr = true;
                }
            } else {
                context.cr()?;
                context.write_str("</div>\n")?;
            }
        }
        NodeValue::List(l) => {
            if entering {
                context.cr()?;
                let sp = sourcepos_attr(&context.user, node);
                let class = if l.is_task_list {
                    r#" class="contains-task-list""#
                } else {
                    ""
                };
                match l.list_type {
                    ListType::Bullet => writeln!(context, "<ul{mark}{class}{sp}>")?,
                    ListType::Ordered => {
                        if l.start == 1 {
                            writeln!(context, "<ol{mark}{class}{sp}>")?;
                        } else {
                            writeln!(context, "<ol{mark}{class} start=\"{}\"{sp}>", l.start)?;
                        }
                    }
                }
            } else if l.list_type == ListType::Bullet {
                context.write_str("</ul>\n")?;
            } else {
                context.write_str("</ol>\n")?;
            }
        }
        NodeValue::Item(_) => {
            if entering {
                context.cr()?;
                write!(context, "<li{mark}>")?;
            } else {
                context.write_str("</li>\n")?;
            }
        }
        NodeValue::TaskItem(t) => {
            if entering {
                context.cr()?;
                write!(context, "<li{mark} class=\"task-list-item\">")?;
                let body = if t.symbol.is_some() {
                    CHECKBOX_DONE
                } else {
                    CHECKBOX_OPEN
                };
                let tight = node.parent().is_some_and(|p| match p.data().value {
                    NodeValue::List(ref l) => l.tight,
                    _ => false,
                });
                if tight {
                    write_checkbox(context, body)?;
                } else {
                    context.user.pending_checkbox = Some(body);
                }
            } else {
                context.write_str("</li>\n")?;
            }
        }
        NodeValue::Paragraph => {
            let tight = tight_paragraph(node);
            if entering {
                if !tight {
                    if context.user.suppress_cr {
                        context.user.suppress_cr = false;
                    } else {
                        context.cr()?;
                    }
                    let sp = sourcepos_attr(&context.user, node);
                    write!(context, "<p{mark}{sp}>")?;
                }
                if let Some(body) = context.user.pending_checkbox.take() {
                    write_checkbox(context, body)?;
                }
            } else {
                let fndef = node.parent().and_then(|p| match p.data().value {
                    NodeValue::FootnoteDefinition(ref d) => Some(d.clone()),
                    _ => None,
                });
                if let Some(d) = fndef.filter(|_| node.next_sibling().is_none()) {
                    write_backrefs(context, &d.name, d.total_references, true)?;
                }
                if !tight {
                    context.write_str("</p>\n")?;
                }
            }
        }
        NodeValue::Heading(h) => {
            if entering {
                context.cr()?;
                let sp = sourcepos_attr(&context.user, node);
                write!(context, "<h{}{mark}{sp}>", h.level)?;
            } else {
                writeln!(context, "</h{}>", h.level)?;
            }
        }
        NodeValue::ThematicBreak => {
            if entering {
                context.cr()?;
                let sp = sourcepos_attr(&context.user, node);
                writeln!(context, "<hr{mark}{sp}>")?;
            }
        }
        NodeValue::CodeBlock(cb) => {
            if entering {
                context.cr()?;
                let lang = cb.info.split_ascii_whitespace().next().unwrap_or("");
                let sp = sourcepos_attr(&context.user, node);
                let index = context.user.enrichments;
                let ctx = context.user.ctx();
                let block = code::block_html(&ctx, lang, &cb.literal, &sp, index);
                context.user.math |= block.math;
                context.user.mermaid |= block.mermaid;
                if block.enrichment {
                    context.user.enrichments += 1;
                }
                context.write_str(&block.html)?;
                context.lf()?;
            }
        }
        NodeValue::HtmlBlock(b) => {
            if entering {
                context.cr()?;
                if context.user.sourcepos && is_top_level(node) {
                    let sp = node.data().sourcepos;
                    let token = context.user.token.clone();
                    write!(context, "<!--{}{token}:{sp}-->", super::SOURCEPOS_COMMENT)?;
                }
                context.write_str(&b.literal)?;
                context.cr()?;
            }
        }
        NodeValue::HtmlInline(lit) | NodeValue::Raw(lit) => {
            if entering {
                context.write_str(lit)?;
            }
        }
        NodeValue::Text(t) => {
            if entering {
                context.escape(t)?;
            }
        }
        NodeValue::SoftBreak => {
            if entering {
                if context.user.comment {
                    // GH §9.B: every newline in a comment is a hard break.
                    writeln!(context, "<br{mark}>")?;
                } else {
                    context.write_str("\n")?;
                }
            }
        }
        NodeValue::LineBreak => {
            if entering {
                writeln!(context, "<br{mark}>")?;
            }
        }
        NodeValue::Code(c) => {
            if entering {
                write!(context, "<code{mark}>")?;
                context.escape(&c.literal)?;
                context.write_str("</code>")?;
            }
        }
        NodeValue::Emph => tag(context, "em", &mark, entering)?,
        NodeValue::Strong => tag(context, "strong", &mark, entering)?,
        NodeValue::Strikethrough => tag(context, "del", &mark, entering)?,
        NodeValue::Link(l) => {
            if entering {
                write!(context, "<a{mark} href=\"")?;
                context.escape_href(&l.url)?;
                if !l.title.is_empty() {
                    context.write_str("\" title=\"")?;
                    context.escape(&l.title)?;
                }
                context.write_str("\">")?;
            } else {
                context.write_str("</a>")?;
            }
        }
        NodeValue::Image(l) => {
            if entering {
                write!(context, "<img{mark} src=\"")?;
                context.escape_href(&l.url)?;
                context.write_str("\" alt=\"")?;
                return Ok(ChildRendering::Plain);
            }
            context.write_str("\"")?;
            if !l.title.is_empty() {
                context.write_str(" title=\"")?;
                context.escape(&l.title)?;
                context.write_str("\"")?;
            }
            context.write_str(">")?;
        }
        NodeValue::FootnoteReference(r) => {
            if entering {
                let label = footnote_label(&r.name);
                let rid = context.user.run_id.clone();
                let sfx = if r.ref_num > 1 {
                    format!("-{}", r.ref_num)
                } else {
                    String::new()
                };
                write!(
                    context,
                    r##"<sup{mark}><a{mark} href="#user-content-fn-{label}-{rid}" id="user-content-fnref-{label}{sfx}-{rid}" data-footnote-ref="" aria-describedby="footnote-label">{}</a></sup>"##,
                    r.ix
                )?;
            }
        }
        NodeValue::FootnoteDefinition(d) => {
            if entering {
                if context.user.footnote_ix == 0 {
                    context.cr()?;
                    let sp = footnotes_sourcepos(&context.user, node);
                    write!(
                        context,
                        "<section{mark} data-footnotes=\"\" class=\"footnotes\"{sp}><h2{mark} id=\"footnote-label\" class=\"sr-only\">Footnotes</h2>\n<ol{mark}>\n"
                    )?;
                }
                context.user.footnote_ix += 1;
                let label = footnote_label(&d.name);
                let rid = context.user.run_id.clone();
                writeln!(context, "<li{mark} id=\"user-content-fn-{label}-{rid}\">")?;
            } else {
                // A definition not ending in a paragraph gets its backrefs on their own
                // line (cmark-gfm; `S/ctx/joseph_*` `[^rich]`).
                let last_is_para = node
                    .last_child()
                    .is_some_and(|c| matches!(c.data().value, NodeValue::Paragraph));
                context.cr()?;
                if !last_is_para {
                    write_backrefs(context, &d.name, d.total_references, false)?;
                    context.cr()?;
                }
                context.write_str("</li>\n")?;
            }
        }
        NodeValue::Math(m) => {
            if entering {
                // `$`…`$` (math_code): inline, rendered with `$` delimiters (GH §8.6).
                let ctx = context.user.ctx();
                let html = math_html(&ctx, &m.literal, "$", false);
                context.user.math = true;
                context.write_str(&html)?;
            }
        }
        NodeValue::Table(_) => {
            if entering {
                context.cr()?;
                let sp = sourcepos_attr(&context.user, node);
                writeln!(context, "<table{mark}{sp}>")?;
            } else {
                let has_body = match (node.first_child(), node.last_child()) {
                    (Some(f), Some(l)) => !f.same_node(l),
                    _ => false,
                };
                if has_body {
                    context.cr()?;
                    context.write_str("</tbody>\n")?;
                }
                context.cr()?;
                context.write_str("</table>\n")?;
            }
        }
        &NodeValue::TableRow(header) => {
            if entering {
                context.cr()?;
                if header {
                    writeln!(context, "<thead{mark}>")?;
                } else if node
                    .previous_sibling()
                    .is_some_and(|p| matches!(p.data().value, NodeValue::TableRow(true)))
                {
                    writeln!(context, "<tbody{mark}>")?;
                }
                write!(context, "<tr{mark}>")?;
            } else {
                context.cr()?;
                context.write_str("</tr>")?;
                if header {
                    context.cr()?;
                    context.write_str("</thead>")?;
                }
            }
        }
        NodeValue::TableCell => {
            let header = node
                .parent()
                .is_some_and(|r| matches!(r.data().value, NodeValue::TableRow(true)));
            if entering {
                context.cr()?;
                let name = if header { "th" } else { "td" };
                let align = cell_alignment(node);
                write!(context, "<{name}{mark}{align}>")?;
            } else if header {
                context.write_str("</th>")?;
            } else {
                context.write_str("</td>")?;
            }
        }
        _ => {
            drop(ast);
            return format_node_default(context, node, entering);
        }
    }
    Ok(ChildRendering::HTML)
}

fn tag(context: &mut Ctxt<'_, '_>, name: &str, mark: &str, entering: bool) -> fmt::Result {
    if entering {
        write!(context, "<{name}{mark}>")
    } else {
        write!(context, "</{name}>")
    }
}

fn cell_alignment(cell: &AstNode<'_>) -> &'static str {
    let Some(row) = cell.parent() else { return "" };
    let Some(table) = row.parent() else { return "" };
    let index = row.children().take_while(|c| !c.same_node(cell)).count();
    match table.data().value {
        NodeValue::Table(ref t) => match t.alignments.get(index) {
            Some(TableAlignment::Left) => " align=\"left\"",
            Some(TableAlignment::Right) => " align=\"right\"",
            Some(TableAlignment::Center) => " align=\"center\"",
            _ => "",
        },
        _ => "",
    }
}

/// Footnote backrefs at the end of a definition's last paragraph (GH §8.4).
fn write_backrefs(
    context: &mut Ctxt<'_, '_>,
    name: &str,
    total: u32,
    leading_space: bool,
) -> fmt::Result {
    let mark = context.user.mark.clone();
    let label = footnote_label(name);
    let rid = context.user.run_id.clone();
    let ix = context.user.footnote_ix;
    for k in 1..=total {
        let (sfx, sup) = if k > 1 {
            (format!("-{k}"), format!("<sup{mark}>{k}</sup>"))
        } else {
            (String::new(), String::new())
        };
        let space = if k > 1 || leading_space { " " } else { "" };
        write!(
            context,
            r##"{space}<a{mark} href="#user-content-fnref-{label}{sfx}-{rid}" data-footnote-backref="" aria-label="Back to reference {ix}{sfx}" class="data-footnote-backref">↩{sup}</a>"##
        )?;
    }
    Ok(())
}

/// `data-sourcepos` spanning all (trailing, top-level) footnote definitions.
fn footnotes_sourcepos(state: &FmtState, first: &AstNode<'_>) -> String {
    if !state.sourcepos || !is_top_level(first) {
        return String::new();
    }
    let first_sp = first.data().sourcepos;
    let (mut start, mut end) = (first_sp.start, first_sp.end);
    let mut cur = first.next_sibling();
    while let Some(n) = cur {
        if matches!(n.data().value, NodeValue::FootnoteDefinition(_)) {
            let sp = n.data().sourcepos;
            if (sp.start.line, sp.start.column) < (start.line, start.column) {
                start = sp.start;
            }
            if (sp.end.line, sp.end.column) > (end.line, end.column) {
                end = sp.end;
            }
        }
        cur = n.next_sibling();
    }
    format!(
        " data-sourcepos=\"{}:{}-{}:{}\"",
        start.line, start.column, end.line, end.column
    )
}
