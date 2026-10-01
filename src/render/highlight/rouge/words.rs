//! Word-level classification ported from Rouge 5.1.0 lexers (MIT, rouge-ruby/rouge).
//!
//! Rouge's regex lexers decide most identifier classes from their own keyword tables and a few
//! look-arounds (`id(?=\s*\()` → `Name::Function`, `id::` → `Name::Namespace`, catch-all
//! `Name`), while TextMate grammars scope by syntax. Where both agree on *what* a token is but
//! not on its class, [`Words::refine`] re-classifies identifier-like text the way the Rouge
//! lexer would ([`Variant::Improved`](super::classes::Variant::Improved) only). The tables are
//! copied verbatim from the lexers' `keywords`/`builtins`/… sets (sorted for binary search);
//! the per-language rule lists mirror the order of the lexers' rules.

use super::Segment;

#[cfg(test)]
#[path = "words_tests.rs"]
mod tests;

/// A condition on one identifier (all given parts must hold).
#[derive(Clone, Copy, Debug)]
pub(super) struct When {
    /// The identifier is in this (sorted) table.
    words: Option<&'static [&'static str]>,
    /// The next non-blank text on the line starts with this.
    before: Option<&'static str>,
    /// The character right after the identifier is none of these.
    not_followed_by: &'static [char],
    /// Whether the identifier directly follows a `.`.
    after_dot: Option<bool>,
    /// The identifier starts with an ASCII uppercase letter.
    capitalized: bool,
    /// The identifier has no lowercase letters (a constant name).
    all_caps: bool,
    /// The next non-blank text does not start with this.
    unless_before: Option<&'static str>,
}

impl When {
    pub(super) const fn any() -> When {
        When {
            words: None,
            before: None,
            not_followed_by: &[],
            after_dot: None,
            capitalized: false,
            all_caps: false,
            unless_before: None,
        }
    }

    pub(super) const fn all_caps(mut self) -> When {
        self.capitalized = true;
        self.all_caps = true;
        self
    }

    pub(super) const fn unless_before(mut self, text: &'static str) -> When {
        self.unless_before = Some(text);
        self
    }

    pub(super) const fn word_in(words: &'static [&'static str]) -> When {
        When { words: Some(words), ..When::any() }
    }

    pub(super) const fn before(text: &'static str) -> When {
        When { before: Some(text), ..When::any() }
    }

    pub(super) const fn not_followed_by(mut self, chars: &'static [char]) -> When {
        self.not_followed_by = chars;
        self
    }

    pub(super) const fn not_after_dot(mut self) -> When {
        self.after_dot = Some(false);
        self
    }

    pub(super) const fn after_dot(mut self) -> When {
        self.after_dot = Some(true);
        self
    }

    pub(super) const fn capitalized(mut self) -> When {
        self.capitalized = true;
        self
    }

    /// `next` is the text right after the identifier.
    fn holds(&self, word: &str, next: &str, after_dot: bool) -> bool {
        let ahead = next.trim_start_matches([' ', '\t']);
        self.words.is_none_or(|w| w.binary_search(&word).is_ok())
            && self.before.is_none_or(|b| ahead.starts_with(b))
            && self.unless_before.is_none_or(|b| !ahead.starts_with(b))
            && !next.starts_with(self.not_followed_by)
            && self.after_dot.is_none_or(|a| a == after_dot)
            && (!self.capitalized || word.starts_with(|c: char| c.is_ascii_uppercase()))
            && (!self.all_caps || !word.chars().any(char::is_lowercase))
    }
}

/// How one Rouge lexer classifies words.
#[derive(Debug)]
pub(super) struct Words {
    /// Ordered rules; the first that holds decides.
    pub(super) rules: &'static [(When, &'static str)],
    /// Class of an identifier that neither a rule nor a TextMate scope classifies.
    pub(super) bare: Option<&'static str>,
    /// Classes whose span also takes a directly preceding `.` (Rouge's `[.]\s*id` rules).
    pub(super) dot_joins: &'static [&'static str],
}

