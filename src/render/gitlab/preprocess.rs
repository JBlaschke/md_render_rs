//! Stage A of the GitLab pipeline (GL §3): source normalisation, the front-matter pre-filter and
//! `::include{file=...}` expansion. All of these work on the raw Markdown text.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// `NormalizeSourceFilter` (GL §3.1): strip a leading UTF-8 BOM.
pub(super) fn normalize_source(text: &str) -> &str {
    text.strip_prefix('\u{FEFF}').unwrap_or(text)
}

/// `\R` of `Gitlab::UntrustedRegexp` (RE2), GL §3.2.
const BACKSLASH_R: &str = r"(?:\n|\x0B|\x0C|\r\n|\r|\x{85}|\x{2028}|\x{2029})";

/// `Gitlab::FrontMatter::PATTERN_UNTRUSTED` (GL §3.2), translated from RE2 to the `regex` crate.
/// RE2's `\s` is `[\t\n\f\r ]` (no `\v`), so the classes are spelled out; `.` excludes `\n` in
/// both engines and multi-line `^` matches after `\n` only in both.
static FRONT_MATTER: LazyLock<Regex> = LazyLock::new(|| {
    let r = BACKSLASH_R;
    let pattern = format!(
        concat!(
            r"(?m)\A(?P<encoding>[^\r\n]*coding:[^\r\n]*{r})?",
            r"(?P<before>[\t\n\x0C\r ]*)",
            r"^(?P<delim>---|\+\+\+|;;;)[ \t]*(?P<lang>[^\t\n\x0C\r ]*){r}",
            r"(?P<front_matter>(?:\n|.)*?)",
            r"^(?:(?P<delim_closing>---|\+\+\+|;;;)|\.{{3}})",
            r"[\t\x0C ]*(?:{r}|\z)"
        ),
        r = r
    );
    Regex::new(&pattern).expect("front matter pattern compiles")
});

/// `FrontMatterFilter` (GL §3.2): turn front matter into a ```` ```lang:frontmatter ```` fence,
/// keeping the line count so that `data-sourcepos` still points at the real lines.
pub(super) fn front_matter(text: &str) -> Cow<'_, str> {
    let Some(caps) = FRONT_MATTER.captures(text) else {
        return Cow::Borrowed(text);
    };
    let whole = caps.get(0).expect("group 0");
    let delim = caps.name("delim").map_or("", |m| m.as_str());
    let lang = match caps.name("lang").map_or("", |m| m.as_str()) {
        "" => match delim {
            "---" => "yaml",
            "+++" => "toml",
            _ => "json",
        },
        lang => lang,
    };
    let mut before = caps.name("before").map_or("", |m| m.as_str()).to_owned();
    if caps
        .name("encoding")
        .is_some_and(|m| !m.as_str().is_empty())
    {
        before.insert(0, '\n');
    }
    let body = caps.name("front_matter").map_or("", |m| m.as_str());
    let mut out = String::with_capacity(text.len() + 32);
    out.push_str(&text[..whole.start()]);
    out.push_str(&before);
    out.push_str("```");
    out.push_str(lang);
    out.push_str(":frontmatter\n");
    out.push_str(body);
    out.push_str("```\n");
    out.push_str(&text[whole.end()..]);
    Cow::Owned(out)
}

/// `asciidoc_max_includes` default on gitlab.com (GL §3.3).
pub(super) const MAX_INCLUDES: usize = 32;

/// Largest file `::include` reads; bigger blobs give GitLab's `too large` error.
const MAX_INCLUDE_BYTES: u64 = 10 * 1024 * 1024;

/// Where `::include` may read from.
#[derive(Clone, Debug, Default)]
pub(super) struct IncludeContext {
    /// Canonical served root (`None`: no repository).
    pub root: Option<PathBuf>,
    /// The including file, relative to `root` (GitLab's `requested_path`).
    pub requested: Option<PathBuf>,
}

impl IncludeContext {
    /// Derives the context from the render options (DESIGN D14: the root defaults to the file's
    /// directory).
    pub(super) fn new(source_path: Option<&Path>, root: Option<&Path>) -> Self {
        let root = root
            .map(Path::to_path_buf)
            .or_else(|| source_path.and_then(Path::parent).map(Path::to_path_buf));
        let Some(root) = root.and_then(|r| {
            let r = if r.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                r
            };
            r.canonicalize().ok()
        }) else {
            return IncludeContext::default();
        };
        let requested = source_path.and_then(|p| {
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::env::current_dir().ok()?.join(p)
            };
            let parent = abs.parent()?.canonicalize().ok()?;
            let name = abs.file_name()?;
            parent
                .join(name)
                .strip_prefix(&root)
                .ok()
                .map(Path::to_path_buf)
        });
        IncludeContext {
            root: Some(root),
            requested,
        }
    }
}

/// Result of include expansion.
#[derive(Debug, Default)]
pub(super) struct Included {
    /// Warnings for the in-page banner (one per failed include).
    pub warnings: Vec<String>,
    /// Files that were read (absolute paths), for the live watcher.
    pub files: Vec<PathBuf>,
}

