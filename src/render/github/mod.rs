//! GitHub flavor offline renderer (owner: render-github). Contract: docs/DESIGN.md §2 D2, §5;
//! `.dev/research/github.md` §9 (`readme` = GH §9.A, `comment` = GH §9.B).
//!
//! Pipeline (CQ §3.1):
//! 1. normalize line endings; README context: split off YAML front matter (GH §8.12),
//! 2. comrak 0.55 parse with GitHub's GFM options (GH §10) and the tagfilter pass (`ast`),
//! 3. neutral formatter output with sanitizer markers (`formatter`): GitHub's shapes for
//!    footnotes, code blocks, ```` ```math ```` and enrichment sections,
//! 4. `dom_query` DOM, sanitized with GitHub's raw-HTML allowlist (`sanitize`),
//! 5. DOM filters: alerts, dollar math, task lists (github.com runs these on the HTML tree,
//!    so raw HTML participates), headings, raw-text autolinks, raw `<pre>`, emoji, tables,
//!    images, links, details, comment-mode code classes, `dir="auto"`,
//! 6. top-level `data-sourcepos` bookkeeping, octicon expansion and serialization.

mod alerts;
mod ast;
mod autolink;
mod budget;
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
    ast::transform(&arena, root, &body);
    let state = formatter::FmtState::new(&ctx);
    let (html, state) = formatter::format(root, &options, state);
    let mut features = Features {
        math: state.math,
        mermaid: state.mermaid,
    };

    // Runaway nesting would make the DOM stage quadratic (GH-PERF-1).
    if budget::nesting_cost(&html) > budget::MAX_NESTING_COST {
        return too_deep(&src, warnings);
    }

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
    code::raw_pre(&doc, &ctx, &raw, &mut enrichments, &mut feats);
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

