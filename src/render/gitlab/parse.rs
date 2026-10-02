//! `ParseHtmlFilter` (GL §2): comrak's HTML becomes the DOM the Banzai filters work on.
//!
//! gitlab.com parses it with GitLab's `HTML::Pipeline::HTML5Patch`
//! (`config/initializers/html_pipeline_html5_patch.rb` at gitlabhq 8396d286, the commit of the
//! restored sources; that file itself is not among them):
//! `Nokogiri::HTML5::DocumentFragment.parse(html, max_tree_depth: 4096)`. Gumbo stops when the
//! stack of open elements holds more than `max_tree_depth` elements (checked before each token,
//! the fragment's `html` root included; `gumbo-parser/src/parser.c`), Nokogiri raises
//! `Document tree depth limit exceeded`, and the patch renders [`NESTING_TOO_DEEP_MESSAGE`]
//! instead of the document; the remaining filters then run on that.
//!
//! The same limit keeps md-render fast (GL-PERF-2): html5ever, like gumbo, scans the stack of
//! open elements for many start tags, so unbounded nesting makes parsing quadratic (80 KB of `*`
//! took 10 s). The input is fed in small chunks, and parsing stops once an element sits deeper
//! than the limit.
//!
//! html5ever's stack is private, so the depth is measured in the tree being built: an element's
//! depth counts its element ancestors, the `html` root being depth 1, and contents of a
//! `<template>` count from the template (gumbo keeps them as its children). In the tree builder
//! the current node is where nearly every element is inserted, so this is the stack position;
//! only foster parenting and the adoption agency place an element away from the current node,
//! and those documents may be cut off a few levels earlier or later than on gitlab.com. Void
//! elements (`<br>`, `<img>`, ...) are popped as soon as they are inserted, so they never make a
//! document too deep, as on gumbo; self-closing foreign elements (`<path/>` in SVG) are counted.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use dom_query::{Document, NodeId};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tokenizer::TokenizerOpts;
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeBuilderOpts, TreeSink};
use html5ever::{Attribute, ParseOpts, QualName, local_name, ns};

/// `HTML::Pipeline::HTML5Patch::HTML5_MAX_TREE_DEPTH`.
pub(super) const HTML5_MAX_TREE_DEPTH: usize = 4096;

/// `HTML::Pipeline::HTML5Patch::NESTING_TOO_DEEP_MESSAGE` (a Ruby `<<~HTML` heredoc, so it keeps
/// its line break and trailing newline).
pub(super) const NESTING_TOO_DEEP_MESSAGE: &str = "<p>Rendering aborted because the document's \
nesting was too deep. Please reduce the\nlevel of nesting in your markup.</p>\n";

/// Bytes handed to html5ever at a time. Parsing stops at the first chunk boundary after the
/// limit is passed, so this bounds the work done beyond it.
const CHUNK_BYTES: usize = 4096;

/// The fragment nests deeper than the limit (Nokogiri's `Document tree depth limit exceeded`).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct TooDeep;

/// `Nokogiri::HTML5::DocumentFragment.parse(html, max_tree_depth: HTML5_MAX_TREE_DEPTH)`: the
/// same tree as `Document::fragment(html)`, or [`TooDeep`].
pub(super) fn fragment(html: &str) -> Result<Document, TooDeep> {
    fragment_within(html, HTML5_MAX_TREE_DEPTH)
}

/// [`fragment`] with an explicit depth limit.
fn fragment_within(html: &str, limit: usize) -> Result<Document, TooDeep> {
    // The options and context of `Document::fragment` (dom_query 0.28).
    let opts = ParseOpts {
        tokenizer: TokenizerOpts::default(),
        tree_builder: TreeBuilderOpts {
            scripting_enabled: false,
            drop_doctype: true,
            ..Default::default()
        },
    };
    let mut parser = html5ever::parse_fragment(
        DepthSink::new(limit),
        opts,
        QualName::new(None, ns!(html), local_name!("body")),
        Vec::new(),
        false,
    );
    let mut rest = html;
    while !rest.is_empty() {
        let mut end = rest.len().min(CHUNK_BYTES);
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (chunk, tail) = rest.split_at(end);
        parser.process(StrTendril::from_slice(chunk));
        if parser.tokenizer.sink.sink.too_deep.get() {
            return Err(TooDeep);
        }
        rest = tail;
    }
    parser.finish()
}

