//! Reader: gzip decompression and tar indexing (spec "Reader").

use std::collections::BTreeMap;
use std::io::{self, Read};

use bytes::Bytes;
use flate2::read::MultiGzDecoder;
use tar::{Archive, EntryType};

use crate::Entry;
use crate::error::{Error, Result};
use crate::path::normalize_member;

/// Decompress a complete gzip stream (every member) into memory.
///
/// `MultiGzDecoder`, not `GzDecoder`: the latter silently stops after the first
/// member of a multi-member stream (`cat a.gz b.gz`, pigz), truncating the tar
/// (CR §6). Errors raised by `source` itself are reported as [`Error::Io`];
/// everything the decoder rejects (bad magic, corrupt deflate data, CRC mismatch,
/// truncation) as [`Error::Corrupt`].
pub(crate) fn gunzip(source: impl Read) -> Result<Vec<u8>> {
    let mut tap = ErrorTap {
        inner: source,
        failed: false,
    };
    let mut raw = Vec::new();
    match MultiGzDecoder::new(&mut tap).read_to_end(&mut raw) {
        Ok(_) => Ok(raw),
        Err(err) if tap.failed => Err(Error::Io(err)),
        Err(err) => Err(Error::Corrupt(format!("gzip: {err}"))),
    }
}

/// Index the regular files of an uncompressed tarball.
///
/// Every key maps to a byte range of `tarball`; nothing is copied. Rules (spec
/// "Reader"): regular and contiguous files only (old-style `'\0'` included); PAX
/// `path`/`size` records and GNU long names are honoured by the `tar` crate;
/// directories, links, devices, FIFOs, PAX globals, unknown types and macOS
/// AppleDouble companions ([`is_apple_double`]) are skipped; GNU sparse members
/// (old `'S'` or PAX `GNU.sparse.*`) are rejected; later duplicates override
/// earlier ones (tar semantics).
pub(crate) fn index(tarball: &Bytes) -> Result<BTreeMap<String, Entry>> {
    let corrupt = |what: &str, err: io::Error| Error::Corrupt(format!("tar: {what}: {err}"));
    let mut entries = BTreeMap::new();
    let mut archive = Archive::new(&tarball[..]);
    let members = archive
        .entries()
        .map_err(|e| corrupt("reading archive", e))?;
    for (number, member) in members.enumerate() {
        let mut member = member.map_err(|e| corrupt(&format!("member #{}", number + 1), e))?;
        let kind = member.header().entry_type();
        if kind.is_gnu_sparse() {
            return Err(sparse_error(&member.path_bytes()));
        }
        if !matches!(kind, EntryType::Regular | EntryType::Continuous) {
            continue;
        }
        if has_pax_sparse_records(&mut member) {
            return Err(sparse_error(&member.path_bytes()));
        }
        let Some(path) = normalize_member(&member.path_bytes())? else {
            continue;
        };
        let (offset, size) = (member.raw_file_position(), member.size());
        let range = usize::try_from(offset)
            .ok()
            .zip(usize::try_from(size).ok())
            .filter(|&(start, len)| {
                start
                    .checked_add(len)
                    .is_some_and(|end| end <= tarball.len())
            });
        let Some((offset, size)) = range else {
            return Err(Error::Corrupt(format!(
                "{path}: data ({size} bytes at offset {offset}) runs past the end of the \
                 {}-byte archive (truncated?)",
                tarball.len()
            )));
        };
        if is_apple_double(&path, &tarball[offset..offset + size]) {
            continue;
        }
        entries.insert(path, Entry::Source { offset, size });
    }
    Ok(entries)
}

/// Magic number that starts every AppleDouble file (`0x00051607`, RFC 1740).
const APPLE_DOUBLE_MAGIC: [u8; 4] = [0x00, 0x05, 0x16, 0x07];

/// Whether a member is a macOS AppleDouble companion: bsdtar on macOS stores a
/// file's extended attributes and resource fork as `._<name>` next to it (CR §6).
/// Both the name and the magic number must match, so an ordinary file that merely
/// has a `._` name is kept (it would otherwise vanish on a read-modify-write).
fn is_apple_double(path: &str, data: &[u8]) -> bool {
    let basename = path.rsplit('/').next().unwrap_or(path);
    basename.starts_with("._") && data.starts_with(&APPLE_DOUBLE_MAGIC)
}

/// GNU tar `--sparse --format=pax` keeps the regular type flag and describes the
/// hole map in `GNU.sparse.*` PAX records; the stored bytes are not the file.
fn has_pax_sparse_records<R: Read>(member: &mut tar::Entry<'_, R>) -> bool {
    // Malformed records are ignored, as the `tar` crate itself does for `path`/`size`.
    match member.pax_extensions() {
        Ok(Some(records)) => records
            .filter_map(io::Result::ok)
            .any(|record| record.key_bytes().starts_with(b"GNU.sparse.")),
        Ok(None) | Err(_) => false,
    }
}

fn sparse_error(path: &[u8]) -> Error {
    Error::Corrupt(format!(
        "{}: GNU sparse members are not supported",
        String::from_utf8_lossy(path)
    ))
}

/// Remembers whether the wrapped reader (rather than the decoder on top of it)
/// produced an error, so the two can be classified differently.
struct ErrorTap<R> {
    inner: R,
    failed: bool,
}

impl<R: Read> Read for ErrorTap<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf).inspect_err(|err| {
            if err.kind() != io::ErrorKind::Interrupted {
                self.failed = true;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "nope"))
        }
    }

    #[test]
    fn source_errors_are_io_and_decoder_errors_are_corrupt() {
        assert!(matches!(
            gunzip(FailingReader),
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::PermissionDenied
        ));
        assert!(
            matches!(gunzip(&b"not gzip at all"[..]), Err(Error::Corrupt(m)) if m.starts_with("gzip:"))
        );
        assert!(matches!(gunzip(&b""[..]), Err(Error::Corrupt(_))));
    }

    #[test]
    fn empty_tar_indexes_to_nothing() {
        let zeros = Bytes::from(vec![0u8; 1024]);
        assert!(index(&zeros).unwrap().is_empty());
        assert!(index(&Bytes::new()).unwrap().is_empty());
    }
}
