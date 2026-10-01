//! Write -> flush -> re-read behaviour (TarFS.jl test parity): round trips,
//! reproducibility, read-modify-write, atomicity, both writers.

mod common;

use std::collections::BTreeMap;
use std::fs;

use common::{Rng, contents, dir_names, fs_from, long_split, long_unsplittable, name_100, tree};
use tarfs::{Error, InMemoryFileSystem, open_tarball_gz};

const GOLDEN_TAR: &[u8] = include_bytes!("fixtures/tarfs_jl.tar");

#[test]
fn default_writer_is_byte_identical_to_tarfs_jl() {
    // tests/fixtures/tarfs_jl.tar was produced by TarFS.jl's own write_tarball
    // from the same tree (see tests/fixtures/tarfs_jl.jl).
    let ours = fs_from(&tree()).write_tarball(Vec::new()).unwrap();
    assert_eq!(ours.len(), GOLDEN_TAR.len());
    for (block, (a, b)) in ours.chunks(512).zip(GOLDEN_TAR.chunks(512)).enumerate() {
        assert_eq!(a, b, "512-byte block #{block} differs from TarFS.jl");
    }
}

#[test]
fn reads_tarfs_jl_output() {
    let fs = InMemoryFileSystem::from_tar_bytes(GOLDEN_TAR).unwrap();
    assert_eq!(contents(&fs), tree());
}

#[test]
fn round_trip_through_gzip_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("one.tar.gz");
    let t = tree();
    open_tarball_gz(&path, None, |fs| {
        for (p, c) in &t {
            fs.write_file(p, c.clone())?;
        }
        Ok::<_, Error>(())
    })
    .unwrap();
    let back = InMemoryFileSystem::open(&path).unwrap();
    assert_eq!(contents(&back), t);
    assert_eq!(back.get(&name_100()), Some(&b"at the boundary"[..]));
    assert_eq!(back.read_to_string(&long_split()).unwrap(), "split me");
    assert_eq!(back.file_size("data/empty.bin"), Some(0));
    assert_eq!(back.read_file("bytes.bin").unwrap().len(), 256);
    assert!(matches!(
        back.read_file("does/not/exist"),
        Err(Error::NotFound(_))
    ));
    assert_eq!(dir_names(dir.path()), ["one.tar.gz"], "no temp-file litter");
}

#[test]
fn seeded_flush_streams_source_entries_and_stays_readable() {
    let dir = tempfile::tempdir().unwrap();
    let (p1, p2, p3) = (
        dir.path().join("one.tar.gz"),
        dir.path().join("two.tar.gz"),
        dir.path().join("three.tar.gz"),
    );
    fs_from(&tree()).write_tarball_gz(&p1).unwrap();
    let seeded = InMemoryFileSystem::open(&p1).unwrap();
    seeded.write_tarball_gz(&p2).unwrap();
    assert_eq!(seeded.read_to_string("a.txt").unwrap(), "hello");
    assert_eq!(contents(&InMemoryFileSystem::open(&p2).unwrap()), tree());
    seeded.write_tarball_gz(&p3).unwrap();
    assert_eq!(fs::read(&p2).unwrap(), fs::read(&p3).unwrap());
    // Source-backed and staged entries of equal content serialize identically.
    assert_eq!(fs::read(&p1).unwrap(), fs::read(&p2).unwrap());
    assert_eq!(
        dir_names(dir.path()),
        ["one.tar.gz", "three.tar.gz", "two.tar.gz"]
    );
}

#[test]
fn reproducible_regardless_of_write_order() {
    let t = tree();
    let forward = fs_from(&t);
    let mut backward = InMemoryFileSystem::new();
    for (p, c) in t.iter().rev() {
        backward.write_file(p, c.clone()).unwrap();
    }
    backward.write_file("a.txt", "scratch").unwrap(); // overwritten below
    backward.write_file("a.txt", "hello").unwrap();
    assert_eq!(
        forward.write_tarball(Vec::new()).unwrap(),
        backward.write_tarball(Vec::new()).unwrap()
    );
    assert_eq!(
        forward.write_tarball_extended(Vec::new()).unwrap(),
        backward.write_tarball_extended(Vec::new()).unwrap()
    );
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.tar.gz"), dir.path().join("b.tar.gz"));
    forward.write_tarball_gz(&a).unwrap();
    backward.write_tarball_gz(&b).unwrap();
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
    forward.write_tarball_gz_extended(&a).unwrap();
    backward.write_tarball_gz_extended(&b).unwrap();
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
}

#[test]
fn gzip_header_is_fixed_for_cross_platform_reproducibility() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.tar.gz");
    fs_from(&tree()).write_tarball_gz(&path).unwrap();
    let gz = fs::read(&path).unwrap();
    assert_eq!(
        &gz[..4],
        [0x1f, 0x8b, 8, 0],
        "magic, deflate, no flags (no name)"
    );
    assert_eq!(&gz[4..8], [0, 0, 0, 0], "mtime 0");
    assert_eq!(gz[9], 255, "OS unknown");
}

