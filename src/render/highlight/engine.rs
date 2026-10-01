//! Shared tokenizer: syntect 5.3 (`parsing` + `regex-fancy`) over the two-face 0.5.2 syntax set
//! (D5, CR §2).
//!
//! The syntax set is loaded once (`OnceLock`) and shared by all threads (`SyntaxSet: Send + Sync`;
//! syntect compiles each grammar's regexes lazily and thread-safely on first use).
//!
//! [`LineTokenizer`] drives `ParseState` + `ScopeStack` line by line (CR §2 "Tokenising API") and
//! reports a flat stream of [`Event`]s: basic scope pushes/pops (`ScopeStack::apply_with_hook`
//! turns syntect's `Clear`/`Restore` ops into plain pops and pushes) and text segments. Text
//! segments never span lines; the line terminator is reported as part of the last segment of a
//! line, exactly as syntect's `*_newlines` grammars see it. [`tokenize`] runs it over a whole
//! block.
//!
//! Work is bounded ([`Limits`]) and every failure mode of the parser (a `ParsingError`, a
//! `ScopeError`, or a panic inside syntect) is reported as [`Error`] so that callers can fall
//! back to unhighlighted output. Nothing in here panics on any input.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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

/// syntect's own token lookup (file extension first, then syntax name, both ASCII
/// case-insensitive; CR §2 "Language token lookup").
pub(crate) fn syntax_by_token(token: &str) -> Option<&'static SyntaxReference> {
    if token.is_empty() {
        return None;
    }
    syntaxes().find_syntax_by_token(token)
}

/// Look up a syntax by file extension only (ASCII case-insensitive).
pub(crate) fn syntax_by_extension(ext: &str) -> Option<&'static SyntaxReference> {
    if ext.is_empty() {
        return None;
    }
    syntaxes().find_syntax_by_extension(ext)
}

/// Upper bounds on the work done for one code block. Inputs beyond them are not tokenized (the
/// caller emits plain escaped text). GitHub and GitLab do not document size limits for fenced
/// code (both cap highlighting time instead: GitLab's `SyntaxHighlightFilter` runs under
/// `TimeoutFilterHandler`); these values keep a pathological block well under the render
/// budget (docs/DESIGN.md §3.2) while covering every realistic README block.
///
/// Measured (release, two-face grammars): the regexes of several grammars (SCSS, JavaScript,
/// Makefile, shell) are quadratic in the length of a run of whitespace or word characters
/// (SCSS: 100 lines of 600 spaces take 10 s), hence the [`Limits::max_run_cost`] budget; and
/// a few patterns hit fancy-regex's backtracking limit on otherwise harmless lines (JavaScript:
/// ~15 ms per line indented ≥ 16 spaces and starting with an operator), hence the time net.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    /// Maximum size of the whole block in bytes.
    pub max_bytes: usize,
    /// Maximum length of a single line in bytes.
    pub max_line_bytes: usize,
    /// Maximum number of lines.
    pub max_lines: usize,
    /// Maximum of Σ run² over all maximal runs of whitespace or of word characters (the
    /// deterministic estimate of the super-linear regex work).
    pub max_run_cost: u64,
    /// Safety net for grammar pathologies the estimate cannot see: tokenizing stops (and the
    /// block is rendered plain) once this much time has been spent on it. Every input
    /// tokenized within the budget gives the same output on every run; only a block whose
    /// tokenizing time is near the budget can come out differently on different machines.
    pub max_time: Option<Duration>,
}

impl Limits {
    pub(crate) const DEFAULT: Limits = Limits {
        max_bytes: 256 * 1024,
        max_line_bytes: 4 * 1024,
        max_lines: 10_000,
        max_run_cost: 1 << 22,
        // The first use of a grammar includes compiling its regexes (TypeScript: ~60 ms in
        // release, seconds in unoptimized builds), hence the larger budget for debug builds.
        max_time: Some(Duration::from_secs(if cfg!(debug_assertions) { 30 } else { 3 })),
    };

