//! Syntax highlighting for both flavors (owner: highlight). Contract: docs/DESIGN.md §2 D5,
//! §3.3.
//!
//! * Engine (D5, CR §2): syntect 5.3 (`parsing` + `regex-fancy`, pure Rust) over the two-face
//!   0.5.2 syntax set, loaded once and shared by all threads ([`engine`]).
//! * GitHub ([`github`], GH §7): Linguist language lookup, `tm_scope` → syntect grammar,
//!   PrettyLights `pl-*` classes as nested spans ([`pl`]).
//! * GitLab ([`gitlab`], GL §8): Rouge registry lookup, scope → Rouge short classes, one HTML
//!   fragment per line ([`rouge`]).
//!
//! Both entry points are pure, deterministic and never panic: tokenizer failures, pathological
//! input and oversized blocks ([`engine::Limits`]) degrade to escaped text without token spans.

mod engine;
#[cfg(test)]
mod eval;
mod pl;
mod rouge;
mod rules;

/// GitHub code block highlighting result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GithubCode {
    /// Linguist `tm_scope`, e.g. `source.rust`; `None` for an unknown language.
    pub scope: Option<String>,
    /// Escaped code with `pl-*` spans, lines joined by `\n`, no trailing newline; `None` when
    /// there is no grammar.
    pub html: Option<String>,
}

/// Highlight `code` for GitHub; `lang` is the info-string language word as written.
///
/// * Unknown language, or a Linguist language with `tm_scope: none` (`text`, `plaintext`):
///   `scope` and `html` are `None` (GitHub emits a plain `<pre lang>` block).
/// * Known `tm_scope` without a grammar (e.g. PowerShell, Mermaid): `scope` is `Some`, `html`
///   is `None` (GitHub still emits the `highlight-<scope>` wrapper; the caller escapes the code
///   itself, e.g. with [`github_plain`]).
/// * Otherwise `html` is the highlighted code. If tokenizing is refused (oversized input) or
///   fails, it is the escaped code without spans.
pub fn github(code: &str, lang: &str) -> GithubCode {
    pl::highlight_with(code, lang, pl::DEFAULT_VARIANT)
}

/// Code escaped exactly as GitHub escapes highlighted code (`&`, `<`, `>`; quotes literal;
/// CRLF normalized; the final newline dropped), for a `highlight-<scope>` block without a
/// grammar.
pub fn github_plain(code: &str) -> String {
    let mut html = pl::escape_text(&code.replace("\r\n", "\n"));
    if html.ends_with('\n') {
        html.pop();
    }
    html
}

/// GitLab code block highlighting result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitlabCode {
    /// Rouge tag (`rust`, `javascript`, ...) or `plaintext`.
    pub tag: String,
    /// One entry per source line: escaped HTML with Rouge short-class spans.
    pub lines: Vec<String>,
}

/// Highlight `code` for GitLab; `lang` is the info-string language word as written
/// (`pre[data-canonical-lang]`, case-sensitive).
///
/// `tag` is the Rouge lexer tag (`language-<tag>`, `data-lang`), `plaintext` for unknown or
/// missing languages, or the language itself for blocks GitLab never highlights (`math`,
/// `suggestion`, `mermaid` and the other Kroki diagram types). `lines[i]` is the inner HTML of
/// `<span id="LC{i+1}" class="line" data-lang="{tag}">`; a trailing newline does not produce an
/// extra line and empty code has no lines (GL §6.8).
pub fn gitlab(code: &str, lang: &str) -> GitlabCode {
    rouge::highlight_with(code, lang, rouge::DEFAULT_VARIANT)
}

/// Minimal HTML text escaping (`&`, `<`, `>`, `"`).
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_helper() {
        assert_eq!(escape("<a href=\"x\">&'</a>"), "&lt;a href=&quot;x&quot;&gt;&amp;'&lt;/a&gt;");
    }

    #[test]
    fn github_plain_escapes_like_github() {
        assert_eq!(github_plain("a < \"b\"\r\nc\n"), "a &lt; \"b\"\nc");
        assert_eq!(github_plain(""), "");
    }

    #[test]
    fn entry_points_are_thread_safe() {
        let handles: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    let code = format!("fn f{i}() -> u32 {{ {i} }}\n");
                    (github(&code, "rust"), gitlab(&code, "rust"))
                })
            })
            .collect();
        for (i, h) in handles.into_iter().enumerate() {
            let (gh, gl) = h.join().expect("no panic");
            assert_eq!(gh, github(&format!("fn f{i}() -> u32 {{ {i} }}\n"), "rust"));
            assert_eq!(gl.tag, "rust");
        }
    }
}