#[test]
fn empty_filesystem_round_trips() {
    let fs = InMemoryFileSystem::new();
    assert_eq!(fs.write_tarball(Vec::new()).unwrap(), vec![0u8; 1024]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.tar.gz");
    fs.write_tarball_gz(&path).unwrap();
    assert!(InMemoryFileSystem::open(&path).unwrap().is_empty());
    fs.write_tarball_gz_extended(&path).unwrap();
    assert!(InMemoryFileSystem::open(&path).unwrap().is_empty());
}

#[test]
fn large_random_content_round_trips() {
    let big = Rng::new(42).bytes(3 * 65536 + 17);
    let dir = tempfile::tempdir().unwrap();
    let p1 = dir.path().join("big.tar.gz");
    open_tarball_gz(&p1, None, |fs| fs.write_file("big.bin", big.clone())).unwrap();
    let fs = InMemoryFileSystem::open(&p1).unwrap();
    assert_eq!(fs.get("big.bin"), Some(&big[..]));
    let p2 = dir.path().join("big2.tar.gz");
    fs.write_tarball_gz(&p2).unwrap();
    assert_eq!(
        InMemoryFileSystem::open(&p2).unwrap().get("big.bin"),
        Some(&big[..])
    );
}

#[test]
fn unsplittable_path_needs_the_extended_writer() {
    let dir = tempfile::tempdir().unwrap();
    let mut fs = fs_from(&tree());
    let long = long_unsplittable();
    assert!(long.len() > 256);
    fs.write_file(&long, "leafdata").unwrap();

    let mem = dir.path().join("mem.tar.gz");
    assert!(matches!(fs.write_tarball_gz(&mem), Err(Error::PathTooLong(ref p)) if *p == long));
    assert!(!mem.exists());
    assert!(matches!(
        fs.write_tarball(Vec::new()),
        Err(Error::PathTooLong(_))
    ));

    let via = dir.path().join("via.tar.gz");
    fs.write_tarball_gz_extended(&via).unwrap();
    let back = InMemoryFileSystem::open(&via).unwrap();
    assert_eq!(back.read_to_string(&long).unwrap(), "leafdata");
    let mut expected = tree();
    expected.insert(long, b"leafdata".to_vec());
    assert_eq!(contents(&back), expected);
    assert_eq!(
        dir_names(dir.path()),
        ["via.tar.gz"],
        "failed write left no litter"
    );
}

#[test]
fn extended_writer_emits_directory_entries() {
    let tar = fs_from(&tree()).write_tarball_extended(Vec::new()).unwrap();
    let mut archive = tar::Archive::new(&tar[..]);
    let mut dirs = Vec::new();
    let mut files = 0;
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let header = entry.header();
        let path = String::from_utf8(entry.path_bytes().into_owned()).unwrap();
        match header.entry_type() {
            tar::EntryType::Directory => {
                assert_eq!(header.mode().unwrap(), 0o755);
                assert_eq!(header.size().unwrap(), 0);
                dirs.push(path);
            }
            tar::EntryType::Regular => {
                assert_eq!(header.mode().unwrap(), 0o644);
                files += 1;
            }
            other => panic!("unexpected {other:?} for {path}"),
        }
        assert_eq!(header.mtime().unwrap(), 0);
        assert_eq!(header.uid().unwrap(), 0);
        assert_eq!(header.gid().unwrap(), 0);
    }
    assert_eq!(files, tree().len());
    let expected_dirs = [
        "data/".to_owned(),
        format!("p{}/", "x".repeat(60)),
        "データ/".to_owned(),
        format!("{}/", "α".repeat(40)),
    ];
    let mut expected_dirs = expected_dirs.to_vec();
    expected_dirs.sort();
    assert_eq!(dirs, expected_dirs);
}

#[test]
fn both_writers_agree_on_contents() {
    let fs = fs_from(&tree());
    let a = InMemoryFileSystem::from_tar_bytes(fs.write_tarball(Vec::new()).unwrap()).unwrap();
    let b =
        InMemoryFileSystem::from_tar_bytes(fs.write_tarball_extended(Vec::new()).unwrap()).unwrap();
    assert_eq!(contents(&a), contents(&b));
}

#[test]
fn read_modify_write_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let q = dir.path().join("rmw.tar.gz");
    open_tarball_gz(&q, None, |fs| {
        fs.write_file("a.txt", "v1")?;
        fs.write_file("keep.txt", "kept")?;
        fs.write_file("gone/x.txt", "x")?;
        fs.write_file("gone/y/z.txt", "z")
    })
    .unwrap();
    let ((), fs) = open_tarball_gz(&q, Some(&q), |fs| {
        assert_eq!(fs.read_to_string("a.txt")?, "v1");
        fs.write_file("a.txt", "v2")?;
        assert!(fs.delete_file("keep.txt"));
        assert_eq!(fs.delete_prefix("gone"), 2);
        fs.write_file("new.txt", "n")
    })
    .unwrap();
    assert_eq!(fs.paths().collect::<Vec<_>>(), ["a.txt", "new.txt"]);
    let back = InMemoryFileSystem::open(&q).unwrap();
    assert_eq!(back.read_to_string("a.txt").unwrap(), "v2");
    assert_eq!(back.read_to_string("new.txt").unwrap(), "n");
    assert!(!back.contains("keep.txt"));
    assert_eq!(dir_names(dir.path()), ["rmw.tar.gz"]);
}