/// Classes that come from tokens Rouge never re-reads as words: strings, comments, numbers,
/// markup, definitions (`def f` → `nf`), tags and attributes, namespaces, decorators.
fn protected(class: &str) -> bool {
    matches!(class.as_bytes().first(), Some(b's' | b'c' | b'm' | b'l' | b'g' | b'e'))
        || matches!(
            class,
            "nf" | "nc" | "nd" | "nt" | "na" | "nn" | "nl" | "ni" | "py" | "nv" | "kn" | "si"
        )
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Words {
    /// Re-classify identifier runs in `segments` (ranges into `code`).
    pub(super) fn refine(&self, code: &str, segments: Vec<Segment>) -> Vec<Segment> {
        let mut out: Vec<Segment> = Vec::with_capacity(segments.len());
        for seg in segments {
            if seg.class.is_some_and(protected) {
                out.push(seg);
                continue;
            }
            let Some(text) = code.get(seg.range.clone()) else {
                out.push(seg);
                continue;
            };
            let base = seg.range.start;
            let mut plain_start = 0;
            let mut iter = text.char_indices().peekable();
            while let Some((i, c)) = iter.next() {
                let prev_is_word = text[..i].chars().next_back().is_some_and(is_ident_continue);
                if !is_ident_start(c) || prev_is_word {
                    continue;
                }
                let mut end = i + c.len_utf8();
                while let Some(&(j, d)) = iter.peek() {
                    if !is_ident_continue(d) {
                        break;
                    }
                    end = j + d.len_utf8();
                    iter.next();
                }
                let word = &text[i..end];
                let next = &code[base + end..];
                let after_dot = code[..base + i].ends_with('.');
                let class = self
                    .rules
                    .iter()
                    .find(|(when, _)| when.holds(word, next, after_dot))
                    .map(|&(_, class)| Some(class))
                    .unwrap_or(match seg.class {
                        None => self.bare,
                        class => class,
                    });
                if plain_start < i {
                    out.push(Segment::new(seg.class, base + plain_start..base + i));
                }
                out.push(Segment::new(class, base + i..base + end));
                plain_start = end;
            }
            if plain_start < text.len() {
                out.push(Segment::new(seg.class, base + plain_start..base + text.len()));
            }
        }
        if !self.dot_joins.is_empty() {
            for k in 1..out.len() {
                let joins = out[k].class.is_some_and(|c| self.dot_joins.contains(&c));
                let dot = &out[k - 1];
                if joins && dot.range.end == out[k].range.start && code.get(dot.range.clone()) == Some(".") {
                    out[k - 1].class = out[k].class;
                }
            }
        }
        out
    }
}

/// The word rules for a Rouge tag, if ported.
pub(super) fn for_tag(tag: &str) -> Option<&'static Words> {
    Some(match tag {
        "rust" => &RUST,
        "python" => &PYTHON,
        "javascript" => &JAVASCRIPT,
        "typescript" => &TYPESCRIPT,
        "c" => &C,
        "cpp" => &CPP,
        "go" => &GO,
        "shell" => &SHELL,
        "java" => &JAVA,
        "sql" => &SQL,
        _ => return None,
    })
}

/// `Rouge::Lexers::Shell` `:basic`: `KEYWORDS` → `k`, `BUILTINS` (not followed by `.`/`-`)
/// → `nb`; other words are `Text`.
static SHELL: Words = Words {
    rules: &[
        (When::word_in(SHELL_KEYWORDS), "k"),
        (When::word_in(SHELL_BUILTINS).not_followed_by(&['.', '-']), "nb"),
    ],
    bare: None,
    dot_joins: &[],
};

const SHELL_KEYWORDS: &[&str] = &[
    "continue", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in",
    "return", "select", "then", "until", "while",
];

