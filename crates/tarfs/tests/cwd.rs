//! Flushing to a bare file name. This is its own test binary because it changes
//! the process-wide current directory, which would race with other tests.

mod common;

use common::{dir_names, fs_from, tree};
use tarfs::InMemoryFileSystem;

#[test]
fn bare_file_name_targets_the_current_directory() {
    // `Path::parent` of a bare file name is "": the temporary file must go to ".".
    let dir = tempfile::tempdir().unwrap();
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let fs = fs_from(&tree());
    let results = (
        fs.write_tarball_gz("bare.tar.gz"),
        fs.write_tarball_gz_extended("bare-ext.tar.gz"),
        InMemoryFileSystem::open("bare.tar.gz").map(|fs| fs.len()),
    );
    std::env::set_current_dir(old).unwrap();
    results.0.unwrap();
    results.1.unwrap();
    assert_eq!(results.2.unwrap(), tree().len());
    assert_eq!(dir_names(dir.path()), ["bare-ext.tar.gz", "bare.tar.gz"]);
}
