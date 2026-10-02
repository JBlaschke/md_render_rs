//! Request-path parsing, filesystem resolution and directory listings (README lookup:
//! [`crate::markdown_files`]).
//!
//! Path safety follows DESIGN D14 / GRIP C39: the URL path is percent-decoded exactly once,
//! NUL bytes, backslashes and `..` segments are rejected, `.git` is never served, and every
//! resolved path is canonicalised and must stay inside the canonical served root (symlink
//! escapes are blocked unless `--follow-symlinks`). Trailing-slash normalisation follows
//! GRIP C35 (`/dir` -> `/dir/`, `/file/` -> `/file`, `.` and `//` collapsed).

use std::cmp::Ordering;
use std::io;
use std::path::{Component, Path, PathBuf};

use percent_encoding::{
    AsciiSet, CONTROLS, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode,
};

use crate::page::ListingEntry;

/// Characters escaped when a path segment is put into a URL path (the WHATWG path
/// percent-encode set plus `%`, `/`, `\` and a few characters browsers treat specially).
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'\\')
    .add(b'^')
    .add(b'|')
    .add(b'[')
    .add(b']');

/// Characters escaped in a query-string value (everything but unreserved characters and `/`).
const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// Why a request path was rejected before touching the filesystem.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("the request path is not valid UTF-8 after percent-decoding")]
    BadEncoding,
    #[error("the request path contains a NUL byte")]
    Nul,
    #[error("the request path contains a backslash")]
    Backslash,
    #[error("the request path contains a `..` segment")]
    Traversal,
    #[error("the request path contains an invalid segment")]
    InvalidSegment,
    #[error("`.git` directories are not served")]
    GitDir,
}

impl PathError {
    /// HTTP status for this rejection: 403 for `.git`, 400 for malformed paths.
    pub fn status(&self) -> u16 {
        match self {
            PathError::GitDir => 403,
            _ => 400,
        }
    }
}

/// A validated request path: decoded segments relative to the served root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestPath {
    /// Decoded segments; never empty strings, `.` or `..`.
    pub segments: Vec<String>,
    /// The request path ended with `/`.
    pub trailing_slash: bool,
    /// `.` or empty segments were collapsed, so the request should be redirected to the
    /// canonical form.
    pub collapsed: bool,
}

impl RequestPath {
    /// Parse the path component of a request URI (percent-decoded exactly once).
    pub fn from_uri_path(raw: &str) -> Result<RequestPath, PathError> {
        let decoded = percent_decode_str(raw)
            .decode_utf8()
            .map_err(|_| PathError::BadEncoding)?;
        RequestPath::from_decoded(&decoded)
    }

    /// Parse an already-decoded path (e.g. a `?path=` query value), with or without a
    /// leading `/`.
    pub fn from_decoded(path: &str) -> Result<RequestPath, PathError> {
        if path.contains('\0') {
            return Err(PathError::Nul);
        }
        if path.contains('\\') {
            return Err(PathError::Backslash);
        }
        let body = path.strip_prefix('/').unwrap_or(path);
        let mut segments = Vec::new();
        let mut collapsed = false;
        let mut trailing_slash = false;
        if !body.is_empty() {
            let parts: Vec<&str> = body.split('/').collect();
            let last = parts.len() - 1;
            for (i, seg) in parts.iter().enumerate() {
                match *seg {
                    "" => {
                        if i == last {
                            trailing_slash = true;
                        } else {
                            collapsed = true;
                        }
                    }
                    "." => {
                        collapsed = true;
                        if i == last {
                            trailing_slash = true;
                        }
                    }
                    ".." => return Err(PathError::Traversal),
                    s => {
                        if s.eq_ignore_ascii_case(".git") {
                            return Err(PathError::GitDir);
                        }
                        // Exactly one normal component (rejects drive prefixes and the like on
                        // Windows, where `Path::join("C:")` would replace the root).
                        let mut comps = Path::new(s).components();
                        if !matches!(
                            (comps.next(), comps.next()),
                            (Some(Component::Normal(_)), None)
                        ) {
                            return Err(PathError::InvalidSegment);
                        }
                        segments.push(s.to_owned());
                    }
                }
            }
        }
        if segments.is_empty() {
            // `/`, `/./`, `//` all mean the root directory, which is always "a directory URL".
            trailing_slash = true;
        }
        Ok(RequestPath {
            segments,
            trailing_slash,
            collapsed,
        })
    }

    /// The root directory (`/`).
    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// Relative path with `/` separators (`""` for the root).
    pub fn rel(&self) -> String {
        self.segments.join("/")
    }
}

