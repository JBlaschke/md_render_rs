//! Which files are Markdown, which README a directory shows (DESIGN §6) and how Markdown bytes
//! are decoded. Shared by the server (`md-render DIR`), the CLI (stdin) and `md-render export`,
//! so all of them pick the same file and report undecodable input the same way.

use std::ffi::OsStr;
use std::path::Path;

use crate::flavor::Flavor;

/// Markdown file extensions rendered by both flavors, in README preference order.
pub(crate) const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkdn", "mkd", "mdwn"];

/// Additional extensions GitLab renders as Markdown (`lib/gitlab/markup_helper.rb`, CQ §1.8).
pub(crate) const GITLAB_EXTRA_EXTENSIONS: &[&str] = &["rmd"];

/// Is `name` (a file name or relative path) a Markdown document for `flavor`?
pub(crate) fn is_markdown(name: &str, flavor: Flavor) -> bool {
    markdown_rank(name, flavor).is_some()
}

/// Index of the file's extension in the flavor's Markdown extension list (case-insensitive).
fn markdown_rank(name: &str, flavor: Flavor) -> Option<usize> {
    let ext = Path::new(name)
        .extension()
        .and_then(OsStr::to_str)?
        .to_ascii_lowercase();
    let extra: &[&str] = match flavor {
        Flavor::Gitlab => GITLAB_EXTRA_EXTENSIONS,
        Flavor::Github => &[],
    };
    MARKDOWN_EXTENSIONS
        .iter()
        .chain(extra)
        .position(|e| *e == ext)
}

/// The README of `dir`: exact `README.md` first, otherwise any file whose stem is `readme`
/// (case-insensitive) with a Markdown extension, preferring extensions in
/// [`MARKDOWN_EXTENSIONS`] order, then by name. Symlinks are followed (containment is checked
/// when the file is served). Returns the real on-disk file name.
pub(crate) fn find_readme(dir: &Path, flavor: Flavor) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(rank) = markdown_rank(&name, flavor) else {
            continue;
        };
        let stem = Path::new(&name)
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or("");
        if !stem.eq_ignore_ascii_case("readme")
            || !std::fs::metadata(entry.path()).is_ok_and(|m| m.is_file())
        {
            continue;
        }
        if name == "README.md" {
            return Some(name);
        }
        if best
            .as_ref()
            .is_none_or(|(r, n)| (rank, name.as_str()) < (*r, n.as_str()))
        {
            best = Some((rank, name));
        }
    }
    best.map(|(_, n)| n)
}

/// The directories (relative to the root, `""` = the root itself) searched for the README of
/// the root, in order. GitHub: `.github/`, the root, then `docs/` ("About READMEs" in GitHub's
/// docs); GitLab shows the root's own README only.
pub(crate) fn root_readme_dirs(flavor: Flavor) -> &'static [&'static str] {
    match flavor {
        Flavor::Github => &[".github", "", "docs"],
        Flavor::Gitlab => &[""],
    }
}

/// README lookup for the served (or exported) root directory, in [`root_readme_dirs`] order.
/// Returns the root-relative path with `/` separators.
pub(crate) fn find_root_readme(root: &Path, flavor: Flavor) -> Option<String> {
    root_readme_dirs(flavor).iter().find_map(|sub| {
        if sub.is_empty() {
            return find_readme(root, flavor);
        }
        let dir = root.join(sub);
        if !dir.is_dir() {
            return None;
        }
        find_readme(&dir, flavor).map(|name| format!("{sub}/{name}"))
    })
}

