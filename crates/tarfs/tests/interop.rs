//! Interoperability with the system `tar` (bsdtar on macOS, GNU tar on Linux, and
//! `gtar` too when installed): archives they create are read correctly, and
//! archives we write are listed and extracted by them with identical contents.
//! Each test skips (with a note on stderr) when no tar binary is available.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{
    TarKind, TarTool, contents, fs_from, long_split, long_unsplittable, read_tree, run, tree,
    write_tree,
};
use tarfs::InMemoryFileSystem;

fn tools() -> Vec<TarTool> {
    let tools = TarTool::all();
    if tools.is_empty() {
        eprintln!("skipping: no tar binary on PATH");
    }
    tools
}

/// The fixture tree plus paths that stress name handling: a 180-byte path made of
/// splittable components and a 297-byte path that only extended formats can hold.
fn interop_tree() -> BTreeMap<String, Vec<u8>> {
    let mut t = tree();
    t.insert(
        format!(
            "{}/{}/{}.css",
            "a".repeat(60),
            "b".repeat(60),
            "c".repeat(50)
        ),
        b"splittable".to_vec(),
    );
    t.insert(long_unsplittable(), b"leafdata".to_vec());
    t
}

/// Formats to exercise per tool, and whether each can store the unsplittable path.
fn formats(kind: TarKind) -> Vec<(Option<&'static str>, bool)> {
    match kind {
        TarKind::Bsd => vec![
            (None, true),
            (Some("--format=pax"), true),
            (Some("--format=gnutar"), true),
            (Some("--format=ustar"), false),
        ],
        TarKind::Gnu => vec![
            (None, true),
            (Some("--format=gnu"), true),
            (Some("--format=pax"), true),
            (Some("--format=ustar"), false),
            (Some("--format=v7"), false),
        ],
    }
}

/// Create `out` from `src` with `tool`, optionally `-C src .` style (./ prefixes).
fn create(tool: &TarTool, format: Option<&str>, src: &Path, out: &Path, dot: bool) {
    let mut cmd = tool.command(false);
    if let Some(f) = format {
        cmd.arg(f);
    }
    cmd.arg("-czf").arg(out).arg("-C").arg(src);
    if dot {
        cmd.arg(".");
    } else {
        let mut top: Vec<_> = std::fs::read_dir(src)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        top.sort();
        cmd.args(top);
    }
    run(cmd);
}

#[test]
fn reads_archives_created_by_system_tar() {
    for tool in tools() {
        for (format, handles_long) in formats(tool.kind) {
            let mut expected = interop_tree();
            if !handles_long {
                expected.remove(&long_unsplittable());
            }
            if format == Some("--format=v7") {
                expected.retain(|p, _| p.len() < 100); // v7: 99-byte names, no prefix
            }
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src");
            write_tree(&src, &expected);
            for dot in [true, false] {
                let out = dir.path().join(format!("sys-{dot}.tar.gz"));
                create(&tool, format, &src, &out, dot);
                let fs = InMemoryFileSystem::open(&out)
                    .unwrap_or_else(|e| panic!("{:?} {format:?} (dot={dot}): {e}", tool.program));
                assert_eq!(
                    contents(&fs),
                    expected,
                    "{:?} {format:?} (dot={dot})",
                    tool.program
                );
            }
        }
    }
}

