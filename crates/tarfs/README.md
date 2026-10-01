# tarfs

An in-memory filesystem backed by a `.tar.gz` archive: a Rust port of
[TarFS.jl](https://github.com/JBlaschke/TarFS.jl).

Load an archive once, read files by path, stage new, changed or deleted files in
memory, and flush everything back to a gzip-compressed POSIX tarball, atomically.
The archive is never unpacked to disk. tarfs is a library-level view of one file.
It is not a mount and it does not give you POSIX semantics.

## Why

* **One file, not thousands.** Shared filesystems and object stores handle
  directories full of small files badly (metadata load, inode quotas). One
  `.tar.gz` is one file on disk and one object in a bucket.
* **Readable forever.** Every platform can read tar and gzip. There is no custom
  format to maintain a reader for.
* **Random access anyway.** tarfs decompresses the archive into memory once and
  indexes it by path. After that, every read is an O(1) zero-copy slice.
* **Safe in-place updates.** Read-modify-write onto the source archive is atomic,
  so a reader of the file never sees a half-written archive.

md-render uses tarfs for its style bundle. The vendored CSS, fonts and client
libraries for both flavors live in one `.tar.gz`. Running servers serve that
archive straight from memory, and `md-render vendor` updates it in place.

Pick something else when the decompressed archive does not fit in RAM, or when you
need real filesystem semantics (permissions, symlinks, timestamps).

## Quick start

Reading:

```rust,no_run
fn main() -> tarfs::Result<()> {
    let fs = tarfs::InMemoryFileSystem::open("data.tar.gz")?;
    let csv = fs.read_file("data/p175.csv")?; // bytes::Bytes, zero-copy
    println!("{} bytes", csv.len());
    for path in fs.read_dir("data") {
        println!("{path}");
    }
    Ok(())
}
```

Writing. Stage files on an empty filesystem. The archive is flushed only if the
closure returns `Ok`:

```rust,no_run
fn main() -> tarfs::Result<()> {
    tarfs::open_tarball_gz("results.tar.gz", None, |fs| {
        fs.write_file("summary.csv", "x,y\n1,2\n")?;
        fs.write_file("meta/run.txt", "seed = 42\n")?;
        Ok::<_, tarfs::Error>(())
    })?;
    Ok(())
}
```

Read-modify-write. Seed the filesystem from an archive and overwrite that same
archive. If anything fails, the original file is left untouched:

```rust,no_run
use std::path::Path;

fn main() -> tarfs::Result<()> {
    let archive = Path::new("data.tar.gz");
    tarfs::open_tarball_gz(archive, Some(archive), |fs| {
        let csv = fs.read_to_string("data/p175.csv")?;
        fs.write_file("data/p175.csv", csv.replace(',', ";"))?;
        fs.delete_prefix("scratch"); // every file under scratch/
        Ok::<_, tarfs::Error>(())
    })?;
    Ok(())
}
```

`InMemoryFileSystem` is `Send + Sync`, so readers can share it, for example as an
`Arc<InMemoryFileSystem>` in server state. Writing needs `&mut self`.

## API

| TarFS.jl | tarfs | Notes |
|---|---|---|
| `InMemoryFileSystem(path)` | `InMemoryFileSystem::open(path)` | also `from_tar_gz_reader`, `from_tar_gz_bytes`, `from_tar_bytes` |
| `InMemoryFileSystem()` | `InMemoryFileSystem::new()` / `Default` | alias `TarFs` |
| `readfile(fs, p)` | `read_file(p) -> Result<Bytes>` | zero-copy; also `get(p) -> Option<&[u8]>` and `read_to_string(p)` |
| `writefile(fs, p, c)` | `write_file(p, c)` | `c: impl Into<Bytes>`; checks `p` with `check_path` |
| `deletefile(fs, p)` | `delete_file(p) -> bool` | plus `delete_prefix(dir) -> usize` |
| `open_tarball_gz(f, p; from)` | `open_tarball_gz(p, from, f)` | flushes only if `f` returns `Ok`; returns `(f's value, fs)` |
| `write_tarball_gz(fs, p)` | `write_tarball_gz(p)` | atomic, reproducible |
| `write_tarball(fs, io)` | `write_tarball(w) -> Result<W>` | uncompressed; byte-identical to TarFS.jl |
| `write_tarball_gz_viatar(fs, p)` | `write_tarball_gz_extended(p)` | plus `write_tarball_extended(w)`; see [Two writers](#two-writers) |
| `_check_path(p)` | `check_path(p)` | public |
| none | `contains`, `len`, `is_empty`, `paths`, `iter`, `file_size`, `read_dir` | listings are sorted by path |

Every fallible call returns `tarfs::Result<T>`. The error type is `tarfs::Error`:

| Variant | Meaning |
|---|---|
| `Io` | OS error (missing file, permissions, disk full); the message names the path |
| `NotFound(path)` | no such file in the filesystem |
| `InvalidPath { path, reason }` | `check_path` rejected `path` |
| `PathTooLong(path)` | the default writer cannot encode `path`; use the extended writer |
| `EntryTooLarge { path, size }` | the default writer cannot encode a file of 8 GiB or more |
| `Corrupt(msg)` | not a valid gzip-compressed tarball, or an unsupported or unsafe member |
| `Utf8(path)` | `read_to_string` found bytes that are not UTF-8 |

### Paths

A member path must be non-empty and relative (no leading `/`). It is
`/`-separated, with no NUL byte, no backslash, and no `""`, `.` or `..` segment.
That rules out `//`, a trailing `/`, `./a` and `a/../b`. `write_file` enforces these
rules. Directories are implicit: they exist only as prefixes of file paths.

### Reading other tools' archives

* Multi-member gzip streams, such as `cat a.gz b.gz` or pigz output, are read
  completely.
* Only regular and contiguous files are indexed. Old-style `'\0'` type flags
  count as regular.
* PAX `path`/`size` records and GNU long names are honored.
* Directories, links, devices, FIFOs and PAX global headers are skipped. GNU
  sparse files are rejected as `Corrupt`.
* A leading `./` is stripped, so `tar -C dir .` output works. The bare `.` entry
  and macOS AppleDouble `._*` files are skipped.
* Member paths that are not UTF-8 are `Corrupt`. Paths that are still unsafe after
  normalization (absolute, `..`, ...) are also `Corrupt`.
* When a path appears twice, the later member wins, as in tar.

## Guarantees

**Atomic flushes.** A flush writes to a temporary file (`.tarfs-*.tmp`) in the
target's directory. It sets the file's mode to `0o644` on Unix, then renames it over
the target once the file is complete. On any error, the temporary file is removed
and an existing archive at the target is untouched. This is what makes
`open_tarball_gz(p, Some(p), …)` safe. tarfs never calls `fsync`: the flush survives
application failure, but surviving a power loss is up to your filesystem.

**Reproducible bytes.** Files are written in sorted path order with fixed metadata
(mode `0o644`, uid/gid/mtime `0`), so equal contents give identical bytes, whatever
order the files were written in.

* The tar layer is byte-identical to TarFS.jl's `write_tarball`. A golden fixture
  produced by TarFS.jl checks this, and an opt-in test cross-checks it live.
* The gzip header is fixed (mtime 0, OS byte 255, no file name), so the `.tar.gz`
  is identical across platforms too. One caveat: a build that switches flate2 away
  from its default pure-Rust backend (`miniz_oxide`) keeps the tar bytes but may
  compress them differently.

**Zero-copy reads, streaming writes.** The decompressed archive is one immutable
`bytes::Bytes` buffer. Reads return slices of it, and flushes stream unmodified
files straight from it.

**No panics on bad input.** Every member range is bounds-checked. The tests feed
the reader truncated, bit-flipped and randomly corrupted archives.

## Two writers

The **default writer** (`write_tarball`, `write_tarball_gz`) emits USTAR headers by
hand, as TarFS.jl does. It has two limits:

* A path must fit in 100 bytes, or be splittable at a `/` into a prefix of at most
  155 bytes and a name of at most 100 bytes.
* A file must be smaller than 8 GiB (the 11-digit octal size field).

It writes no directory entries. Every entry is checked before the first byte is
written, so a `PathTooLong` or `EntryTooLarge` error never leaves partial output.

The **extended writer** (`write_tarball_extended`, `write_tarball_gz_extended`) has
no path or size limits. It writes ustar headers, adding a PAX extended header only
for a path or size that does not fit. It also writes an explicit directory entry
(mode `0o755`) before the contents of each directory. Use it when the archive will
be extracted by `tar -x`, docker, or other tools that care about directory entries.
Its output is deterministic, and it writes straight from memory: TarFS.jl's
`write_tarball_gz_viatar` goes through a temporary directory. If one path is both a
file and the parent of other files (`a` and `a/b`), no extractor can recreate that,
so the extended writer refuses it with `InvalidPath`. When tarfs reads the archive
back, the two writers are interchangeable.

## Limitations

* Regular files only. tarfs keeps no symlinks, hard links, permissions (everything
  becomes `0o644`) or timestamps, and it does not support GNU sparse files.
* The whole decompressed archive stays in memory for the lifetime of the value.
  There is no cap on decompressed size, so be careful with untrusted archives.
* Data after the last gzip member, even zero padding, makes the archive `Corrupt`.

## Differences from TarFS.jl

* **No lazy read cache.** TarFS.jl decodes a file on first `readfile` and caches it.
  tarfs indexes byte ranges of one shared buffer, so there is nothing to cache.
* **Thread safety.** Concurrent reads through `&self` are safe. TarFS.jl shares
  scratch buffers and seeks, so it is not thread-safe.
* **Return values.** `read_file` returns bytes, and `read_to_string` checks UTF-8.
  `write_file` returns `()` instead of the path, and `delete_file` reports whether
  the file existed.
* **Errors.** Failures are typed `Error` values instead of `KeyError`,
  `ArgumentError` and `ErrorException`. The `open_tarball_gz` closure fails by
  returning `Err`, which can be any error type that `tarfs::Error` converts into,
  including `anyhow::Error`.
* **Paths.** `check_path` also rejects backslashes, which Windows tools treat as
  separators.
* **Reader rules.** tarfs accepts multi-member gzip, strips `./` and skips
  AppleDouble files. It rejects unsafe or non-UTF-8 member paths and sparse files.
* **Gzip output.** The `.tar.gz` bytes are reproducible across platforms. TarFS.jl's
  gzip OS byte varies by platform.
* **Extended writer.** It needs no temporary directory and writes PAX headers
  (`Tar.create` writes its own).

## Testing

```sh
cargo test -p tarfs
# Live byte-identity cross-check against TarFS.jl. Needs `julia` on PATH, a TarFS.jl
# checkout (found at $TARFS_JL or .dev/TarFS.jl), and CodecZlib and
# TranscodingStreams already in the Julia depot.
cargo test -p tarfs --test julia -- --ignored
```

The suite covers:

* unit tests of the header, split and path rules;
* round trips, reproducibility, read-modify-write and atomicity;
* the reader on hand-crafted archives, plus a corruption fuzz loop;
* interop with the system `tar`: bsdtar, and GNU tar when it is installed. These
  tests skip when no `tar` is found.

`tests/fixtures/tarfs_jl.tar` is TarFS.jl's output for the shared fixture tree.
`tests/fixtures/tarfs_jl.jl` regenerates it.

## License

BSD-3-Clause, like TarFS.jl (same author).
