//! GitLab flavor offline renderer (owner: render-gitlab). Contract: docs/DESIGN.md §2 D3, §5;
//! .dev/research/gitlab.md (GL) §2-§6.
//!
//! Pipeline (GL §2), all pure and deterministic:
//! 1. Stage A, text (GL §3, [`preprocess`]): strip the BOM, rewrite front matter into a
//!    ```` ```lang:frontmatter ```` fence, expand `::include{file=...}` (readme mode only, confined
//!    to the served root), delete `\r`.
//! 2. Stage B (GL §4): comrak 0.55.0 with GitLab's exact option set and the ported
//!    gitlab-glfm-markdown formatter ([`glfm`], [`formatter`]); output `rstrip`ped.
//! 3. Stage C (GL §5, [`filters`]): the Banzai filters in GitLab's order on a `dom_query`
//!    fragment, sanitizer included ([`sanitize`]), syntax highlighting last ([`highlight`]).
//! 4. The post-JavaScript DOM (GL §0.3, [`postjs`]): no lazy-image placeholders,
//!    `code-syntax-highlight-theme`, GitLab's table wrapper; plus `data-mdr-key` and optional
//!    removal of `data-sourcepos`.
//!
//! `Mode::Comment` (DESIGN D16): GitLab has no separate Markdown dialect for notes; issue and
//! comment bodies go through the same `FullPipeline`, minus `IncludeFilter` (it only runs for
//! `text_source: :blob`). That is exactly what this mode does; the page chrome differs.

mod dom;
mod emoji;
mod filters;
// Ported from the gitlab-glfm-markdown gem; kept verbatim, so not reformatted.
#[rustfmt::skip]
mod formatter;
#[rustfmt::skip]
mod glfm;
mod highlight;
mod postjs;
mod preprocess;
mod sanitize;
mod spaced_link;

use std::borrow::Cow;
use std::path::PathBuf;

use dom_query::Document;

use super::{Features, RenderOptions, Rendered};
use crate::flavor::Mode;

/// GitLab's exact comrak option set (`Banzai::Filter::MarkdownEngines::GlfmMarkdown::OPTIONS`,
/// GL §4.1). `placeholder_detection` is off on gitlab.com (feature flag) and `github_pre_lang`
/// is on (`use_css_language_classes` disabled).
fn gitlab_options() -> glfm::RenderOptions {
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

/// Stage A + B: the Markdown text GitLab hands to comrak, and comrak's (rstripped) HTML.
fn markdown_to_html(
    markdown: &str,
    opts: &RenderOptions,
    included: &mut preprocess::Included,
) -> String {
    let mut text = preprocess::front_matter(preprocess::normalize_source(markdown));
    if opts.mode == Mode::Readme {
        let ctx =
            preprocess::IncludeContext::new(opts.source_path.as_deref(), opts.root.as_deref());
        if let Cow::Owned(expanded) = preprocess::includes(&text, &ctx, included) {
            text = Cow::Owned(expanded);
        }
    }
    render_snippet(&text)
}

/// `MarkdownFilter#call`: `@text.delete("\r")`, comrak with GitLab's options, `rstrip`; empty
/// input renders "". Also used by SpacedLinkFilter for its snippets.
fn render_snippet(text: &str) -> String {
    let text = text.replace('\r', "");
    if text.is_empty() {
        return String::new();
    }
    let html = glfm::render(&text, &gitlab_options()).unwrap_or_default();
    html.trim_end_matches([' ', '\t', '\n', '\x0B', '\x0C', '\r', '\0'])
        .to_owned()
}

/// Raw comrak + GitLab formatter output after Stage A (debugging aid and test oracle).
#[doc(hidden)]
pub fn comrak_html(markdown: &str, opts: &RenderOptions) -> String {
    markdown_to_html(markdown, opts, &mut preprocess::Included::default())
}

/// Files that `::include{file=...}` directives in `markdown` read (absolute paths inside the
/// served root), so a live server can watch them too. Empty in comment mode.
pub fn included_files(markdown: &str, opts: &RenderOptions) -> Vec<PathBuf> {
    if opts.mode != Mode::Readme {
        return Vec::new();
    }
    let mut included = preprocess::Included::default();
    let text = preprocess::front_matter(preprocess::normalize_source(markdown)).into_owned();
    let ctx = preprocess::IncludeContext::new(opts.source_path.as_deref(), opts.root.as_deref());
    let _ = preprocess::includes(&text, &ctx, &mut included);
    included.files
}

/// Render GitLab-flavored output.
pub fn render(markdown: &str, opts: &RenderOptions) -> Rendered {
    let mut included = preprocess::Included::default();
    let html = markdown_to_html(markdown, opts, &mut included);
    if html.is_empty() {
        return Rendered {
            warnings: included.warnings,
            ..Default::default()
        };
    }

    let doc = Document::fragment(html);
    let root = doc.html_root();
    let mut warnings = included.warnings;
    warnings.extend(filters::run(&root, &filters::FilterContext::default()));
    postjs::apply(&root);
    postjs::add_keys(&root);
    if !opts.sourcepos {
        postjs::strip_sourcepos(&root);
    }

    let title = dom::elements_named(&root, &["h1"])
        .into_iter()
        .next()
        .map(|id| {
            let h1 = root.tree.get_unchecked(&id);
            dom::ruby_strip(&dom::text(&h1)).to_owned()
        });
    let features = features(&root);
    Rendered {
        html: root.inner_html().to_string(),
        title: title.filter(|t| !t.is_empty()),
        features,
        warnings,
    }
}

fn features(root: &dom_query::NodeRef<'_>) -> Features {
    let mut f = Features::default();
    for id in dom::descendants(root) {
        let n = root.tree.get_unchecked(&id);
        if !n.is_element() {
            continue;
        }
        let classes = dom::classes(&n);
        if classes.iter().any(|c| c == "js-render-math") {
            f.math = true;
        }
        if classes.iter().any(|c| c == "js-render-mermaid") {
            f.mermaid = true;
        }
        if f.math && f.mermaid {
            break;
        }
    }
    f
}
