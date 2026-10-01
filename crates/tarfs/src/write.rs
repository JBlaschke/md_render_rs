//! Writers: the default hand-rolled USTAR writer (TarFS.jl `write_tarball`), the
//! extended writer (`tar` crate; TarFS.jl `write_tarball_gz_viatar`) and the atomic,
//! reproducible gzip flush (TarFS.jl `_atomic_gz`).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use flate2::{Compression, GzBuilder};
use tar::{Builder, EntryType, Header};

use crate::InMemoryFileSystem;
use crate::error::{Error, Result, io_context};
use crate::ustar::{self, NAME_LEN, PREFIX_OFFSET, ZERO_BLOCK};

/// The default writer: sorted entries, one TarFS.jl-identical ustar header each,
/// data padded to 512 bytes, two zero blocks at the end.
///
/// Every entry is validated before the first byte is written, so an unencodable
/// entry never leaves partial output behind.
pub(crate) fn write_ustar<W: Write>(fs: &InMemoryFileSystem, mut w: W) -> Result<W> {
    for (path, data) in fs.iter() {
        ustar::validate(path, data.len() as u64)?;
    }
    for (path, data) in fs.iter() {
        let size = data.len() as u64;
        w.write_all(&ustar::header(path, size)?)?;
        w.write_all(data)?; // source-backed entries stream from the shared buffer
        w.write_all(&ZERO_BLOCK[..ustar::padding(size)])?;
    }
    w.write_all(&ZERO_BLOCK)?;
    w.write_all(&ZERO_BLOCK)?;
    Ok(w)
}

/// One member of the extended archive, in output order.
enum Member<'a> {
    /// A directory, as `"a/b/"` (trailing slash, the convention of GNU tar and bsdtar).
    Dir(&'a str),
    /// A regular file and its contents.
    File(&'a str, &'a [u8]),
}

impl Member<'_> {
    fn path(&self) -> &str {
        match self {
            Member::Dir(p) | Member::File(p, _) => p,
        }
    }
}

/// The extended writer: like [`write_ustar`], plus an explicit directory entry
/// (mode `0o755`) for every parent directory, and PAX extended headers for paths
/// the ustar name/prefix fields cannot hold and for sizes of 8 GiB or more.
///
/// Members are emitted in byte order of their paths, directories spelled with a
/// trailing `/`. Since a string sorts after each of its prefixes, every directory
/// precedes its contents, as extractors expect. Headers are POSIX ustar with fixed
/// metadata (uid/gid/mtime 0, empty user/group names), so the output is
/// deterministic.
pub(crate) fn write_extended<W: Write>(fs: &InMemoryFileSystem, w: W) -> Result<W> {
    // Every parent directory of every file, as "a/", "a/b/", ...
    let mut dirs = BTreeSet::new();
    for path in fs.paths() {
        for (i, _) in path.match_indices('/') {
            dirs.insert(&path[..=i]);
        }
    }
    // A file "a" next to a file "a/b" cannot be extracted: refuse up front.
    if let Some(dir) = dirs.iter().find(|d| fs.contains(&d[..d.len() - 1])) {
        return Err(Error::InvalidPath {
            path: dir[..dir.len() - 1].to_owned(),
            reason: "is both a file and the parent directory of other files",
        });
    }
    let mut members: Vec<Member<'_>> = dirs.into_iter().map(Member::Dir).collect();
    members.extend(fs.iter().map(|(p, d)| Member::File(p, d)));
    members.sort_unstable_by(|a, b| a.path().cmp(b.path())); // paths are unique

    let mut builder = Builder::new(w);
    for member in &members {
        let (kind, mode, path, data): (_, _, _, &[u8]) = match *member {
            Member::Dir(p) => (EntryType::Directory, 0o755, p, &[]),
            Member::File(p, d) => (EntryType::Regular, 0o644, p, d),
        };
        let (header, pax) = member_headers(kind, mode, path, data.len() as u64);
        if let Some((pax_header, records)) = pax {
            builder.append(&pax_header, &records[..])?;
        }
        builder.append(&header, data)?;
    }
    Ok(builder.into_inner()?)
}

