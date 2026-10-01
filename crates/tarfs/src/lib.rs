//! An in-memory filesystem backed by a `.tar.gz` archive: a Rust port of
//! [TarFS.jl](https://github.com/JBlaschke/TarFS.jl).
//!
//! Load an archive once, read files by path, stage new, changed or deleted files
//! in memory, and flush everything back to a gzip-compressed POSIX tarball
//! atomically. The archive is never unpacked to disk. This is a library-level view
//! of one file, not a mount, and not POSIX.
//!
//! ```
//! use tarfs::{InMemoryFileSystem, open_tarball_gz};
//!
//! # fn main() -> tarfs::Result<()> {
//! let dir = tempfile::tempdir()?;
//! let archive = dir.path().join("results.tar.gz");
//!
//! // Write: stage files, flush on success.
//! open_tarball_gz(&archive, None, |fs| {
//!     fs.write_file("summary.csv", "x,y\n1,2\n")?;
//!     fs.write_file("meta/run.txt", "seed = 42\n")?;
//!     Ok::<_, tarfs::Error>(())
//! })?;
//!
//! // Read: decompressed once, every read is a zero-copy slice.
//! let fs = InMemoryFileSystem::open(&archive)?;
//! assert_eq!(fs.read_to_string("meta/run.txt")?, "seed = 42\n");
//!
//! // Read-modify-write in place: safe, because the flush is atomic.
//! open_tarball_gz(&archive, Some(&archive), |fs| {
//!     let old = fs.read_to_string("summary.csv")?;
//!     fs.write_file("summary.csv", old + "3,4\n")?;
//!     fs.delete_file("meta/run.txt");
//!     Ok::<_, tarfs::Error>(())
//! })?;
//! let fs = InMemoryFileSystem::open(&archive)?;
//! assert_eq!(fs.paths().collect::<Vec<_>>(), ["summary.csv"]);
//! # Ok(())
//! # }
//! ```
//!
//! # Guarantees
//!
//! * **Atomic flushes.** [`InMemoryFileSystem::write_tarball_gz`] writes a
//!   temporary file next to the target and renames it into place only once
//!   complete. A failure leaves any existing archive untouched, which is what makes
//!   read-modify-write onto the source archive safe. There is no `fsync`:
//!   durability across power loss is left to the filesystem.
//! * **Reproducible bytes.** Entries are written in sorted path order with fixed
//!   metadata (mode `0o644`, uid/gid/mtime `0`). The tar bytes are identical to
//!   TarFS.jl's for the same contents, and, unlike TarFS.jl, the gzip layer is
//!   reproducible across platforms too: the gzip header is fixed (mtime 0, OS byte
//!   "unknown") and the compressor is flate2's default pure-Rust backend
//!   (`miniz_oxide`). A build that switches flate2 to another backend (its `zlib*`
//!   features) still writes identical tar bytes, but may compress them differently.
//! * **Zero-copy reads, streaming writes.** The decompressed archive is one
//!   immutable [`Bytes`] buffer; reading a file is an O(1) reference-counted slice
//!   and flushing streams source-backed files straight from it.
//! * **Thread safe.** [`InMemoryFileSystem`] is `Send + Sync`: share it (e.g. in an
//!   `Arc`) and read from many threads at once. Mutation needs `&mut self`.
//!
//! # Limitations
//!
//! Regular files only: no symlinks, hard links, permissions (everything is
//! `0o644`), timestamps or GNU sparse members. The whole decompressed archive
//! lives in memory for the lifetime of the value; cap its size with [`Limits`]
//! when reading archives you did not create. The default writer is limited to
//! USTAR paths (100 bytes, or a 155-byte prefix plus a 100-byte name split at a
//! `/`) and entries under 8 GiB; [`InMemoryFileSystem::write_tarball_gz_extended`]
//! lifts both limits and adds directory entries.
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::ops::Bound;
use std::path::Path;

use bytes::Bytes;

mod error;
mod path;
mod read;
mod ustar;
mod write;

pub use error::{Error, Result};
pub use path::check_path;

// Compile and run the README's examples as doctests, so they cannot go stale.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// Where an entry's bytes live.
#[derive(Debug, Clone)]
enum Entry {
    /// Backed by the source tarball: byte range into `InMemoryFileSystem::tarball`.
    Source { offset: usize, size: usize },
    /// Staged by `write_file`; owns its bytes (TarFS.jl `pos = -1`).
    Owned(Bytes),
}

