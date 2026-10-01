//! Reader rules against hand-crafted archives: long names, odd member types,
//! unsafe paths, multi-member gzip, and corrupt input (which must never panic).

mod common;

use std::io::Write;

use common::{Rng, contents, fs_from, tree};
use flate2::Compression;
use flate2::write::GzEncoder;
use tar::{Builder, EntryType, Header};
use tarfs::{Error, InMemoryFileSystem};

/// One raw ustar member: header (with `name` stored verbatim, possibly invalid)
/// followed by padded data.
fn member(name: &[u8], typeflag: u8, data: &[u8]) -> Vec<u8> {
    let mut h = Header::new_ustar();
    h.set_mode(0o644);
    h.set_size(data.len() as u64);
    h.set_mtime(0);
    h.as_mut_bytes()[..name.len()].copy_from_slice(name);
    h.as_mut_bytes()[156] = typeflag;
    h.set_cksum();
    let mut out = h.as_bytes().to_vec();
    out.extend_from_slice(data);
    out.resize(out.len().div_ceil(512) * 512, 0);
    out
}

fn archive(members: &[Vec<u8>]) -> Vec<u8> {
    let mut out = members.concat();
    out.extend_from_slice(&[0u8; 1024]);
    out
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(data).unwrap();
    gz.finish().unwrap()
}

