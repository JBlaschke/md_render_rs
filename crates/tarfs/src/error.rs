//! Error type shared by every tarfs operation.

use std::io;
use std::path::Path;

/// Errors produced by tarfs.
///
/// The variants are part of the frozen API. Errors that come from the archive
/// *content* (bad gzip stream, bad tar header, unsafe member path, ...) are
/// reported as [`Error::Corrupt`]; errors from the operating system (missing file,
/// permission denied, disk full, ...) as [`Error::Io`], with the offending path
/// folded into the message.
///
/// ```
/// let fs = tarfs::InMemoryFileSystem::new();
/// let err = fs.read_file("missing.txt").unwrap_err();
/// assert!(matches!(err, tarfs::Error::NotFound(ref p) if p == "missing.txt"));
/// assert_eq!(err.to_string(), "no such file in archive: missing.txt");
/// ```
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An operating-system I/O error (reading the archive, writing the temporary
    /// file, renaming it into place, ...). The [`io::ErrorKind`] is preserved.
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    /// The requested path is not in the filesystem.
    #[error("no such file in archive: {0}")]
    NotFound(String),
    /// A path rejected by [`check_path`](crate::check_path).
    #[error("invalid path {path:?}: {reason}")]
    InvalidPath {
        /// The rejected path.
        path: String,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// The default USTAR writer cannot encode this path (longer than 100 bytes and
    /// not splittable into a <= 155-byte prefix and a <= 100-byte name at a `/`).
    #[error("path too long for USTAR (use write_tarball_gz_extended): {0}")]
    PathTooLong(String),
    /// The default USTAR writer cannot encode this size (11 octal digits: < 8 GiB).
    #[error("entry too large for USTAR: {path} ({size} bytes)")]
    EntryTooLarge {
        /// The offending entry.
        path: String,
        /// Its size in bytes.
        size: u64,
    },
    /// The input is not a valid (supported) gzip-compressed tarball.
    #[error("corrupt archive: {0}")]
    Corrupt(String),
    /// The archive decompresses to more than
    /// [`Limits::max_decompressed_size`](crate::Limits::max_decompressed_size)
    /// bytes (a "gzip bomb", or just an archive too large for the caller's
    /// limits). Decompression stops there, so no more than the limit is ever held
    /// in memory.
    #[error("archive too large: it decompresses to more than {limit} bytes")]
    TooLarge {
        /// The limit that was exceeded, in bytes.
        limit: u64,
    },
    /// [`read_to_string`](crate::InMemoryFileSystem::read_to_string) found
    /// non-UTF-8 content in this file.
    #[error("file is not valid UTF-8: {0}")]
    Utf8(String),
}

/// Result alias for tarfs operations.
///
/// ```
/// fn first_line(fs: &tarfs::InMemoryFileSystem) -> tarfs::Result<String> {
///     let text = fs.read_to_string("notes.txt")?;
///     Ok(text.lines().next().unwrap_or_default().to_owned())
/// }
///
/// let mut fs = tarfs::InMemoryFileSystem::new();
/// fs.write_file("notes.txt", "one\ntwo\n")?;
/// assert_eq!(first_line(&fs)?, "one");
/// # Ok::<(), tarfs::Error>(())
/// ```
pub type Result<T> = std::result::Result<T, Error>;

/// Wrap an OS error with a description of what was being done to which path,
/// keeping its [`io::ErrorKind`] so callers can still match on it.
pub(crate) fn io_context(err: io::Error, action: &str, path: &Path) -> Error {
    Error::Io(io::Error::new(
        err.kind(),
        format!("{action} {}: {err}", path.display()),
    ))
}