/// The ustar header of one member, preceded (when needed) by a PAX extended
/// header and its records.
fn member_headers(
    kind: EntryType,
    mode: u32,
    path: &str,
    size: u64,
) -> (Header, Option<(Header, Vec<u8>)>) {
    // Base-256 (GNU extension) at >= 8 GiB; the PAX `size` record is the portable form.
    let mut header = fixed_header(kind, mode, size);
    let mut records = Vec::new(); // keys in sorted order, like GNU tar and Tar.jl
    if !set_path(&mut header, path) {
        pax_record(&mut records, "path", path.as_bytes());
    }
    if size >= ustar::MAX_SIZE {
        pax_record(&mut records, "size", size.to_string().as_bytes());
    }
    header.set_cksum();
    if records.is_empty() {
        return (header, None);
    }
    // Not `Builder::append_pax_extensions`: its header leaves mode/uid/gid empty,
    // which strict readers (Tar.jl) reject as malformed. Named like GNU tar's.
    let base = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    let mut pax = fixed_header(EntryType::XHeader, 0o644, records.len() as u64);
    set_path(&mut pax, &format!("PaxHeaders/{base}"));
    pax.set_cksum();
    (header, Some((pax, records)))
}

/// A ustar header with the fixed metadata of every member we write.
fn fixed_header(kind: EntryType, mode: u32, size: u64) -> Header {
    let mut header = Header::new_ustar();
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(size);
    header
}

/// Store `path` in the ustar name/prefix fields, split as the default writer does
/// (independent of the platform's `Path` rules). Returns `false` if it does not
/// fit; the name field then gets a best-effort truncation (cut at a char boundary)
/// for readers that ignore PAX, and the caller must emit a PAX `path` record.
fn set_path(header: &mut Header, path: &str) -> bool {
    let bytes = header.as_mut_bytes();
    if let Some((name, prefix)) = ustar::split_path(path) {
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        bytes[PREFIX_OFFSET..PREFIX_OFFSET + prefix.len()].copy_from_slice(prefix.as_bytes());
        return true;
    }
    let mut cut = NAME_LEN.min(path.len());
    while !path.is_char_boundary(cut) {
        cut -= 1;
    }
    bytes[..cut].copy_from_slice(&path.as_bytes()[..cut]);
    false
}

/// Append one PAX record, `"<len> <key>=<value>\n"`, where `<len>` counts the
/// whole record including its own digits (POSIX.1-2001 pax format).
fn pax_record(out: &mut Vec<u8>, key: &str, value: &[u8]) {
    let rest = key.len() + value.len() + 3; // ' ', '=', '\n'
    let mut len = rest + 1;
    while len != rest + digits(len) {
        len = rest + digits(len);
    }
    out.extend_from_slice(format!("{len} {key}=").as_bytes());
    out.extend_from_slice(value);
    out.push(b'\n');
}

/// Number of decimal digits of `n`.
fn digits(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |d| d as usize + 1)
}