/// `IncludeFilter` (GL §3.3). Not recursive (included text is not scanned again), at most
/// [`MAX_INCLUDES`] directives are replaced, URLs become links (gitlab.com does not fetch them),
/// and files are read only from inside the served root.
pub(super) fn includes<'t>(
    text: &'t str,
    ctx: &IncludeContext,
    out: &mut Included,
) -> Cow<'t, str> {
    if !text.contains("::include{file=") {
        return Cow::Borrowed(text);
    }
    let mut result = String::with_capacity(text.len());
    let mut cache: HashMap<String, String> = HashMap::new();
    let mut replaced = 0usize;
    let mut line_start = 0usize;
    while line_start <= text.len() {
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |i| line_start + i);
        let line = &text[line_start..line_end];
        let at_eos = line_end == text.len();
        match (replaced < MAX_INCLUDES)
            .then(|| match_include(line))
            .flatten()
        {
            Some((file, consumed)) => {
                let content = cache
                    .entry(file.to_owned())
                    .or_insert_with(|| read_include(file, ctx, out))
                    .clone();
                result.push_str(&content);
                result.push_str(&line[consumed..]);
                replaced += 1;
            }
            None => result.push_str(line),
        }
        if at_eos {
            break;
        }
        result.push('\n');
        line_start = line_end + 1;
    }
    Cow::Owned(result)
}

/// Line terminators accepted by Ruby's `\R` lookahead (other than `\n`, which ends the line).
fn is_ruby_r(c: char) -> bool {
    matches!(
        c,
        '\r' | '\x0B' | '\x0C' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Ruby `^::include\{file=(?<include>.{1,1024})}(?=\R|\z)` on one line (no `\n` inside).
/// Returns the file name and the number of bytes of the line the directive consumed.
fn match_include(line: &str) -> Option<(&str, usize)> {
    let rest = line.strip_prefix("::include{file=")?;
    let prefix = line.len() - rest.len();
    // Greedy `.{1,1024}` followed by `}` and a line terminator or the end of the text.
    let mut best = None;
    for (count, (i, c)) in rest.char_indices().enumerate() {
        if count > 1024 {
            break;
        }
        if c == '}' && count >= 1 {
            let after = &rest[i + 1..];
            let ok = match after.chars().next() {
                None => true, // the line ends: either `\n` follows or it is the end of the text
                Some(n) => is_ruby_r(n),
            };
            if ok {
                best = Some(i);
            }
        }
    }
    best.map(|i| (&rest[..i], prefix + i + 1))
}

/// `markdown_link` of IncludeFilter.
fn markdown_link(url: &str) -> String {
    format!(
        "[{}]({})",
        comrak::escape_commonmark_inline(url),
        comrak::escape_commonmark_link_destination(url)
    )
}

/// `error_message` of IncludeFilter.
fn error_message(file: &str, reason: &str, out: &mut Included) -> String {
    out.warnings
        .push(format!("::include{{file={file}}}: {reason}"));
    format!("**Error including '{}': {reason}**", markdown_link(file))
}

/// `target_http?` of IncludeFilter.
fn target_http(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && url::Url::parse(file).is_ok_and(|u| u.has_host())
}

fn read_include(file: &str, ctx: &IncludeContext, out: &mut Included) -> String {
    if target_http(file) {
        // `wiki_asciidoc_allow_uri_includes` is off on gitlab.com: the directive becomes a link.
        return markdown_link(file);
    }
    let Some(root) = &ctx.root else {
        return error_message(file, "no repository", out);
    };
    let Some(requested) = &ctx.requested else {
        return error_message(file, "not found", out);
    };
    let Some(target) = resolve_relative_path(file, requested) else {
        return error_message(file, "not found", out);
    };
    // Confine to the root: canonicalize (resolving symlinks) and require the root prefix.
    let Ok(abs) = root.join(&target).canonicalize() else {
        return error_message(file, "not found", out);
    };
    if !abs.starts_with(root) || !abs.is_file() {
        return error_message(file, "not found", out);
    }
    match std::fs::metadata(&abs) {
        Ok(m) if m.len() > MAX_INCLUDE_BYTES => return error_message(file, "too large", out),
        Ok(_) => {}
        Err(_) => return error_message(file, "not found", out),
    }
    let Ok(bytes) = std::fs::read(&abs) else {
        return error_message(file, "not readable", out);
    };
    out.files.push(abs);
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return error_message(file, "not readable", out);
    }
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => error_message(file, "not readable", out),
    }
}

