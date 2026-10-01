//! `SyntaxHighlightFilter` (GL §5.16, §8): wraps every `pre > code:only-child` (except Kroki
//! blocks) in GitLab's code-block markup with per-line `span#LCn.line` elements
//! (`Rouge::Formatters::HTMLGitlab`, GL §8.2). Tokens come from
//! [`crate::render::highlight::gitlab`].

use dom_query::NodeRef;

use super::{dom, filters};

/// `math`, `suggestion` and asciidoctor-kroki's `SUPPORTED_DIAGRAM_NAMES` bypass Rouge
/// (GL §8.1): the tag is the language as written and the lines are plain text.
pub(super) const NO_ROUGE: &[&str] = &[
    "math", "suggestion", "actdiag", "blockdiag", "bpmn", "bytefield", "c4plantuml", "d2", "dbml",
    "ditaa", "erd", "excalidraw", "graphviz", "mermaid", "nomnoml", "nwdiag", "packetdiag",
    "pikchr", "plantuml", "rackdiag", "seqdiag", "svgbob", "symbolator", "umlet", "vega",
    "vegalite", "wavedrom", "structurizr", "diagramsnet", "wireviz",
];

/// Title of the bidi warning span (`Gitlab::Unicode.bidi_warning`).
const BIDI_WARNING: &str = "Potentially unwanted character detected: Unicode BiDi Control";

/// `\p{Bidi_Control}`.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Non-ASCII `\p{Space_Separator}`.
fn is_non_ascii_space(c: char) -> bool {
    matches!(c, '\u{00A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}')
}

/// `replace_space_characters` + `highlight_unicode_control_characters` on one line of escaped
/// HTML (the highlighter's markup is ASCII, so only text is affected).
pub(super) fn fix_unicode(line: &str) -> String {
    if line.is_ascii() {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len() + 16);
    for c in line.chars() {
        if is_non_ascii_space(c) {
            out.push(' ');
        } else if is_bidi_control(c) {
            out.push_str("<span class=\"unicode-bidi has-tooltip\" data-toggle=\"tooltip\" title=\"");
            out.push_str(BIDI_WARNING);
            out.push_str("\">");
            out.push(c);
            out.push_str("</span>");
        } else {
            out.push(c);
        }
    }
    out
}

/// Rouge `token_lines` for plain text: split on `\n`, drop a final empty line.
pub(super) fn plain_lines(code: &str) -> Vec<String> {
    let mut lines: Vec<&str> = code.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines.into_iter().map(filters::escape_text).collect()
}

/// Returns `(tag, lines)` for a block with the given `data-canonical-lang`.
pub(super) fn highlight_block(lang: Option<&str>, code: &str) -> (String, Vec<String>) {
    match lang {
        Some(l) if NO_ROUGE.contains(&l) => (l.to_owned(), plain_lines(code)),
        _ => {
            if code.is_empty() {
                let tag = crate::render::highlight::gitlab("", lang.unwrap_or("")).tag;
                return (tag, Vec::new());
            }
            let res = crate::render::highlight::gitlab(code, lang.unwrap_or(""));
            let tag = if res.tag.is_empty() { "plaintext".to_owned() } else { res.tag };
            let expected = plain_lines(code).len();
            if res.lines.len() == expected {
                (tag, res.lines)
            } else {
                // Defensive: a highlighter that disagrees about line splitting falls back to
                // plain text, as GitLab does when Rouge raises.
                (tag, plain_lines(code))
            }
        }
    }
}