/// Decodes Markdown source bytes (GRIP C38): strips a UTF-8 byte order mark and replaces
/// invalid UTF-8 with U+FFFD. The warning, when the bytes were not valid UTF-8, names the first
/// invalid byte by its offset in `bytes` (counting the byte order mark).
pub(crate) fn decode(mut bytes: Vec<u8>, name: &str) -> (String, Option<String>) {
    const BOM: &[u8] = b"\xEF\xBB\xBF";
    let skipped = if bytes.starts_with(BOM) {
        bytes.drain(..BOM.len());
        BOM.len()
    } else {
        0
    };
    match String::from_utf8(bytes) {
        Ok(s) => (s, None),
        Err(e) => {
            let offset = skipped + e.utf8_error().valid_up_to();
            let warning = format!(
                "{name} is not valid UTF-8 (first invalid byte at offset {offset}); invalid bytes were replaced"
            );
            (
                String::from_utf8_lossy(e.as_bytes()).into_owned(),
                Some(warning),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn decoding() {
        assert_eq!(
            decode(b"\xEF\xBB\xBF# a".to_vec(), "a.md"),
            ("# a".into(), None)
        );
        assert_eq!(decode(b"# a".to_vec(), "a.md"), ("# a".into(), None));
        // A BOM that is not at the start is content (U+FEFF).
        assert_eq!(decode(b"a\xEF\xBB\xBF".to_vec(), "a.md").0, "a\u{feff}");
        let (text, w) = decode(b"caf\xe9".to_vec(), "a.md");
        assert_eq!(text, "caf\u{FFFD}");
        assert_eq!(
            w.as_deref(),
            Some(
                "a.md is not valid UTF-8 (first invalid byte at offset 3); invalid bytes were replaced"
            )
        );
        // The offset counts the BOM: it is the byte offset in the file.
        let (text, w) = decode(b"\xEF\xBB\xBF# a\xFFb\n".to_vec(), "stdin");
        assert_eq!(text, "# a\u{FFFD}b\n");
        assert!(
            w.unwrap()
                .starts_with("stdin is not valid UTF-8 (first invalid byte at offset 6)")
        );
    }

    #[test]
    fn markdown_extensions() {
        assert!(is_markdown("README.md", Flavor::Github));
        assert!(is_markdown("a/b.MarkDown", Flavor::Github));
        assert!(is_markdown("x.mdwn", Flavor::Gitlab));
        assert!(!is_markdown("x.rmd", Flavor::Github));
        assert!(is_markdown("x.Rmd", Flavor::Gitlab));
        assert!(!is_markdown("x.txt", Flavor::Github));
        assert!(!is_markdown("md", Flavor::Github));
    }

    #[test]
    fn readme_lookup_prefers_exact_then_extension_order() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert_eq!(find_readme(d, Flavor::Github), None);
        fs::write(d.join("readme.markdown"), "x").unwrap();
        assert_eq!(
            find_readme(d, Flavor::Github).as_deref(),
            Some("readme.markdown")
        );
        fs::write(d.join("Readme.mkd"), "x").unwrap();
        fs::write(d.join("readme.rmd"), "x").unwrap();
        fs::write(d.join("README.txt"), "x").unwrap();
        fs::create_dir(d.join("README.mdown")).unwrap(); // a directory is never a README
        // `.markdown` ranks before `.mkd`; GitLab also knows `.rmd`, but it ranks last.
        for flavor in Flavor::ALL {
            assert_eq!(
                find_readme(d, flavor).as_deref(),
                Some("readme.markdown"),
                "{flavor:?}"
            );
        }
        fs::remove_file(d.join("readme.markdown")).unwrap();
        fs::remove_file(d.join("Readme.mkd")).unwrap();
        assert_eq!(find_readme(d, Flavor::Github), None);
        assert_eq!(
            find_readme(d, Flavor::Gitlab).as_deref(),
            Some("readme.rmd")
        );
        fs::write(d.join("README.markdown"), "x").unwrap();
        fs::write(d.join("README.md"), "x").unwrap();
        assert_eq!(find_readme(d, Flavor::Github).as_deref(), Some("README.md"));
    }

    /// CLI-17: which README the root shows when several locations have one (the same table
    /// decides for `md-render DIR` and `md-render export DIR`).
    #[test]
    fn root_readme_precedence() {
        // (files present, GitHub's choice, GitLab's choice)
        let table: &[(&[&str], Option<&str>, Option<&str>)] = &[
            (&[], None, None),
            (&["docs/README.md"], Some("docs/README.md"), None),
            (
                &["README.md", "docs/README.md"],
                Some("README.md"),
                Some("README.md"),
            ),
            (
                &["Readme.markdown", ".github/readme.md", "docs/README.md"],
                Some(".github/readme.md"),
                Some("Readme.markdown"),
            ),
            (
                &[".github/CONTRIBUTING.md", "docs/README.md"],
                Some("docs/README.md"),
                None,
            ),
            (&[".gitlab/README.md"], None, None),
        ];
        for (files, github, gitlab) in table {
            let t = tempfile::tempdir().unwrap();
            for f in *files {
                let p = t.path().join(f);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, "x").unwrap();
            }
            let got = |flavor| find_root_readme(t.path(), flavor);
            assert_eq!(got(Flavor::Github).as_deref(), *github, "github {files:?}");
            assert_eq!(got(Flavor::Gitlab).as_deref(), *gitlab, "gitlab {files:?}");
        }
    }
}
