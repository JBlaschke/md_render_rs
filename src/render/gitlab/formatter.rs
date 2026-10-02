// Ported from the gitlab-glfm-markdown gem v0.0.43 (`src/formatter.rs`),
// https://gitlab.com/gitlab-org/ruby/gems/gitlab-glfm-markdown
//
// Copyright (c) 2011-present GitLab B.V.
// Licensed under the MIT Expat license; see THIRD_PARTY_NOTICES.md for the full text.
//
// Kept verbatim (GL §4.2, DESIGN D3) apart from: the module path of `RenderOptions`
// (`super::glfm`), this header and the `#![allow]` below (so the upstream style stays untouched).
// The `panic!`s in `render_table_cell` assert comrak AST invariants; comrak's own table-cell
// renderer has the identical checks, so they cannot fire for a parsed document.
#![allow(clippy::all)]

use std::fmt::{self, Write};
use std::sync::LazyLock;

use comrak::html::{format_node_default, render_sourcepos, ChildRendering, Context};
use comrak::nodes::{
    ListType, NodeHeading, NodeLink, NodeList, NodeTaskItem, NodeValue, TableAlignment,
};
use comrak::{create_formatter, html, node_matches, Node};
use regex::Regex;

use super::glfm::RenderOptions;

static PLACEHOLDER_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"%\{(?:\w{1,30})}").unwrap());

#[derive(Default)]
pub(super) struct RenderUserData {
    pub default_html: bool,
    pub inapplicable_tasks: bool,
    pub placeholder_detection: bool,
    pub only_escape_chars: Option<Vec<char>>,
    pub header_accessibility: bool,
    pub header_slug_prefix: Option<String>,

    last_heading: Option<String>,
}

impl From<&RenderOptions> for RenderUserData {
    fn from(options: &RenderOptions) -> Self {
        RenderUserData {
            default_html: options.default_html,
            inapplicable_tasks: options.inapplicable_tasks,
            placeholder_detection: options.placeholder_detection,
            only_escape_chars: options.only_escape_chars.clone(),
            header_accessibility: options.header_accessibility,
            header_slug_prefix: options.header_slug_prefix.clone(),
            last_heading: None,
        }
    }
}

// The important thing to remember is that this overrides the default behavior of the
// specified nodes. If we do override a node, then it's our responsibility to ensure that
// any changes in the Comrak code for those nodes is backported to here, such as when
// `figcaption` support was added.
//
// One idea to limit that would be having the ability to specify attributes that would
// be inserted when a node is rendered. That would allow us to (in many cases) just
// inject the changes we need. Such a feature would need to be added to Comrak.
create_formatter!(CustomFormatter<RenderUserData>, {
    NodeValue::Text(ref literal) => |context, node, entering| {
        return render_text(context, node, entering, literal);
    },
    NodeValue::Link(ref nl) => |context, node, entering| {
        return render_link(context, node, entering, nl);
    },
    NodeValue::Image(ref nl) => |context, node, entering| {
        return render_image(context, node, entering, nl);
    },
    NodeValue::List(ref nl) => |context, node, entering| {
        return render_list(context, node, entering, nl);
    },
    NodeValue::TaskItem(ref nti) => |context, node, entering| {
        return render_task_item(context, node, entering, nti);
    },
    NodeValue::TableCell => |context, node, entering| {
        return render_table_cell(context, node, entering);
    },
    NodeValue::Escaped => |context, node, entering| {
        return render_escaped(context, node, entering);
    },
    NodeValue::Heading(ref nh) => |context, node, entering| {
        return render_heading(context, node, entering, nh);
    },
});

fn render_text(
    context: &mut Context<RenderUserData>,
    node: Node<'_>,
    entering: bool,
    literal: &str,
) -> Result<ChildRendering, fmt::Error> {
    if !(context.user.placeholder_detection && PLACEHOLDER_REGEX.is_match(literal)) {
        return html::format_node_default(context, node, entering);
    }

    if entering {
        let mut cursor: usize = 0;

        for mat in PLACEHOLDER_REGEX.find_iter(literal) {
            if mat.start() > cursor {
                context.escape(&literal[cursor..mat.start()])?;
            }

            context.write_str("<span data-placeholder>")?;
            context.escape(&literal[mat.start()..mat.end()])?;
            context.write_str("</span>")?;

            cursor = mat.end();
        }

        if cursor < literal.len() {
            context.escape(&literal[cursor..])?;
        }
    }

    Ok(ChildRendering::HTML)
}

