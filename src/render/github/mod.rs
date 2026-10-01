//! GitHub flavor offline renderer (owner: render-github). Contract: docs/DESIGN.md §2 D2, §5;
//! `.dev/research/github.md` §9 (`readme` = GH §9.A, `comment` = GH §9.B).
//!
//! Pipeline (CQ §3.1):
//! 1. normalize line endings; README context: split off YAML front matter (GH §8.12),
//! 2. comrak 0.55 parse with GitHub's GFM options (GH §10) and AST passes (`ast`),
//! 3. neutral formatter output with sanitizer markers (`formatter`),
//! 4. `dom_query` DOM, sanitized with GitHub's raw-HTML allowlist (`sanitize`),
//! 5. DOM filters in CQ §3.1 order: alerts, dollar math, task lists, headings, raw-text autolinks, raw `<pre>`, emoji,
//!    tables, images, links, details, comment-mode code classes, `dir="auto"`,
//! 6. top-level `data-sourcepos` bookkeeping and serialization.

mod alerts;
mod ast;
mod autolink;
mod code;
mod dom;
mod emoji;
mod filters;
mod formatter;
mod frontmatter;
mod headings;
mod math;
mod octicons;
mod sanitize;
mod slug;
mod tasks;
mod util;

use std::borrow::Cow;

use dom_query::{Document, NodeData};

use super::{Features, RenderOptions, Rendered};
use crate::flavor::Mode;

/// Attribute marking formatter-generated elements for the sanitizer (removed by it).
const MARK_ATTR: &str = "data-mdr-g";
/// Prefix of the comment that carries a raw HTML block's sourcepos to the DOM stage.
const SOURCEPOS_COMMENT: &str = "mdr-sp:";

/// Per-render settings shared by the passes.
#[derive(Debug, Clone, Default)]
struct Ctx {
    /// GH §9.B comment context.
    comment: bool,
    /// Emit `data-sourcepos` on top-level blocks.
    sourcepos: bool,
    /// Footnote/math run id (GH §8.4), 32 hex, deterministic.
    run_id: String,
    /// Per-render sanitizer token (derived from the input).
    token: String,
    /// ` data-mdr-g="TOKEN"`: trusted element.
    mark: String,
    /// ` data-mdr-g="TOKEN-t"`: trusted element and subtree.
    mark_tree: String,
    /// Bundle URL prefix (custom emoji images).
    bundle_url: String,
}

impl Ctx {
    fn new(src: &str, opts: &RenderOptions) -> Ctx {
        // Run ids only need to differ between documents shown on one page and must stay
        // stable across live reloads, so they derive from the file path when there is one
        // (GH §8.4 "emulate with a deterministic value"); stdin input uses its content.
        let run_id = match &opts.source_path {
            Some(p) => {
                util::hex(&util::digest(&["md-render/github/run-id", &p.to_string_lossy()])[..16])
            }
            None => util::hex(&util::digest(&["md-render/github/run-id", src])[..16]),
        };
        let token = util::hex(&util::digest(&["md-render/github/marker", src])[..8]);
        Ctx {
            comment: opts.mode == Mode::Comment,
            sourcepos: opts.sourcepos,
            run_id,
            mark: format!(" {MARK_ATTR}=\"{token}\""),
            mark_tree: format!(" {MARK_ATTR}=\"{token}-t\""),
            token,
            bundle_url: opts.bundle_url.trim_end_matches('/').to_owned(),
        }
    }

    /// The same settings for markup inserted after sanitizing (no markers).
    fn without_marks(&self) -> Ctx {
        Ctx {
            mark: String::new(),
            mark_tree: String::new(),
            ..self.clone()
        }
    }

    #[cfg(test)]
    fn for_tests(comment: bool) -> Ctx {
        let token = "0123456789abcdef".to_owned();
        Ctx {
            comment,
            sourcepos: false,
            run_id: "00112233445566778899aabbccddeeff".to_owned(),
            mark: format!(" {MARK_ATTR}=\"{token}\""),
            mark_tree: format!(" {MARK_ATTR}=\"{token}-t\""),
            token,
            bundle_url: "/b".to_owned(),
        }
    }
}

/// `\r\n` and lone `\r` become `\n`; a leading BOM is dropped.
fn normalize(markdown: &str) -> Cow<'_, str> {
    let s = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    if s.contains('\r') {
        Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(s)
    }
}

