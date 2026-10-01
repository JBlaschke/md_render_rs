// Ported from the gitlab-glfm-markdown gem v0.0.43 (`src/glfm.rs`),
// https://gitlab.com/gitlab-org/ruby/gems/gitlab-glfm-markdown
//
// Copyright (c) 2011-present GitLab B.V.
// Licensed under the MIT Expat license; see THIRD_PARTY_NOTICES.md for the full text.
//
// Kept as close to verbatim as possible (GL §4.1, DESIGN D3). Local changes: the module path of
// the formatter (`super::formatter`), this header, the `#![allow]` below (so the upstream style
// stays untouched), and `render` returning the `fmt::Error` instead of unwrapping it.
#![allow(clippy::all, dead_code)]

use comrak::options::Plugins;
use comrak::{parse_document, Arena};
use serde::Deserialize;

use super::formatter::{CustomFormatter, RenderUserData};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RenderOptions {
    pub alerts: bool,
    pub autolink: bool,
    pub cjk_friendly_emphasis: bool,
    /// Only use default comrak HTML formatting
    pub default_html: bool,
    // pub default_info_string: String,
    pub description_lists: bool,
    pub escape: bool,
    pub escaped_char_spans: bool,
    pub figure_with_caption: bool,
    pub footnotes: bool,
    // pub front_matter_delimiter: String,
    pub full_info_string: bool,
    pub gemojis: bool,
    pub gfm_quirks: bool,
    pub github_pre_lang: bool,
    pub greentext: bool,
    pub hardbreaks: bool,
    /// Use new header/anchor combination in HTML output, per
    /// https://gitlab.com/gitlab-org/ruby/gems/gitlab-glfm-markdown/-/merge_requests/112.
    /// Requires `header_id_prefix` to generate heading anchors.
    pub header_accessibility: bool,
    pub header_id_prefix: Option<String>,
    /// Include `header_id_prefix` in the heading anchor's `href`.
    pub header_id_prefix_in_href: bool,
    /// Prefix to prepend to heading anchor slugs.
    /// When set, slugs become `{header_slug_prefix}{anchorized_text}`.
    /// Requires `header_accessibility` to be set.
    pub header_slug_prefix: Option<String>,
    pub ignore_empty_links: bool,
    pub ignore_setext: bool,
    /// Detect inapplicable tasks (`- [~]`)
    pub inapplicable_tasks: bool,
    pub math_code: bool,
    pub math_dollars: bool,
    pub multiline_block_quotes: bool,
    /// When the 'escaped_char_spans' render option is enabled, only emit `<span
    /// data-escaped-char>` around the given characters.
    pub only_escape_chars: Option<Vec<char>>,
    /// Detect and mark potential placeholder variables, which
    /// have the format `%{PLACEHOLDER}`
    pub placeholder_detection: bool,
    pub relaxed_autolinks: bool,
    pub relaxed_tasklist_character: bool,
    pub smart: bool,
    pub sourcepos: bool,
    pub spoiler: bool,
    pub strikethrough: bool,
    pub subscript: bool,
    pub superscript: bool,
    // pub syntax_highlighting: String,
    pub table: bool,
    pub tasklist: bool,
    pub tasklist_classes: bool,
    pub tasklist_in_table: bool,
    pub underline: bool,
    pub r#unsafe: bool,
    pub wikilinks_title_after_pipe: bool,
    pub wikilinks_title_before_pipe: bool,
}

impl From<&RenderOptions> for comrak::Options<'_> {
    fn from(options: &RenderOptions) -> Self {
        let mut comrak_options = comrak::Options::default();

        comrak_options.extension.alerts = options.alerts;
        comrak_options.extension.autolink = options.autolink;
        comrak_options.extension.cjk_friendly_emphasis = options.cjk_friendly_emphasis;
        comrak_options.extension.description_lists = options.description_lists;
        comrak_options.extension.footnotes = options.footnotes;
        // comrak_options.extension.front_matter_delimiter = options.front_matter_delimiter;
        comrak_options.extension.greentext = options.greentext;
        comrak_options.extension.header_id_prefix = options.header_id_prefix.clone();
        comrak_options.extension.header_id_prefix_in_href = options.header_id_prefix_in_href;
        comrak_options.extension.math_code = options.math_code;
        comrak_options.extension.math_dollars = options.math_dollars;
        comrak_options.extension.multiline_block_quotes = options.multiline_block_quotes;
        comrak_options.extension.shortcodes = options.gemojis;
        comrak_options.extension.spoiler = options.spoiler;
        comrak_options.extension.strikethrough = options.strikethrough;
        comrak_options.extension.subscript = options.subscript;
        comrak_options.extension.superscript = options.superscript;
        comrak_options.extension.table = options.table;
        comrak_options.extension.tasklist = options.tasklist;
        comrak_options.extension.underline = options.underline;
        comrak_options.extension.wikilinks_title_after_pipe = options.wikilinks_title_after_pipe;
        comrak_options.extension.wikilinks_title_before_pipe = options.wikilinks_title_before_pipe;

        comrak_options.render.escape = options.escape;
        comrak_options.render.escaped_char_spans = options.escaped_char_spans;
        comrak_options.render.figure_with_caption = options.figure_with_caption;
        comrak_options.render.full_info_string = options.full_info_string;
        comrak_options.render.gfm_quirks = options.gfm_quirks;
        comrak_options.render.github_pre_lang = options.github_pre_lang;
        comrak_options.render.hardbreaks = options.hardbreaks;
        comrak_options.render.ignore_empty_links = options.ignore_empty_links;
        comrak_options.render.sourcepos = options.sourcepos;
        comrak_options.render.tasklist_classes = options.tasklist_classes;
        // comrak_options.render.syntax_highlighting = options.syntax_highlighting;

        comrak_options.render.r#unsafe = options.r#unsafe;

        // comrak_options.parse.default_info_string = options.default_info_string;
        comrak_options.parse.ignore_setext = options.ignore_setext;
        comrak_options.parse.relaxed_autolinks = options.relaxed_autolinks;
        comrak_options.parse.relaxed_tasklist_matching = options.relaxed_tasklist_character;
        comrak_options.parse.smart = options.smart;
        comrak_options.parse.tasklist_in_table = options.tasklist_in_table;

        comrak_options
    }
}

pub fn render(text: &str, options: &RenderOptions) -> Result<String, std::fmt::Error> {
    render_with_plugins(text, options, &Plugins::default())
}

fn render_with_plugins(
    text: &str,
    render_options: &RenderOptions,
    plugins: &Plugins,
) -> Result<String, std::fmt::Error> {
    let user_data = RenderUserData::from(render_options);
    let options = comrak::Options::from(render_options);

    if user_data.default_html {
        return Ok(comrak::markdown_to_html_with_plugins(text, &options, plugins));
    }

    let arena = Arena::new();
    let root = parse_document(&arena, text, &options);

    let mut out = String::new();
    CustomFormatter::format_document_with_plugins(root, &options, &mut out, plugins, user_data)?;
    Ok(out)
}