/// An in-memory filesystem backed by a (decompressed) tarball.
///
/// Construct it empty with [`new`](Self::new) or from an archive with
/// [`open`](Self::open) (or the `from_*` constructors); read with
/// [`read_file`](Self::read_file) / [`get`](Self::get); stage changes with
/// [`write_file`](Self::write_file) / [`delete_file`](Self::delete_file); flush
/// with [`write_tarball_gz`](Self::write_tarball_gz) or [`open_tarball_gz`].
///
/// Cloning copies the path index only: the source buffer and staged contents are
/// reference-counted and shared.
///
/// ```
/// let mut fs = tarfs::InMemoryFileSystem::new();
/// fs.write_file("a.txt", "hello")?;
/// let copy = fs.clone();
/// fs.delete_file("a.txt");
/// assert!(copy.contains("a.txt") && !fs.contains("a.txt"));
/// # Ok::<(), tarfs::Error>(())
/// ```
#[derive(Clone, Default)]
pub struct InMemoryFileSystem {
    tarball: Bytes,
    entries: BTreeMap<String, Entry>,
}

/// Short alias for [`InMemoryFileSystem`].
///
/// ```
/// let fs: tarfs::TarFs = tarfs::TarFs::new();
/// assert!(fs.is_empty());
/// ```
pub type TarFs = InMemoryFileSystem;

/// Resource limits for reading an archive (see
/// [`InMemoryFileSystem::open_with_limits`]).
///
/// The whole decompressed tarball is held in memory, and gzip compresses runs of
/// identical bytes about 1000-fold: a 1 MB `.tar.gz` can expand to 1 GB. Set a
/// limit before reading archives you did not create. The default is no limit, which
/// is what [`InMemoryFileSystem::open`] and the other constructors without
/// `_with_limits` use.
///
/// ```
/// let limits = tarfs::Limits::default().with_max_decompressed_size(512 << 20);
/// assert_eq!(limits.max_decompressed_size, Some(512 << 20));
/// assert_eq!(tarfs::Limits::default().max_decompressed_size, None);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Largest accepted size of the decompressed tarball, in bytes; `None` means
    /// unlimited. Exceeding it stops decompression with [`Error::TooLarge`].
    pub max_decompressed_size: Option<u64>,
}

impl Limits {
    /// These limits with the decompressed size capped at `bytes`.
    #[must_use]
    pub const fn with_max_decompressed_size(mut self, bytes: u64) -> Self {
        self.max_decompressed_size = Some(bytes);
        self
    }
}

impl fmt::Debug for InMemoryFileSystem {
    /// Summarizes instead of dumping the (possibly huge) buffer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryFileSystem")
            .field("files", &self.entries.len())
            .field("tarball_bytes", &self.tarball.len())
            .finish_non_exhaustive()
    }
}