    /// Whether `code` is small enough to be tokenized (the deterministic limits).
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
        run_cost(code) <= self.max_run_cost
    }
}

/// Σ run² over the maximal runs of whitespace bytes and of word bytes (ASCII alphanumerics,
/// `_`, and all non-ASCII bytes).
fn run_cost(code: &str) -> u64 {
    #[derive(PartialEq, Clone, Copy)]
    enum Kind {
        Space,
        Word,
        Other,
    }
    let kind = |b: u8| {
        if b.is_ascii_whitespace() {
            Kind::Space
        } else if b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80 {
            Kind::Word
        } else {
            Kind::Other
        }
    };
    let (mut cost, mut run, mut last) = (0u64, 0u64, Kind::Other);
    for b in code.bytes() {
        let k = kind(b);
        if k == last && k != Kind::Other {
            run += 1;
        } else {
            cost += run * run;
            run = u64::from(k != Kind::Other);
            last = k;
        }
    }
    cost + run * run
}

/// Wall-clock guard for [`Limits::max_time`], checked between lines.
#[derive(Debug)]
pub(crate) struct Deadline(Option<Instant>);

impl Deadline {
    pub(crate) fn start(limits: &Limits) -> Self {
        Deadline(limits.max_time.and_then(|d| Instant::now().checked_add(d)))
    }