fn corrupt_message(result: tarfs::Result<InMemoryFileSystem>) -> String {
    match result {
        Err(Error::Corrupt(msg)) => msg,
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn gnu_long_names_and_pax_paths_are_honoured() {
    let long = format!("deep/{}/leaf.txt", vec!["d".repeat(60); 5].join("/"));
    let mut b = Builder::new(Vec::new());
    // GNU: `append_data` emits a ././@LongLink entry for names over 100 bytes.
    let mut h = Header::new_gnu();
    h.set_size(4);
    h.set_mode(0o644);
    b.append_data(&mut h, &long, &b"gnu!"[..]).unwrap();
    // PAX: an `x` header carrying `path` (and an overriding `size`).
    let pax_path = format!("pax/{}", "p".repeat(200));
    b.append_pax_extensions([("path", pax_path.as_bytes()), ("size", &b"3"[..])])
        .unwrap();
    let mut h = Header::new_ustar();
    h.set_path("truncated").unwrap();
    h.set_size(3);
    h.set_entry_type(EntryType::Regular);
    h.set_cksum();
    b.append(&h, &b"pax"[..]).unwrap();
    let fs = InMemoryFileSystem::from_tar_bytes(b.into_inner().unwrap()).unwrap();
    assert_eq!(
        fs.paths().collect::<Vec<_>>(),
        [long.as_str(), pax_path.as_str()]
    );
    assert_eq!(fs.get(&long), Some(&b"gnu!"[..]));
    assert_eq!(fs.get(&pax_path), Some(&b"pax"[..]));
}

#[test]
fn only_regular_files_are_indexed() {
    let tar = archive(&[
        member(b"dir/", b'5', b""),
        member(b"dir/file.txt", b'0', b"regular"),
        member(b"old.txt", b'\0', b"old-style regular"),
        member(b"contig.bin", b'7', b"contiguous"),
        member(b"link", b'1', b""),
        member(b"symlink", b'2', b""),
        member(b"chardev", b'3', b""),
        member(b"blockdev", b'4', b""),
        member(b"fifo", b'6', b""),
        member(b"olddir/", b'\0', b""), // V7 directory: regular flag, trailing '/'
        member(b"volume", b'V', b""),   // GNU volume label: unknown, skipped
        member(b"global", b'g', b"11 path=x\n"),
    ]);
    let fs = InMemoryFileSystem::from_tar_bytes(tar).unwrap();
    assert_eq!(
        fs.paths().collect::<Vec<_>>(),
        ["contig.bin", "dir/file.txt", "old.txt"]
    );
    assert_eq!(fs.read_to_string("old.txt").unwrap(), "old-style regular");
    assert_eq!(fs.read_to_string("contig.bin").unwrap(), "contiguous");
}

/// The body of an AppleDouble file: magic `0x00051607`, version 2, 16 filler bytes,
/// no entries (what bsdtar on macOS stores as `._<name>`).
fn apple_double(rest: &[u8]) -> Vec<u8> {
    let mut body = vec![0x00, 0x05, 0x16, 0x07, 0x00, 0x02, 0x00, 0x00];
    body.extend_from_slice(&[0u8; 18]);
    body.extend_from_slice(rest);
    body
}

#[test]
fn dot_slash_prefixes_and_apple_double_files() {
    let tar = archive(&[
        member(b"./", b'5', b""),
        member(b".", b'0', b""),
        member(b"._.", b'0', &apple_double(b"of .")),
        member(b"./a.txt", b'0', b"a"),
        member(b"./._a.txt", b'0', &apple_double(b"")),
        member(b"./sub/._b.txt", b'0', &apple_double(b"xattrs")),
        member(b"././sub/b.txt", b'0', b"b"),
        member(b"./.hidden", b'0', b"dotfile"),
    ]);
    let fs = InMemoryFileSystem::from_tar_bytes(tar).unwrap();
    assert_eq!(
        fs.paths().collect::<Vec<_>>(),
        [".hidden", "a.txt", "sub/b.txt"]
    );
}

#[test]
fn files_named_like_apple_double_but_without_its_magic_are_kept() {
    let tar = archive(&[
        member(b"._notes.txt", b'0', b"not an AppleDouble file"),
        member(b"dir/._config", b'0', b""),
        member(b"dir/._", b'0', &[0x00, 0x05, 0x16]), // shorter than the magic
        member(b"._magic", b'0', &apple_double(b"")),
    ]);
    let fs = InMemoryFileSystem::from_tar_bytes(tar).unwrap();
    assert_eq!(
        fs.paths().collect::<Vec<_>>(),
        ["._notes.txt", "dir/._", "dir/._config"]
    );
    assert_eq!(
        fs.read_to_string("._notes.txt").unwrap(),
        "not an AppleDouble file"
    );
    // The magic alone does not make a file AppleDouble: the name must match too.
    let tar = archive(&[member(b"a.bin", b'0', &apple_double(b""))]);
    assert!(
        InMemoryFileSystem::from_tar_bytes(tar)
            .unwrap()
            .contains("a.bin")
    );
}

#[test]
fn files_named_like_apple_double_survive_a_round_trip_and_read_modify_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.tar.gz");
    let mut fs = InMemoryFileSystem::new();
    fs.write_file("._notes.txt", "important").unwrap();
    fs.write_file("dir/._config", "also important").unwrap();
    fs.write_file("keep.txt", "k").unwrap();
    fs.write_tarball_gz(&path).unwrap();
    let back = InMemoryFileSystem::open(&path).unwrap();
    assert_eq!(
        back.paths().collect::<Vec<_>>(),
        ["._notes.txt", "dir/._config", "keep.txt"]
    );

    // A foreign archive with a real file named `._data`, updated in place.
    std::fs::write(
        &path,
        gzip(&archive(&[
            member(b"._data", b'0', b"precious"),
            member(b"other.txt", b'0', b"x"),
        ])),
    )
    .unwrap();
    tarfs::open_tarball_gz(&path, Some(&path), |fs| fs.write_file("new.txt", "n")).unwrap();
    let back = InMemoryFileSystem::open(&path).unwrap();
    assert_eq!(
        back.paths().collect::<Vec<_>>(),
        ["._data", "new.txt", "other.txt"]
    );
    assert_eq!(back.read_to_string("._data").unwrap(), "precious");
}

#[test]
fn later_duplicates_override_earlier_ones() {
    let tar = archive(&[
        member(b"a.txt", b'0', b"first"),
        member(b"./a.txt", b'0', b"second"),
        member(b"b.txt", b'0', b"only"),
    ]);
    let fs = InMemoryFileSystem::from_tar_bytes(tar).unwrap();
    assert_eq!(fs.read_to_string("a.txt").unwrap(), "second");
    assert_eq!(fs.len(), 2);
}

#[test]
fn unsafe_member_paths_are_corrupt() {
    for bad in [
        &b"../evil.txt"[..],
        b"/etc/passwd",
        b"a/../../b",
        b"a//b",
        b"a/./b",
        b"back\\slash",
    ] {
        let tar = archive(&[member(b"fine.txt", b'0', b"ok"), member(bad, b'0', b"x")]);
        let msg = corrupt_message(InMemoryFileSystem::from_tar_bytes(tar));
        assert!(msg.contains("unsafe member path"), "{msg}");
    }
    // ...but unsafe *non-file* members are simply skipped.
    let tar = archive(&[member(b"../up", b'5', b""), member(b"/abs", b'2', b"")]);
    assert!(InMemoryFileSystem::from_tar_bytes(tar).unwrap().is_empty());
}

#[test]
fn non_utf8_member_paths_are_corrupt_not_lossy() {
    let tar = archive(&[member(b"caf\xe9.txt", b'0', b"latin-1 name")]);
    let msg = corrupt_message(InMemoryFileSystem::from_tar_bytes(tar));
    assert!(msg.contains("UTF-8"), "{msg}");
}

#[test]
fn gnu_sparse_members_are_rejected() {
    // Old GNU sparse ('S'): one data block covering the whole file.
    let mut h = Header::new_gnu();
    h.set_path("sparse.bin").unwrap();
    h.set_entry_type(EntryType::GNUSparse);
    h.set_size(5);
    let gnu = h.as_gnu_mut().unwrap();
    gnu.set_real_size(5);
    gnu.sparse[0].set_offset(0);
    gnu.sparse[0].set_length(5);
    h.set_cksum();
    let mut b = Builder::new(Vec::new());
    b.append(&h, &b"12345"[..]).unwrap();
    let msg = corrupt_message(InMemoryFileSystem::from_tar_bytes(b.into_inner().unwrap()));
    assert!(msg.contains("sparse"), "{msg}");

    // PAX sparse (GNU tar --sparse --format=pax): regular type flag + GNU.sparse.*.
    let mut b = Builder::new(Vec::new());
    b.append_pax_extensions([
        ("GNU.sparse.major", &b"1"[..]),
        ("GNU.sparse.minor", &b"0"[..]),
        ("GNU.sparse.name", &b"sparse.bin"[..]),
        ("GNU.sparse.realsize", &b"5"[..]),
    ])
    .unwrap();
    let mut h = Header::new_ustar();
    h.set_path("GNUSparseFile.0/sparse.bin").unwrap();
    h.set_size(512);
    h.set_cksum();
    b.append(&h, &[0u8; 512][..]).unwrap();
    let msg = corrupt_message(InMemoryFileSystem::from_tar_bytes(b.into_inner().unwrap()));
    assert!(msg.contains("sparse"), "{msg}");
}

#[test]
fn multi_member_gzip_is_read_completely() {
    let tar = fs_from(&tree()).write_tarball(Vec::new()).unwrap();
    let mid = tar.len() / 2 + 100; // split inside a member, not at a block boundary
    let mut gz = gzip(&tar[..mid]);
    gz.extend(gzip(&tar[mid..]));
    let fs = InMemoryFileSystem::from_tar_gz_bytes(&gz).unwrap();
    assert_eq!(contents(&fs), tree());
    // Also via a reader that hands out one byte at a time.
    struct Trickle<'a>(&'a [u8]);
    impl std::io::Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Some((&first, rest)) = self.0.split_first() else {
                return Ok(0);
            };
            if buf.is_empty() {
                return Ok(0);
            }
            buf[0] = first;
            self.0 = rest;
            Ok(1)
        }
    }
    let fs = InMemoryFileSystem::from_tar_gz_reader(Trickle(&gz)).unwrap();
    assert_eq!(contents(&fs), tree());
}