#[test]
fn skips_macos_metadata_from_bsdtar() {
    // With copyfile enabled (the macOS default) bsdtar adds AppleDouble `._*`
    // entries and LIBARCHIVE.xattr PAX records; neither may show up as files.
    let Some(tool) = tools().into_iter().find(|t| t.kind == TarKind::Bsd) else {
        eprintln!("skipping: no bsdtar");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    write_tree(&src, &tree());
    if cfg!(target_os = "macos") {
        // An extended attribute guarantees AppleDouble entries are emitted.
        let mut cmd = std::process::Command::new("xattr");
        cmd.args(["-w", "org.tarfs.test", "value"])
            .arg(src.join("a.txt"));
        let _ = cmd.output();
    }
    let out = dir.path().join("mac.tar.gz");
    let mut cmd = tool.command(true);
    cmd.arg("-czf").arg(&out).arg("-C").arg(&src).arg(".");
    run(cmd);
    if cfg!(target_os = "macos") {
        // `bsdtar -t` hides AppleDouble members on macOS; list the raw members.
        let gz = std::fs::read(&out).unwrap();
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::MultiGzDecoder::new(&gz[..]), &mut raw)
            .unwrap();
        let mut archive = tar::Archive::new(&raw[..]);
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| String::from_utf8_lossy(&e.unwrap().path_bytes()).into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("/._a.txt")),
            "expected AppleDouble members: {names:?}"
        );
    }
    let fs = InMemoryFileSystem::open(&out).unwrap();
    assert_eq!(contents(&fs), tree());
}

#[test]
fn system_tar_lists_and_extracts_our_archives() {
    for tool in tools() {
        for extended in [false, true] {
            let mut expected = tree();
            if extended {
                expected.insert(long_unsplittable(), b"leafdata".to_vec());
            }
            let fs = fs_from(&expected);
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("ours.tar.gz");
            if extended {
                fs.write_tarball_gz_extended(&out).unwrap();
            } else {
                fs.write_tarball_gz(&out).unwrap();
            }

            let mut list = tool.command(false);
            list.arg("-tzvf").arg(&out);
            let listing = run(list);
            let file_lines = listing.lines().filter(|l| l.starts_with('-')).count();
            assert_eq!(file_lines, expected.len(), "{:?}:\n{listing}", tool.program);
            assert!(
                listing
                    .lines()
                    .filter(|l| l.starts_with('-'))
                    .all(|l| l.starts_with("-rw-r--r--")),
                "{listing}"
            );
            assert!(listing.contains("a.txt") && listing.contains("data/p175.csv"));
            assert!(
                listing.contains(&long_split()),
                "prefix-split path:\n{listing}"
            );
            let dir_lines = listing.lines().filter(|l| l.starts_with('d')).count();
            if extended {
                assert!(dir_lines > 0, "directory entries expected:\n{listing}");
                assert!(
                    listing
                        .lines()
                        .filter(|l| l.starts_with('d'))
                        .all(|l| l.starts_with("drwxr-xr-x")),
                    "{listing}"
                );
            } else {
                assert_eq!(dir_lines, 0, "default writer emits no directories");
            }

            let x = dir.path().join("x");
            std::fs::create_dir(&x).unwrap();
            let mut extract = tool.command(false);
            extract.arg("-xzf").arg(&out).arg("-C").arg(&x);
            run(extract);
            assert_eq!(
                read_tree(&x),
                expected,
                "{:?} extended={extended}",
                tool.program
            );
        }
    }
}

#[test]
fn system_tar_reads_our_uncompressed_tarball_from_stdin() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for tool in tools() {
        let tar = fs_from(&tree()).write_tarball(Vec::new()).unwrap();
        let mut child = Command::new(&tool.program)
            .args(["-tf", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&tar).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut names: Vec<String> = unescape_listing(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        names.sort();
        assert_eq!(
            names,
            tree().into_keys().collect::<Vec<_>>(),
            "{:?}",
            tool.program
        );
    }
}

/// Undo the `\ooo` octal escapes bsdtar applies to non-ASCII bytes in listings.
fn unescape_listing(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let octal = raw
            .get(i + 1..i + 4)
            .filter(|d| d.iter().all(|b| (b'0'..=b'7').contains(b)));
        match (raw[i], octal) {
            (b'\\', Some(d)) => {
                out.push(
                    d.iter()
                        .fold(0u8, |acc, b| acc.wrapping_mul(8) + (b - b'0')),
                );
                i += 4;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).expect("listing is UTF-8 once unescaped")
}