fn render_link(
    context: &mut Context<RenderUserData>,
    node: Node<'_>,
    entering: bool,
    nl: &NodeLink,
) -> Result<ChildRendering, fmt::Error> {
    if !(context.user.placeholder_detection && PLACEHOLDER_REGEX.is_match(&nl.url)) {
        return html::format_node_default(context, node, entering);
    }

    let parent_node = node.parent();

    if !context.options.parse.relaxed_autolinks
        || (parent_node.is_none()
            || !matches!(
                parent_node.unwrap().data.borrow().value,
                NodeValue::Link(..)
            ))
    {
        if entering {
            context.write_str("<a")?;
            html::render_sourcepos(context, node)?;
            context.write_str(" href=\"")?;
            if context.options.render.r#unsafe || !html::dangerous_url(&nl.url) {
                if let Some(rewriter) = &context.options.extension.link_url_rewriter {
                    context.escape_href(&rewriter.to_html(&nl.url))?;
                } else {
                    context.escape_href(&nl.url)?;
                }
            }
            context.write_str("\"")?;

            if !nl.title.is_empty() {
                context.write_str(" title=\"")?;
                context.escape(&nl.title)?;
            }

            // This path only taken if placeholder detection is enabled, and the regex matched.
            context.write_str(" data-placeholder")?;

            context.write_str(">")?;
        } else {
            context.write_str("</a>")?;
        }
    }

    Ok(ChildRendering::HTML)
}

fn render_image(
    context: &mut Context<RenderUserData>,
    node: Node<'_>,
    entering: bool,
    nl: &NodeLink,
) -> Result<ChildRendering, fmt::Error> {
    if !(context.user.placeholder_detection && PLACEHOLDER_REGEX.is_match(&nl.url)) {
        return html::format_node_default(context, node, entering);
    }

    if entering {
        if context.options.render.figure_with_caption {
            context.write_str("<figure>")?;
        }
        context.write_str("<img")?;
        html::render_sourcepos(context, node)?;
        context.write_str(" src=\"")?;
        if context.options.render.r#unsafe || !html::dangerous_url(&nl.url) {
            if let Some(rewriter) = &context.options.extension.image_url_rewriter {
                context.escape_href(&rewriter.to_html(&nl.url))?;
            } else {
                context.escape_href(&nl.url)?;
            }
        }

        context.write_str("\"")?;

        // This path only taken if placeholder detection is enabled, and the regex matched.
        context.write_str(" data-placeholder")?;

        context.write_str(" alt=\"")?;

        return Ok(ChildRendering::Plain);
    } else {
        if !nl.title.is_empty() {
            context.write_str("\" title=\"")?;
            context.escape(&nl.title)?;
        }
        context.write_str("\" />")?;
        if context.options.render.figure_with_caption {
            if !nl.title.is_empty() {
                context.write_str("<figcaption>")?;
                context.escape(&nl.title)?;
                context.write_str("</figcaption>")?;
            }
            context.write_str("</figure>")?;
        };
    }

    Ok(ChildRendering::HTML)
}

// Overridden to use class `task-list` instead of `contains-task-list`
// to align with GitLab class usage
fn render_list(
    context: &mut Context<RenderUserData>,
    node: Node<'_>,
    entering: bool,
    nl: &NodeList,
) -> Result<ChildRendering, fmt::Error> {
    if !entering || !context.options.render.tasklist_classes {
        return html::format_node_default(context, node, entering);
    }

    context.cr()?;
    match nl.list_type {
        ListType::Bullet => {
            context.write_str("<ul")?;
            if nl.is_task_list {
                context.write_str(" class=\"task-list\"")?;
            }
            html::render_sourcepos(context, node)?;
            context.write_str(">\n")?;
        }
        ListType::Ordered => {
            context.write_str("<ol")?;
            if nl.is_task_list {
                context.write_str(" class=\"task-list\"")?;
            }
            html::render_sourcepos(context, node)?;
            if nl.start == 1 {
                context.write_str(">\n")?;
            } else {
                writeln!(context, " start=\"{}\">", nl.start)?;
            }
        }
    }

    Ok(ChildRendering::HTML)
}