#[test]
fn truncated_input_is_corrupt() {
    let tar = fs_from(&tree()).write_tarball(Vec::new()).unwrap();
    let gz = gzip(&tar);
    for cut in [0, 1, 9, 10, 20, gz.len() / 2, gz.len() - 8, gz.len() - 1] {
        let msg = corrupt_message(InMemoryFileSystem::from_tar_gz_bytes(&gz[..cut]));
        assert!(msg.starts_with("gzip:"), "cut {cut}: {msg}");
    }
    // A complete gzip stream around a truncated tar: caught by the tar layer.
    for cut in [100, 512 + 3, 2048 + 7] {
        let gz = gzip(&tar[..cut]);
        assert!(
            matches!(
                InMemoryFileSystem::from_tar_gz_bytes(&gz),
                Err(Error::Corrupt(_))
            ),
            "cut {cut}"
        );
    }
    // Truncated exactly after the header of a non-empty member.
    let msg = corrupt_message(InMemoryFileSystem::from_tar_bytes(tar[..512].to_vec()));
    assert!(msg.contains("a.txt"), "{msg}");
}

#[test]
fn garbage_and_bad_checksums_are_corrupt() {
    assert!(matches!(
        InMemoryFileSystem::from_tar_gz_bytes(b"definitely not gzip"),
        Err(Error::Corrupt(_))
    ));
    // Valid gzip, garbage tar.
    let gz = gzip(&[0x55u8; 2048]);
    assert!(matches!(
        InMemoryFileSystem::from_tar_gz_bytes(&gz),
        Err(Error::Corrupt(_))
    ));
    // One flipped byte in a header breaks its checksum.
    let mut tar = fs_from(&tree()).write_tarball(Vec::new()).unwrap();
    tar[3] ^= 0x01;
    let msg = corrupt_message(InMemoryFileSystem::from_tar_bytes(tar));
    assert!(msg.contains("checksum"), "{msg}");
    // A flipped byte in the gzip payload fails the CRC or the deflate stream.
    let mut gz = gzip(&fs_from(&tree()).write_tarball(Vec::new()).unwrap());
    let i = gz.len() / 2;
    gz[i] ^= 0xff;
    assert!(matches!(
        InMemoryFileSystem::from_tar_gz_bytes(&gz),
        Err(Error::Corrupt(_))
    ));
}