#[test]
fn failing_closure_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    // No archive is created at all...
    let never = dir.path().join("never.tar.gz");
    let r = open_tarball_gz(&never, None, |fs| {
        fs.write_file("x.txt", "y")?;
        Err::<(), _>(Error::Corrupt("boom".into()))
    });
    assert!(matches!(r, Err(Error::Corrupt(ref m)) if m == "boom"));
    assert!(!never.exists());

    // ...and a seeded original is left byte-for-byte untouched.
    let q = dir.path().join("rmw.tar.gz");
    open_tarball_gz(&q, None, |fs| fs.write_file("a.txt", "v2")).unwrap();
    let before = fs::read(&q).unwrap();
    #[derive(Debug)]
    enum AppError {
        Tar(Error),
        Boom,
    }
    impl From<Error> for AppError {
        fn from(e: Error) -> Self {
            AppError::Tar(e)
        }
    }
    let r = open_tarball_gz(&q, Some(&q), |fs| {
        fs.write_file("a.txt", "SHOULD NOT LAND")?;
        Err::<(), AppError>(AppError::Boom)
    });
    assert!(matches!(r, Err(AppError::Boom)));
    assert_eq!(fs::read(&q).unwrap(), before);
    assert_eq!(dir_names(dir.path()), ["rmw.tar.gz"]);

    // Errors from tarfs itself convert into the caller's error type.
    let missing = dir.path().join("missing.tar.gz");
    let r = open_tarball_gz(&q, Some(&missing), |_| Ok::<(), AppError>(()));
    assert!(matches!(r, Err(AppError::Tar(Error::Io(_)))));
    assert_eq!(fs::read(&q).unwrap(), before);
}

#[test]
fn unencodable_flush_preserves_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.tar.gz");
    open_tarball_gz(&p, None, |fs| fs.write_file("a.txt", "original")).unwrap();
    let before = fs::read(&p).unwrap();
    let mut fs = InMemoryFileSystem::open(&p).unwrap();
    // Sorts last, so a streaming writer would already have emitted "a.txt".
    fs.write_file(&format!("zzz{}", "q".repeat(200)), "abc")
        .unwrap();
    assert!(matches!(
        fs.write_tarball_gz(&p),
        Err(Error::PathTooLong(_))
    ));
    assert_eq!(fs::read(&p).unwrap(), before);
    assert_eq!(dir_names(dir.path()), ["t.tar.gz"]);
    assert_eq!(
        InMemoryFileSystem::open(&p)
            .unwrap()
            .read_to_string("a.txt")
            .unwrap(),
        "original"
    );
}

#[test]
fn failed_rename_cleans_up_the_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("occupied");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("child"), "x").unwrap();
    let fs = fs_from(&tree());
    assert!(matches!(fs.write_tarball_gz(&target), Err(Error::Io(_))));
    assert!(matches!(
        fs.write_tarball_gz_extended(&target),
        Err(Error::Io(_))
    ));
    assert_eq!(dir_names(dir.path()), ["occupied"]);
    assert_eq!(dir_names(&target), ["child"]);
}

#[test]
fn missing_parent_directory_is_an_io_error_with_context() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("no/such/dir/x.tar.gz");
    match fs_from(&tree()).write_tarball_gz(&target) {
        Err(Error::Io(e)) => {
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
            assert!(e.to_string().contains("no/such/dir"), "{e}");
        }
        other => panic!("expected Io, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn flushed_archives_are_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("perm.tar.gz");
    fs_from(&tree()).write_tarball_gz(&p).unwrap();
    assert_eq!(
        fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o644
    );
    fs_from(&tree()).write_tarball_gz_extended(&p).unwrap();
    assert_eq!(
        fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[test]
fn staged_bytes_and_source_bytes_mix() {
    let seeded = InMemoryFileSystem::from_tar_bytes(GOLDEN_TAR).unwrap();
    let mut fs = seeded.clone();
    let shout = fs.read_to_string("a.txt").unwrap().to_uppercase();
    fs.write_file("a.txt", shout).unwrap();
    fs.write_file("copy.bin", fs.read_file("bytes.bin").unwrap())
        .unwrap();
    let back = InMemoryFileSystem::from_tar_bytes(fs.write_tarball(Vec::new()).unwrap()).unwrap();
    let mut expected: BTreeMap<String, Vec<u8>> = tree();
    expected.insert("a.txt".into(), b"HELLO".to_vec());
    expected.insert("copy.bin".into(), (0..=255u8).collect());
    assert_eq!(contents(&back), expected);
    // The clone it came from is unaffected.
    assert_eq!(seeded.read_to_string("a.txt").unwrap(), "hello");
}
