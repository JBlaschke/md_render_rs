//! Live cross-check against TarFS.jl, the reference implementation.
//!
//! Ignored by default: it needs `julia` on PATH and a TarFS.jl checkout (found at
//! `$TARFS_JL`, or as `.dev/TarFS.jl` in an ancestor of this crate), and its
//! dependencies (CodecZlib, TranscodingStreams) already in the Julia depot: the
//! test runs Pkg offline in a throwaway environment and never touches the
//! checkout. Run with
//!
//! ```text
//! cargo test -p tarfs --test julia -- --ignored
//! ```
//!
//! It checks, in both directions:
//! * our default writer is byte-identical to `TarFS.write_tarball`, and the golden
//!   file `tests/fixtures/tarfs_jl.tar` is still what TarFS.jl produces;
//! * `Tar.list` (strict) and `Tar.extract` accept our default and extended output;
//! * we read TarFS.jl's `write_tarball_gz` and `write_tarball_gz_viatar` output and
//!   archives made by `Tar.create`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{contents, fs_from, long_unsplittable, run, tree};
use tarfs::InMemoryFileSystem;

const GOLDEN_TAR: &[u8] = include_bytes!("fixtures/tarfs_jl.tar");

fn julia_available() -> bool {
    Command::new("julia")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn tarfs_jl_checkout() -> Option<PathBuf> {
    let is_checkout = |p: &Path| p.join("src/TarFS.jl").is_file();
    if let Some(dir) = std::env::var_os("TARFS_JL") {
        let dir = PathBuf::from(dir);
        return is_checkout(&dir).then_some(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .map(|a| a.join(".dev/TarFS.jl"))
        .find(|p| is_checkout(p))
}

fn julia(project: &Path) -> Command {
    let mut cmd = Command::new("julia");
    cmd.arg("--startup-file=no")
        .arg(format!("--project={}", project.display()))
        .env("JULIA_PKG_OFFLINE", "true");
    cmd
}

#[test]
#[ignore = "needs julia and a TarFS.jl checkout; run with --ignored"]
fn byte_identity_and_cross_validation_with_tarfs_jl() {
    if !julia_available() {
        eprintln!("skipping: julia is not on PATH");
        return;
    }
    let Some(checkout) = tarfs_jl_checkout() else {
        eprintln!("skipping: no TarFS.jl checkout (set TARFS_JL)");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let env = tmp.path().join("env");
    let out = tmp.path().join("out");

    let mut develop = julia(&env);
    develop
        .arg("-e")
        .arg("using Pkg; Pkg.develop(path=ARGS[1])")
        .arg(&checkout);
    run(develop);

    let t = tree();
    let ours = tmp.path().join("ours.tar");
    std::fs::write(&ours, fs_from(&t).write_tarball(Vec::new()).unwrap()).unwrap();
    let mut extended_tree = t.clone();
    extended_tree.insert(long_unsplittable(), b"leafdata".to_vec());
    let ours_ext = tmp.path().join("ours_ext.tar");
    std::fs::write(
        &ours_ext,
        fs_from(&extended_tree)
            .write_tarball_extended(Vec::new())
            .unwrap(),
    )
    .unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tarfs_jl.jl");
    let mut check = julia(&env);
    check.arg(&script).arg(&out).arg(&ours).arg(&ours_ext);
    let stdout = run(check);
    assert!(stdout.contains("TARFS_JL_OK"), "{stdout}");

    let reference = std::fs::read(out.join("tarfs_jl.tar")).unwrap();
    assert_eq!(
        std::fs::read(&ours).unwrap(),
        reference,
        "default writer vs TarFS.jl"
    );
    assert_eq!(
        GOLDEN_TAR,
        &reference[..],
        "tests/fixtures/tarfs_jl.tar is stale"
    );

    let read = |name: &str| contents(&InMemoryFileSystem::open(out.join(name)).unwrap());
    assert_eq!(read("tarfs_jl.tar.gz"), t);
    assert_eq!(read("tar_create.tar.gz"), t);
    assert_eq!(read("tarfs_jl_viatar.tar.gz"), extended_tree);
}