#[test]
fn data_after_the_last_gzip_member_is_corrupt() {
    // Documented limitation (README "Limitations"): unlike GNU gzip, which warns
    // about and ignores trailing zero bytes, the decoder reads anything after the
    // last member as the start of a malformed next member.
    let gz = gzip(&fs_from(&tree()).write_tarball(Vec::new()).unwrap());
    for trailer in [&[0u8; 512][..], b"garbage"] {
        let mut padded = gz.clone();
        padded.extend_from_slice(trailer);
        let msg = corrupt_message(InMemoryFileSystem::from_tar_gz_bytes(&padded));
        assert!(msg.starts_with("gzip:"), "{msg}");
    }
}

#[test]
fn open_reports_missing_and_corrupt_files_with_their_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.tar.gz");
    match InMemoryFileSystem::open(&missing) {
        Err(Error::Io(e)) => {
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
            assert!(e.to_string().contains("missing.tar.gz"), "{e}");
        }
        other => panic!("expected Io, got {other:?}"),
    }
    let bad = dir.path().join("bad.tar.gz");
    std::fs::write(&bad, b"nope").unwrap();
    let msg = corrupt_message(InMemoryFileSystem::open(&bad));
    assert!(msg.contains("bad.tar.gz") && msg.contains("gzip"), "{msg}");
    let evil = dir.path().join("evil.tar.gz");
    std::fs::write(&evil, gzip(&archive(&[member(b"../x", b'0', b"")]))).unwrap();
    let msg = corrupt_message(InMemoryFileSystem::open(&evil));
    assert!(msg.contains("evil.tar.gz") && msg.contains("../x"), "{msg}");
    // A directory is not an archive.
    assert!(matches!(
        InMemoryFileSystem::open(dir.path()),
        Err(Error::Io(_))
    ));
}