const SHELL_BUILTINS: &[&str] = &[
    "alias", "arch", "awk", "b2sum", "base32", "base64", "basename", "bg", "bind", "break",
    "builtin", "caller", "cat", "cd", "chcon", "chgrp", "chmod", "chown", "chroot", "cksum",
    "comm", "command", "compgen", "complete", "cp", "csplit", "cut", "date", "dd", "declare", "df",
    "dir", "dircolors", "dirname", "dirs", "disown", "du", "echo", "enable", "env", "eval", "exec",
    "exit", "expand", "export", "expr", "factor", "false", "fc", "fg", "fmt", "fold", "getopts",
    "grep", "groups", "gunzip", "gzip", "hash", "head", "help", "history", "hostid", "hostname",
    "id", "install", "jobs", "join", "kill", "let", "link", "ln", "local", "logname", "logout",
    "ls", "mapfile", "md5sum", "mkdir", "mkfifo", "mknod", "mktemp", "mv", "nice", "nl", "nohup",
    "nproc", "numfmt", "od", "paste", "pathchk", "popd", "pr", "printenv", "printf", "ptx",
    "pushd", "pwd", "read", "readlink", "readonly", "realpath", "rm", "rmdir", "runcon", "sed",
    "seq", "set", "sha1sum", "sha224sum", "sha256sum", "sha384sum", "sha512sum", "shift", "shopt",
    "shred", "shuf", "sleep", "sort", "source", "split", "stat", "stdbuf", "stty", "sudo", "sum",
    "suspend", "sync", "tac", "tail", "tar", "tee", "test", "time", "timeout", "times", "touch",
    "tr", "trap", "true", "truncate", "tsort", "tty", "type", "typeset", "ulimit", "umask",
    "unalias", "uname", "unexpand", "uniq", "unlink", "unset", "uptime", "users", "vdir", "wait",
    "wc", "who", "whoami", "yes",
];

/// `Rouge::Lexers::Rust` root: keywords, `[=-]>`, `([.]\s*)?id(?=\s*[(])` → `nf`,
/// `[.]\s*id` → `py`, `id::` → `nn`, `id!` → `nd`, builtins → `nb`, else `n`.
static RUST: Words = Words {
    rules: &[
        (When::word_in(RUST_KEYWORDS), "k"),
        (When::before("("), "nf"),
        (When::any().after_dot(), "py"),
        (When::before("!"), "nd"),
        (When::before("::"), "nn"),
        (When::word_in(RUST_BUILTINS), "nb"),
    ],
    bare: Some("n"),
    dot_joins: &["nf", "py"],
};

/// `Rouge::Lexers::Python`: after a `.`: `Capitalized(` → `nc`, `id(` → `nf`, else `n`;
/// otherwise keywords → `k`, exceptions → `ne`, builtins → `nb`, pseudo-builtins → `bp`,
/// else `n`.
static PYTHON: Words = Words {
    rules: &[
        (When::before("(").after_dot().capitalized(), "nc"),
        (When::before("(").after_dot(), "nf"),
        (When::any().after_dot(), "n"),
        (When::word_in(PYTHON_KEYWORDS).not_after_dot(), "k"),
        (When::word_in(PYTHON_EXCEPTIONS).not_after_dot(), "ne"),
        (When::word_in(PYTHON_BUILTINS).not_after_dot(), "nb"),
        (When::word_in(PYTHON_BUILTINS_PSEUDO).not_after_dot(), "bp"),
    ],
    bare: Some("n"),
    dot_joins: &[],
};

/// `Rouge::Lexers::Javascript` statement rules: `Capitalized(` → `nc`, `id(` → keyword or
/// `nf`, then keywords `k`, declarations `kd`, reserved `kr`, constants `kc`, builtins `nb`,
/// else `nx` (`Name.Other`).
static JAVASCRIPT: Words = Words {
    rules: &[
        (When::before("(").capitalized(), "nc"),
        (When::word_in(JS_KEYWORDS), "k"),
        (When::before("("), "nf"),
        (When::word_in(JS_DECLARATIONS), "kd"),
        (When::word_in(JS_RESERVED), "kr"),
        (When::word_in(JS_CONSTANTS), "kc"),
        (When::word_in(JS_BUILTINS), "nb"),
    ],
    bare: Some("nx"),
    dot_joins: &[],
};