// Overridden to:
// 1. Detect inapplicable task list items; and,
// 2. Output checkbox sourcepos.
fn render_task_item(
    context: &mut Context<RenderUserData>,
    node: Node<'_>,
    entering: bool,
    nti: &NodeTaskItem,
) -> Result<ChildRendering, fmt::Error> {
    let write_li = node
        .parent()
        .map(|p| node_matches!(p, NodeValue::List(_)))
        .unwrap_or_default();

    if !entering {
        if write_li {
            context.write_str("</li>\n")?;
        }
        return Ok(ChildRendering::HTML);
    }

    match nti.symbol {
        Some('~') => {
            render_task_item_with(
                context,
                node,
                nti,
                if context.user.inapplicable_tasks {
                    TaskItemState::Inapplicable
                } else {
                    TaskItemState::Checked
                },
                write_li,
            )?;
        }
        None => {
            render_task_item_with(context, node, nti, TaskItemState::Unchecked, write_li)?;
        }
        Some(ws) if ws.is_whitespace() => {
            render_task_item_with(context, node, nti, TaskItemState::Unchecked, write_li)?;
        }
        Some('x' | 'X') => {
            render_task_item_with(context, node, nti, TaskItemState::Checked, write_li)?;
        }
        Some(symbol) => {
            context.cr()?;
            if write_li {
                context.write_str("<li")?;
                if context.options.render.tasklist_classes {
                    context.write_str(" class=\"task-list-item\"")?;
                }
                html::render_sourcepos(context, node)?;
                context.write_str(">")?;
            }
            context.write_str("[")?;
            context.escape(&symbol.to_string())?;
            context.write_str("] ")?;
        }
    }

    Ok(ChildRendering::HTML)
}

#[derive(PartialEq)]
enum TaskItemState {
    Checked,
    Unchecked,
    Inapplicable,
}

fn render_task_item_with(
    context: &mut Context<RenderUserData>,
    node: Node<'_>,
    nti: &NodeTaskItem,
    state: TaskItemState,
    write_li: bool,
) -> fmt::Result {
    context.cr()?;
    if write_li {
        context.write_str("<li")?;
        if context.options.render.tasklist_classes {
            context.write_str(" class=\"")?;
            if state == TaskItemState::Inapplicable {
                context.write_str("inapplicable ")?;
            }
            context.write_str("task-list-item\"")?;
        }
        render_sourcepos(context, node)?;
        context.write_str(">")?;
    }

    context.write_str("<input type=\"checkbox\"")?;
    if !write_li {
        html::render_sourcepos(context, node)?;
    }
    if context.options.render.sourcepos {
        write!(
            context,
            " data-checkbox-sourcepos=\"{:?}\"",
            nti.symbol_sourcepos
        )?;
    }

    if context.options.render.tasklist_classes {
        context.write_str(" class=\"task-list-item-checkbox\"")?;
    }

    match state {
        TaskItemState::Checked => {
            context.write_str(" checked=\"\"")?;
        }
        TaskItemState::Unchecked => {}
        TaskItemState::Inapplicable => {
            context.write_str(" data-inapplicable")?;
        }
    }
    context.write_str(" disabled=\"\" /> ")?;

    Ok(())
}

fn render_table_cell<T>(
    context: &mut Context<T>,
    node: Node<'_>,
    entering: bool,
) -> Result<ChildRendering, fmt::Error> {
    let Some(row_node) = node.parent() else {
        panic!("rendered a table cell without a containing table row");
    };
    let row = &row_node.data().value;
    let in_header = match *row {
        NodeValue::TableRow(header) => header,
        _ => panic!("rendered a table cell contained by something other than a table row"),
    };

    let Some(table_node) = row_node.parent() else {
        panic!("rendered a table cell without a containing table");
    };
    let table = &table_node.data().value;
    let alignments = match table {
        NodeValue::Table(nt) => &nt.alignments,
        _ => {
            panic!("rendered a table cell in a table row contained by something other than a table")
        }
    };

    if entering {
        context.cr()?;
        if in_header {
            context.write_str("<th")?;
            render_sourcepos(context, node)?;
        } else {
            context.write_str("<td")?;
            render_sourcepos(context, node)?;
        }

        let mut start = row_node.first_child().unwrap(); // guaranteed to exist because `node' itself does!
        let mut i = 0;
        while !start.same_node(node) {
            i += 1;
            start = start.next_sibling().unwrap();
        }

        match alignments[i] {
            TableAlignment::Left => {
                context.write_str(" align=\"left\"")?;
            }
            TableAlignment::Right => {
                context.write_str(" align=\"right\"")?;
            }
            TableAlignment::Center => {
                context.write_str(" align=\"center\"")?;
            }
            TableAlignment::None => (),
        }

        if context.options.parse.tasklist_in_table
            && context.options.render.tasklist_classes
            && contains_only_task_item(node)
        {
            context.write_str(" class=\"task-table-item\"")?;
        }

        context.write_str(">")?;
    } else if in_header {
        context.write_str("</th>")?;
    } else {
        context.write_str("</td>")?;
    }

    Ok(ChildRendering::HTML)
}

