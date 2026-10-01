//! Unit tests for `words.rs`.

use super::*;

#[test]
fn tables_are_sorted() {
    let tables: &[&[&str]] = &[
        RUST_KEYWORDS,
        RUST_BUILTINS,
        PYTHON_KEYWORDS,
        PYTHON_BUILTINS,
        PYTHON_BUILTINS_PSEUDO,
        PYTHON_EXCEPTIONS,
        JS_KEYWORDS,
        JS_DECLARATIONS,
        JS_RESERVED,
        JS_CONSTANTS,
        JS_BUILTINS,
        TS_KEYWORDS,
        TS_DECLARATIONS,
        TS_RESERVED,
        TS_CONSTANTS,
        TS_BUILTINS,
        C_KEYWORDS,
        C_KEYWORDS_TYPE,
        C_RESERVED,
        CPP_KEYWORDS,
        CPP_KEYWORDS_TYPE,
        CPP_RESERVED,
        C_BUILTIN_CONSTANTS,
        GO_KEYWORDS,
        GO_TYPES,
        GO_FUNCTIONS,
        GO_CONSTANTS,
        SHELL_KEYWORDS,
        SHELL_BUILTINS,
        CPP_EXTRA_KEYWORDS,
        JAVA_KEYWORDS,
        JAVA_DECLARATIONS,
        JAVA_TYPES,
    ];
    for t in tables {
        assert!(t.windows(2).all(|w| w[0] < w[1]), "{t:?}");
    }
}

fn seg(class: Option<&'static str>, r: std::ops::Range<usize>) -> Segment {
    Segment::new(class, r)
}

#[test]
fn rust_words() {
    let code = "let v = Vec::new(); map.insert(x);";
    let segs = vec![seg(Some("kt"), 0..3), seg(None, 3..code.len())];
    let out = RUST.refine(code, segs);
    let class_at = |i: usize| {
        out.iter()
            .find(|s| s.range.start == i)
            .and_then(|s| s.class)
    };
    assert_eq!(class_at(0), Some("k"), "let");
    assert_eq!(class_at(4), Some("n"), "v");
    assert_eq!(class_at(8), Some("nn"), "Vec");
    assert_eq!(class_at(13), Some("nf"), "new");
    assert_eq!(class_at(20), Some("n"), "map");
    assert_eq!(class_at(23), Some("nf"), "the dot joins the call");
    assert_eq!(class_at(24), Some("nf"), "insert");
    assert_eq!(class_at(31), Some("n"), "x");
}

#[test]
fn protected_and_boundaries() {
    let code = "\"let\" 1x _b9";
    let segs = vec![seg(Some("s2"), 0..5), seg(None, 5..code.len())];
    let out = RUST.refine(code, segs);
    assert_eq!(out[0], seg(Some("s2"), 0..5), "strings are not re-read");
    assert!(out.contains(&seg(Some("n"), 9..12)), "{out:?}");
    assert!(
        !out.iter().any(|s| s.range == (7..8)),
        "no identifier inside 1x: {out:?}"
    );
}

#[test]
fn python_after_dot() {
    let code = "self.get(x).y len(True) print";
    let out = PYTHON.refine(code, vec![seg(None, 0..code.len())]);
    let class_at = |i: usize| {
        out.iter()
            .find(|s| s.range.start == i)
            .and_then(|s| s.class)
    };
    assert_eq!(class_at(0), Some("n"), "self");
    assert_eq!(class_at(5), Some("nf"), "get");
    assert_eq!(class_at(12), Some("n"), "y");
    assert_eq!(class_at(14), Some("nb"), "len");
    assert_eq!(class_at(18), Some("bp"), "True");
    assert_eq!(
        class_at(24),
        Some("k"),
        "print is a (Python 2) keyword in Rouge"
    );
}

#[test]
fn shell_builtins() {
    let code = "ls -la; ls-files";
    let out = SHELL.refine(code, vec![seg(None, 0..code.len())]);
    assert_eq!(out[0], seg(Some("nb"), 0..2));
    assert!(
        out.contains(&seg(None, 8..10)),
        "a builtin followed by '-' is text: {out:?}"
    );
}