/// `Rouge::Lexers::Typescript` (the JavaScript rules with TypeScript's tables).
static TYPESCRIPT: Words = Words {
    rules: &[
        (When::before("(").capitalized(), "nc"),
        (When::word_in(TS_KEYWORDS), "k"),
        (When::before("("), "nf"),
        (When::word_in(TS_DECLARATIONS), "kd"),
        (When::word_in(TS_RESERVED), "kr"),
        (When::word_in(TS_CONSTANTS), "kc"),
        (When::word_in(TS_BUILTINS), "nb"),
    ],
    bare: Some("nx"),
    dot_joins: &[],
};

/// `Rouge::Lexers::C` statements: `true|false|NULL|nullptr` → `nb`, keywords `k`, types `kt`,
/// reserved `kr`, else `n`.
static C: Words = Words {
    rules: &[
        (When::before(":").unless_before("::"), "nl"),
        (When::word_in(C_BUILTIN_CONSTANTS), "nb"),
        (When::word_in(C_KEYWORDS), "k"),
        (When::word_in(C_KEYWORDS_TYPE), "kt"),
        (When::word_in(C_RESERVED), "kr"),
    ],
    bare: Some("n"),
    dot_joins: &[],
};

/// `Rouge::Lexers::Cpp` (the C rules with C++'s tables, plus its `class`/`template` rules).
static CPP: Words = Words {
    rules: &[
        (When::before(":").unless_before("::"), "nl"),
        (When::word_in(C_BUILTIN_CONSTANTS), "nb"),
        (When::word_in(CPP_KEYWORDS), "k"),
        (When::word_in(CPP_EXTRA_KEYWORDS), "k"),
        (When::word_in(CPP_KEYWORDS_TYPE), "kt"),
        (When::word_in(CPP_RESERVED), "kr"),
    ],
    bare: Some("n"),
    dot_joins: &[],
};

const CPP_EXTRA_KEYWORDS: &[&str] = &["class", "template"];

/// `Rouge::Lexers::SQL`: words the grammar leaves unscoped are names.
static SQL: Words = Words { rules: &[], bare: Some("n"), dot_joins: &[] };

/// `Rouge::Lexers::Java` root: keywords `k`, declarations `kd`, types `kt`, constants `kc`,
/// `.id` → `na`, `id:` → `nl`, `CONST` → `no`, `Class` → `nc`, else `n`.
static JAVA: Words = Words {
    rules: &[
        (When::word_in(JAVA_KEYWORDS), "k"),
        (When::word_in(JAVA_DECLARATIONS), "kd"),
        (When::word_in(JAVA_TYPES), "kt"),
        (When::word_in(&["false", "null", "true"]), "kc"),
        (When::word_in(&["class", "enum", "interface", "record"]), "kd"),
        (When::any().after_dot(), "na"),
        (When::before(":").unless_before("::"), "nl"),
        (When::any().all_caps(), "no"),
        (When::any().capitalized(), "nc"),
    ],
    bare: Some("n"),
    dot_joins: &[],
};

const JAVA_KEYWORDS: &[&str] = &[
    "assert", "break", "case", "catch", "continue", "default", "do", "else", "finally", "for",
    "goto", "if", "instanceof", "new", "return", "switch", "this", "throw", "try", "when", "while",
    "yield",
];

const JAVA_DECLARATIONS: &[&str] = &[
    "abstract", "const", "extends", "final", "implements", "native", "permits", "private",
    "protected", "public", "sealed", "static", "strictfp", "super", "synchronized", "throws",
    "transient", "volatile",
];