/// `resolve_relative_path` of IncludeFilter: `Pathname(base)`, `dirname` unless the base has no
/// extension, `+ path`, `cleanpath`. Absolute paths are repository-root relative (DESIGN D14).
/// Returns `None` when the result leaves the repository (GitLab: the blob is not found).
fn resolve_relative_path(path: &str, base: &Path) -> Option<PathBuf> {
    let base_dir = if base.extension().is_some() {
        base.parent().unwrap_or(Path::new(""))
    } else {
        base
    };
    let joined = if path.starts_with('/') {
        PathBuf::from(path.trim_start_matches('/'))
    } else {
        base_dir.join(path)
    };
    let mut clean: Vec<Component<'_>> = Vec::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop()?;
            }
            Component::Normal(_) => clean.push(c),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if clean.is_empty() {
        return None;
    }
    Some(clean.iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_is_stripped() {
        assert_eq!(normalize_source("\u{FEFF}# x"), "# x");
        assert_eq!(normalize_source("# x"), "# x");
    }

    #[test]
    fn yaml_front_matter() {
        let out = front_matter("---\na: 1\n---\n# T\n");
        assert_eq!(out, "```yaml:frontmatter\na: 1\n```\n# T\n");
    }

    #[test]
    fn front_matter_variants() {
        // toml / json defaults, explicit language, `...` closing, leading blank lines.
        assert_eq!(
            front_matter("+++\na = 1\n+++\n"),
            "```toml:frontmatter\na = 1\n```\n"
        );
        assert_eq!(
            front_matter(";;;\n{}\n;;;\n"),
            "```json:frontmatter\n{}\n```\n"
        );
        assert_eq!(
            front_matter("--- toml\na\n...\nx"),
            "```toml:frontmatter\na\n```\nx"
        );
        assert_eq!(
            front_matter("\n\n---\na\n---"),
            "\n\n```yaml:frontmatter\na\n```\n"
        );
        // Mixed closing delimiters are accepted (RE2 has no back-references).
        assert_eq!(
            front_matter("---\na\n+++\n"),
            "```yaml:frontmatter\na\n```\n"
        );
        // Encoding line is dropped, a newline keeps the line count.
        assert_eq!(
            front_matter("# coding: utf-8\n---\na\n---\n"),
            "\n```yaml:frontmatter\na\n```\n"
        );
        // CRLF line ends.
        assert_eq!(
            front_matter("---\r\na\r\n---\r\n"),
            "```yaml:frontmatter\na\r\n```\n"
        );
    }

    #[test]
    fn no_front_matter() {
        assert!(matches!(
            front_matter("# x\n---\na\n---\n"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(front_matter("  ---\na\n---\n"), Cow::Borrowed(_)));
        assert!(matches!(
            front_matter("---\nunterminated"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(front_matter(""), Cow::Borrowed(_)));
    }

    #[test]
    fn include_line_matching() {
        assert_eq!(match_include("::include{file=a.md}"), Some(("a.md", 20)));
        assert_eq!(match_include("::include{file=a}b}"), Some(("a}b", 19)));
        assert_eq!(match_include("::include{file=a.md}\r"), Some(("a.md", 20)));
        assert_eq!(match_include("::include{file=a.md} x"), None);
        assert_eq!(match_include("::include{file=}"), None);
        assert_eq!(match_include(" ::include{file=a}"), None);
    }

    #[test]
    fn relative_paths() {
        let p = |a: &str, b: &str| resolve_relative_path(a, Path::new(b));
        assert_eq!(
            p("users.md", "doc/api/README.md"),
            Some(PathBuf::from("doc/api/users.md"))
        );
        assert_eq!(
            p("users.md", "doc/api"),
            Some(PathBuf::from("doc/api/users.md"))
        );
        assert_eq!(
            p("../u/x.md", "doc/api/README.md"),
            Some(PathBuf::from("doc/u/x.md"))
        );
        assert_eq!(
            p("/top.md", "doc/api/README.md"),
            Some(PathBuf::from("top.md"))
        );
        assert_eq!(p("../../../etc/passwd", "doc/README.md"), None);
    }

    #[test]
    fn includes_are_confined_and_cached() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/part.md"), "PART").unwrap();
        std::fs::write(root.join("bin.md"), b"a\0b").unwrap();
        let ctx = IncludeContext {
            root: Some(root.clone()),
            requested: Some(PathBuf::from("sub/main.md")),
        };
        let mut out = Included::default();
        let text = "a\n::include{file=part.md}\n::include{file=part.md}\n::include{file=../../x.md}\n\
                    ::include{file=../bin.md}\n::include{file=https://example.com/x.md}";
        let res = includes(text, &ctx, &mut out);
        assert_eq!(
            res,
            "a\nPART\nPART\n**Error including '[\\.\\./\\.\\./x\\.md](<../../x.md>)': not found**\n\
             **Error including '[\\.\\./bin\\.md](<../bin.md>)': not readable**\n\
             [https://example\\.com/x\\.md](<https://example.com/x.md>)"
        );
        assert_eq!(out.warnings.len(), 2);
        assert_eq!(out.files.len(), 2);
    }

    #[test]
    fn include_limit() {
        let ctx = IncludeContext::default();
        let mut out = Included::default();
        let text = "::include{file=x}\n".repeat(MAX_INCLUDES + 3);
        let res = includes(&text, &ctx, &mut out);
        assert_eq!(res.matches("no repository").count(), MAX_INCLUDES);
        assert_eq!(out.warnings.len(), 1, "cached per file name");
        assert_eq!(res.matches("::include{file=x}").count(), 3);
    }
}
