//! tarfs: an in-memory filesystem backed by a `.tar.gz` archive (Rust port of TarFS.jl).
//!
//! SKELETON PLACEHOLDER: functional but minimal. The `tarfs` owner replaces this with the full
//! TarFS.jl-parity implementation (hand-rolled deterministic USTAR writer, extended writer,
//! `open_tarball_gz`, exhaustive tests). The public API below is the frozen contract
//! (docs/DESIGN.md, .dev/design/tarfs-spec.md).
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use bytes::Bytes;
use flate2::Compression;
use flate2::read::MultiGzDecoder;
use tar::{Archive, EntryType, Header, HeaderMode};

/// Errors produced by tarfs.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("no such file in archive: {0}")]
    NotFound(String),
    #[error("invalid path {path:?}: {reason}")]
    InvalidPath { path: String, reason: &'static str },
    #[error("path too long for USTAR (use write_tarball_gz_extended): {0}")]
    PathTooLong(String),
    #[error("entry too large for USTAR: {path} ({size} bytes)")]
    EntryTooLarge { path: String, size: u64 },
    #[error("corrupt archive: {0}")]
    Corrupt(String),
    #[error("file is not valid UTF-8: {0}")]
    Utf8(String),
}

/// Result alias for tarfs operations.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
enum Entry {
    /// Backed by the source tarball: byte range into `InMemoryFileSystem::tarball`.
    Source { offset: usize, size: usize },
    /// Staged by `write_file`; owns its bytes.
    Owned(Bytes),
}

/// An in-memory filesystem backed by a (decompressed) tarball.
#[derive(Debug, Clone, Default)]
pub struct InMemoryFileSystem {
    tarball: Bytes,
    entries: BTreeMap<String, Entry>,
}

/// Short alias.
pub type TarFs = InMemoryFileSystem;

/// TarFS.jl `_check_path`: relative, '/'-separated, no "", ".", ".." segments, no NUL.
pub fn check_path(path: &str) -> Result<()> {
    let bad = |reason| Err(Error::InvalidPath { path: path.to_owned(), reason });
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
    if path.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return bad("empty, '.' or '..' segment");
    }
    Ok(())
}

fn normalize_member(raw: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(raw).ok()?;
    let s = s.trim_start_matches("./").trim_end_matches('/');
    check_path(s).ok()?;
    Some(s.to_owned())
}