fn render_escaped(
    context: &mut Context<RenderUserData>,
    node: Node<'_>,
    entering: bool,
) -> Result<ChildRendering, fmt::Error> {
    if !context.options.render.escaped_char_spans {
        return Ok(ChildRendering::HTML);
    }

    if context
        .user
        .only_escape_chars
        .as_ref()
        .is_none_or(|only_escape_chars| {
            with_node_text_content(node, false, |content| {
                content.chars().count() == 1
                    && only_escape_chars.contains(&content.chars().next().unwrap())
            })
        })
    {
        if entering {
            context.write_str("<span data-escaped-char")?;
            render_sourcepos(context, node)?;
            context.write_str(">")?;
        } else {
            context.write_str("</span>")?;
        }
    }

    Ok(ChildRendering::HTML)
}

fn render_heading(
    context: &mut Context<RenderUserData>,
    node: Node<'_>,
    entering: bool,
    nh: &NodeHeading,
) -> Result<ChildRendering, fmt::Error> {
    if !context.user.header_accessibility || context.plugins.render.heading_adapter.is_some() {
        return format_node_default(context, node, entering);
    }

    if entering {
        context.cr()?;
        write!(context, "<h{}", nh.level)?;

        if let Some(ref prefix) = context.options.extension.header_id_prefix {
            let text_content = node.collect_text();
            let id = context.anchorizer.anchorize(&text_content);
            let id = if let Some(ref slug_prefix) = context.user.header_slug_prefix {
                format!("{slug_prefix}{id}")
            } else {
                id
            };
            write!(context, r##" id="{prefix}{id}""##)?;
            context.user.last_heading = Some(id);
        }

        render_sourcepos(context, node)?;
        context.write_str(">")?;
    } else {
        if let Some(ref prefix) = context.options.extension.header_id_prefix {
            let href_prefix = if context.options.extension.header_id_prefix_in_href {
                prefix.clone()
            } else {
                String::new()
            };
            let id = context.user.last_heading.take().unwrap();
            let text_content = node.collect_text();
            write!(
                context,
                r##"<a href="#{href_prefix}{id}" aria-label="Link to heading '"##
            )?;
            context.escape(&text_content)?;
            context.write_str(r#"'" data-heading-content=""#)?;
            context.escape(&text_content)?;
            context.write_str(r#"" class="anchor"></a>"#)?;
        }
        writeln!(context, "</h{}>", nh.level)?;
    }
    Ok(ChildRendering::HTML)
}

/// If the given node has a single text child, apply a function to the text content
/// of that node. Otherwise, return the given default value.
fn with_node_text_content<U, F>(node: Node<'_>, default: U, f: F) -> U
where
    F: FnOnce(&str) -> U,
{
    let Some(child) = node.first_child() else {
        return default;
    };
    let Some(last_child) = node.last_child() else {
        return default;
    };
    if !child.same_node(last_child) {
        return default;
    }
    let NodeValue::Text(ref text) = child.data.borrow().value else {
        return default;
    };
    f(text)
}

fn contains_only_task_item(node: Node<'_>) -> bool {
    let Some(first) = node.first_child() else {
        return false;
    };
    if !matches!(first.data().value, NodeValue::TaskItem(..)) {
        return false;
    }
    if first.next_sibling().is_some() {
        return false;
    }
    true
}
