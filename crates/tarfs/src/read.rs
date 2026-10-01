//! Reader: gzip decompression and tar indexing (spec "Reader").

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Read};

use bytes::Bytes;
use flate2::bufread::GzDecoder;
use tar::{Archive, EntryType};

use crate::error::{Error, Result};
use crate::path::normalize_member;
use crate::{Entry, Limits};

/// Decompress a complete gzip stream (every member) into memory, stopping with
/// [`Error::TooLarge`] as soon as the output exceeds
/// [`Limits::max_decompressed_size`].
///
/// Every member is decoded, not just the first: a plain `GzDecoder` silently stops
/// after the first member of a multi-member stream (`cat a.gz b.gz`, pigz),
/// truncating the tar (CR §6). Zero bytes after the last member (block padding
/// added by some devices and downloads) are ignored, as gzip(1) does; any other
/// trailing data is an error. Errors raised by `source` itself are reported as
/// [`Error::Io`]; everything the decoder rejects (bad magic, corrupt deflate data,
/// CRC mismatch, truncation, trailing garbage) as [`Error::Corrupt`].
pub(crate) fn gunzip(source: impl Read, limits: Limits) -> Result<Vec<u8>> {
    let max = limits.max_decompressed_size.unwrap_or(u64::MAX);
    let mut input = BufReader::new(ErrorTap {
        inner: source,
        failed: false,
    });
    let mut raw = Vec::new();
    match decode_members(&mut input, &mut raw, max) {
        Ok(()) if raw.len() as u64 > max => Err(Error::TooLarge { limit: max }),
        Ok(()) => Ok(raw),
        Err(err) if input.get_ref().failed => Err(Error::Io(err)),
        Err(err) => Err(Error::Corrupt(format!("gzip: {err}"))),
    }
}

/// Decode the gzip members of `input` into `out` until the end of the input or of
/// its zero padding, or until `out` holds more than `max` bytes (one byte more than
/// allowed proves the limit is exceeded while keeping memory bounded by it: a gzip
/// bomb expands ~1000-fold).
fn decode_members(input: &mut impl BufRead, out: &mut Vec<u8>, max: u64) -> io::Result<()> {
    loop {
        let budget = max.saturating_add(1).saturating_sub(out.len() as u64);
        // Reads exactly one member and leaves whatever follows it in `input`.
        GzDecoder::new(&mut *input).take(budget).read_to_end(out)?;
        if out.len() as u64 > max {
            return Ok(());
        }
        match peek(input)? {
            None => return Ok(()),
            // 0x00 cannot start a member (the magic is 1f 8b): padding.
            Some(0) => return skip_zero_padding(input),
            // Another member; a bad header is reported by its decoder.
            Some(_) => {}
        }
    }
}

/// The next byte of `input` without consuming it (`None` at the end).
fn peek(input: &mut impl BufRead) -> io::Result<Option<u8>> {
    loop {
        match input.fill_buf() {
            Ok(buf) => return Ok(buf.first().copied()),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

/// Consume the rest of `input`, which must be all zero bytes.
fn skip_zero_padding(input: &mut impl BufRead) -> io::Result<()> {
    loop {
        let n = match input.fill_buf() {
            Ok([]) => return Ok(()),
            Ok(buf) if buf.iter().all(|&b| b == 0) => buf.len(),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected data after the zero padding at the end of the stream",
                ));
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        input.consume(n);
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

    fn gzip(data: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(data).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn source_errors_are_io_and_decoder_errors_are_corrupt() {
        let none = Limits::default();
        assert!(matches!(
            gunzip(FailingReader, none),
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::PermissionDenied
        ));
        assert!(
            matches!(gunzip(&b"not gzip at all"[..], none), Err(Error::Corrupt(m)) if m.starts_with("gzip:"))
        );
        assert!(matches!(gunzip(&b""[..], none), Err(Error::Corrupt(_))));
    }

    #[test]
    fn decompressed_size_limit_is_exact_and_spans_members() {
        let gz = gzip(&[0u8; 10_000]);
        let limit = |n| Limits::default().with_max_decompressed_size(n);
        assert_eq!(gunzip(&gz[..], Limits::default()).unwrap().len(), 10_000);
        assert_eq!(gunzip(&gz[..], limit(10_000)).unwrap().len(), 10_000);
        assert!(matches!(
            gunzip(&gz[..], limit(9_999)),
            Err(Error::TooLarge { limit: 9_999 })
        ));
        assert!(matches!(
            gunzip(&gz[..], limit(0)),
            Err(Error::TooLarge { limit: 0 })
        ));
        let mut two = gzip(&[1u8; 600]);
        two.extend(gzip(&[2u8; 600]));
        assert_eq!(gunzip(&two[..], limit(1200)).unwrap().len(), 1200);
        assert!(matches!(
            gunzip(&two[..], limit(1000)),
            Err(Error::TooLarge { limit: 1000 })
        ));
    }

    #[test]
    fn members_and_zero_padding() {
        let none = Limits::default();
        let mut two = gzip(b"first,");
        two.extend(gzip(b""));
        two.extend(gzip(b"second"));
        assert_eq!(gunzip(&two[..], none).unwrap(), b"first,second");
        // Zero padding after the last member is ignored, however long.
        for pad in [1, 511, 512, 10_000] {
            let mut padded = two.clone();
            padded.resize(two.len() + pad, 0);
            assert_eq!(gunzip(&padded[..], none).unwrap(), b"first,second", "{pad}");
        }
        // Anything else after the last member is not.
        for trailer in [&b"x"[..], b"\0\0\0x", b"\x1f\x8b", b"\x1f"] {
            let mut bad = two.clone();
            bad.extend_from_slice(trailer);
            assert!(
                matches!(gunzip(&bad[..], none), Err(Error::Corrupt(m)) if m.starts_with("gzip:")),
                "{trailer:?}"
            );
        }
        // Padding does not make an empty or zero-only input a gzip stream.
        assert!(matches!(
            gunzip(&[0u8; 512][..], none),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn empty_tar_indexes_to_nothing() {
        let zeros = Bytes::from(vec![0u8; 1024]);
        assert!(index(&zeros).unwrap().is_empty());
        assert!(index(&Bytes::new()).unwrap().is_empty());
    }
}