/// Render GitHub-flavored output.
pub fn render(markdown: &str, opts: &RenderOptions) -> Rendered {
    let src = normalize(markdown);
    let ctx = Ctx::new(&src, opts);
    let mut warnings = Vec::new();

    // 1. Front matter (README context only, GH §8.12). Its lines are blanked rather than cut
    //    so that comrak's line numbers stay those of the file.
    let mut front: Option<frontmatter::FrontMatter> = None;
    let mut body: Cow<'_, str> = Cow::Borrowed(&src);
    if !ctx.comment {
        match frontmatter::extract(&src) {
            frontmatter::Extracted::None => {}
            frontmatter::Extracted::Invalid(msg) => warnings.push(msg),
            frontmatter::Extracted::Table(fm) => {
                let mut b = String::with_capacity(src.len());
                let mut rest: &str = &src;
                for _ in 0..fm.lines {
                    let cut = rest.find('\n').map_or(rest.len(), |i| i + 1);
                    rest = &rest[cut..];
                    b.push('\n');
                }
                b.push_str(rest);
                body = Cow::Owned(b);
                front = Some(fm);
            }
        }
    }

    // 2.–3. Parse, AST passes, format.
    let options = ast::options();
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, &body, &options);
    ast::transform(root);
    let state = formatter::FmtState::new(&ctx);
    let (html, state) = formatter::format(root, &options, state);
    let mut features = Features {
        math: state.math,
        mermaid: state.mermaid,
    };

    // 4.–5. DOM stage.
    let doc = Document::fragment(html);
    let raw = sanitize::run(&doc, &ctx);
    alerts::run(&doc);
    features.math |= math::run(&doc, &ctx);
    tasks::run(&doc);
    headings::run(&doc, &ctx);
    if !ctx.comment {
        autolink::run(&doc, &raw);
    }
    let mut enrichments = state.enrichments;
    let mut feats = (false, false);
    code::raw_pre(&doc, &ctx, &mut enrichments, &mut feats);
    features.math |= feats.0;
    features.mermaid |= feats.1;
    filters::emoji(&doc, &ctx);
    filters::tables(&doc, &ctx);
    filters::images(&doc, &ctx);
    filters::links(&doc);
    filters::details(&doc);
    if ctx.comment {
        filters::comment_code(&doc);
    }
    filters::dir_auto(&doc);
    finish_sourcepos(&doc, &ctx);
    let title = first_h1(&doc);

    // 6. Serialize.
    let mut out = String::new();
    if let Some(fm) = &front {
        if let Some(table) = &fm.table {
            out.push_str("<markdown-accessiblity-table");
            if ctx.sourcepos {
                out.push_str(&format!(
                    " data-sourcepos=\"1:1-{}:{}\"",
                    fm.lines,
                    fm.last_line_len.max(1)
                ));
            }
            out.push('>');
            out.push_str(table);
            out.push_str("</markdown-accessiblity-table>\n\n");
        }
    }
    let inner = dom::root(&doc).inner_html();
    out.push_str(&octicons::expand(&inner));

    Rendered {
        html: out,
        title,
        features,
        warnings,
    }
}

/// Top-level blocks keep (or, for raw HTML blocks, receive) `data-sourcepos`; nested elements
/// lose it (GitHub emits none; DESIGN §5).
fn finish_sourcepos(doc: &Document, ctx: &Ctx) {
    if !ctx.sourcepos {
        return;
    }
    let prefix = format!("{SOURCEPOS_COMMENT}{}:", ctx.token);
    let root = dom::root(doc);
    let mut current: Option<String> = None;
    for child in root.children() {
        let comment = child.query(|t| match &t.data {
            NodeData::Comment { contents } => Some(contents.to_string()),
            _ => None,
        });
        if let Some(c) = comment.flatten() {
            current = c.strip_prefix(prefix.as_str()).map(str::to_owned);
            child.remove_from_parent();
            continue;
        }
        if !child.is_element() {
            continue;
        }
        if child.has_attr("data-sourcepos") {
            current = None;
        } else if let Some(sp) = &current {
            child.set_attr("data-sourcepos", sp);
        }
        for d in child.descendants() {
            if d.is_element() && d.has_attr("data-sourcepos") {
                d.remove_attr("data-sourcepos");
            } else if d.is_comment() {
                d.remove_from_parent();
            }
        }
    }
}

/// Plain text of the first `h1` (for `<title>`).
fn first_h1(doc: &Document) -> Option<String> {
    let h1 = dom::elements_named(doc, "h1").into_iter().next()?;
    let t = h1.text().trim().to_owned();
    (!t.is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flavor::Flavor;

    fn opts(mode: Mode) -> RenderOptions {
        let mut o = RenderOptions::new(Flavor::Github);
        o.mode = mode;
        o.sourcepos = false;
        o.bundle_url = "/b".into();
        o
    }

    #[test]
    fn normalizes_line_endings() {
        assert_eq!(normalize("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(normalize("\u{feff}x"), "x");
    }

    #[test]
    fn smoke() {
        let r = render("# Title :smile:\n\nHello *world*\n", &opts(Mode::Readme));
        assert_eq!(r.title.as_deref(), Some("Title 😄"));
        assert!(r.html.contains(r##"<a id="user-content-title-smile" class="anchor" aria-label="Permalink: Title :smile:" href="#title-smile"><svg"##), "{}", r.html);
        assert!(r.html.contains(r#"<p dir="auto">Hello <em>world</em></p>"#));
        assert!(!r.html.contains(MARK_ATTR));
    }
}