/// The output for a document whose nesting is over the tree-building budget
/// ([`budget::MAX_NESTING_COST`]): its source as plain text, with a warning.
fn too_deep(src: &str, mut warnings: Vec<String>) -> Rendered {
    let mut html = String::from(r#"<pre class="notranslate"><code class="notranslate">"#);
    util::push_text(&mut html, src);
    html.push_str("</code></pre>");
    warnings.push(
        "The document nests HTML elements or block quotes too deeply to render in reasonable \
         time; showing its source as plain text."
            .to_owned(),
    );
    Rendered {
        html,
        title: None,
        features: Features::default(),
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

    /// GH-FN-1: duplicate footnote definitions collapse to the first, like cmark-gfm.
    #[test]
    fn duplicate_footnote_definitions_collapse() {
        // Same label: the first definition wins, the duplicate is dropped and the body text
        // between them stays in the body, before the footnotes section.
        let r = render(
            "A[^a].\n\n[^a]: one\n\n[^a]: dup\n\nAfter.\n",
            &opts(Mode::Comment),
        );
        assert_eq!(
            r.html.matches(r#"<li id="user-content-fn-a-"#).count(),
            1,
            "{}",
            r.html
        );
        assert!(r.html.contains(">one "), "{}", r.html);
        assert!(!r.html.contains("dup"), "{}", r.html);
        let after = r.html.find("After.").expect("body text kept");
        let section = r.html.find("<section").expect("footnotes section");
        assert!(after < section, "body before footnotes: {}", r.html);
        assert!(
            r.html.contains(r#"aria-label="Back to reference 1""#),
            "{}",
            r.html
        );

        // Case variants fold together (cmark-gfm case-folds labels): the first spelling wins
        // the id, its body is kept, and `[^b]` is still reference 2.
        let r = render(
            "X[^a] Y[^A] Z[^b].\n\n[^a]: one\n\n[^A]: two\n\n[^b]: bee\n",
            &opts(Mode::Comment),
        );
        assert_eq!(
            r.html.matches(r#"<li id="user-content-fn-a-"#).count(),
            1,
            "{}",
            r.html
        );
        assert!(
            !r.html.contains("user-content-fn-A-") && !r.html.contains("two"),
            "{}",
            r.html
        );
        assert!(
            r.html.contains(r#"<li id="user-content-fn-b-"#),
            "{}",
            r.html
        );
        assert!(
            r.html.contains(r#"aria-label="Back to reference 2""#),
            "{}",
            r.html
        );

        // A footnote that is never referenced produces no section at all.
        let r = render("[^a]: one\n\n[^a]: dup\n\nAfter.\n", &opts(Mode::Comment));
        assert!(!r.html.contains("<section"), "{}", r.html);
        assert!(r.html.contains("After."), "{}", r.html);

        // Readme mode behaves the same (the pass runs on the AST).
        let r = render(
            "A[^a].\n\n[^a]: one\n\n[^a]: dup\n\nAfter.\n",
            &opts(Mode::Readme),
        );
        assert_eq!(
            r.html.matches(r#"<li id="user-content-fn-a-"#).count(),
            1,
            "{}",
            r.html
        );
        assert!(!r.html.contains("dup"), "{}", r.html);
    }

    /// Asserts that the DOM of `html` carries nothing dangerous that only raw HTML could have
    /// put there.
    fn assert_clean(input: &str, html: &str) {
        let doc = Document::fragment(html);
        for el in dom::elements(&doc) {
            let name = dom::name(&el).map(|n| n.to_string()).unwrap_or_default();
            assert!(
                !matches!(&*name, "base" | "form" | "link" | "script" | "style"),
                "<{name}> in {input:?} -> {html}"
            );
            if name == "input" {
                assert!(
                    el.has_class("task-list-item-checkbox"),
                    "{input:?} -> {html}"
                );
            }
            let attrs = el
                .element_ref()
                .map(|e| e.attrs.clone())
                .unwrap_or_default();
            for a in attrs {
                let (n, v) = (a.name.local.to_string(), a.value.to_ascii_lowercase());
                assert!(
                    !n.starts_with("on") && !n.contains('<') && n != MARK_ATTR,
                    "{n} in {input:?} -> {html}"
                );
                assert!(
                    !(n == "style" && v == "s"),
                    "raw style in {input:?} -> {html}"
                );
                assert!(!v.contains("javascript:"), "{n}={v} in {input:?} -> {html}");
            }
        }
    }

    /// GH-SEC-1: an unterminated raw block tag followed by a blank line used to absorb the
    /// next generated start tag together with its sanitizer marker.
    #[test]
    fn unterminated_raw_tags_stay_raw() {
        let tags = [
            r#"details open ontoggle="x""#,
            r#"div onclick="x" style="s""#,
            r#"base href="https://e.com/""#,
            r#"form action="https://e.com/""#,
            r#"link rel="stylesheet" href="https://e.com/s.css""#,
            r#"section onclick="x""#,
            r#"table onclick="x""#,
            r#"p onclick="x""#,
            r#"input type="text" onfocus="x""#,
        ];
        let blocks = [
            "para",
            "# head",
            "- item",
            "1. one",
            "- [ ] task",
            "> quote",
            "> [!NOTE]\n> note",
            "```\ncode\n```",
            "```js\nlet a = 1;\n```",
            "```math\nx^2\n```",
            "```mermaid\ngraph TD\n```",
            "| a |\n|---|\n| b |",
            "x[^1]\n\n[^1]: note",
            "$$x$$",
            "---",
        ];
        for mode in [Mode::Readme, Mode::Comment] {
            for sourcepos in [false, true] {
                let mut o = opts(mode);
                o.sourcepos = sourcepos;
                for tag in tags {
                    for block in blocks {
                        let md = format!("<{tag}\n\n{block}\n");
                        assert_clean(&md, &render(&md, &o).html);
                    }
                }
            }
        }
        // github.com (`gh api /markdown`, gfm + context): the raw `<div>` keeps only what
        // its allowlist permits and the absorbed `<p>` start tag is gone.
        let r = render("<div onclick=\"x\"\n\npara\n", &opts(Mode::Comment));
        assert_eq!(
            r.html.replace('\n', ""),
            r#"<div dir="auto">para<p dir="auto"></p></div>"#
        );
        let r = render(
            "<details open ontoggle=\"x\"\n\npara\n",
            &opts(Mode::Readme),
        );
        assert!(
            r.html
                .starts_with(r#"<details open=""><summary>Details</summary>para"#),
            "{}",
            r.html
        );
    }

    /// GH-SEC-2: an unclosed raw formatting element is cloned by the HTML parser into the
    /// generated code block that follows a raw `<table>`; the clone is filtered like raw
    /// HTML (github.com keeps a bare `<b>` there).
    #[test]
    fn reconstructed_formatting_elements_are_sanitized() {
        let md = "<table>\n\nx <b onmouseover=\"x\" style=\"s\">y\n\n```\ncode\n```\n";
        let r = render(md, &opts(Mode::Comment));
        assert!(
            r.html.contains(
                "<pre class=\"notranslate\"><b><code class=\"notranslate\">code\n</code></b></pre>"
            ),
            "{}",
            r.html
        );
        assert_clean(md, &r.html);
        let r = render(md, &opts(Mode::Readme));
        assert!(
            r.html
                .contains("<pre class=\"notranslate\"><b><code>code\n</code></b></pre>"),
            "{}",
            r.html
        );
        assert_clean(md, &r.html);
        let tags = [
            "a", "b", "big", "code", "em", "font", "i", "nobr", "s", "small", "strike", "strong",
            "tt", "u",
        ];
        let fences = [
            "```\ncode\n```",
            "```js\nlet a = 1;\n```",
            "```math\nx^2\n```",
            "```mermaid\ngraph TD\n```",
            "```nosuchlang\nz\n```",
        ];
        for mode in [Mode::Readme, Mode::Comment] {
            for tag in tags {
                for fence in fences {
                    for attrs in [
                        r#"onclick="x" style="s""#,
                        r#"href="javascript:x" onclick="x""#,
                        r#"tabindex="1" autofocus onfocus="x""#,
                    ] {
                        for prefix in ["<table>\n\n", "<table><tr><td>\n\n", ""] {
                            let md = format!("{prefix}x <{tag} {attrs}>y\n\n{fence}\n\nafter\n");
                            assert_clean(&md, &render(&md, &opts(mode)).html);
                        }
                    }
                }
            }
        }
    }

    /// GH-AUTO-1: bare http(s) URLs with a dot-less host are links on github.com (cmark-gfm's
    /// `url_match` allows short domains), comrak leaves them as text.
    #[test]
    fn short_host_urls_are_autolinked() {
        // `gh api /markdown` (gfm + context), `cases/autolink_short.gh.html`.
        let md = "Open http://localhost:3000 now.\n\nhttp://localhost\n\nhttps://intranet/path\n\nhttp://127.0.0.1:8080/\n\nhttps://my-host:9000/x\n";
        let gh = concat!(
            "<p dir=\"auto\">Open <a href=\"http://localhost:3000\" rel=\"nofollow\">http://localhost:3000</a> now.</p>\n",
            "<p dir=\"auto\"><a href=\"http://localhost\" rel=\"nofollow\">http://localhost</a></p>\n",
            "<p dir=\"auto\"><a href=\"https://intranet/path\" rel=\"nofollow\">https://intranet/path</a></p>\n",
            "<p dir=\"auto\"><a href=\"http://127.0.0.1:8080/\" rel=\"nofollow\">http://127.0.0.1:8080/</a></p>\n",
            "<p dir=\"auto\"><a href=\"https://my-host:9000/x\" rel=\"nofollow\">https://my-host:9000/x</a></p>",
        );
        for mode in [Mode::Comment, Mode::Readme] {
            assert_eq!(render(md, &opts(mode)).html.trim_end(), gh, "{mode:?}");
        }
        let link = |url: &str| format!("<a href=\"{url}\" rel=\"nofollow\">{url}</a>");
        for mode in [Mode::Comment, Mode::Readme] {
            let o = opts(mode);
            let html = |md: &str| render(md, &o).html;
            // Inside other inline and block containers.
            for md in [
                "# Run http://localhost:3000\n",
                "> see http://localhost:3000.\n",
                "- item http://localhost:3000\n",
                "**http://localhost:3000**\n",
                "| a |\n|---|\n| http://localhost:3000 |\n",
                "x[^1]\n\n[^1]: at http://localhost:3000\n",
                "a <span>http://localhost:3000</span> b\n",
                "(http://localhost:3000)\n",
            ] {
                assert!(
                    html(md).contains(&link("http://localhost:3000")),
                    "{md:?} -> {}",
                    html(md)
                );
            }
            // The dot after `www` counts for cmark-gfm's `www_match`.
            assert!(
                html("go www.localhost now\n")
                    .contains(r#"<a href="http://www.localhost" rel="nofollow">www.localhost</a>"#),
                "{}",
                html("go www.localhost now\n")
            );
            // The scheme compares case-insensitively.
            assert!(html("HTTP://localhost:1\n").contains(&link("HTTP://localhost:1")));
            // Trailing punctuation and an entity after the URL are not part of it.
            assert!(
                html("at http://localhost:3000.\n")
                    .contains(&format!("{}.", link("http://localhost:3000")))
            );
            assert!(html("at https://h/&amp;\n").contains(&format!("{}&amp;", link("https://h/"))));
            // Not links on github.com either.
            for md in [
                "ftp://localhost\n",
                "see http:// alone\n",
                "see http://-x\n",
                "`http://localhost:3000`\n",
                "```\nhttp://localhost:3000\n```\n",
                "[in http://localhost:3000 brackets]\n",
                "xhttp://localhost\n",
                "http\\://localhost:3000\n",
                "http&#58;//localhost:3000\n",
                "`c`www.localhost\n",
                "www.\n",
                "http://a_b/x\n",
            ] {
                assert!(!html(md).contains("<a "), "{md:?} -> {}", html(md));
            }
            // An existing link is not linked again.
            let r = html("[http://localhost:3000](http://x.com)\n");
            assert_eq!(r.matches("<a ").count(), 1, "{r}");
            assert!(r.contains(r#"href="http://x.com""#), "{r}");
        }
        // Raw HTML block text stays unlinked in the comment context (`S/api_gfm_ctx.html`).
        let r = render(
            "<div>\nhttp://localhost:3000\n</div>\n",
            &opts(Mode::Comment),
        );
        assert!(!r.html.contains("<a "), "{}", r.html);
    }

    /// GH-MATH-1: empty math edge cases (`cases2/math_blocks2.gh.html`,
    /// `cases2/math_display_variants.gh.html`, `gh api /markdown` gfm + context).
    #[test]
    fn empty_math_edge_cases() {
        for mode in [Mode::Comment, Mode::Readme] {
            let r = render("```math\n```\n", &opts(mode));
            assert_eq!(
                r.html.trim_end(),
                r#"<pre lang="math" class="notranslate">$$$$</pre>"#,
                "{mode:?}"
            );
            assert!(!r.features.math);
            // The generated `<pre lang="math">` is not taken for a raw one and re-wrapped.
            let r = render("```math\n```\n\n```math\nx\n```\n", &opts(mode));
            assert_eq!(r.html.matches("<math-renderer").count(), 1, "{}", r.html);
            assert!(r.html.contains(">$$$$</pre>"), "{}", r.html);

            let r = render("$$\n$$\n", &opts(mode));
            let html = r.html.trim_end();
            assert!(
                html.starts_with(r#"<p dir="auto"><math-renderer class="js-display-math" style="display: block" data-run-id=""#)
                    && html.ends_with(">$$\n$$</math-renderer></p>"),
                "{mode:?}: {html}"
            );
            assert!(r.features.math);
            // Blank spans on one line stay text.
            for md in ["$$ $$\n", "$$$$\n"] {
                assert!(
                    !render(md, &opts(mode)).html.contains("<math-renderer"),
                    "{md:?}"
                );
            }
        }
    }

    /// GH-PERF-1: runaway nesting (quadratic in html5ever) is shown as plain text instead of
    /// stalling the render; moderately deep documents still render normally.
    #[test]
    fn runaway_nesting_falls_back_to_text() {
        let n = 20_000;
        for md in [
            format!("{}x{}\n", "<div>".repeat(n), "</div>".repeat(n)),
            format!("{} x\n", ">".repeat(n)),
            format!("{}x <script>\n", "<blockquote>".repeat(n)),
        ] {
            for mode in [Mode::Readme, Mode::Comment] {
                let r = render(&md, &opts(mode));
                assert!(
                    r.html
                        .starts_with(r#"<pre class="notranslate"><code class="notranslate">"#),
                    "{}",
                    &r.html[..200.min(r.html.len())]
                );
                assert!(!r.html.contains("<div") && !r.html.contains("<blockquote"));
                assert!(!r.html.contains("<script"));
                let mut escaped = String::new();
                util::push_text(&mut escaped, &md);
                assert!(r.html.contains(&escaped), "the source, escaped");
                assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
                assert!(r.warnings[0].contains("too deeply"));
                assert_eq!(r.title, None);
            }
        }
        // A few hundred levels are far below the budget.
        let r = render(&format!("{} deep\n", ">".repeat(300)), &opts(Mode::Comment));
        assert_eq!(r.html.matches("<blockquote>").count(), 300);
        assert!(r.warnings.is_empty());
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