/// dom_query's [`Document`] sink, measuring element depth as the tree is built.
struct DepthSink {
    doc: Document,
    limit: usize,
    too_deep: Cell<bool>,
    /// Depth of nodes whose depth is known, tagged with the `generation` it was computed in.
    depth: RefCell<HashMap<NodeId, (u32, usize)>>,
    /// Bumped whenever nodes move (adoption agency, foster parenting): every recorded depth may
    /// then be stale and is recomputed on demand.
    generation: Cell<u32>,
    /// Template contents fragment -> its `<template>` element.
    template_of: RefCell<HashMap<NodeId, NodeId>>,
}

impl DepthSink {
    fn new(limit: usize) -> Self {
        DepthSink {
            doc: Document::fragment_sink(),
            limit,
            too_deep: Cell::new(false),
            depth: RefCell::new(HashMap::new()),
            generation: Cell::new(0),
            template_of: RefCell::new(HashMap::new()),
        }
    }

    /// The node a depth counts from: the parent, or the template owning a contents fragment.
    fn up(&self, id: NodeId) -> Option<NodeId> {
        self.doc
            .tree
            .parent_of(&id)
            .map(|p| p.id)
            .or_else(|| self.template_of.borrow().get(&id).copied())
    }

    /// Element depth of `id` (parentless nodes are 0). Walks up only as far as the first node
    /// whose depth is current, and records every depth on the way, so the walk is amortised
    /// O(1) while nothing moves and O(depth) once after a move.
    fn depth_of(&self, id: NodeId) -> usize {
        let generation = self.generation.get();
        let mut known = self.depth.borrow_mut();
        let mut path = Vec::new();
        let mut cur = id;
        let mut depth = loop {
            if let Some(&(g, d)) = known.get(&cur)
                && g == generation
            {
                break d;
            }
            match self.up(cur) {
                Some(next) => {
                    path.push(cur);
                    cur = next;
                }
                None => {
                    known.insert(cur, (generation, 0));
                    break 0;
                }
            }
        };
        let templates = self.template_of.borrow();
        for &n in path.iter().rev() {
            // Template contents are not an element of their own (gumbo: template children).
            if !templates.contains_key(&n) {
                depth += 1;
            }
            known.insert(n, (generation, depth));
        }
        depth
    }

    /// After `child` was inserted: flag the document when it is an element that stays on the
    /// stack of open elements deeper than the limit.
    fn placed(&self, child: Option<NodeId>) {
        let Some(id) = &child else {
            return;
        };
        let stays_open = self
            .doc
            .tree
            .get_name(id)
            .is_some_and(|name| !is_void(&name));
        if stays_open && self.depth_of(*id) > self.limit {
            self.too_deep.set(true);
        }
    }

    fn moved(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }
}

/// HTML elements the tree builder pops right after inserting them (WHATWG "in body": "insert an
/// HTML element for the token. Immediately pop the current node").
fn is_void(name: &QualName) -> bool {
    name.ns == ns!(html)
        && matches!(
            name.local,
            local_name!("area")
                | local_name!("base")
                | local_name!("basefont")
                | local_name!("bgsound")
                | local_name!("br")
                | local_name!("col")
                | local_name!("embed")
                | local_name!("frame")
                | local_name!("hr")
                | local_name!("img")
                | local_name!("input")
                | local_name!("keygen")
                | local_name!("link")
                | local_name!("meta")
                | local_name!("param")
                | local_name!("source")
                | local_name!("track")
                | local_name!("wbr")
        )
}

/// Delegates every call to dom_query's own sink.
impl TreeSink for DepthSink {
    type Handle = NodeId;
    type Output = Result<Document, TooDeep>;
    type ElemName<'a> = <Document as TreeSink>::ElemName<'a>;

    fn finish(self) -> Self::Output {
        if self.too_deep.get() {
            Err(TooDeep)
        } else {
            Ok(self.doc.finish())
        }
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        self.doc.parse_error(msg);
    }

