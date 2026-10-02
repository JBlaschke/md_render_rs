//! File-system helpers for the CLI: lexical path normalization and lenient text decoding
//! (GRIP C38). README lookup is shared with the server: [`crate::markdown_files`].

use std::path::{Component, Path, PathBuf};

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