impl InMemoryFileSystem {
    /// Empty, writable filesystem with no backing tarball.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load `path` (.tar.gz), decompress it into memory and index its regular files.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let gz = fs::read(path)?;
        Self::from_tar_gz_bytes(&gz)
    }

    /// Decompress a `.tar.gz` stream (multi-member gzip supported) and index it.
    pub fn from_tar_gz_reader(r: impl Read) -> Result<Self> {
        let mut raw = Vec::new();
        MultiGzDecoder::new(r).read_to_end(&mut raw)?;
        Self::from_tar_bytes(raw)
    }

    /// Decompress `.tar.gz` bytes and index them.
    pub fn from_tar_gz_bytes(gz: &[u8]) -> Result<Self> {
        Self::from_tar_gz_reader(gz)
    }

    /// Index an uncompressed tarball held in memory.
    pub fn from_tar_bytes(buf: impl Into<Bytes>) -> Result<Self> {
        let buf: Bytes = buf.into();
        let mut entries = BTreeMap::new();
        {
            let mut ar = Archive::new(&buf[..]);
            for e in ar.entries()? {
                let e = e?;
                if !matches!(e.header().entry_type(), EntryType::Regular | EntryType::Continuous) {
                    continue;
                }
                let Some(p) = normalize_member(&e.path_bytes()) else { continue };
                if p.rsplit('/').next().is_some_and(|b| b.starts_with("._")) {
                    continue;
                }
                let offset = e.raw_file_position() as usize;
                let size = e.size() as usize;
                if offset.checked_add(size).is_none_or(|end| end > buf.len()) {
                    return Err(Error::Corrupt(format!("{p}: data out of bounds")));
                }
                entries.insert(p, Entry::Source { offset, size });
            }
        }
        Ok(Self { tarball: buf, entries })
    }

    /// Contents of `path` (zero-copy, reference-counted slice).
    pub fn read_file(&self, path: &str) -> Result<Bytes> {
        match self.entries.get(path) {
            Some(Entry::Source { offset, size }) => Ok(self.tarball.slice(*offset..*offset + *size)),
            Some(Entry::Owned(b)) => Ok(b.clone()),
            None => Err(Error::NotFound(path.to_owned())),
        }
    }

    /// Contents of `path` as UTF-8 text.
    pub fn read_to_string(&self, path: &str) -> Result<String> {
        let b = self.read_file(path)?;
        String::from_utf8(b.to_vec()).map_err(|_| Error::Utf8(path.to_owned()))
    }

    /// Borrowed contents of `path`, if present.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        match self.entries.get(path)? {
            Entry::Source { offset, size } => Some(&self.tarball[*offset..*offset + *size]),
            Entry::Owned(b) => Some(&b[..]),
        }
    }

    /// Whether `path` exists.
    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// Number of files.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when there are no files.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sorted paths.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Size of `path` in bytes.
    pub fn file_size(&self, path: &str) -> Option<u64> {
        self.get(path).map(|b| b.len() as u64)
    }

    /// Sorted paths below `prefix/` (directory-style listing; recursive).
    pub fn read_dir<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let p = prefix.trim_end_matches('/');
        self.paths().filter(move |k| {
            p.is_empty() || (k.len() > p.len() && k.starts_with(p) && k.as_bytes()[p.len()] == b'/')
        })
    }

    /// Stage (or overwrite) a file.
    pub fn write_file(&mut self, path: &str, content: impl Into<Bytes>) -> Result<()> {
        check_path(path)?;
        self.entries.insert(path.to_owned(), Entry::Owned(content.into()));
        Ok(())
    }

    /// Remove `path`; returns whether it existed.
    pub fn delete_file(&mut self, path: &str) -> bool {
        self.entries.remove(path).is_some()
    }

    /// Remove every file below `prefix/`; returns how many were removed.
    pub fn delete_prefix(&mut self, prefix: &str) -> usize {
        let doomed: Vec<String> = self.read_dir(prefix).map(str::to_owned).collect();
        for p in &doomed {
            self.entries.remove(p);
        }
        doomed.len()
    }

    /// Write the filesystem as an uncompressed tarball to `w`.
    pub fn write_tarball<W: Write>(&self, w: W) -> Result<W> {
        let mut b = tar::Builder::new(w);
        b.mode(HeaderMode::Deterministic);
        for path in self.entries.keys() {
            let data = self.read_file(path)?;
            let mut h = Header::new_gnu();
            h.set_entry_type(EntryType::Regular);
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_mtime(0);
            h.set_uid(0);
            h.set_gid(0);
            b.append_data(&mut h, path, &data[..])?;
        }
        Ok(b.into_inner()?)
    }

    /// Flush to a `.tar.gz` at `path`, atomically (temp file in the same directory + rename).
    pub fn write_tarball_gz(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let dir = match path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        let tmp = tempfile::NamedTempFile::new_in(dir)?;
        {
            let gz = flate2::GzBuilder::new()
                .mtime(0)
                .operating_system(255)
                .write(tmp.as_file(), Compression::default());
            let gz = self.write_tarball(gz)?;
            gz.finish()?.flush()?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o644))?;
        }
        tmp.persist(path).map_err(|e| Error::Io(e.error))?;
        Ok(())
    }

    /// Flush via the `tar` crate with long-name support and explicit directory entries.
    pub fn write_tarball_gz_extended(&self, path: impl AsRef<Path>) -> Result<()> {
        self.write_tarball_gz(path)
    }
}

/// Run `f` on a writable filesystem (seeded from `from` if given), then flush it to `path`
/// atomically — only if `f` returns `Ok`. `from == path` (read-modify-write) is safe.
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
        Some(p) => InMemoryFileSystem::open(p)?,
        None => InMemoryFileSystem::new(),
    };
    let out = f(&mut fs)?;
    fs.write_tarball_gz(path)?;
    Ok((out, fs))
}
