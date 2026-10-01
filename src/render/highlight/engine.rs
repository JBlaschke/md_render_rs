//! Shared tokenizer: syntect 5.3 (`parsing` + `regex-fancy`) over the two-face 0.5.2 syntax set
//! (D5, CR §2).
//!
//! The syntax set is loaded once (`OnceLock`) and shared by all threads (`SyntaxSet: Send + Sync`;
//! syntect compiles each grammar's regexes lazily and thread-safely on first use).
//!
//! [`tokenize`] drives `ParseState` + `ScopeStack` line by line (CR §2 "Tokenising API") and
//! reports a flat stream of [`Event`]s: basic scope pushes/pops (`ScopeStack::apply_with_hook`
//! turns syntect's `Clear`/`Restore` ops into plain pops and pushes) and text segments. Text
//! segments never span lines; the line terminator is reported as part of the last segment of a
//! line, exactly as syntect's `*_newlines` grammars see it.
//!
//! Work is bounded ([`Limits`]) and every failure mode of the parser (a `ParsingError`, a
//! `ScopeError`, or a panic inside syntect) is reported as [`Error`] so that callers can fall
//! back to unhighlighted output. Nothing in here panics on any input.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use syntect::parsing::{
    BasicScopeStackOp, ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet,
};

/// The two-face "extra" syntax set with newline-terminated lines (CR §2).
pub(crate) fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// Look up a syntax by its exact top-level scope name (e.g. `source.rust`).
pub(crate) fn syntax_by_scope(scope: &str) -> Option<&'static SyntaxReference> {
    let scope = Scope::new(scope).ok()?;
    syntaxes().find_syntax_by_scope(scope)
}

/// Upper bounds on the work done for one code block. Inputs beyond them are not tokenized (the
/// caller emits plain escaped text). GitHub and GitLab do not document their own limits for
/// fenced code; these values keep a pathological block well under the render budget
/// (docs/DESIGN.md §3.2) while covering every realistic README block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    /// Maximum size of the whole block in bytes.
    pub max_bytes: usize,
    /// Maximum length of a single line in bytes (syntect's regex scanning is super-linear in the
    /// line length for some grammars).
    pub max_line_bytes: usize,
    /// Maximum number of lines.
    pub max_lines: usize,
}

impl Limits {
    pub(crate) const DEFAULT: Limits = Limits {
        max_bytes: 512 * 1024,
        max_line_bytes: 8 * 1024,
        max_lines: 20_000,
    };

    /// Whether `code` is small enough to be tokenized.
    pub(crate) fn admits(&self, code: &str) -> bool {
        if code.len() > self.max_bytes {
            return false;
        }
        let mut lines = 0usize;
        for line in code.split('\n') {
            lines += 1;
            if lines > self.max_lines || line.len() > self.max_line_bytes {
                return false;
            }
        }
        true
    }
}

/// One tokenizer event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event<'a> {
    /// A scope was pushed; the stack passed alongside already contains it (as its last entry).
    Push(Scope),
    /// The innermost scope was popped; the stack passed alongside no longer contains it.
    Pop,
    /// Text covered by the current stack. Never contains a `\n` except as its last byte.
    Text(&'a str),
}

/// The tokenizer failed; the caller must fall back to unhighlighted output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Error;

/// Tokenize `code` with `syntax`, calling `sink(event, stack)` for every event, where `stack` is
/// the scope stack after the event was applied.
///
/// Every line is fed to syntect with its terminator (`\n`; the last line without one is fed as
/// is). Input that exceeds `limits` is rejected up front with [`Error`] before any event is
/// emitted. If parsing fails half-way, some events may already have been delivered; callers
/// buffer their output and discard it on error.
pub(crate) fn tokenize<'c>(
    code: &'c str,
    syntax: &SyntaxReference,
    limits: Limits,
    mut sink: impl FnMut(Event<'c>, &[Scope]),
) -> Result<(), Error> {
    if !limits.admits(code) {
        return Err(Error);
    }
    // syntect has a few internal `unwrap`s (lazy regex compilation, context resolution); a
    // panic there must not take down the render, so it is contained and reported as an error.
    catch_unwind(AssertUnwindSafe(|| tokenize_inner(code, syntax, &mut sink)))
        .unwrap_or(Err(Error))
}

fn tokenize_inner<'c>(
    code: &'c str,
    syntax: &SyntaxReference,
    sink: &mut impl FnMut(Event<'c>, &[Scope]),
) -> Result<(), Error> {
    let set = syntaxes();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    for line in code.split_inclusive('\n') {
        let ops = state.parse_line(line, set).map_err(|_| Error)?;
        let mut pos = 0usize;
        for (offset, op) in &ops {
            // Offsets are byte offsets into `line` and lie on char boundaries; be defensive
            // anyway, a bad offset must never slice mid-character.
            let offset = (*offset).min(line.len());
            if offset > pos {
                let text = line.get(pos..offset).ok_or(Error)?;
                sink(Event::Text(text), stack.as_slice());
                pos = offset;
            }
            // The hook sees the stack after each basic op (for `Clear`, after the whole clear).
            stack
                .apply_with_hook(op, |basic, scopes| match basic {
                    BasicScopeStackOp::Push(scope) => sink(Event::Push(scope), scopes),
                    BasicScopeStackOp::Pop => sink(Event::Pop, scopes),
                })
                .map_err(|_| Error)?;
        }
        if pos < line.len() {
            let text = line.get(pos..).ok_or(Error)?;
            sink(Event::Text(text), stack.as_slice());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust() -> &'static SyntaxReference {
        syntax_by_scope("source.rust").expect("two-face has Rust")
    }

    #[test]
    fn syntax_set_loads_and_has_core_grammars() {
        for scope in [
            "source.rust",
            "source.python",
            "source.js",
            "source.shell.bash",
            "source.diff",
            "text.html.basic",
        ] {
            assert!(syntax_by_scope(scope).is_some(), "{scope}");
        }
        assert!(syntax_by_scope("not a scope at all").is_none());
    }

    #[test]
    fn events_reconstruct_text_and_balance() {
        let code = "fn main() {\n    let x = \"a\\n\"; // c\n}\n";
        let mut text = String::new();
        let mut depth = 0i64;
        tokenize(code, rust(), Limits::DEFAULT, |e, stack| match e {
            Event::Push(s) => {
                depth += 1;
                assert_eq!(stack.last(), Some(&s));
            }
            Event::Pop => depth -= 1,
            Event::Text(t) => {
                assert!(!t[..t.len() - 1].contains('\n'));
                text.push_str(t);
            }
        })
        .unwrap();
        assert_eq!(text, code);
        assert!(depth >= 1, "root scope stays pushed");
    }

    #[test]
    fn limits_reject_oversized_input() {
        let long = "x".repeat(Limits::DEFAULT.max_line_bytes + 1);
        assert_eq!(tokenize(&long, rust(), Limits::DEFAULT, |_, _| {}), Err(Error));
        let many = "x\n".repeat(Limits::DEFAULT.max_lines + 1);
        assert_eq!(tokenize(&many, rust(), Limits::DEFAULT, |_, _| {}), Err(Error));
        assert!(Limits::DEFAULT.admits(""));
    }
}