const JAVA_TYPES: &[&str] =
    &["boolean", "byte", "char", "double", "float", "int", "long", "short", "var", "void"];

/// `Rouge::Lexers::Go`: `KEYWORD` → `k`, `PREDECLARED_TYPES` → `kt`, `PREDECLARED_FUNCTIONS`
/// → `nb`, `PREDECLARED_CONSTANTS` → `no`, else `n`.
static GO: Words = Words {
    rules: &[
        (When::word_in(GO_KEYWORDS), "k"),
        (When::word_in(GO_TYPES), "kt"),
        (When::word_in(GO_FUNCTIONS), "nb"),
        (When::word_in(GO_CONSTANTS), "no"),
    ],
    bare: Some("n"),
    dot_joins: &[],
};

const C_BUILTIN_CONSTANTS: &[&str] = &["NULL", "false", "nullptr", "true"];

const GO_KEYWORDS: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for",
    "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return",
    "select", "struct", "switch", "type", "var",
];

const GO_TYPES: &[&str] = &[
    "bool", "byte", "complex128", "complex64", "error", "float32", "float64", "int", "int16",
    "int32", "int64", "int8", "rune", "string", "uint", "uint16", "uint32", "uint64", "uint8",
    "uintptr",
];

const GO_FUNCTIONS: &[&str] = &[
    "append", "cap", "close", "complex", "copy", "delete", "imag", "len", "make", "new", "panic",
    "print", "println", "real", "recover",
];

const GO_CONSTANTS: &[&str] = &["false", "iota", "nil", "true"];

// ---- Tables generated from Rouge 5.1.0 (`Lexer.keywords` etc., sorted) ----

