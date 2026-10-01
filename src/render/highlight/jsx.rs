//! JSX detection for JavaScript code blocks.
//!
//! github.com highlights `source.js` (` ```js `, ` ```jsx `, ...) with tree-sitter-javascript,
//! which parses JSX (GH §7.7). two-face's JavaScript grammar does not: in `<div>x</div>` the
//! `</` reads as a division followed by a regex literal that swallows the rest of the line and
//! the next one, and the closing brackets of the enclosing code become `invalid.illegal`.
//! (two-face's only JSX-capable JavaScript grammar, "JavaScript (Babel)", is excluded from its
//! fancy-regex syntax set because fancy-regex cannot compile it.) Blocks that contain JSX are
//! therefore tokenized with two-face's TypeScriptReact grammar, a superset of JavaScript with
//! JSX, and everything else keeps the JavaScript grammar, which matches github.com better on
//! plain JavaScript.
//!
//! Detection is lexical: a `<` that opens an element (`<>`, or `<` + a JSX element name followed
//! by whitespace, `/` or `>`) in a position where an expression can start: at the start of the
//! block, after an opening bracket, an operator that takes an operand on its right (`=`, `=>`,
//! `:`, `?`, `&&`, `||`, `,`, ...) or the keywords `return`, `yield` and `default`. Comparisons
//! (`i < n`, `a<b`) follow an operand and never qualify; nor do HTML strings like `"<b>"` or
//! comments like `// <App />`.

/// Whether `code` (JavaScript) contains a JSX element.
pub(crate) fn contains_jsx(code: &str) -> bool {
    let b = code.as_bytes();
    b.iter()
        .enumerate()
        .any(|(i, &c)| c == b'<' && opens_element(&b[i + 1..]) && expression_may_start(&b[..i]))
}

/// Whether the text after a `<` starts a JSX opening tag or fragment.
fn opens_element(rest: &[u8]) -> bool {
    match rest.first() {
        Some(b'>') => true,
        Some(&c) if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => {
            let name_len = rest
                .iter()
                .position(|&c| !(c.is_ascii_alphanumeric() || b"_$.:-".contains(&c)))
                .unwrap_or(rest.len());
            rest.get(name_len)
                .is_some_and(|&c| c.is_ascii_whitespace() || c == b'/' || c == b'>')
        }
        _ => false,
    }
}

/// Whether an expression can start right after `before` (lexically).
fn expression_may_start(before: &[u8]) -> bool {
    let before = before.trim_ascii_end();
    let Some(&last) = before.last() else {
        return true;
    };
    if b"([{,=:?&|>;}".contains(&last) {
        return true;
    }
    if !last.is_ascii_alphabetic() {
        return false;
    }
    let word_start = before
        .iter()
        .rposition(|&c| !(c.is_ascii_alphanumeric() || c == b'_' || c == b'$'))
        .map_or(0, |p| p + 1);
    // `obj.return` is a property name, not the keyword.
    let is_property = word_start > 0 && before[word_start - 1] == b'.';
    !is_property && matches!(&before[word_start..], b"return" | b"yield" | b"default")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_jsx() {
        for code in [
            "const a = <div>x</div>;\n",
            "<App />\n",
            "  <p>hi</p>\n",
            "return (\n  <div className=\"app\">\n",
            "return <Foo.Bar/>;\n",
            "const f = () => <>x</>;\n",
            "{cond && <span>yes</span>}\n",
            "x ? <A /> : <B />\n",
            "render(<App />, root);\n",
            "const icons = { home: <Icon\n  name=\"home\" /> };\n",
            "export default <svg:rect width=\"1\"/>;\n",
            "function* g() { yield <li key={1} /> }\n",
        ] {
            assert!(contains_jsx(code), "{code:?}");
        }
    }

    #[test]
    fn ignores_plain_javascript() {
        for code in [
            "for (let i = 0; i < n; i++) {}\n",
            "if (a<b && c>d) {}\n",
            "while (i <len) i++;\n",
            "x = a <b ? 1 : 2;\n",
            "el.innerHTML = '<div class=\"x\">';\n",
            "const s = \"<b>bold</b>\";\n",
            "const t = `<p>${x}</p>`;\n",
            "// renders <App /> into #root\n",
            "/** @returns <div> */\n",
            "const re = /<(\\w+)>/;\n",
            "obj.return <b>c;\n",
            "a << 2; b <= c; d <!-- e\n",
            "const ref = useRef<HTMLDivElement>(null);\n",
            "const id = <T,>(x) => x;\n",
            "",
            "<",
            "x = <div",
        ] {
            assert!(!contains_jsx(code), "{code:?}");
        }
    }
}