    fn get_document(&self) -> NodeId {
        self.doc.get_document()
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> Self::ElemName<'a> {
        self.doc.elem_name(target)
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> NodeId {
        self.doc.create_element(name, attrs, flags)
    }

    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.doc.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        self.doc.create_pi(target, data)
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        let placed = node_of(&child);
        self.doc.append(parent, child);
        self.placed(placed);
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let placed = node_of(&child);
        self.doc
            .append_based_on_parent_node(element, prev_element, child);
        self.placed(placed);
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        self.doc
            .append_doctype_to_document(name, public_id, system_id);
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        let contents = self.doc.get_template_contents(target);
        self.template_of.borrow_mut().insert(contents, *target);
        contents
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        self.doc.same_node(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.doc.set_quirks_mode(mode);
    }

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        let placed = node_of(&new_node);
        self.doc.append_before_sibling(sibling, new_node);
        self.placed(placed);
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        self.doc.add_attrs_if_missing(target, attrs);
    }

    fn remove_from_parent(&self, target: &NodeId) {
        self.doc.remove_from_parent(target);
        self.moved();
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        self.doc.reparent_children(node, new_parent);
        self.moved();
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &NodeId) -> bool {
        self.doc.is_mathml_annotation_xml_integration_point(handle)
    }
}

/// The node `child` inserts (`None` for text, which never changes the depth).
fn node_of(child: &NodeOrText<NodeId>) -> Option<NodeId> {
    match child {
        NodeOrText::AppendNode(id) => Some(*id),
        NodeOrText::AppendText(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(tag: &str, n: usize) -> String {
        format!("<{tag}>").repeat(n) + "x"
    }

    #[test]
    fn same_tree_as_dom_query() {
        let samples = [
            String::new(),
            "<p>a<b>b<i>c</b>d</i>e</p>".to_owned(),
            "<table><div>foster</div><tr><td>x</td></tr></table>".to_owned(),
            "<template><p>t</p></template><svg><path/></svg>".to_owned(),
            "<a><div><a>x</a></div></a><!-- c --><p>é ü 😀</p>".to_owned(),
            // Long enough for several chunks, with a multi-byte character on a boundary.
            "<p>".to_owned() + &"é".repeat(CHUNK_BYTES) + "</p>" + &"<b>x</b>\n".repeat(2000),
        ];
        for html in samples {
            let ours = fragment(&html).expect("shallow").html_root().html();
            let theirs = Document::fragment(html.as_str()).html_root().html();
            assert_eq!(ours, theirs, "{html}");
        }
    }

    /// The `html` root counts, as in gumbo's stack of open elements: 4095 nested elements
    /// parse, 4096 do not.
    #[test]
    fn depth_limit_matches_gumbo() {
        let ok = fragment(&nested("div", HTML5_MAX_TREE_DEPTH - 1));
        assert!(ok.is_ok());
        assert_eq!(
            fragment(&nested("div", HTML5_MAX_TREE_DEPTH)).err(),
            Some(TooDeep)
        );
        // Nothing inside the deepest element is needed: it is on the stack all the same.
        assert!(fragment(&"<div>".repeat(HTML5_MAX_TREE_DEPTH)).is_err());
        // A void element is popped at once, so it does not count.
        let void = "<div>".repeat(HTML5_MAX_TREE_DEPTH - 1) + "<br><img src=x>";
        assert!(fragment(&void).is_ok());
        // Closing elements again keeps a long document shallow.
        let flat = ("<div>".repeat(1000) + &"</div>".repeat(1000)).repeat(10);
        assert!(fragment(&flat).is_ok());
    }

    #[test]
    fn depth_counts_formatting_and_template_nesting() {
        assert!(fragment_within(&nested("em", 10), 11).is_ok());
        assert!(fragment_within(&nested("em", 11), 11).is_err());
        // Template contents are counted from the template (gumbo keeps them as children).
        assert!(fragment_within(&nested("template", 10), 11).is_ok());
        assert!(fragment_within(&nested("template", 11), 11).is_err());
        // Depths stay right after the adoption agency moves nodes: `</b>` moves the `p` out of
        // the `b` (depth 3 -> 2), so the six `i` inside it are at depths 3 to 8.
        let misnested = "<b><p>x</b>".to_owned() + &nested("i", 6);
        assert_eq!(
            Document::fragment(misnested.as_str())
                .html_root()
                .html()
                .to_string(),
            "<html><b></b><p><b>x</b><i><i><i><i><i><i>x</i></i></i></i></i></i></p></html>"
        );
        assert!(fragment_within(&misnested, 8).is_ok());
        assert!(fragment_within(&misnested, 7).is_err());
    }
}