pub(super) const RUST_KEYWORDS: &[&str] = &[
    "Self", "abstract", "as", "async", "await", "become", "box", "break", "const", "continue",
    "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "if", "impl",
    "in", "let", "log", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub",
    "ref", "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "union", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

pub(super) const RUST_BUILTINS: &[&str] = &[
    "Add", "Arc", "AsMut", "AsRef", "BitAnd", "BitOr", "BitXor", "Box", "Clone", "Copy", "DIR",
    "Default", "Div", "DoubleEndedIterator", "Drop", "Eq", "Err", "ExactSizeIterator", "Extend",
    "FILE", "Fn", "FnMut", "FnOnce", "From", "FromIterator", "Index", "Into", "IntoIterator",
    "Iterator", "Mul", "Neg", "None", "Ok", "Option", "Ord", "Owned", "PartialEq", "PartialOrd",
    "Pin", "Rc", "Result", "Send", "Shl", "Shr", "Sized", "Some", "String", "Sub", "Sync",
    "ToOwned", "ToString", "TryFrom", "TryInto", "Unpin", "Vec", "bool", "c_char", "c_double",
    "c_float", "c_int", "c_long", "c_longlong", "c_schar", "c_short", "c_uchar", "c_uint",
    "c_ulong", "c_ulonglong", "c_ushort", "c_void", "char", "clock_t", "dev_t", "dirent", "drop",
    "f32", "f64", "float", "fpos_t", "i128", "i16", "i32", "i64", "i8", "ino_t", "int", "intptr_t",
    "isize", "mode_t", "off_t", "pid_t", "ptrdiff_t", "size_t", "ssize_t", "str", "time_t", "u128",
    "u16", "u32", "u64", "u8", "uint", "uintptr_t", "usize",
];

pub(super) const PYTHON_KEYWORDS: &[&str] = &[
    "as", "assert", "async", "await", "break", "continue", "del", "elif", "else", "except", "exec",
    "finally", "for", "from", "global", "if", "import", "lambda", "nonlocal", "pass", "print",
    "raise", "return", "try", "while", "with", "yield",
];

pub(super) const PYTHON_BUILTINS: &[&str] = &[
    "__import__", "abs", "aiter", "all", "anext", "any", "apply", "ascii", "basestring", "bin",
    "bool", "breakpoint", "buffer", "bytearray", "bytes", "callable", "chr", "classmethod", "cmp",
    "coerce", "compile", "complex", "delattr", "dict", "dir", "divmod", "enumerate", "eval",
    "exec", "execfile", "exit", "file", "filter", "float", "format", "frozenset", "getattr",
    "globals", "hasattr", "hash", "help", "hex", "id", "input", "int", "intern", "isinstance",
    "issubclass", "iter", "len", "list", "locals", "long", "map", "max", "memoryview", "min",
    "next", "object", "oct", "open", "ord", "pow", "print", "property", "range", "raw_input",
    "reduce", "reload", "repr", "reversed", "round", "set", "setattr", "slice", "sorted",
    "staticmethod", "str", "sum", "super", "tuple", "type", "unichr", "unicode", "vars", "xrange",
    "zip",
];

pub(super) const PYTHON_BUILTINS_PSEUDO: &[&str] = &[
    "Ellipsis", "False", "None", "NotImplemented", "True",
];

pub(super) const PYTHON_EXCEPTIONS: &[&str] = &[
    "ArithmeticError", "AssertionError", "AttributeError", "BaseException", "BaseExceptionGroup",
    "BlockingIOError", "BrokenPipeError", "BufferError", "BytesWarning", "ChildProcessError",
    "ConnectionAbortedError", "ConnectionError", "ConnectionRefusedError", "ConnectionResetError",
    "DeprecationWarning", "EOFError", "EncodingWarning", "EnvironmentError", "Exception",
    "ExceptionGroup", "FileExistsError", "FileNotFoundError", "FloatingPointError",
    "FutureWarning", "GeneratorExit", "IOError", "ImportError", "ImportWarning",
    "IndentationError", "IndexError", "InterruptedError", "IsADirectoryError", "KeyError",
    "KeyboardInterrupt", "LookupError", "MemoryError", "ModuleNotFoundError", "NameError",
    "NotADirectoryError", "NotImplemented", "NotImplementedError", "OSError", "OverflowError",
    "OverflowWarning", "PendingDeprecationWarning", "PermissionError", "ProcessLookupError",
    "PythonFinalizationError", "RecursionError", "ReferenceError", "ResourceWarning",
    "RuntimeError", "RuntimeWarning", "StandardError", "StopAsyncIteration", "StopIteration",
    "SyntaxError", "SyntaxWarning", "SystemError", "SystemExit", "TabError", "TimeoutError",
    "TypeError", "UnboundLocalError", "UnicodeDecodeError", "UnicodeEncodeError", "UnicodeError",
    "UnicodeTranslateError", "UnicodeWarning", "UserWarning", "VMSError", "ValueError", "Warning",
    "WindowsError", "ZeroDivisionError",
];

pub(super) const JS_KEYWORDS: &[&str] = &[
    "async", "await", "break", "case", "catch", "continue", "debugger", "default", "delete", "do",
    "else", "export", "finally", "for", "from", "if", "import", "in", "instanceof", "new", "of",
    "return", "super", "switch", "this", "throw", "try", "typeof", "void", "while", "yield",
];

pub(super) const JS_DECLARATIONS: &[&str] = &[
    "class", "const", "constructor", "extends", "function", "get", "let", "set", "static", "var",
    "with",
];

pub(super) const JS_RESERVED: &[&str] = &[
    "enum", "implements", "interface", "package", "private", "protected", "public",
];

pub(super) const JS_CONSTANTS: &[&str] = &[
    "Infinity", "NaN", "false", "null", "true", "undefined",
];

pub(super) const JS_BUILTINS: &[&str] = &[
    "Array", "ArrayBuffer", "Boolean", "DataView", "Date", "Error", "Float32Array", "Float64Array",
    "Function", "Int16Array", "Int32Array", "Int8Array", "Map", "Math", "Number", "Object",
    "Packages", "Promise", "Proxy", "Reflect", "RegExp", "Set", "String", "Symbol", "Uint16Array",
    "Uint16ClampedArray", "Uint32Array", "Uint32ClampedArray", "Uint8Array", "Uint8ClampedArray",
    "WeakMap", "WeakSet", "decodeURI", "decodeURIComponent", "document", "encodeURI",
    "encodeURIComponent", "eval", "global", "isFinite", "isNaN", "navigator", "netscape",
    "parseFloat", "parseInt", "self", "sun", "window",
];

pub(super) const TS_KEYWORDS: &[&str] = &[
    "async", "await", "break", "case", "catch", "continue", "debugger", "default", "delete", "do",
    "else", "export", "finally", "for", "from", "if", "implements", "import", "in", "instanceof",
    "is", "namespace", "new", "of", "private", "protected", "public", "readonly", "return",
    "static", "super", "switch", "this", "throw", "try", "typeof", "void", "while", "yield",
];

pub(super) const TS_DECLARATIONS: &[&str] = &[
    "abstract", "class", "const", "constructor", "extends", "function", "get", "let", "set",
    "static", "type", "var", "with",
];

pub(super) const TS_RESERVED: &[&str] = &[
    "any", "declare", "default", "enum", "implements", "interface", "keyof", "module", "namespace",
    "number", "package", "private", "protected", "public", "string", "void",
];

pub(super) const TS_CONSTANTS: &[&str] = &[
    "Infinity", "NaN", "false", "null", "true", "undefined",
];

pub(super) const TS_BUILTINS: &[&str] = &[
    "Array", "ArrayBuffer", "Boolean", "Capitalize", "ConstructorParameters", "DataView", "Date",
    "Error", "Exclude", "Extract", "Float32Array", "Float64Array", "Function", "InstanceType",
    "Int16Array", "Int32Array", "Int8Array", "Lowercase", "Map", "Math", "NonNullable", "Number",
    "Object", "Omit", "OmitThisParameter", "Packages", "Parameters", "Partial", "Pick", "Promise",
    "Proxy", "Readonly", "Record", "Reflect", "RegExp", "Required", "ReturnType", "Set", "String",
    "Symbol", "ThisParameterType", "ThisType", "Uint16Array", "Uint16ClampedArray", "Uint32Array",
    "Uint32ClampedArray", "Uint8Array", "Uint8ClampedArray", "Uncapitalize", "Uppercase",
    "WeakMap", "WeakSet", "decodeURI", "decodeURIComponent", "document", "encodeURI",
    "encodeURIComponent", "eval", "global", "isFinite", "isNaN", "navigator", "netscape",
    "parseFloat", "parseInt", "self", "sun", "window",
];

pub(super) const C_KEYWORDS: &[&str] = &[
    "_Alignas", "_Alignof", "_Atomic", "_Generic", "_Imaginary", "_Noreturn", "_Static_assert",
    "_Thread_local", "alignas", "alignof", "auto", "break", "case", "const", "constexpr",
    "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if", "register",
    "return", "sizeof", "static", "static_assert", "struct", "switch", "typedef", "typeof",
    "typeof_unqual", "union", "volatile", "while",
];

pub(super) const C_KEYWORDS_TYPE: &[&str] = &[
    "DIR", "FILE", "_BitInt", "_Bool", "_Complex", "_Decimal128", "_Decimal32", "_Decimal64",
    "bool", "char", "char16_t", "char32_t", "clock_t", "div_t", "double", "float", "fpos_t", "int",
    "int16_t", "int32_t", "int64_t", "int8_t", "int_fast16_t", "int_fast32_t", "int_fast64_t",
    "int_fast8_t", "int_least16_t", "int_least32_t", "int_least64_t", "int_least8_t", "intmax_t",
    "intptr_t", "jmp_buf", "ldiv_t", "long", "mbstate_t", "nullptr_t", "off_t", "ptrdiff_t",
    "short", "sig_atomic_t", "signed", "size_t", "ssize_t", "time_t", "uint16_t", "uint32_t",
    "uint64_t", "uint8_t", "uint_fast16_t", "uint_fast32_t", "uint_fast64_t", "uint_fast8_t",
    "uint_least16_t", "uint_least32_t", "uint_least64_t", "uint_least8_t", "uintmax_t",
    "uintptr_t", "unsigned", "va_list", "void", "wchar_t", "wctrans_t", "wctype_t", "wint_t",
];

pub(super) const C_RESERVED: &[&str] = &[
    "__asm", "__based", "__cdecl", "__declspec", "__except", "__fastcall", "__finally", "__inline",
    "__int16", "__int32", "__int61", "__int8", "__leave", "__naked", "__restrict", "__stdcall",
    "__thread", "__try", "__typename", "_inline", "_naked", "_restrict", "_thread", "_typename",
    "inline", "naked", "restrict", "thread", "thread_local", "typename",
];

pub(super) const CPP_KEYWORDS: &[&str] = &[
    "_Alignas", "_Alignof", "_Atomic", "_Generic", "_Imaginary", "_Noreturn", "_Static_assert",
    "_Thread_local", "alignas", "alignof", "and", "and_eq", "asm", "auto", "bitand", "bitor",
    "break", "case", "catch", "co_await", "co_return", "co_yield", "compl", "concept", "const",
    "const_cast", "consteval", "constexpr", "constinit", "continue", "decltype", "default",
    "delete", "do", "dynamic_cast", "else", "enum", "explicit", "export", "extern", "final", "for",
    "friend", "goto", "if", "import", "module", "mutable", "namespace", "new", "noexcept", "not",
    "not_eq", "operator", "or", "or_eq", "override", "private", "protected", "public", "register",
    "reinterpret_cast", "requires", "return", "size_of", "sizeof", "static", "static_assert",
    "static_cast", "struct", "switch", "this", "throw", "throws", "try", "typedef", "typeid",
    "typename", "typeof", "typeof_unqual", "union", "using", "virtual", "volatile", "while", "xor",
    "xor_eq",
];

pub(super) const CPP_KEYWORDS_TYPE: &[&str] = &[
    "DIR", "FILE", "_BitInt", "_Bool", "_Complex", "_Decimal128", "_Decimal32", "_Decimal64",
    "bool", "char", "char16_t", "char32_t", "char8_t", "clock_t", "div_t", "double", "float",
    "fpos_t", "int", "int16_t", "int32_t", "int64_t", "int8_t", "int_fast16_t", "int_fast32_t",
    "int_fast64_t", "int_fast8_t", "int_least16_t", "int_least32_t", "int_least64_t",
    "int_least8_t", "intmax_t", "intptr_t", "jmp_buf", "ldiv_t", "long", "mbstate_t", "nullptr_t",
    "off_t", "ptrdiff_t", "short", "sig_atomic_t", "signed", "size_t", "ssize_t", "time_t",
    "uint16_t", "uint32_t", "uint64_t", "uint8_t", "uint_fast16_t", "uint_fast32_t",
    "uint_fast64_t", "uint_fast8_t", "uint_least16_t", "uint_least32_t", "uint_least64_t",
    "uint_least8_t", "uintmax_t", "uintptr_t", "unsigned", "va_list", "void", "wchar_t",
    "wctrans_t", "wctype_t", "wint_t",
];

pub(super) const CPP_RESERVED: &[&str] = &[
    "__asm", "__based", "__cdecl", "__declspec", "__event", "__except", "__fastcall", "__finally",
    "__inline", "__int16", "__int32", "__int61", "__int8", "__interface", "__leave",
    "__multiple_inheritance", "__naked", "__restrict", "__single_inheritance", "__stdcall",
    "__super", "__thread", "__try", "__typename", "__uuidof", "__virtual_inheritance", "_inline",
    "_naked", "_restrict", "_thread", "_typename", "inline", "naked", "restrict", "thread",
    "thread_local", "typename",
];

