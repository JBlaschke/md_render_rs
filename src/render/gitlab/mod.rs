//! GitLab flavor offline renderer (owner: render-gitlab). Contract: docs/DESIGN.md §2 D3, §5;
//! .dev/research/gitlab.md §2-§6.

mod formatter;
mod glfm;

use super::{RenderOptions, Rendered};

/// GitLab's exact comrak option set (GL §4.1).
pub(crate) fn gitlab_options() -> glfm::RenderOptions {
    glfm::RenderOptions {
        alerts: true,
        autolink: true,
        cjk_friendly_emphasis: true,
        description_lists: true,
        escaped_char_spans: true,
        footnotes: true,
        full_info_string: true,
        hardbreaks: false,
        header_accessibility: true,
        header_id_prefix: Some("user-content-".to_string()),
        inapplicable_tasks: true,
        math_code: true,
        math_dollars: true,
        multiline_block_quotes: true,
        only_escape_chars: Some(vec!['$', '%', '#', '&', '@', '!', '~', '^', ':']),
        placeholder_detection: false,
        relaxed_autolinks: true,
        relaxed_tasklist_character: true,
        smart: false,
        sourcepos: true,
        strikethrough: true,
        table: true,
        tasklist: true,
        tasklist_classes: true,
        tasklist_in_table: true,
        wikilinks_title_before_pipe: true,
        r#unsafe: true,
        github_pre_lang: true,
        ..Default::default()
    }
}

/// Raw comrak + GitLab formatter output (debug helper).
#[doc(hidden)]
pub fn comrak_html(markdown: &str) -> String {
    glfm::render(markdown, &gitlab_options()).unwrap_or_default()
}

/// Render GitLab-flavored output.
pub fn render(markdown: &str, opts: &RenderOptions) -> Rendered {
    super::placeholder_render(markdown, opts)
}
