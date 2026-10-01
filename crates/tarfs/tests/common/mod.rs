//! Shared fixtures for the tarfs integration tests.
//!
//! `tree()` must stay identical to `TREE` in `tests/fixtures/tarfs_jl.jl`: the golden
//! file `tests/fixtures/tarfs_jl.tar` is TarFS.jl's `write_tarball` of it.
#![allow(dead_code)] // each test binary uses a different subset

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use tarfs::InMemoryFileSystem;

/// Exactly fills the 100-byte ustar name field.
pub fn name_100() -> String {
    "m".repeat(100)
}

/// 126 bytes: needs the ustar prefix split.
pub fn long_split() -> String {
    format!("p{}/{}.txt", "x".repeat(60), "y".repeat(60))
}

/// 165 bytes of two-byte characters: the split must not cut a character.
pub fn multibyte_split() -> String {
    format!("{}/{}.txt", "α".repeat(40), "β".repeat(40))
}

/// About 297 bytes with 40-byte segments: no valid ustar split exists.
pub fn long_unsplittable() -> String {
    format!("L/{}/leaf.txt", vec!["s".repeat(40); 7].join("/"))
}

/// The cross-checked fixture tree (TarFS.jl's test `TREE` plus a few extras).
pub fn tree() -> BTreeMap<String, Vec<u8>> {
    let mut t = BTreeMap::new();
    let mut put = |p: &str, c: &[u8]| {
        t.insert(p.to_owned(), c.to_vec());
    };
    put("a.txt", b"hello");
    put("data/p175.csv", b"x,y\n1,2\n3,4\n");
    put("data/empty.bin", b"");
    put("bytes.bin", &(0..=255u8).collect::<Vec<_>>());
    put("データ/日本語.txt", "unicode ✓ content".as_bytes());
    put(&name_100(), b"at the boundary");
    put(&long_split(), b"split me");
    put(&multibyte_split(), b"multibyte split");
    let big: Vec<u8> = (0..3 * 512 + 17u32)
        .map(|i| ((i * 31 + 7) % 256) as u8)
        .collect();
    put("big.bin", &big);
    put("exact512.bin", &[b'z'; 512]);
    t
}

/// A filesystem holding `tree`.
pub fn fs_from(tree: &BTreeMap<String, Vec<u8>>) -> InMemoryFileSystem {
    let mut fs = InMemoryFileSystem::new();
    for (p, c) in tree {
        fs.write_file(p, c.clone()).expect("valid fixture path");
    }
    fs
}

/// Every file of `fs` with its contents.
pub fn contents(fs: &InMemoryFileSystem) -> BTreeMap<String, Vec<u8>> {
    fs.iter().map(|(p, c)| (p.to_owned(), c.to_vec())).collect()
}

/// Materialize `tree` under `dir`.
pub fn write_tree(dir: &Path, tree: &BTreeMap<String, Vec<u8>>) {
    for (p, c) in tree {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, c).unwrap();
    }
}

/// Every regular file under `dir`, keyed by its `/`-separated relative path.
pub fn read_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                walk(root, &path, out);
            } else if kind.is_file() {
                let rel = path.strip_prefix(root).unwrap();
                let key = rel
                    .components()
                    .map(|c| c.as_os_str().to_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(key, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// Names of the entries in `dir` (to detect stray temporary files).
pub fn dir_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// Deterministic xorshift64* generator (no `rand` dependency).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next_u64() as u8).collect()
    }
}

/// Which implementation a `tar` binary is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarKind {
    Bsd,
    Gnu,
}

/// A `tar`-compatible binary found on PATH.
#[derive(Debug, Clone)]
pub struct TarTool {
    pub program: PathBuf,
    pub kind: TarKind,
}

impl TarTool {
    /// Probe `program --version`; `None` if absent or unrecognized.
    pub fn probe(program: &str) -> Option<TarTool> {
        let out = Command::new(program).arg("--version").output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let kind = if text.contains("bsdtar") {
            TarKind::Bsd
        } else if text.contains("GNU tar") {
            TarKind::Gnu
        } else {
            return None;
        };
        Some(TarTool {
            program: program.into(),
            kind,
        })
    }

    /// Every distinct tar implementation available (`tar`, `gtar`, `bsdtar`).
    pub fn all() -> Vec<TarTool> {
        let mut tools: Vec<TarTool> = Vec::new();
        for name in ["tar", "gtar", "bsdtar"] {
            if let Some(t) = TarTool::probe(name) {
                if !tools.iter().any(|known| known.kind == t.kind) {
                    tools.push(t);
                }
            }
        }
        tools
    }

    /// A command for this tool with macOS metadata (AppleDouble, xattrs) disabled
    /// unless `mac_metadata` is set.
    pub fn command(&self, mac_metadata: bool) -> Command {
        let mut cmd = Command::new(&self.program);
        if !mac_metadata {
            cmd.env("COPYFILE_DISABLE", "1");
        }
        cmd
    }
}

/// Run `cmd`, panicking with its stderr on failure; returns stdout.
pub fn run(mut cmd: Command) -> String {
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawning {cmd:?}: {e}"));
    assert!(
        out.status.success(),
        "{cmd:?} failed ({}):\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}
