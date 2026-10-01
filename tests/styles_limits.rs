//! Size limits on style bundles: a small hostile `.tar.gz` (a "gzip bomb", SEC-4) is rejected
//! without being decompressed into memory, and the user falls back to the embedded styles.

use md_render::Flavor;
use md_render::styles::{Bundle, BundleOrigin, MAX_BUNDLE_SIZE, SharedBundle, load_or_fallback};

/// A small `.tar.gz` that decompresses to more than `size_mib` MiB: a ~1 KB archive holding one
/// MiB of zeros, repeated as a multi-member gzip stream (decompressed, it is that tarball over
/// and over; readers must decompress everything before they can index it).
fn bomb(size_mib: u64) -> Vec<u8> {
    let dir = tempfile::tempdir().expect("tempdir");
    let one = dir.path().join("one.tar.gz");
    let mut fs = tarfs::InMemoryFileSystem::new();
    fs.write_file("github/zero.bin", vec![0u8; 1 << 20])
        .expect("write");
    fs.write_tarball_gz(&one).expect("flush");
    std::fs::read(&one).expect("read").repeat(size_mib as usize)
}

#[test]
fn gzip_bomb_bundles_are_rejected_at_the_size_limit() {
    let limit_mib = MAX_BUNDLE_SIZE >> 20;
    let gz = bomb(limit_mib + 1);
    assert!(gz.len() < 1 << 20, "the bomb is small: {} bytes", gz.len());
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bomb.tar.gz");
    std::fs::write(&path, &gz).expect("write");

    let e = Bundle::open(&path).expect_err("too large");
    let msg = format!("{e:#}");
    assert!(
        msg.contains("archive too large") && msg.contains(&MAX_BUNDLE_SIZE.to_string()),
        "{msg}"
    );
    let e = Bundle::from_tar_gz_bytes(&gz, BundleOrigin::Fallback).expect_err("too large");
    assert!(format!("{e:#}").contains("archive too large"), "{e:#}");

    let (bundle, warning) = load_or_fallback(&path);
    assert_eq!(bundle.origin(), &BundleOrigin::Fallback);
    assert!(warning.expect("warning").contains("archive too large"));

    // A running server keeps its styles when the bundle is replaced by a bomb.
    let (shared, _) = SharedBundle::load(dir.path().join("styles.tar.gz"));
    let before = shared.etag();
    std::fs::rename(&path, shared.path()).expect("rename");
    assert!(shared.reload().is_err());
    assert_eq!(shared.etag(), before);
    assert!(shared.get().is_fallback(Flavor::Github));
}

#[test]
fn bundle_files_larger_than_the_limit_are_not_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("huge.tar.gz");
    let file = std::fs::File::create(&path).expect("create");
    // Sparse on most filesystems: nothing is written.
    file.set_len(MAX_BUNDLE_SIZE + 1).expect("set_len");
    let e = Bundle::open(&path).expect_err("too large");
    assert!(
        format!("{e:#}").contains("larger than the 512 MiB limit"),
        "{e:#}"
    );
}