impl InMemoryFileSystem {
    /// Empty, writable filesystem with no backing tarball (TarFS.jl
    /// `InMemoryFileSystem()`).
    ///
    /// ```
    /// let fs = tarfs::InMemoryFileSystem::new();
    /// assert!(fs.is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the `.tar.gz` at `path`, decompress it into memory and index its
    /// regular files (TarFS.jl `InMemoryFileSystem(path)`).
    ///
    /// The file is read once and not kept open. Multi-member gzip is supported.
    /// Directories, links and other non-regular members are skipped, as are macOS
    /// AppleDouble companions (`._*` members whose data starts with the AppleDouble
    /// magic number; other `._*` files are kept); a leading `./` is stripped from
    /// member paths.
    ///
    /// The decompressed size is not limited: use
    /// [`open_with_limits`](Self::open_with_limits) for archives you did not create.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if the file cannot be read; [`Error::Corrupt`] if it is not a
    /// valid gzip-compressed tarball, contains GNU sparse members, or has a member
    /// path that is not UTF-8 or not safe (absolute, `..`, ...).
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// # let dir = tempfile::tempdir()?;
    /// # let path = dir.path().join("data.tar.gz");
    /// # let mut fs = tarfs::InMemoryFileSystem::new();
    /// # fs.write_file("data/p175.csv", "x,y\n1,2\n")?;
    /// # fs.write_tarball_gz(&path)?;
    /// let fs = tarfs::InMemoryFileSystem::open(&path)?;
    /// assert_eq!(fs.get("data/p175.csv"), Some(&b"x,y\n1,2\n"[..]));
    /// assert!(matches!(
    ///     tarfs::InMemoryFileSystem::open(dir.path().join("missing.tar.gz")),
    ///     Err(tarfs::Error::Io(_))
    /// ));
    /// # Ok(())
    /// # }
    /// ```
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_limits(path, Limits::default())
    }

    /// [`open`](Self::open) with resource limits: decompression stops with
    /// [`Error::TooLarge`] as soon as the tarball exceeds
    /// [`Limits::max_decompressed_size`], so memory use stays bounded however
    /// small and hostile the file is.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open), plus [`Error::TooLarge`].
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// # let dir = tempfile::tempdir()?;
    /// let path = dir.path().join("zeros.tar.gz");
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("zeros.bin", vec![0u8; 1 << 20])?;
    /// fs.write_tarball_gz(&path)?; // about 1 KB on disk
    ///
    /// let small = tarfs::Limits::default().with_max_decompressed_size(64 << 10);
    /// assert!(matches!(
    ///     tarfs::InMemoryFileSystem::open_with_limits(&path, small),
    ///     Err(tarfs::Error::TooLarge { limit }) if limit == 64 << 10
    /// ));
    /// let roomy = tarfs::Limits::default().with_max_decompressed_size(8 << 20);
    /// assert_eq!(tarfs::InMemoryFileSystem::open_with_limits(&path, roomy)?.len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub fn open_with_limits(path: impl AsRef<Path>, limits: Limits) -> Result<Self> {
        let path = path.as_ref();
        let with_path = |err| match err {
            Error::Io(e) => error::io_context(e, "reading", path),
            Error::Corrupt(msg) => Error::Corrupt(format!("{}: {msg}", path.display())),
            other => other,
        };
        let file = File::open(path).map_err(|e| error::io_context(e, "opening", path))?;
        read::gunzip(file, limits)
            .and_then(Self::from_tar_bytes)
            .map_err(with_path)
    }

    /// Decompress a `.tar.gz` stream (multi-member gzip supported) and index it.
    /// The decompressed size is not limited (see
    /// [`from_tar_gz_reader_with_limits`](Self::from_tar_gz_reader_with_limits)).
    ///
    /// # Errors
    ///
    /// [`Error::Io`] for errors raised by `reader` itself, [`Error::Corrupt`] for
    /// invalid gzip or tar data (see [`open`](Self::open)).
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// # let mut fs = tarfs::InMemoryFileSystem::new();
    /// # fs.write_file("a.txt", "hello")?;
    /// # let dir = tempfile::tempdir()?;
    /// # let path = dir.path().join("a.tar.gz");
    /// # fs.write_tarball_gz(&path)?;
    /// let file = std::fs::File::open(&path)?;
    /// let fs = tarfs::InMemoryFileSystem::from_tar_gz_reader(file)?;
    /// assert_eq!(fs.read_to_string("a.txt")?, "hello");
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_tar_gz_reader(r: impl Read) -> Result<Self> {
        Self::from_tar_gz_reader_with_limits(r, Limits::default())
    }

    /// [`from_tar_gz_reader`](Self::from_tar_gz_reader) with resource limits (see
    /// [`open_with_limits`](Self::open_with_limits)).
    ///
    /// # Errors
    ///
    /// As [`from_tar_gz_reader`](Self::from_tar_gz_reader), plus
    /// [`Error::TooLarge`].
    pub fn from_tar_gz_reader_with_limits(r: impl Read, limits: Limits) -> Result<Self> {
        Self::from_tar_bytes(read::gunzip(r, limits)?)
    }

    /// Decompress `.tar.gz` bytes and index them. The decompressed size is not
    /// limited (see [`from_tar_gz_bytes_with_limits`](Self::from_tar_gz_bytes_with_limits)).
    ///
    /// # Errors
    ///
    /// [`Error::Corrupt`] for invalid gzip or tar data (see [`open`](Self::open)).
    ///
    /// ```
    /// let err = tarfs::InMemoryFileSystem::from_tar_gz_bytes(b"not gzip").unwrap_err();
    /// assert!(matches!(err, tarfs::Error::Corrupt(_)));
    /// ```
    pub fn from_tar_gz_bytes(gz: &[u8]) -> Result<Self> {
        Self::from_tar_gz_reader(gz)
    }

    /// [`from_tar_gz_bytes`](Self::from_tar_gz_bytes) with resource limits (see
    /// [`open_with_limits`](Self::open_with_limits)).
    ///
    /// # Errors
    ///
    /// As [`from_tar_gz_bytes`](Self::from_tar_gz_bytes), plus [`Error::TooLarge`].
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("a.txt", "hello")?;
    /// let dir = tempfile::tempdir()?;
    /// let path = dir.path().join("a.tar.gz");
    /// fs.write_tarball_gz(&path)?;
    /// let gz = std::fs::read(&path)?;
    /// // Tar data comes in 512-byte blocks: one header, one data block, two end blocks.
    /// let limits = tarfs::Limits::default().with_max_decompressed_size(2048);
    /// assert!(tarfs::InMemoryFileSystem::from_tar_gz_bytes_with_limits(&gz, limits).is_ok());
    /// let limits = tarfs::Limits::default().with_max_decompressed_size(2047);
    /// assert!(tarfs::InMemoryFileSystem::from_tar_gz_bytes_with_limits(&gz, limits).is_err());
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_tar_gz_bytes_with_limits(gz: &[u8], limits: Limits) -> Result<Self> {
        Self::from_tar_gz_reader_with_limits(gz, limits)
    }

    /// Index an uncompressed tarball held in memory. The buffer is kept (shared,
    /// not copied) and every source-backed read is a slice of it.
    ///
    /// # Errors
    ///
    /// [`Error::Corrupt`] for invalid tar data (see [`open`](Self::open)).
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("a.txt", "hello")?;
    /// let tar: Vec<u8> = fs.write_tarball(Vec::new())?;
    ///
    /// let back = tarfs::InMemoryFileSystem::from_tar_bytes(tar)?;
    /// assert_eq!(back.get("a.txt"), Some(&b"hello"[..]));
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_tar_bytes(buf: impl Into<Bytes>) -> Result<Self> {
        let tarball: Bytes = buf.into();
        let entries = read::index(&tarball)?;
        Ok(Self { tarball, entries })
    }

    /// Contents of `path` as a zero-copy, reference-counted slice (TarFS.jl
    /// `readfile`).
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] if there is no such file.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("bytes.bin", vec![0x00, 0xff, 0x10])?;
    /// assert_eq!(&fs.read_file("bytes.bin")?[..], [0x00, 0xff, 0x10]);
    /// assert!(matches!(fs.read_file("nope"), Err(tarfs::Error::NotFound(_))));
    /// # Ok(())
    /// # }
    /// ```
    pub fn read_file(&self, path: &str) -> Result<Bytes> {
        match self.entries.get(path) {
            Some(Entry::Source { offset, size }) => {
                Ok(self.tarball.slice(*offset..*offset + *size))
            }
            Some(Entry::Owned(bytes)) => Ok(bytes.clone()),
            None => Err(Error::NotFound(path.to_owned())),
        }
    }

    /// Contents of `path` as UTF-8 text.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] if there is no such file, [`Error::Utf8`] if its
    /// contents are not valid UTF-8.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("unicode.txt", "unicode ✓ content")?;
    /// fs.write_file("binary.bin", vec![0xff, 0xfe])?;
    /// assert_eq!(fs.read_to_string("unicode.txt")?, "unicode ✓ content");
    /// assert!(matches!(fs.read_to_string("binary.bin"), Err(tarfs::Error::Utf8(_))));
    /// # Ok(())
    /// # }
    /// ```
    pub fn read_to_string(&self, path: &str) -> Result<String> {
        let bytes = self
            .get(path)
            .ok_or_else(|| Error::NotFound(path.to_owned()))?;
        // Validate before copying, so a binary file costs no allocation.
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Error::Utf8(path.to_owned()))
    }

    /// Borrowed contents of `path`, if present.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("a.txt", "hello")?;
    /// assert_eq!(fs.get("a.txt"), Some(&b"hello"[..]));
    /// assert_eq!(fs.get("b.txt"), None);
    /// # Ok(())
    /// # }
    /// ```
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.entries.get(path).map(|entry| self.bytes_of(entry))
    }

    /// Whether `path` is a file in this filesystem (directories are implicit and
    /// never "exist" on their own).
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("dir/a.txt", "")?;
    /// assert!(fs.contains("dir/a.txt"));
    /// assert!(!fs.contains("dir"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// Number of files.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("a", "1")?;
    /// fs.write_file("a", "2")?; // overwrite
    /// fs.write_file("b", "3")?;
    /// assert_eq!(fs.len(), 2);
    /// # Ok(())
    /// # }
    /// ```
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when there are no files.
    ///
    /// ```
    /// assert!(tarfs::InMemoryFileSystem::new().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All file paths, sorted by bytes (the order the writers use).
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// for p in ["b/x", "a", "b-c"] {
    ///     fs.write_file(p, "")?;
    /// }
    /// assert_eq!(fs.paths().collect::<Vec<_>>(), ["a", "b-c", "b/x"]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// All files as `(path, contents)` pairs, sorted by path.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("b", "2")?;
    /// fs.write_file("a", "1")?;
    /// let all: Vec<(&str, &[u8])> = fs.iter().collect();
    /// assert_eq!(all, [("a", &b"1"[..]), ("b", &b"2"[..])]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries
            .iter()
            .map(|(path, entry)| (path.as_str(), self.bytes_of(entry)))
    }

    /// Size of `path` in bytes, if present.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("a.txt", "hello")?;
    /// assert_eq!(fs.file_size("a.txt"), Some(5));
    /// assert_eq!(fs.file_size("b.txt"), None);
    /// # Ok(())
    /// # }
    /// ```
    pub fn file_size(&self, path: &str) -> Option<u64> {
        self.get(path).map(|bytes| bytes.len() as u64)
    }

    /// Sorted paths of every file below the directory `prefix` (recursive).
    ///
    /// Trailing slashes on `prefix` are ignored, and `""` lists everything. The
    /// match is per path segment: `read_dir("a")` lists `a/x` and `a/b/y`, but not
    /// `a`, `ab` or `a-b/z`.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// for p in ["a", "a/x", "a/b/y", "ab", "a-b/z"] {
    ///     fs.write_file(p, "")?;
    /// }
    /// assert_eq!(fs.read_dir("a").collect::<Vec<_>>(), ["a/b/y", "a/x"]);
    /// assert_eq!(fs.read_dir("a/b/").collect::<Vec<_>>(), ["a/b/y"]);
    /// assert_eq!(fs.read_dir("").count(), 5);
    /// # Ok(())
    /// # }
    /// ```
    pub fn read_dir<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let dir = prefix.trim_end_matches('/');
        // Keys starting with `dir` form one contiguous run of the sorted map; within
        // it keep those continuing with '/' ("a-b" and "ab" sort among them too).
        self.entries
            .range::<str, _>((Bound::Included(dir), Bound::Unbounded))
            .map(|(key, _)| key.as_str())
            .take_while(move |key| key.starts_with(dir))
            .filter(move |key| dir.is_empty() || key.as_bytes().get(dir.len()) == Some(&b'/'))
    }

    /// Stage (or overwrite) a file (TarFS.jl `writefile`).
    ///
    /// `content` is anything convertible into [`Bytes`] (`Vec<u8>`, `String`,
    /// `&'static str`, `Bytes`, ...); it is taken over without copying. Overwriting
    /// a source-backed file replaces the entry, so read, transform, write is a
    /// read-modify-write. Nothing touches the disk until a flush.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidPath`] if `path` fails [`check_path`].
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("a.txt", "v1")?;
    /// fs.write_file("a.txt", String::from("v2"))?;
    /// assert_eq!(fs.read_to_string("a.txt")?, "v2");
    /// assert!(matches!(
    ///     fs.write_file("../escape", "x"),
    ///     Err(tarfs::Error::InvalidPath { .. })
    /// ));
    /// # Ok(())
    /// # }
    /// ```
    pub fn write_file(&mut self, path: &str, content: impl Into<Bytes>) -> Result<()> {
        check_path(path)?;
        self.entries
            .insert(path.to_owned(), Entry::Owned(content.into()));
        Ok(())
    }

    /// Remove `path`; returns whether it existed (TarFS.jl `deletefile`, which is a
    /// no-op for absent paths).
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("a.txt", "x")?;
    /// assert!(fs.delete_file("a.txt"));
    /// assert!(!fs.delete_file("a.txt"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn delete_file(&mut self, path: &str) -> bool {
        self.entries.remove(path).is_some()
    }

    /// Remove every file below the directory `prefix` (same matching as
    /// [`read_dir`](Self::read_dir), so `""` removes everything); returns how many
    /// were removed.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// for p in ["github/a.css", "github/img/b.png", "gitlab/c.css", "github.txt"] {
    ///     fs.write_file(p, "")?;
    /// }
    /// assert_eq!(fs.delete_prefix("github"), 2);
    /// assert_eq!(fs.paths().collect::<Vec<_>>(), ["github.txt", "gitlab/c.css"]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn delete_prefix(&mut self, prefix: &str) -> usize {
        let doomed: Vec<String> = self.read_dir(prefix).map(str::to_owned).collect();
        for path in &doomed {
            self.entries.remove(path);
        }
        doomed.len()
    }

    /// Write the filesystem as an uncompressed POSIX (ustar) tarball to `w` and
    /// return `w` (TarFS.jl `write_tarball`).
    ///
    /// The bytes are identical to TarFS.jl's: entries in sorted order, mode
    /// `0o644`, uid/gid/mtime `0`, no directory entries, two zero blocks at the
    /// end. Every entry is validated before anything is written. `w` is not
    /// flushed.
    ///
    /// # Errors
    ///
    /// [`Error::PathTooLong`] for a path the ustar name/prefix fields cannot hold,
    /// [`Error::EntryTooLarge`] for a file of 8 GiB or more (use
    /// [`write_tarball_extended`](Self::write_tarball_extended) for either), and
    /// [`Error::Io`] if `w` fails.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("a.txt", "hello")?;
    /// let tar = fs.write_tarball(Vec::new())?;
    /// assert_eq!(tar.len(), 512 + 512 + 1024); // header, padded data, end marker
    /// assert_eq!(&tar[..5], b"a.txt");
    ///
    /// assert!(tarfs::InMemoryFileSystem::new().write_tarball(Vec::new())? == vec![0; 1024]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn write_tarball<W: Write>(&self, w: W) -> Result<W> {
        write::write_ustar(self, w)
    }

    /// Write the filesystem as an uncompressed tarball with explicit directory
    /// entries and no path or size limits, and return `w`.
    ///
    /// Members are POSIX ustar headers (mode `0o644` for files, `0o755` for
    /// directories, uid/gid/mtime `0`) in sorted path order, every directory before
    /// its contents; a PAX extended header carries any path that does not fit the
    /// ustar fields and any size of 8 GiB or more. Deterministic. Use this format
    /// when the archive is consumed by `tar -x`, docker and other tools that care
    /// about directory entries; for archives read back by tarfs both writers are
    /// interchangeable.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidPath`] if a file path is also the parent directory of other
    /// files (`a` and `a/b`), which no extractor can reproduce; [`Error::Io`] if
    /// `w` fails.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// let long = format!("deep/{}/leaf.txt", vec!["x".repeat(90); 3].join("/"));
    /// fs.write_file(&long, "leaf")?;
    /// assert!(matches!(fs.write_tarball(Vec::new()), Err(tarfs::Error::PathTooLong(_))));
    ///
    /// let tar = fs.write_tarball_extended(Vec::new())?;
    /// let back = tarfs::InMemoryFileSystem::from_tar_bytes(tar)?;
    /// assert_eq!(back.get(&long), Some(&b"leaf"[..]));
    /// # Ok(())
    /// # }
    /// ```
    pub fn write_tarball_extended<W: Write>(&self, w: W) -> Result<W> {
        write::write_extended(self, w)
    }

    /// Flush to a `.tar.gz` at `path`, atomically (TarFS.jl `write_tarball_gz`).
    ///
    /// The tar layer is [`write_tarball`](Self::write_tarball). The gzip layer has a
    /// fixed header (mtime 0, OS "unknown", no name) and default compression, so
    /// equal contents give byte-identical files on every platform. The archive is
    /// written to a temporary file in the same directory (created `0o600`, set to
    /// `0o644` on Unix) and renamed over `path` once complete: on any error the
    /// temporary file is removed and an existing archive at `path` is untouched. A
    /// symlink at `path` is replaced, as `mv` would. No `fsync`.
    ///
    /// # Errors
    ///
    /// As [`write_tarball`](Self::write_tarball), plus [`Error::Io`] for failures
    /// creating, writing or renaming the temporary file.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let dir = tempfile::tempdir()?;
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file("a.txt", "hello")?;
    /// let (one, two) = (dir.path().join("1.tar.gz"), dir.path().join("2.tar.gz"));
    /// fs.write_tarball_gz(&one)?;
    /// fs.write_tarball_gz(&two)?;
    /// assert_eq!(std::fs::read(&one)?, std::fs::read(&two)?); // reproducible
    /// # Ok(())
    /// # }
    /// ```
    pub fn write_tarball_gz(&self, path: impl AsRef<Path>) -> Result<()> {
        // Validate before creating the temporary file: cheap, and keeps the
        // directory untouched for an unencodable filesystem.
        for (p, data) in self.iter() {
            ustar::validate(p, data.len() as u64)?;
        }
        write::atomic_gz(path.as_ref(), |gz| self.write_tarball(gz).map(drop))
    }

    /// Flush to a `.tar.gz` at `path` in the extended format of
    /// [`write_tarball_extended`](Self::write_tarball_extended) (TarFS.jl
    /// `write_tarball_gz_viatar`), with the same atomic, reproducible discipline as
    /// [`write_tarball_gz`](Self::write_tarball_gz). Unlike TarFS.jl, no temporary
    /// directory is involved: the `tar` crate writes straight from memory.
    ///
    /// # Errors
    ///
    /// As [`write_tarball_extended`](Self::write_tarball_extended), plus
    /// [`Error::Io`] for failures creating, writing or renaming the temporary file.
    ///
    /// ```
    /// # fn main() -> tarfs::Result<()> {
    /// let dir = tempfile::tempdir()?;
    /// let path = dir.path().join("via.tar.gz");
    /// let mut fs = tarfs::InMemoryFileSystem::new();
    /// fs.write_file(&format!("L/{}/leaf.txt", "s".repeat(300)), "leafdata")?;
    /// fs.write_tarball_gz_extended(&path)?;
    /// let back = tarfs::InMemoryFileSystem::open(&path)?;
    /// assert_eq!(back.len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub fn write_tarball_gz_extended(&self, path: impl AsRef<Path>) -> Result<()> {
        write::atomic_gz(path.as_ref(), |gz| {
            self.write_tarball_extended(gz).map(drop)
        })
    }

    fn bytes_of<'a>(&'a self, entry: &'a Entry) -> &'a [u8] {
        match entry {
            // In bounds: `read::index` checked every range against this buffer.
            Entry::Source { offset, size } => &self.tarball[*offset..*offset + *size],
            Entry::Owned(bytes) => bytes,
        }
    }
}

