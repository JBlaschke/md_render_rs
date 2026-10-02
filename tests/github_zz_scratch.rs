// Temporary exploration harness (not committed).
use md_render::flavor::{Flavor, Mode};
use md_render::render::{RenderOptions, github};

#[test]
#[ignore]
fn scratch() {
    let path = std::env::var("SCRATCH_MD").unwrap();
    let src = std::fs::read_to_string(path).unwrap();
    let mut o = RenderOptions::new(Flavor::Github);
    o.sourcepos = std::env::var_os("SCRATCH_SP").is_some();
    if std::env::var_os("SCRATCH_COMMENT").is_some() {
        o.mode = Mode::Comment;
    }
    o.bundle_url = "/b".into();
    println!("<<<\n{}\n>>>", github::render(&src, &o).html);
}

#[test]
#[ignore]
fn ast_dump() {
    use comrak::nodes::NodeValue;
    let path = std::env::var("SCRATCH_MD").unwrap();
    let src = std::fs::read_to_string(path).unwrap();
    let mut o = comrak::Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.footnotes = true;
    o.extension.math_code = true;
    o.render.r#unsafe = true;
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, &src, &o);
    fn walk<'a>(n: &'a comrak::nodes::AstNode<'a>, depth: usize) {
        let d = n.data.borrow();
        let label = match &d.value {
            NodeValue::FootnoteDefinition(f) => {
                format!("FootnoteDefinition(name={:?}, total_refs={})", f.name, f.total_references)
            }
            NodeValue::FootnoteReference(r) => format!(
                "FootnoteReference(name={:?}, ix={}, ref_num={})",
                r.name, r.ix, r.ref_num
            ),
            other => format!("{:?}", std::mem::discriminant(other)).replace("Discriminant", &format!("{other:?}")),
        };
        let short: String = label.chars().take(90).collect();
        println!("{}{}", "  ".repeat(depth), short);
        for c in n.children() {
            walk(c, depth + 1);
        }
    }
    println!("<<<AST");
    walk(root, 0);
    println!("AST>>>");
}
