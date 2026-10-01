//! Member-path rules: validation of paths we store or write, and normalization of
//! paths found in foreign archives.

use crate::error::{Error, Result};

/// Validate a member path (TarFS.jl `_check_path`, plus a backslash rule).
///
/// A valid path is non-empty, relative, `/`-separated and free of NUL bytes,
/// backslashes and `""`, `"."` or `".."` segments. This rules out `//`, a leading
/// or trailing `/`, `./a`, `a/../b`, and anything that could escape a directory
/// when the archive is extracted. Backslashes are rejected (TarFS.jl allows them)
/// because Windows tools treat them as separators.
///
/// [`InMemoryFileSystem::write_file`](crate::InMemoryFileSystem::write_file)
/// enforces this; the reader rejects archives whose members violate it.
///
/// ```
/// use tarfs::check_path;
///
/// assert!(check_path("a/b/c.txt").is_ok());
/// assert!(check_path("日本語/weird name.txt").is_ok());
/// for bad in ["", "/abs", "a//b", "a/", "./a", "a/../b", "..", "a\\b", "a\0b"] {
///     assert!(check_path(bad).is_err(), "{bad:?} should be rejected");
/// }
/// ```
pub fn check_path(path: &str) -> Result<()> {
    let bad = |reason| {
        Err(Error::InvalidPath {
            path: path.to_owned(),
            reason,
        })
    };
    if path.is_empty() {
        return bad("empty path");
    }
    if path.contains('\0') {
        return bad("NUL byte in path");
    }
    if path.starts_with('/') {
        return bad("absolute path");
    }
    if path.contains('\\') {
        return bad("backslash in path");
    }
    for segment in path.split('/') {
        match segment {
            "" => return bad("empty segment (\"//\" or trailing '/')"),
            "." => return bad("'.' segment"),
            ".." => return bad("'..' segment"),
            _ => {}
        }
    }
    Ok(())
}

/// Turn the raw path of a regular-file member of a foreign archive into a key.
///
/// * `Ok(None)`: skip the member: the bare `.` (or empty) entry, an old-style
///   directory (regular type flag but a trailing `/`), or a macOS AppleDouble
///   `._*` companion (bsdtar adds these when packing on macOS; CR §6).
/// * `Err(Corrupt)`: non-UTF-8, or still invalid after stripping leading `./`
///   (absolute, `..`, `//`, ...): we never want such paths as keys.
pub(crate) fn normalize_member(raw: &[u8]) -> Result<Option<String>> {
    let Ok(text) = std::str::from_utf8(raw) else {
        return Err(Error::Corrupt(format!(
            "member path is not valid UTF-8: {:?}",
            String::from_utf8_lossy(raw)
        )));
    };
    // `tar -C dir .` stores every member as "./<path>".
    let mut path = text;
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    if path.is_empty() || path == "." || path.ends_with('/') {
        return Ok(None);
    }
    check_path(path).map_err(|err| match err {
        Error::InvalidPath { reason, .. } => {
            Error::Corrupt(format!("unsafe member path {text:?}: {reason}"))
        }
        other => other,
    })?;
    let basename = path.rsplit('/').next().unwrap_or(path);
    if basename.starts_with("._") {
        return Ok(None);
    }
    Ok(Some(path.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_path_accepts_good_paths() {
        for good in [
            "a",
            "a.b",
            "a/b/c.txt",
            "weird name.txt",
            "-dash",
            "日本語",
            "...",
            "a/.hidden",
            "a/..b",
            "._x",
        ] {
            assert!(check_path(good).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn check_path_rejects_every_bad_form_with_a_reason() {
        let cases = [
            ("", "empty path"),
            ("a\0b", "NUL byte in path"),
            ("/abs.txt", "absolute path"),
            ("/", "absolute path"),
            ("a\\b", "backslash in path"),
            ("a//b", "empty segment (\"//\" or trailing '/')"),
            ("a/", "empty segment (\"//\" or trailing '/')"),
            (".", "'.' segment"),
            ("./a", "'.' segment"),
            ("a/./b", "'.' segment"),
            ("..", "'..' segment"),
            ("a/../b", "'..' segment"),
            ("a/..", "'..' segment"),
        ];
        for (bad, want) in cases {
            match check_path(bad) {
                Err(Error::InvalidPath { path, reason }) => {
                    assert_eq!(path, bad);
                    assert_eq!(reason, want, "{bad:?}");
                }
                other => panic!("{bad:?}: expected InvalidPath, got {other:?}"),
            }
        }
    }

    #[test]
    fn normalize_strips_dot_slash_and_skips_specials() {
        let n = |s: &str| normalize_member(s.as_bytes()).unwrap();
        assert_eq!(n("a.txt").as_deref(), Some("a.txt"));
        assert_eq!(n("./a/b.txt").as_deref(), Some("a/b.txt"));
        assert_eq!(n("././a").as_deref(), Some("a"));
        assert_eq!(n("."), None);
        assert_eq!(n("./"), None);
        assert_eq!(n(""), None);
        assert_eq!(n("dir/"), None);
        assert_eq!(n("./dir/"), None);
        assert_eq!(n("._a.txt"), None);
        assert_eq!(n("./d/._a.txt"), None);
        assert_eq!(n("d/.__x").as_deref(), None);
        assert_eq!(n("d/._").as_deref(), None);
        assert_eq!(n("d/.a").as_deref(), Some("d/.a"));
    }

    #[test]
    fn normalize_rejects_unsafe_and_non_utf8() {
        for bad in ["../x", "/etc/passwd", "a//b", "a/../b", "./../x", "a\\b"] {
            assert!(
                matches!(normalize_member(bad.as_bytes()), Err(Error::Corrupt(_))),
                "{bad:?}"
            );
        }
        assert!(matches!(
            normalize_member(b"bad\xff.txt"),
            Err(Error::Corrupt(msg)) if msg.contains("UTF-8")
        ));
    }
}