/// URL path (`/`-prefixed, percent-encoded) of a root-relative path; `dir` appends `/`.
pub fn url_for(rel: &str, dir: bool) -> String {
    let mut out = String::from("/");
    let segs: Vec<String> = rel
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| utf8_percent_encode(s, SEGMENT).to_string())
        .collect();
    out.push_str(&segs.join("/"));
    if dir && !segs.is_empty() {
        out.push('/');
    }
    out
}

/// Percent-encode one path segment (e.g. a file name in a listing `href`).
pub fn encode_segment(s: &str) -> String {
    utf8_percent_encode(s, SEGMENT).to_string()
}

/// Percent-encode a query-string value (keeps `/` readable).
pub fn encode_query_value(s: &str) -> String {
    utf8_percent_encode(s, QUERY_VALUE).to_string()
}

/// Outcome of resolving a validated relative path against the served root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// An existing regular file (canonical path).
    File(PathBuf),
    /// An existing directory (canonical path).
    Dir(PathBuf),
    /// Nothing exists at this path (or a component is not a directory).
    Missing,
}

/// Why a path could not be served after looking at the filesystem.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Is this I/O error a "does not exist" condition (including a file used as a directory)?
pub fn is_not_found(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// Resolve `rel` (validated, `/`-separated) inside the canonical `root`.
///
/// For a missing path the nearest existing ancestor must still be inside the root, so a
/// symlinked directory cannot be used to wait for files outside it.
pub fn resolve(root: &Path, rel: &str, follow_symlinks: bool) -> Result<Resolved, ResolveError> {
    let full = join_rel(root, rel);
    match std::fs::canonicalize(&full) {
        Ok(canon) => {
            check_contained(root, &canon, follow_symlinks)?;
            let meta = std::fs::metadata(&canon)?;
            if meta.is_dir() {
                Ok(Resolved::Dir(canon))
            } else if meta.is_file() {
                Ok(Resolved::File(canon))
            } else {
                // FIFOs, sockets and device nodes: reading them can block forever.
                Err(ResolveError::Forbidden(
                    "only regular files and directories are served".into(),
                ))
            }
        }
        Err(e) if is_not_found(&e) => {
            let mut ancestor = full.parent();
            while let Some(a) = ancestor {
                match std::fs::canonicalize(a) {
                    Ok(canon) => {
                        check_contained(root, &canon, follow_symlinks)?;
                        break;
                    }
                    Err(_) => ancestor = a.parent(),
                }
            }
            Ok(Resolved::Missing)
        }
        Err(e) => Err(ResolveError::Io(e)),
    }
}

/// `root` joined with a `/`-separated relative path.
pub fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    p
}

/// Require a canonical path to stay inside the canonical root (unless symlinks may be
/// followed anywhere) and to never enter a `.git` directory.
pub fn check_contained(
    root: &Path,
    canon: &Path,
    follow_symlinks: bool,
) -> Result<(), ResolveError> {
    let inside = canon.strip_prefix(root);
    if inside.is_err() && !follow_symlinks {
        return Err(ResolveError::Forbidden(
            "the path resolves outside the served root through a symlink \
             (start md-render with --follow-symlinks to serve such files)"
                .into(),
        ));
    }
    let checked = inside.unwrap_or(canon);
    let in_git = checked.components().any(|c| match c {
        Component::Normal(s) => s.to_str().is_some_and(|s| s.eq_ignore_ascii_case(".git")),
        _ => false,
    });
    if in_git {
        return Err(ResolveError::Forbidden(
            "`.git` directories are not served".into(),
        ));
    }
    Ok(())
}

/// Entries of a directory for the listing page: `.git` hidden, symlinks escaping the root
/// hidden (unless `follow_symlinks`), directories first, then case-insensitive by name; a
/// `..` entry first for every directory below the root.
pub fn list_dir(
    root: &Path,
    dir: &Path,
    rel: &str,
    follow_symlinks: bool,
) -> io::Result<Vec<ListingEntry>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case(".git") {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            match std::fs::canonicalize(&path) {
                Ok(c) if check_contained(root, &c, follow_symlinks).is_ok() => {}
                _ => continue, // dangling or escaping symlink
            }
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let is_dir = meta.is_dir();
        let href = if is_dir {
            format!("{}/", encode_segment(&name))
        } else {
            encode_segment(&name)
        };
        out.push(ListingEntry {
            name,
            href,
            is_dir,
            size: (!is_dir).then_some(meta.len()),
        });
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a
            .name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name)),
    });
    if !rel.is_empty() {
        out.insert(
            0,
            ListingEntry {
                name: "..".into(),
                href: "../".into(),
                is_dir: true,
                size: None,
            },
        );
    }
    Ok(out)
}