#[test]
fn random_corruption_never_panics() {
    let tar = fs_from(&tree()).write_tarball_extended(Vec::new()).unwrap();
    let gz = gzip(&tar);
    let mut rng = Rng::new(0x7a7f5);
    for round in 0..1500 {
        let (mut t, mut g) = (tar.clone(), gz.clone());
        for _ in 0..1 + rng.below(8) {
            let i = rng.below(t.len());
            t[i] = rng.next_u64() as u8;
            let j = rng.below(g.len());
            g[j] = rng.next_u64() as u8;
        }
        if round % 3 == 0 {
            t.truncate(rng.below(t.len()));
            g.truncate(rng.below(g.len()));
        }
        // Any outcome but a panic is fine; successful parses must be readable.
        for fs in [
            InMemoryFileSystem::from_tar_bytes(t).ok(),
            InMemoryFileSystem::from_tar_gz_bytes(&g).ok(),
        ]
        .into_iter()
        .flatten()
        {
            for (path, data) in fs.iter() {
                assert_eq!(fs.read_file(path).unwrap().len(), data.len());
                tarfs::check_path(path).unwrap();
            }
            fs.write_tarball_extended(Vec::new()).ok();
        }
    }
}

#[test]
fn reads_archives_written_by_the_tar_crate() {
    // An independent writer: tar::Builder with GNU headers in deterministic mode.
    let mut b = Builder::new(Vec::new());
    b.mode(tar::HeaderMode::Deterministic);
    for (p, c) in tree() {
        let mut h = Header::new_gnu();
        h.set_size(c.len() as u64);
        h.set_mode(0o644);
        b.append_data(&mut h, &p, &c[..]).unwrap();
    }
    let fs = InMemoryFileSystem::from_tar_bytes(b.into_inner().unwrap()).unwrap();
    assert_eq!(contents(&fs), tree());
}

#[test]
fn concurrent_reads_from_many_threads() {
    let tar = fs_from(&tree()).write_tarball(Vec::new()).unwrap();
    let fs = std::sync::Arc::new(InMemoryFileSystem::from_tar_bytes(tar).unwrap());
    let expected = std::sync::Arc::new(tree());
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let (fs, expected) = (fs.clone(), expected.clone());
            std::thread::spawn(move || {
                for round in 0..200 {
                    for (i, (p, c)) in expected.iter().enumerate() {
                        if (i + t + round) % 2 == 0 {
                            assert_eq!(&fs.read_file(p).unwrap()[..], &c[..]);
                        } else {
                            assert_eq!(fs.get(p), Some(&c[..]));
                        }
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    // Scoped threads can borrow the filesystem directly.
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| assert_eq!(fs.read_dir("data").count(), 2));
        }
    });
}

#[test]
fn gzip_bombs_stop_at_the_decompressed_size_limit() {
    // 16 MiB of zeros in one member: a few KB of gzip.
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut h = Header::new_ustar();
    h.set_path("zero.bin").unwrap();
    h.set_size(16 << 20);
    h.set_mode(0o644);
    h.set_cksum();
    gz.write_all(h.as_bytes()).unwrap();
    for _ in 0..16 {
        gz.write_all(&[0u8; 1 << 20]).unwrap();
    }
    gz.write_all(&[0u8; 1024]).unwrap();
    let bomb = gz.finish().unwrap();
    assert!(bomb.len() < 100_000, "{} bytes", bomb.len());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bomb.tar.gz");
    std::fs::write(&path, &bomb).unwrap();
    let limits = tarfs::Limits::default().with_max_decompressed_size(1 << 20);
    for result in [
        InMemoryFileSystem::open_with_limits(&path, limits),
        InMemoryFileSystem::from_tar_gz_bytes_with_limits(&bomb, limits),
        InMemoryFileSystem::from_tar_gz_reader_with_limits(&bomb[..], limits),
    ] {
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::TooLarge { limit } if limit == 1 << 20),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "archive too large: it decompresses to more than 1048576 bytes"
        );
    }
    // A limit that fits, and no limit at all (the default), both read it.
    let roomy = tarfs::Limits::default().with_max_decompressed_size(17 << 20);
    let fs = InMemoryFileSystem::open_with_limits(&path, roomy).unwrap();
    assert_eq!(fs.file_size("zero.bin"), Some(16 << 20));
    assert_eq!(InMemoryFileSystem::open(&path).unwrap().len(), 1);
}