/// Write one complete gzip member to `path` atomically (TarFS.jl `_atomic_gz`).
///
/// The data goes to a temporary file in the destination directory, which is
/// renamed over `path` only once complete; on any error the temporary file is
/// removed and an existing file at `path` is untouched. Atomic with respect to
/// application failure; there is deliberately no `fsync`, so power-loss
/// durability is left to the filesystem, as in TarFS.jl.
///
/// The gzip header is fixed (mtime 0, OS byte 255 "unknown", no file name) and
/// the compressor is pure Rust (miniz_oxide), so equal input gives equal
/// `.tar.gz` bytes on every platform.
pub(crate) fn atomic_gz(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> Result<()>,
) -> Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let mut tmp = tempfile::Builder::new()
        .prefix(".tarfs-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .map_err(|e| io_context(e, "creating a temporary file in", dir))?;
    let tmp_path = tmp.path().to_owned();
    // "writing out.tar.gz via ./.tarfs-XXXX.tmp: No space left on device"
    let write_failed =
        |e: io::Error| io_context(e, &format!("writing {} via", path.display()), &tmp_path);

    let mut gz = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(tmp.as_file_mut(), Compression::default());
    write(&mut gz).map_err(|e| match e {
        Error::Io(e) => write_failed(e),
        other => other,
    })?;
    let file: &mut File = gz.finish().map_err(write_failed)?; // gzip trailer
    file.flush().map_err(write_failed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // NamedTempFile is created 0o600; archives are meant to be shared.
        file.set_permissions(std::fs::Permissions::from_mode(0o644))
            .map_err(|e| io_context(e, "setting permissions of", &tmp_path))?;
    }
    // Same-directory rename: atomic. On failure `PersistError` hands the temporary
    // file back and dropping it deletes it.
    tmp.persist(path).map_err(|e| {
        io_context(
            e.error,
            &format!("renaming {} to", tmp_path.display()),
            path,
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ustar::BLOCK;

    /// A writer that fails after `budget` bytes.
    struct Limited {
        budget: usize,
        written: Vec<u8>,
    }

    impl Write for Limited {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.budget == 0 {
                return Err(io::Error::new(io::ErrorKind::StorageFull, "disk full"));
            }
            let n = buf.len().min(self.budget);
            self.budget -= n;
            self.written.extend_from_slice(&buf[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn sample() -> InMemoryFileSystem {
        let mut fs = InMemoryFileSystem::new();
        fs.write_file("b/c.txt", "see").unwrap();
        fs.write_file("a.txt", "hello").unwrap();
        fs
    }

    #[test]
    fn ustar_layout_is_sorted_padded_and_terminated() {
        let tar = write_ustar(&sample(), Vec::new()).unwrap();
        assert_eq!(tar.len(), 2 * (BLOCK + BLOCK) + 2 * BLOCK);
        assert_eq!(&tar[..5], b"a.txt");
        assert_eq!(&tar[BLOCK..BLOCK + 5], b"hello");
        assert!(tar[BLOCK + 5..2 * BLOCK].iter().all(|&b| b == 0));
        assert_eq!(&tar[2 * BLOCK..2 * BLOCK + 7], b"b/c.txt");
        assert!(tar[tar.len() - 2 * BLOCK..].iter().all(|&b| b == 0));
    }

    #[test]
    fn ustar_writes_nothing_when_any_entry_is_invalid() {
        let mut fs = sample();
        fs.write_file(&format!("zzz{}", "q".repeat(200)), "x")
            .unwrap();
        let mut out = Vec::new();
        assert!(matches!(
            write_ustar(&fs, &mut out),
            Err(Error::PathTooLong(_))
        ));
        assert!(out.is_empty(), "no partial output");
    }

    #[test]
    fn writer_errors_propagate_instead_of_panicking() {
        for budget in [0, 100, 512, 1000, 2047] {
            let w = Limited {
                budget,
                written: Vec::new(),
            };
            assert!(
                matches!(write_ustar(&sample(), w), Err(Error::Io(_))),
                "{budget}"
            );
            let w = Limited {
                budget,
                written: Vec::new(),
            };
            assert!(
                matches!(write_extended(&sample(), w), Err(Error::Io(_))),
                "{budget}"
            );
        }
    }

    #[test]
    fn extended_emits_parent_directories_first() {
        let mut fs = sample();
        fs.write_file("b/d/e.txt", "e").unwrap();
        fs.write_file("b-c/x", "x").unwrap();
        let tar = write_extended(&fs, Vec::new()).unwrap();
        let mut archive = tar::Archive::new(&tar[..]);
        let listing: Vec<(String, EntryType, u32)> = archive
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let h = e.header();
                (
                    String::from_utf8(e.path_bytes().into_owned()).unwrap(),
                    h.entry_type(),
                    h.mode().unwrap(),
                )
            })
            .collect();
        let expected = [
            ("a.txt", EntryType::Regular, 0o644),
            ("b-c/", EntryType::Directory, 0o755),
            ("b-c/x", EntryType::Regular, 0o644),
            ("b/", EntryType::Directory, 0o755),
            ("b/c.txt", EntryType::Regular, 0o644),
            ("b/d/", EntryType::Directory, 0o755),
            ("b/d/e.txt", EntryType::Regular, 0o644),
        ];
        let got: Vec<_> = listing
            .iter()
            .map(|(p, k, m)| (p.as_str(), *k, *m))
            .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn extended_uses_pax_only_when_needed() {
        let long = format!("L/{}/leaf.txt", vec!["s".repeat(40); 7].join("/"));
        let mut fs = sample();
        fs.write_file(&long, "leafdata").unwrap();
        let tar = write_extended(&fs, Vec::new()).unwrap();
        let mut archive = tar::Archive::new(&tar[..]);
        let mut pax_paths = Vec::new();
        for e in archive.entries().unwrap().raw(true) {
            let mut e = e.unwrap();
            if e.header().entry_type() == EntryType::XHeader {
                for rec in e.pax_extensions().unwrap().unwrap() {
                    let rec = rec.unwrap();
                    assert_eq!(rec.key().unwrap(), "path");
                    pax_paths.push(rec.value().unwrap().to_owned());
                }
            }
        }
        // The file and its deepest parent directories need PAX; short paths do not.
        assert!(pax_paths.contains(&long));
        assert!(pax_paths.iter().all(|p| p.len() > 100));
        assert!(!pax_paths.iter().any(|p| p == "a.txt" || p == "b/"));
    }

    #[test]
    fn pax_records_count_their_own_length() {
        let cases = [
            ("path", "a".to_owned()),
            ("path", "x".repeat(91)),
            ("path", "y".repeat(990)),
            ("size", "8589934592".to_owned()),
        ];
        for (key, value) in cases {
            let mut out = Vec::new();
            pax_record(&mut out, key, value.as_bytes());
            let text = String::from_utf8(out.clone()).unwrap();
            let (len, rest) = text.split_once(' ').unwrap();
            assert_eq!(len.parse::<usize>().unwrap(), out.len(), "{text:?}");
            assert_eq!(rest, format!("{key}={value}\n"));
            let parsed = tar::PaxExtensions::new(&out).next().unwrap().unwrap();
            assert_eq!(parsed.key().unwrap(), key);
            assert_eq!(parsed.value_bytes(), value.as_bytes());
        }
        assert_eq!(
            [digits(0), digits(9), digits(10), digits(100)],
            [1, 1, 2, 3]
        );
    }

    #[test]
    fn pax_headers_have_well_formed_numeric_fields() {
        let mut fs = InMemoryFileSystem::new();
        let long = format!("d/{}", "n".repeat(150));
        fs.write_file(&long, "x").unwrap();
        let tar = write_extended(&fs, Vec::new()).unwrap();
        let mut archive = tar::Archive::new(&tar[..]);
        let raw: Vec<Vec<u8>> = archive
            .entries()
            .unwrap()
            .raw(true)
            .map(|e| e.unwrap().header().as_bytes().to_vec())
            .collect();
        assert_eq!(raw.len(), 3, "d/, the PAX header, the file");
        let pax = tar::Header::from_byte_slice(&raw[1]);
        assert_eq!(pax.entry_type(), EntryType::XHeader);
        assert_eq!(pax.mode().unwrap(), 0o644);
        assert_eq!(pax.uid().unwrap(), 0);
        assert_eq!(pax.gid().unwrap(), 0);
        assert_eq!(pax.mtime().unwrap(), 0);
        let pax_name = format!("PaxHeaders/{}", "n".repeat(150));
        assert_eq!(pax.path_bytes().as_ref(), &pax_name.as_bytes()[..100]);
        let file = tar::Header::from_byte_slice(&raw[2]);
        assert_eq!(file.path_bytes().as_ref(), &long.as_bytes()[..100]);
    }

    #[test]
    fn huge_sizes_get_a_pax_size_record_and_base256_field() {
        // Header logic only: never allocate 8 GiB.
        let size = 10 * (1u64 << 30);
        let (header, pax) = member_headers(EntryType::Regular, 0o644, "huge.bin", size);
        assert_eq!(header.entry_size().unwrap(), size);
        assert_eq!(header.as_bytes()[124] & 0x80, 0x80, "base-256 marker");
        assert_eq!(header.path_bytes().as_ref(), b"huge.bin");
        let (pax_header, records) = pax.expect("PAX header for >= 8 GiB");
        assert_eq!(records, b"20 size=10737418240\n");
        assert_eq!(pax_header.entry_size().unwrap(), records.len() as u64);
        assert_eq!(pax_header.path_bytes().as_ref(), b"PaxHeaders/huge.bin");

        let (header, pax) =
            member_headers(EntryType::Regular, 0o644, "ok.bin", ustar::MAX_SIZE - 1);
        assert!(pax.is_none());
        assert_eq!(&header.as_bytes()[124..136], b"77777777777\0");
    }

    #[test]
    fn extended_rejects_file_directory_conflicts() {
        let mut fs = InMemoryFileSystem::new();
        fs.write_file("a", "file").unwrap();
        fs.write_file("a/b", "nested").unwrap();
        assert!(matches!(
            write_extended(&fs, Vec::new()),
            Err(Error::InvalidPath { ref path, .. }) if path == "a"
        ));
    }

    #[test]
    fn atomic_gz_failure_names_the_target_and_leaves_no_litter() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.tar.gz");
        std::fs::write(&target, b"original").unwrap();
        let err = atomic_gz(&target, |w| {
            w.write_all(b"partial")?;
            Err(io::Error::new(io::ErrorKind::StorageFull, "disk full").into())
        })
        .unwrap_err();
        let Error::Io(e) = err else {
            panic!("expected Io, got {err:?}");
        };
        assert_eq!(e.kind(), io::ErrorKind::StorageFull);
        let msg = e.to_string();
        assert!(
            msg.starts_with("writing ") && msg.contains("out.tar.gz via "),
            "{msg}"
        );
        assert!(
            msg.contains(".tarfs-") && msg.ends_with(": disk full"),
            "{msg}"
        );
        // Non-I/O errors pass through unchanged.
        let err = atomic_gz(&target, |_| Err(Error::PathTooLong("p".into()))).unwrap_err();
        assert!(matches!(err, Error::PathTooLong(ref p) if p == "p"));
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["out.tar.gz"]);
    }

    #[test]
    fn extended_of_empty_fs_is_two_zero_blocks() {
        let tar = write_extended(&InMemoryFileSystem::new(), Vec::new()).unwrap();
        assert_eq!(tar, vec![0u8; 2 * BLOCK]);
    }
}
