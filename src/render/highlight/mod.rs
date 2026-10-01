//! Syntax highlighting (owner: highlight). Contract: docs/DESIGN.md §2 D5, §3.3.

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
pub fn github(code: &str, lang: &str) -> GithubCode {
    let _ = (code, lang);
    GithubCode::default()
}

/// GitLab code block highlighting result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitlabCode {
    /// Rouge tag (`rust`, `javascript`, ...) or `plaintext`.
    pub tag: String,
    /// One entry per source line: escaped HTML with Rouge short-class spans.
    pub lines: Vec<String>,
}

/// Highlight `code` for GitLab; `lang` is the info-string language word as written.
pub fn gitlab(code: &str, lang: &str) -> GitlabCode {
    let _ = lang;
    let code = code.strip_suffix('\n').unwrap_or(code);
    GitlabCode {
        tag: "plaintext".into(),
        lines: code.split('\n').map(escape).collect(),
    }
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