pub(super) fn syntax_highlight(root: &NodeRef<'_>) {
    for id in dom::elements_named(root, &["code"]) {
        let code = root.tree.get_unchecked(&id);
        if !filters::is_only_code_child(&code) {
            continue;
        }
        let Some(pre) = code.parent() else { continue };
        if pre.has_attr("data-kroki-style") || pre.parent().is_none() || !dom::is_attached(&pre, root) {
            continue;
        }
        let lang = dom::attr(&pre, "data-canonical-lang");
        let text = dom::text(&code);
        let (tag, lines) = highlight_block(lang.as_deref(), &text);
        let data_lang = if tag.is_empty() { "plaintext" } else { tag.as_str() };
        let mut inner = String::with_capacity(text.len() * 2 + lines.len() * 48);
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                inner.push('\n');
            }
            inner.push_str("<span id=\"LC");
            inner.push_str(&(i + 1).to_string());
            inner.push_str("\" class=\"line\" data-lang=\"");
            inner.push_str(&dom::escape_html(data_lang));
            inner.push_str("\">");
            inner.push_str(&fix_unicode(line));
            inner.push_str("</span>");
        }
        code.remove_children();
        if !inner.is_empty() {
            code.append_html(inner);
        }
        // `pre_node.children = code_node`: drop stray text next to the <code>.
        for c in pre.children() {
            if c.id != code.id {
                c.remove_from_parent();
            }
        }
        dom::add_class(&pre, "code highlight js-syntax-highlight");
        if !tag.is_empty() {
            dom::add_class(&pre, &format!("language-{tag}"));
        }
        pre.set_attr("v-pre", "true");
        let wrapper = dom::new_element(root.tree, "div", &[("class", "gl-relative markdown-code-block js-markdown-code")]);
        pre.insert_before(&wrapper);
        wrapper.append_child(&pre);
        if tag != "suggestion" {
            wrapper.append_child(&root.tree.new_element("copy-code"));
            wrapper.append_child(&root.tree.new_element("insert-code-snippet"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dom_query::Document;

    fn run(html: &str) -> String {
        let doc = Document::fragment(html);
        let root = doc.html_root();
        syntax_highlight(&root);
        root.inner_html().to_string()
    }

    #[test]
    fn plain_block_markup() {
        assert_eq!(
            run("<pre data-sourcepos=\"1:1-4:3\"><code>a &lt;b&gt;\n\nc\n</code></pre>"),
            "<div class=\"gl-relative markdown-code-block js-markdown-code\"><pre data-sourcepos=\"1:1-4:3\" class=\"code highlight js-syntax-highlight language-plaintext\" v-pre=\"true\"><code>\
             <span id=\"LC1\" class=\"line\" data-lang=\"plaintext\">a &lt;b&gt;</span>\n\
             <span id=\"LC2\" class=\"line\" data-lang=\"plaintext\"></span>\n\
             <span id=\"LC3\" class=\"line\" data-lang=\"plaintext\">c</span></code></pre><copy-code></copy-code><insert-code-snippet></insert-code-snippet></div>"
        );
    }

    #[test]
    fn empty_block_and_suggestion() {
        assert_eq!(
            run("<pre data-canonical-lang=\"suggestion\"><code></code></pre>"),
            "<div class=\"gl-relative markdown-code-block js-markdown-code\"><pre data-canonical-lang=\"suggestion\" class=\"code highlight js-syntax-highlight language-suggestion\" v-pre=\"true\"><code></code></pre></div>"
        );
    }

    #[test]
    fn diagrams_keep_their_tag() {
        let out = run("<pre data-canonical-lang=\"mermaid\"><code class=\"js-render-mermaid\">graph TD\n</code></pre>");
        assert!(out.contains("language-mermaid"), "{out}");
        assert!(out.contains("<code class=\"js-render-mermaid\"><span id=\"LC1\" class=\"line\" data-lang=\"mermaid\">graph TD</span></code>"), "{out}");
    }

    #[test]
    fn kroki_and_multi_child_skipped() {
        let k = "<pre data-kroki-style=\"x\"><code>a</code></pre>";
        assert_eq!(run(k), k);
        let m = "<pre><code>a</code><b>x</b></pre>";
        assert_eq!(run(m), m);
    }

    #[test]
    fn unicode_cleanup() {
        assert_eq!(fix_unicode("a\u{a0}b"), "a b");
        assert!(fix_unicode("x\u{202E}y").contains("class=\"unicode-bidi has-tooltip\""));
    }
}