/// The directory to watch for a file and the path events for it will carry.
///
/// FSEvents reports canonical paths (`/private/tmp/...`, CR §5), so both are canonical: the
/// file itself when it exists (this also resolves symlinked documents and the on-disk case
/// of the name), otherwise the canonical parent joined with the file name (the file may be
/// briefly missing during an atomic save).
pub fn watch_target(path: &Path) -> Option<(PathBuf, PathBuf)> {
    if let Ok(canon) = std::fs::canonicalize(path)
        && canon.is_file()
        && let Some(parent) = canon.parent()
    {
        return Some((parent.to_path_buf(), canon));
    }
    entry_watch_target(path)
}

/// The directory that holds the directory entry `path` and the path events for that entry
/// carry, without resolving a symlink at `path` itself (re-pointing a symlinked document
/// changes this entry, not the old target).
pub fn entry_watch_target(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let name = path.file_name()?;
    let parent = std::fs::canonicalize(path.parent()?).ok()?;
    let file = parent.join(name);
    Some((parent, file))
}

/// Where to wait for a file whose directory does not exist yet: the nearest existing
/// ancestor directory (canonical, inside `root` per [`check_contained`]) and the canonical
/// path the file will have once the missing directories are created. None when an
/// ancestor is not a directory (the file can never appear) or escapes the root.
pub fn ancestor_watch_target(
    root: &Path,
    path: &Path,
    follow_symlinks: bool,
) -> Option<(PathBuf, PathBuf)> {
    let mut missing = vec![path.file_name()?];
    let mut dir = path.parent()?;
    loop {
        match std::fs::canonicalize(dir) {
            Ok(canon) => {
                if !canon.is_dir() || check_contained(root, &canon, follow_symlinks).is_err() {
                    return None;
                }
                let mut file = canon.clone();
                file.extend(missing.iter().rev());
                return Some((canon, file));
            }
            Err(_) => {
                missing.push(dir.file_name()?);
                dir = dir.parent()?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(p: &str) -> Result<RequestPath, PathError> {
        RequestPath::from_uri_path(p)
    }

    #[test]
    fn parses_plain_paths() {
        let p = parse("/docs/README.md").unwrap();
        assert_eq!(p.segments, ["docs", "README.md"]);
        assert!(!p.trailing_slash && !p.collapsed);
        assert_eq!(p.rel(), "docs/README.md");
        let p = parse("/docs/").unwrap();
        assert!(p.trailing_slash && !p.collapsed);
        let root = parse("/").unwrap();
        assert!(root.is_root() && root.trailing_slash && !root.collapsed);
    }

    #[test]
    fn collapses_dot_and_empty_segments() {
        let p = parse("/./other.md").unwrap();
        assert_eq!(p.segments, ["other.md"]);
        assert!(p.collapsed);
        let p = parse("/a//b.md").unwrap();
        assert_eq!(p.rel(), "a/b.md");
        assert!(p.collapsed);
        let p = parse("/docs/.").unwrap();
        assert!(p.collapsed && p.trailing_slash);
        let p = parse("//").unwrap();
        assert!(p.is_root());
    }

    #[test]
    fn decodes_exactly_once() {
        let p = parse("/my%20file.md").unwrap();
        assert_eq!(p.segments, ["my file.md"]);
        // Double encoding stays a literal `%2e%2e` file name after one decode.
        let p = parse("/%252e%252e/x").unwrap();
        assert_eq!(p.segments, ["%2e%2e", "x"]);
        // An encoded slash becomes a separator (and is then checked like any other).
        assert_eq!(parse("/a%2Fb.md").unwrap().segments, ["a", "b.md"]);
    }

    #[test]
    fn rejects_traversal_and_tricks() {
        assert_eq!(parse("/../etc/passwd"), Err(PathError::Traversal));
        assert_eq!(parse("/%2e%2e/etc/passwd"), Err(PathError::Traversal));
        assert_eq!(
            parse("/docs/%2E%2E/%2e%2e/secret"),
            Err(PathError::Traversal)
        );
        assert_eq!(parse("/docs%2f..%2f..%2fsecret"), Err(PathError::Traversal));
        assert_eq!(parse("/a%5c..%5csecret"), Err(PathError::Backslash));
        assert_eq!(parse("/a\\b"), Err(PathError::Backslash));
        assert_eq!(parse("/a%00.md"), Err(PathError::Nul));
        assert_eq!(parse("/%ff%fe"), Err(PathError::BadEncoding));
        assert_eq!(parse("/.git/config"), Err(PathError::GitDir));
        assert_eq!(parse("/sub/.GIT/HEAD"), Err(PathError::GitDir));
        assert_eq!(PathError::GitDir.status(), 403);
        assert_eq!(PathError::Traversal.status(), 400);
        // `.github` and `.gitignore` are fine.
        assert!(parse("/.github/README.md").is_ok());
        assert!(parse("/.gitignore").is_ok());
    }

    #[test]
    fn decoded_query_paths() {
        let p = RequestPath::from_decoded("docs/README.md").unwrap();
        assert_eq!(p.rel(), "docs/README.md");
        assert_eq!(
            RequestPath::from_decoded("../x.md"),
            Err(PathError::Traversal)
        );
        assert!(RequestPath::from_decoded("").unwrap().is_root());
    }

    #[test]
    fn url_encoding() {
        assert_eq!(url_for("", true), "/");
        assert_eq!(url_for("docs", true), "/docs/");
        assert_eq!(
            url_for("docs/my file#1.md", false),
            "/docs/my%20file%231.md"
        );
        assert_eq!(url_for("ü.md", false), "/%C3%BC.md");
        assert_eq!(encode_query_value("docs/a b&c.md"), "docs/a%20b%26c.md");
        assert_eq!(encode_segment("a/b"), "a%2Fb");
    }

    #[test]
    fn resolve_and_containment() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.md"), "x").unwrap();
        assert_eq!(
            resolve(&root, "sub", false).unwrap(),
            Resolved::Dir(root.join("sub"))
        );
        assert_eq!(
            resolve(&root, "sub/a.md", false).unwrap(),
            Resolved::File(root.join("sub/a.md"))
        );
        assert_eq!(resolve(&root, "nope.md", false).unwrap(), Resolved::Missing);
        // A file used as a directory is "missing", not an I/O error.
        assert_eq!(
            resolve(&root, "sub/a.md/b", false).unwrap(),
            Resolved::Missing
        );
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let fifo = root.join("pipe.md");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !made.is_ok_and(|s| s.success()) {
            return; // no mkfifo on this system
        }
        assert!(matches!(
            resolve(&root, "pipe.md", false),
            Err(ResolveError::Forbidden(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_blocked_unless_following() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), root.join("link.txt"))
            .unwrap();
        std::os::unix::fs::symlink(root.join("inside.txt"), root.join("dangling.txt")).unwrap();
        assert!(matches!(
            resolve(&root, "link.txt", false),
            Err(ResolveError::Forbidden(_))
        ));
        assert!(matches!(
            resolve(&root, "link.txt", true),
            Ok(Resolved::File(_))
        ));
        assert_eq!(
            resolve(&root, "dangling.txt", false).unwrap(),
            Resolved::Missing
        );
        // Waiting for a file below an escaping directory symlink is refused too.
        std::os::unix::fs::symlink(outside.path(), root.join("outdir")).unwrap();
        assert!(matches!(
            resolve(&root, "outdir/new.md", false),
            Err(ResolveError::Forbidden(_))
        ));
        assert!(matches!(
            resolve(&root, "outdir/new.md", true),
            Ok(Resolved::Missing)
        ));
        // Listings hide escaping and dangling symlinks.
        let names: Vec<_> = list_dir(&root, &root, "", false)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.is_empty(), "{names:?}");
        let names: Vec<_> = list_dir(&root, &root, "", true)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["outdir", "link.txt"]);
    }

    #[test]
    fn listing_order_and_hrefs() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("zdir")).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join("B file.txt"), "12345").unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        let entries = list_dir(&root, &root, "", false).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zdir", "a.md", "B file.txt"]);
        assert_eq!(entries[0].href, "zdir/");
        assert_eq!(entries[2].href, "B%20file.txt");
        assert_eq!(entries[2].size, Some(5));
        let sub = list_dir(&root, &root.join("zdir"), "zdir", false).unwrap();
        assert_eq!(sub[0].name, "..");
    }

    #[test]
    fn watch_target_canonicalises() {
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        let (d, f) = watch_target(&dir.path().join("missing.md")).unwrap();
        assert_eq!(d, canon);
        assert_eq!(f, canon.join("missing.md"));
        std::fs::write(dir.path().join("x.md"), "x").unwrap();
        let (_, f) = watch_target(&dir.path().join("x.md")).unwrap();
        assert_eq!(f, canon.join("x.md"));
        assert!(watch_target(&dir.path().join("no/such/dir.md")).is_none());
    }

    #[test]
    fn ancestor_watch_targets() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("a")).unwrap();
        assert_eq!(
            ancestor_watch_target(&root, &dir.path().join("a/b/c/x.md"), false),
            Some((root.join("a"), root.join("a/b/c/x.md")))
        );
        assert_eq!(
            ancestor_watch_target(&root, &root.join("new/x.md"), false),
            Some((root.clone(), root.join("new/x.md")))
        );
        // A file where a directory would have to be: the path can never exist.
        std::fs::write(root.join("f"), "x").unwrap();
        assert_eq!(
            ancestor_watch_target(&root, &root.join("f/x.md"), false),
            None
        );
        // Outside the root.
        let other = tempfile::tempdir().unwrap();
        assert_eq!(
            ancestor_watch_target(&root, &other.path().join("n/x.md"), false),
            None
        );
    }
}
