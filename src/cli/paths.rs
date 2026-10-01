//! File-system helpers for the CLI: lexical path normalization, README lookup (DESIGN §6,
//! GRIP C5) and lenient text decoding (GRIP C38).

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::flavor::Flavor;

/// Markdown extensions recognised for README lookup (DESIGN §6), in preference order.
pub const MARKDOWN_EXTENSIONS: [&str; 6] = ["md", "markdown", "mdown", "mkdn", "mkd", "mdwn"];

/// Extensions for `flavor` (GitLab additionally renders `.rmd`, CQ §1.8).
pub fn markdown_extensions(flavor: Flavor) -> Vec<&'static str> {
    let mut exts = MARKDOWN_EXTENSIONS.to_vec();
    if flavor == Flavor::Gitlab {
        exts.push("rmd");
    }
    exts
}

/// `cwd.join(path)` with `.` components removed and `..` folded lexically where possible
/// (symlinks are **not** resolved, so the user's spelling of a symlinked file is kept).
pub fn absolutize(cwd: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    normalize(&joined)
}

/// Lexical normalization: drop `.`, fold `a/..`, keep leading `..` of relative paths.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if can_pop {
                    out.pop();
                } else if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir | Component::Prefix(_))
                ) {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Find the README of `dir`: case-insensitive `readme.<markdown ext>`, exact `README.md`
/// first, then by extension order and name; when `is_root`, also `.github/` then `docs/`
/// (DESIGN §6). Returns the real on-disk name.
pub fn find_readme(dir: &Path, flavor: Flavor, is_root: bool) -> Option<PathBuf> {
    let exts = markdown_extensions(flavor);
    let in_dir = |d: &Path| -> Option<PathBuf> {
        let mut best: Option<(usize, String)> = None;
        for entry in fs::read_dir(d).ok()?.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Some((stem, ext)) = name.rsplit_once('.') else {
                continue;
            };
            if !stem.eq_ignore_ascii_case("readme") {
                continue;
            }
            let ext = ext.to_ascii_lowercase();
            let Some(rank) = exts.iter().position(|e| *e == ext) else {
                continue;
            };
            if !entry.path().is_file() {
                continue;
            }
            // Exact `README.md` always wins (rank 0 before every other candidate).
            let rank = if name == "README.md" { 0 } else { rank + 1 };
            if best.as_ref().is_none_or(|(r, n)| (rank, &name) < (*r, n)) {
                best = Some((rank, name));
            }
        }
        best.map(|(_, name)| d.join(name))
    };
    in_dir(dir).or_else(|| {
        if is_root {
            in_dir(&dir.join(".github")).or_else(|| in_dir(&dir.join("docs")))
        } else {
            None
        }
    })
}

/// Decode Markdown bytes: strip a UTF-8 BOM, replace invalid UTF-8 (with a warning).
pub fn decode_text(bytes: Vec<u8>, what: &str, warnings: &mut Vec<String>) -> String {
    let bytes = match bytes.strip_prefix(b"\xEF\xBB\xBF") {
        Some(rest) => rest.to_vec(),
        None => bytes,
    };
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            let at = e.utf8_error().valid_up_to();
            warnings.push(format!(
                "{what} is not valid UTF-8 (first invalid byte at offset {at}); invalid bytes were replaced"
            ));
            String::from_utf8_lossy(e.as_bytes()).into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        let cwd = Path::new("/w/d");
        assert_eq!(
            absolutize(cwd, Path::new("./a/./b.md")),
            Path::new("/w/d/a/b.md")
        );
        assert_eq!(absolutize(cwd, Path::new("../x.md")), Path::new("/w/x.md"));
        assert_eq!(absolutize(cwd, Path::new("../../../x")), Path::new("/x"));
        assert_eq!(absolutize(cwd, Path::new("/abs/./y")), Path::new("/abs/y"));
        assert_eq!(absolutize(cwd, Path::new(".")), Path::new("/w/d"));
        assert_eq!(normalize(Path::new("a/../..")), Path::new(".."));
        assert_eq!(normalize(Path::new("a/..")), Path::new("."));
    }

    #[test]
    fn readme_lookup() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert_eq!(find_readme(d, Flavor::Github, true), None);
        fs::create_dir(d.join("docs")).unwrap();
        fs::write(d.join("docs/README.md"), "d").unwrap();
        assert_eq!(find_readme(d, Flavor::Github, false), None);
        assert_eq!(
            find_readme(d, Flavor::Github, true),
            Some(d.join("docs/README.md"))
        );
        fs::create_dir(d.join(".github")).unwrap();
        fs::write(d.join(".github/readme.markdown"), "g").unwrap();
        assert_eq!(
            find_readme(d, Flavor::Github, true),
            Some(d.join(".github/readme.markdown"))
        );
        fs::write(d.join("Readme.mkd"), "x").unwrap();
        fs::write(d.join("readme.rmd"), "x").unwrap();
        fs::write(d.join("readme.txt"), "x").unwrap();
        fs::create_dir(d.join("README.mdown")).unwrap(); // a directory is never a README
        assert_eq!(
            find_readme(d, Flavor::Github, true),
            Some(d.join("Readme.mkd"))
        );
        fs::write(d.join("readme.markdown"), "x").unwrap();
        assert_eq!(
            find_readme(d, Flavor::Github, true),
            Some(d.join("readme.markdown"))
        );
        // GitLab also knows .rmd, but it ranks last.
        assert_eq!(
            find_readme(d, Flavor::Gitlab, true),
            Some(d.join("readme.markdown"))
        );
        fs::write(d.join("README.md"), "x").unwrap();
        assert_eq!(
            find_readme(d, Flavor::Github, true),
            Some(d.join("README.md"))
        );
    }

    #[test]
    fn decoding() {
        let mut w = Vec::new();
        assert_eq!(
            decode_text(b"\xEF\xBB\xBF# a".to_vec(), "stdin", &mut w),
            "# a"
        );
        assert!(w.is_empty());
        assert_eq!(
            decode_text(b"a\xFFb".to_vec(), "stdin", &mut w),
            "a\u{FFFD}b"
        );
        assert_eq!(
            w,
            [
                "stdin is not valid UTF-8 (first invalid byte at offset 1); invalid bytes were replaced"
            ]
        );
    }
}
