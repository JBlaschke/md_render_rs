//! Heading anchors (GH §6.1): every `h1`–`h6`, raw HTML ones included, gets
//! `<div class="markdown-heading"><hN class="heading-element">…</hN><a id="user-content-SLUG"
//! class="anchor" aria-label="Permalink: LABEL" href="#SLUG"><svg …octicon-link…></a></div>`
//! in the README context. Comment context: no wrapper, no anchor, no id (GH §9.B).
//!
//! Runs before the emoji filter so slugs and labels see shortcodes, not emoji (GH §6.2).

use dom_query::{Document, NodeRef};

use super::octicons;
use super::slug::Slugger;
use super::{Ctx, dom};

const HEADINGS: [&str; 6] = ["h1", "h2", "h3", "h4", "h5", "h6"];

fn is_heading(n: &NodeRef<'_>) -> bool {
    dom::name(n).is_some_and(|l| HEADINGS.contains(&&*l))
}

/// The footnotes' `h2#footnote-label` is not a document heading.
fn is_footnote_label(n: &NodeRef<'_>) -> bool {
    n.parent()
        .is_some_and(|p| p.has_name("section") && p.has_attr("data-footnotes"))
}

/// Text for `aria-label`: the slug source text without footnote reference numbers (GH §6.3).
fn label_text(h: &NodeRef<'_>) -> String {
    let mut s = String::new();
    for d in h.descendants() {
        if let Some(t) = dom::text_of(&d) {
            let in_fnref = dom::has_ancestor(&d, |a| a.has_attr("data-footnote-ref"));
            if !in_fnref {
                s.push_str(&t);
            }
        }
    }
    s
}

/// Adds wrappers and anchors (README context).
pub(super) fn run(doc: &Document, ctx: &Ctx) {
    if ctx.comment {
        return;
    }
    let mut slugger = Slugger::default();
    let headings: Vec<NodeRef<'_>> = dom::root(doc)
        .descendants()
        .into_iter()
        .filter(|n| is_heading(n) && !is_footnote_label(n))
        .collect();
    for h in headings {
        if h.first_child().is_none() {
            continue; // `#` alone: `<h1></h1>`, no wrapper (GH §6.1)
        }
        let slug = slugger.slug(&h.text());
        let label = label_text(&h);
        h.add_class("heading-element");

        let wrapper = doc.tree.new_element("div");
        wrapper.set_attr("class", "markdown-heading");
        if let Some(sp) = h.attr("data-sourcepos") {
            wrapper.set_attr("data-sourcepos", &sp);
            h.remove_attr("data-sourcepos");
        }
        let anchor = doc.tree.new_element("a");
        let id = if slug.is_empty() {
            String::new()
        } else {
            format!("user-content-{slug}")
        };
        anchor.set_attr("id", &id);
        anchor.set_attr("class", "anchor");
        anchor.set_attr("aria-label", &format!("Permalink: {label}"));
        anchor.set_attr("href", &format!("#{slug}"));
        anchor.append_child(&doc.tree.new_element(&octicons::placeholder("link")));

        dom::wrap(&h, &wrapper);
        wrapper.append_child(&anchor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_on(html: &str, comment: bool) -> String {
        let doc = Document::fragment(html);
        run(&doc, &Ctx::for_tests(comment));
        dom::root(&doc).inner_html().to_string()
    }

    #[test]
    fn wraps_and_dedupes() {
        let out = run_on("<h2>Hello</h2><h2>Hello</h2>", false);
        assert_eq!(
            out,
            r##"<div class="markdown-heading"><h2 class="heading-element">Hello</h2><a id="user-content-hello" class="anchor" aria-label="Permalink: Hello" href="#hello"><mdr-octicon-link></mdr-octicon-link></a></div><div class="markdown-heading"><h2 class="heading-element">Hello</h2><a id="user-content-hello-1" class="anchor" aria-label="Permalink: Hello" href="#hello-1"><mdr-octicon-link></mdr-octicon-link></a></div>"##
        );
    }

    #[test]
    fn edge_cases() {
        assert_eq!(run_on("<h1></h1>", false), "<h1></h1>");
        let out = run_on("<h1>???</h1><h1>!!!</h1>", false);
        assert!(out.contains(r##"<a id="" class="anchor" aria-label="Permalink: ???" href="#">"##));
        assert!(out.contains(r##"id="user-content--1""##) && out.contains(r##"href="#-1""##));
        let out = run_on(
            r#"<h2 id="user-content-raw" data-sourcepos="3:1-3:9">Raw <img alt="x"> heading</h2>"#,
            false,
        );
        assert!(out.starts_with(r#"<div class="markdown-heading" data-sourcepos="3:1-3:9"><h2 id="user-content-raw" class="heading-element">"#));
        assert!(out.contains(r##"href="#raw--heading""##));
        let out = run_on(
            r##"<h2>Note<sup><a data-footnote-ref="" href="#x">1</a></sup></h2>"##,
            false,
        );
        assert!(
            out.contains(r#"aria-label="Permalink: Note""#) && out.contains(r##"href="#note1""##)
        );
        let footnotes =
            r#"<section data-footnotes=""><h2 id="footnote-label">Footnotes</h2></section>"#;
        assert_eq!(run_on(footnotes, false), footnotes);
        assert_eq!(run_on("<h3>Comment</h3>", true), "<h3>Comment</h3>");
    }
}