    /// `Err` once the budget is spent.
    pub(crate) fn check(&self) -> Result<(), Error> {
        match self.0 {
            Some(end) if Instant::now() >= end => Err(Error),
            _ => Ok(()),
        }
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

/// Run `f`, turning a panic inside syntect into [`Error`].
///
/// syntect has a few internal `unwrap`s (lazy regex compilation, context resolution); a panic
/// there must not take down the render, so it is contained here and reported as an error.
pub(crate) fn guarded<T>(f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(Err(Error))
}

/// Incremental tokenizer for one syntax: feed it one line at a time (with its terminator).
///
/// Callers must run it inside [`guarded`].
pub(crate) struct LineTokenizer {
    syntax: &'static SyntaxReference,
    state: ParseState,
    stack: ScopeStack,
}

impl LineTokenizer {
    pub(crate) fn new(syntax: &'static SyntaxReference) -> Self {
        LineTokenizer { syntax, state: ParseState::new(syntax), stack: ScopeStack::new() }
    }

    /// Forget all state (as if freshly created). Scopes still on the stack are *not* reported as
    /// popped; callers track their own output state.
    pub(crate) fn reset(&mut self) {
        self.state = ParseState::new(self.syntax);
        self.stack = ScopeStack::new();
    }

    /// The current scope stack (outermost first).
    pub(crate) fn stack(&self) -> &[Scope] {
        self.stack.as_slice()
    }

    /// Tokenize one line (`\n` only as its last byte, if at all), calling `sink(event, stack)`
    /// for every event, where `stack` is the scope stack after the event was applied.
    pub(crate) fn feed<'c>(
        &mut self,
        line: &'c str,
        sink: &mut impl FnMut(Event<'c>, &[Scope]),
    ) -> Result<(), Error> {
        let ops = self.state.parse_line(line, syntaxes()).map_err(|_| Error)?;
        let mut pos = 0usize;
        for (offset, op) in &ops {
            // Offsets are byte offsets into `line` and lie on char boundaries; be defensive
            // anyway, a bad offset must never slice mid-character.
            let offset = (*offset).min(line.len());
            if offset > pos {
                let text = line.get(pos..offset).ok_or(Error)?;
                sink(Event::Text(text), self.stack.as_slice());
                pos = offset;
            }
            // The hook sees the stack after each basic op (for `Clear`, after the whole clear).
            self.stack
                .apply_with_hook(op, |basic, scopes| match basic {
                    BasicScopeStackOp::Push(scope) => sink(Event::Push(scope), scopes),
                    BasicScopeStackOp::Pop => sink(Event::Pop, scopes),
                })
                .map_err(|_| Error)?;
        }
        if pos < line.len() {
            let text = line.get(pos..).ok_or(Error)?;
            sink(Event::Text(text), self.stack.as_slice());
        }
        Ok(())
    }
}

/// Tokenize `code` with `syntax`, calling `sink(event, stack)` for every event, where `stack` is
/// the scope stack after the event was applied.
///
/// Every line is fed to syntect with its terminator (`\n`; the last line without one is fed as
/// is). Input that exceeds `limits` is rejected up front with [`Error`] before any event is
/// emitted. If parsing fails half-way, some events may already have been delivered; callers
/// buffer their output and discard it on error.
pub(crate) fn tokenize<'c>(
    code: &'c str,
    syntax: &'static SyntaxReference,
    limits: Limits,
    mut sink: impl FnMut(Event<'c>, &[Scope]),
) -> Result<(), Error> {
    if !limits.admits(code) {
        return Err(Error);
    }
    let deadline = Deadline::start(&limits);
    guarded(|| {
        let mut tokenizer = LineTokenizer::new(syntax);
        for line in code.split_inclusive('\n') {
            deadline.check()?;
            tokenizer.feed(line, &mut sink)?;
        }
        Ok(())
    })
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
        assert!(syntax_by_token("").is_none());
        assert_eq!(syntax_by_token("rs").map(|s| s.name.as_str()), Some("Rust"));
        assert_eq!(syntax_by_extension("py").map(|s| s.name.as_str()), Some("Python"));
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
    fn line_tokenizer_resets() {
        let mut t = LineTokenizer::new(rust());
        let mut texts = Vec::new();
        guarded(|| {
            t.feed("/* open\n", &mut |e, _| {
                if let Event::Text(s) = e {
                    texts.push(s.to_owned())
                }
            })
        })
        .unwrap();
        assert!(t.stack().len() > 1, "inside the block comment");
        t.reset();
        assert!(t.stack().is_empty());
    }

    #[test]
    fn limits_reject_oversized_input() {
        let long = "x".repeat(Limits::DEFAULT.max_line_bytes + 1);
        assert_eq!(tokenize(&long, rust(), Limits::DEFAULT, |_, _| {}), Err(Error));
        let many = "x\n".repeat(Limits::DEFAULT.max_lines + 1);
        assert_eq!(tokenize(&many, rust(), Limits::DEFAULT, |_, _| {}), Err(Error));
        assert!(Limits::DEFAULT.admits(""));
        // 100 lines of 600 spaces: 36M run cost (10 s for SCSS without the budget).
        let spaces = (" ".repeat(600) + "x\n").repeat(100);
        assert!(!Limits::DEFAULT.admits(&spaces));
        // Ordinary code is far below the budget.
        let code = "    let value = compute(items.iter().map(|x| x * 2));\n".repeat(2000);
        assert!(run_cost(&code) < Limits::DEFAULT.max_run_cost / 8, "{}", run_cost(&code));
        assert!(Limits::DEFAULT.admits(&code));
    }

    #[test]
    fn run_cost_counts_runs() {
        assert_eq!(run_cost(""), 0);
        assert_eq!(run_cost("ab  cd"), 4 + 4 + 4);
        assert_eq!(run_cost("a+b"), 1 + 1);
        assert_eq!(run_cost("é"), 4, "non-ASCII bytes are word bytes");
    }

    #[test]
    fn deadline_expires() {
        let limits = Limits { max_time: Some(Duration::ZERO), ..Limits::DEFAULT };
        assert_eq!(Deadline::start(&limits).check(), Err(Error));
        let limits = Limits { max_time: None, ..Limits::DEFAULT };
        assert_eq!(Deadline::start(&limits).check(), Ok(()));
        assert_eq!(tokenize("x\n", rust(), Limits { max_time: Some(Duration::ZERO), ..Limits::DEFAULT }, |_, _| {}), Err(Error));
    }

    #[test]
    fn guarded_contains_panics() {
        let r: Result<(), Error> = guarded(|| panic!("boom"));
        assert_eq!(r, Err(Error));
    }
}