/// Run `f` on a writable filesystem, then flush it to `path` atomically (TarFS.jl
/// `open_tarball_gz`).
///
/// The filesystem starts empty, or seeded from the archive at `from`. The flush
/// ([`InMemoryFileSystem::write_tarball_gz`]) happens only if `f` returns `Ok`; on
/// `Err` nothing is written and the error is returned. `from == path`
/// (read-modify-write in place) is safe: the source is fully loaded up front and the
/// flush is atomic. Returns `f`'s value together with the final filesystem.
///
/// `f` may use any error type that tarfs errors convert into (`tarfs::Error`
/// itself, `anyhow::Error`, ...).
///
/// # Errors
///
/// Loading `from` or flushing to `path` ([`Error`], converted into `E`), or
/// whatever `f` returns.
///
/// ```
/// # fn main() -> tarfs::Result<()> {
/// let dir = tempfile::tempdir()?;
/// let path = dir.path().join("rmw.tar.gz");
/// tarfs::open_tarball_gz(&path, None, |fs| fs.write_file("a.txt", "v1"))?;
///
/// // A failing closure leaves the archive untouched.
/// let failed = tarfs::open_tarball_gz(&path, Some(&path), |fs| {
///     fs.write_file("a.txt", "SHOULD NOT LAND")?;
///     Err::<(), _>(tarfs::Error::Corrupt("boom".into()))
/// });
/// assert!(failed.is_err());
///
/// let (old, fs) = tarfs::open_tarball_gz(&path, Some(&path), |fs| {
///     let old = fs.read_to_string("a.txt")?;
///     fs.write_file("a.txt", "v2")?;
///     Ok::<_, tarfs::Error>(old)
/// })?;
/// assert_eq!(old, "v1");
/// assert_eq!(fs.read_to_string("a.txt")?, "v2");
/// # Ok(())
/// # }
/// ```
pub fn open_tarball_gz<T, E, F>(
    path: impl AsRef<Path>,
    from: Option<&Path>,
    f: F,
) -> std::result::Result<(T, InMemoryFileSystem), E>
where
    E: From<Error>,
    F: FnOnce(&mut InMemoryFileSystem) -> std::result::Result<T, E>,
{
    let mut fs = match from {
        Some(source) => InMemoryFileSystem::open(source)?,
        None => InMemoryFileSystem::new(),
    };
    let out = f(&mut fs)?;
    fs.write_tarball_gz(path)?;
    Ok((out, fs))
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn assert_send_sync<T: Send + Sync>() {}
    const _: () = assert_send_sync::<InMemoryFileSystem>();
    const _: () = assert_send_sync::<Error>();

    #[test]
    fn debug_summarizes_instead_of_dumping() {
        let mut fs = InMemoryFileSystem::new();
        fs.write_file("a", vec![0u8; 10_000]).unwrap();
        let text = format!("{fs:?}");
        assert!(text.contains("files: 1"), "{text}");
        assert!(text.len() < 100, "{text}");
    }

    #[test]
    fn source_backed_reads_are_zero_copy_slices() {
        let mut fs = InMemoryFileSystem::new();
        fs.write_file("a.txt", "hello").unwrap();
        let tar = Bytes::from(fs.write_tarball(Vec::new()).unwrap());
        let back = InMemoryFileSystem::from_tar_bytes(tar.clone()).unwrap();
        let got = back.read_file("a.txt").unwrap();
        assert_eq!(&got[..], b"hello");
        // Same allocation: the slice points into the shared tarball.
        assert_eq!(got.as_ptr(), tar[512..].as_ptr());
        assert_eq!(back.get("a.txt").unwrap().as_ptr(), tar[512..].as_ptr());
    }

    #[test]
    fn read_dir_handles_neighbours_and_slashes() {
        let mut fs = InMemoryFileSystem::new();
        for p in ["a", "a.txt", "a-b/z", "a/x", "a/b/y", "ab", "b/a/x"] {
            fs.write_file(p, "").unwrap();
        }
        let ls = |p| fs.read_dir(p).collect::<Vec<_>>();
        assert_eq!(ls("a"), ["a/b/y", "a/x"]);
        assert_eq!(ls("a/"), ["a/b/y", "a/x"]);
        assert_eq!(ls("a//"), ["a/b/y", "a/x"]);
        assert_eq!(ls("a/b"), ["a/b/y"]);
        assert_eq!(ls("a/x"), Vec::<&str>::new()); // a file is not a directory
        assert_eq!(ls("b/a"), ["b/a/x"]);
        assert_eq!(ls("zzz"), Vec::<&str>::new());
        assert_eq!(ls("").len(), 7);
        assert_eq!(ls("/").len(), 7);
    }

    #[test]
    fn delete_prefix_counts_and_spares_neighbours() {
        let mut fs = InMemoryFileSystem::new();
        for p in ["a", "a/x", "a/b/y", "ab"] {
            fs.write_file(p, "").unwrap();
        }
        assert_eq!(fs.delete_prefix("a"), 2);
        assert_eq!(fs.paths().collect::<Vec<_>>(), ["a", "ab"]);
        assert_eq!(fs.delete_prefix("a"), 0);
        assert_eq!(fs.delete_prefix(""), 2);
        assert!(fs.is_empty());
    }

    #[test]
    fn write_file_takes_ownership_without_aliasing_the_caller() {
        let mut fs = InMemoryFileSystem::new();
        let mut v = vec![1u8, 2, 3];
        fs.write_file("b.bin", v.clone()).unwrap();
        v[0] = 9;
        assert_eq!(fs.get("b.bin"), Some(&[1u8, 2, 3][..]));
        fs.write_file("w.bin", Bytes::copy_from_slice(&v[1..]))
            .unwrap();
        assert_eq!(fs.get("w.bin"), Some(&[2u8, 3][..]));
    }

    #[test]
    fn write_file_rejects_bad_paths_and_keeps_state() {
        let mut fs = InMemoryFileSystem::new();
        for bad in [
            "", "/abs.txt", "a/../b", "..", ".", "./a", "a//b", "a/", "a\0b", "a\\b",
        ] {
            assert!(
                matches!(fs.write_file(bad, "x"), Err(Error::InvalidPath { .. })),
                "{bad:?}"
            );
        }
        assert!(fs.is_empty());
    }
}
