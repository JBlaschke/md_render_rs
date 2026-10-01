//! Hand-rolled USTAR headers: a byte-for-byte port of TarFS.jl's `_split_path` and
//! `_ustar_header`.
//!
//! Field layout (POSIX.1-1988 ustar, offsets in bytes):
//!
//! | offset | len | field    | TarFS.jl value                           |
//! |-------:|----:|----------|------------------------------------------|
//! |      0 | 100 | name     | path, or the part after the split `/`    |
//! |    100 |   8 | mode     | `"0000644\0"`                            |
//! |    108 |   8 | uid      | `"0000000\0"`                            |
//! |    116 |   8 | gid      | `"0000000\0"`                            |
//! |    124 |  12 | size     | 11 octal digits + NUL                    |
//! |    136 |  12 | mtime    | `"00000000000\0"`                        |
//! |    148 |   8 | chksum   | 6 octal digits + NUL + space             |
//! |    156 |   1 | typeflag | `'0'` (regular file)                     |
//! |    257 |   6 | magic    | `"ustar\0"`                              |
//! |    263 |   2 | version  | `"00"`                                   |
//! |    345 | 155 | prefix   | the part before the split `/`, or empty  |
//!
//! Everything else (linkname, uname, gname, devmajor, devminor, padding) is NUL.

use crate::error::{Error, Result};

/// Size of a tar block (headers and data padding).
pub(crate) const BLOCK: usize = 512;

/// One all-zero block: data padding source and end-of-archive marker.
pub(crate) const ZERO_BLOCK: [u8; BLOCK] = [0; BLOCK];

/// Capacity of the ustar `name` field.
pub(crate) const NAME_LEN: usize = 100;

/// Capacity of the ustar `prefix` field.
pub(crate) const PREFIX_LEN: usize = 155;

/// Exclusive upper bound of the 11-octal-digit size field: 8^11 bytes = 8 GiB.
pub(crate) const MAX_SIZE: u64 = 1 << 33;

/// Offset of the ustar `prefix` field.
pub(crate) const PREFIX_OFFSET: usize = 345;

/// Split `path` into `(name, prefix)` for the ustar `name` (<= 100 B) and `prefix`
/// (<= 155 B) fields, as TarFS.jl's `_split_path` does.
///
/// A path of at most 100 bytes is stored whole. Otherwise it is split at the
/// RIGHTMOST `/` that leaves at most 155 bytes before it and between 1 and 100
/// bytes after it; the `/` itself is dropped (readers rejoin `prefix + "/" + name`).
/// The scan is byte-level on ASCII `/`, so multi-byte UTF-8 sequences are never cut.
/// `None` when no such `/` exists.
///
/// The "at least 1 byte after" condition is not in TarFS.jl, but it never changes
/// the result for a file path (those never end in `/`); it keeps the extended
/// writer from splitting a directory path like `a/b/` at its trailing slash.
pub(crate) fn split_path(path: &str) -> Option<(&str, &str)> {
    let len = path.len();
    if len <= NAME_LEN {
        return Some((path, ""));
    }
    // Index `i` of a '/' leaves a prefix of `i` bytes and a name of `len - i - 1`.
    let (split, _) =
        path.bytes().enumerate().rev().find(|&(i, b)| {
            b == b'/' && i <= PREFIX_LEN && (1..=NAME_LEN).contains(&(len - i - 1))
        })?;
    // `split` indexes an ASCII '/', so both slices fall on char boundaries.
    Some((&path[split + 1..], &path[..split]))
}

/// Check that `path`/`size` can be encoded by [`header`], without building it.
pub(crate) fn validate(path: &str, size: u64) -> Result<()> {
    if size >= MAX_SIZE {
        return Err(Error::EntryTooLarge {
            path: path.to_owned(),
            size,
        });
    }
    if split_path(path).is_none() {
        return Err(Error::PathTooLong(path.to_owned()));
    }
    Ok(())
}

/// One 512-byte ustar header for a regular file (TarFS.jl `_ustar_header`).
///
/// Mirrors Tar.jl's normalization (mode `0o644`, uid/gid/mtime `0`, magic
/// `"ustar\0"`, version `"00"`) and produces exactly the bytes TarFS.jl does.
pub(crate) fn header(path: &str, size: u64) -> Result<[u8; BLOCK]> {
    validate(path, size)?;
    let (name, prefix) = split_path(path).ok_or_else(|| Error::PathTooLong(path.to_owned()))?;

    let mut h = [0u8; BLOCK];
    put_str(&mut h, 0, NAME_LEN, name.as_bytes());
    put_octal(&mut h, 100, 8, 0o644); // mode
    put_octal(&mut h, 108, 8, 0); // uid
    put_octal(&mut h, 116, 8, 0); // gid
    put_octal(&mut h, 124, 12, size); // size: 11 octal digits + NUL
    put_octal(&mut h, 136, 12, 0); // mtime
    h[148..156].fill(b' '); // chksum field counts as spaces while summing
    h[156] = b'0'; // typeflag: regular file
    put_str(&mut h, 257, 6, b"ustar"); // magic "ustar\0" (NUL already there)
    h[263] = b'0'; // version "00"
    h[264] = b'0';
    put_str(&mut h, PREFIX_OFFSET, PREFIX_LEN, prefix.as_bytes());

    // At most 512 * 255 = 130560 < 8^6, so six octal digits always suffice.
    let checksum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    put_str(&mut h, 148, 6, format!("{checksum:06o}").as_bytes());
    h[154] = 0; // field encoding: "dddddd\0 "
    h[155] = b' ';
    Ok(h)
}

/// Number of NUL bytes needed after `size` data bytes to reach a block boundary.
pub(crate) fn padding(size: u64) -> usize {
    // `size % 512` < 512, so the cast is lossless.
    (BLOCK - (size % BLOCK as u64) as usize) % BLOCK
}

/// TarFS.jl `putstr!`: copy at most `len` bytes of `s` to `off`.
fn put_str(h: &mut [u8; BLOCK], off: usize, len: usize, s: &[u8]) {
    let n = s.len().min(len);
    h[off..off + n].copy_from_slice(&s[..n]);
}

/// TarFS.jl `putoct!`: `value` in octal, zero-padded to `len - 1` digits (the
/// remaining byte stays NUL), truncated to `len` like `putstr!`.
fn put_octal(h: &mut [u8; BLOCK], off: usize, len: usize, value: u64) {
    let digits = format!("{value:0width$o}", width = len - 1);
    put_str(h, off, len, digits.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME_100: &str = concat!(
        "mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm",
        "mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm"
    );

    fn long_split() -> String {
        format!("p{}/{}.txt", "x".repeat(60), "y".repeat(60)) // 126 B
    }

    fn octal_field(h: &[u8], range: std::ops::Range<usize>) -> u64 {
        let text = std::str::from_utf8(&h[range]).unwrap();
        u64::from_str_radix(text, 8).unwrap()
    }

    #[test]
    fn split_short_and_boundary_paths_are_not_split() {
        assert_eq!(split_path("short.txt"), Some(("short.txt", "")));
        assert_eq!(NAME_100.len(), 100);
        assert_eq!(split_path(NAME_100), Some((NAME_100, "")));
    }

    #[test]
    fn split_long_path_round_trips() {
        let p = long_split();
        let (name, prefix) = split_path(&p).unwrap();
        assert_eq!(format!("{prefix}/{name}"), p);
        assert!(name.len() <= 100 && prefix.len() <= 155);
    }

    #[test]
    fn split_multibyte_path_stays_valid_utf8() {
        let p = format!("{}/{}.txt", "α".repeat(40), "β".repeat(40)); // 165 B
        assert_eq!(p.len(), 165);
        let (name, prefix) = split_path(&p).unwrap();
        assert_eq!(format!("{prefix}/{name}"), p);
        assert_eq!(prefix, "α".repeat(40));
    }

    #[test]
    fn split_picks_the_rightmost_valid_slash() {
        // Both slashes are valid splits; TarFS.jl keeps the rightmost one.
        let p = format!("{}/{}/{}", "a".repeat(10), "b".repeat(50), "c".repeat(60));
        let (name, prefix) = split_path(&p).unwrap();
        assert_eq!(name, "c".repeat(60));
        assert_eq!(prefix, format!("{}/{}", "a".repeat(10), "b".repeat(50)));
        // The rightmost slash leaves a 156-byte prefix: too long, so the one before wins.
        let p = format!("{}/{}/{}", "a".repeat(100), "b".repeat(55), "c".repeat(30));
        let (name, prefix) = split_path(&p).unwrap();
        assert_eq!(prefix, "a".repeat(100));
        assert_eq!(name, format!("{}/{}", "b".repeat(55), "c".repeat(30)));
    }

    #[test]
    fn split_limits_match_tarfs_jl() {
        let q150 = "q".repeat(150);
        assert_eq!(
            split_path(&format!("{q150}/leaf")),
            Some(("leaf", q150.as_str()))
        );
        assert_eq!(split_path(&q150), None); // no slash at all
        assert_eq!(split_path(&format!("a/{q150}")), None); // name side too long
        // Exact limits: 155-byte prefix and 100-byte name (256 bytes total).
        let p = format!("{}/{}", "p".repeat(155), "n".repeat(100));
        assert_eq!(
            split_path(&p),
            Some(("n".repeat(100).as_str(), "p".repeat(155).as_str()))
        );
        assert_eq!(split_path(&format!("{}/{}", "p".repeat(156), "n")), None);
        assert_eq!(split_path(&format!("p/{}", "n".repeat(101))), None);
    }

    #[test]
    fn split_never_produces_an_empty_name() {
        let dir = format!("{}/{}/", "a".repeat(60), "b".repeat(60));
        let (name, prefix) = split_path(&dir).unwrap();
        assert_eq!(name, format!("{}/", "b".repeat(60)));
        assert_eq!(prefix, "a".repeat(60));
        assert_eq!(split_path(&format!("{}/", "a".repeat(150))), None);
    }

    #[test]
    fn header_fields_match_tarfs_jl_layout() {
        let h = header("data/x.csv", 5).unwrap();
        assert_eq!(&h[..10], b"data/x.csv");
        assert_eq!(h[10], 0);
        assert_eq!(&h[100..108], b"0000644\0");
        assert_eq!(&h[108..116], b"0000000\0");
        assert_eq!(&h[116..124], b"0000000\0");
        assert_eq!(&h[124..136], b"00000000005\0");
        assert_eq!(octal_field(&h, 124..135), 5);
        assert_eq!(&h[136..148], b"00000000000\0");
        assert_eq!(h[156], b'0');
        assert_eq!(&h[257..263], b"ustar\0");
        assert_eq!(&h[263..265], b"00");
        assert!(h[157..257].iter().all(|&b| b == 0), "linkname");
        assert!(h[265..345].iter().all(|&b| b == 0), "uname/gname/dev");
        assert!(h[345..].iter().all(|&b| b == 0), "prefix + pad");
    }

    #[test]
    fn header_checksum_is_six_digits_nul_space() {
        let h = header("data/x.csv", 5).unwrap();
        let stored = octal_field(&h, 148..154);
        let mut blanked = h;
        blanked[148..156].fill(b' ');
        let sum: u64 = blanked.iter().map(|&b| u64::from(b)).sum();
        assert_eq!(stored, sum);
        assert_eq!(h[154], 0);
        assert_eq!(h[155], b' ');
        // The tar crate (an independent implementation) agrees.
        let parsed = tar::Header::from_byte_slice(&h);
        assert_eq!(parsed.cksum().unwrap() as u64, sum);
        assert_eq!(parsed.path_bytes().as_ref(), b"data/x.csv");
        assert_eq!(parsed.size().unwrap(), 5);
        assert_eq!(parsed.mode().unwrap(), 0o644);
    }

    #[test]
    fn header_golden_bytes_for_a_tiny_entry() {
        // Derived by hand: the sum of every byte of the header of ("a.txt", 5) with
        // the checksum field blanked to spaces.
        let mut expected = [0u8; BLOCK];
        expected[..5].copy_from_slice(b"a.txt");
        expected[100..108].copy_from_slice(b"0000644\0");
        expected[108..116].copy_from_slice(b"0000000\0");
        expected[116..124].copy_from_slice(b"0000000\0");
        expected[124..136].copy_from_slice(b"00000000005\0");
        expected[136..148].copy_from_slice(b"00000000000\0");
        expected[148..156].copy_from_slice(b"        ");
        expected[156] = b'0';
        expected[257..263].copy_from_slice(b"ustar\0");
        expected[263..265].copy_from_slice(b"00");
        let field_sum = |s: &[u8]| s.iter().map(|&b| u32::from(b)).sum::<u32>();
        let by_hand = (97 + 46 + 116 + 120 + 116) // name "a.txt"
            + (4 * 48 + 54 + 52 + 52) // mode "0000644"
            + 7 * 48 // uid
            + 7 * 48 // gid
            + (10 * 48 + 53) // size "00000000005"
            + 11 * 48 // mtime
            + 8 * 32 // blank checksum
            + 48 // typeflag '0'
            + (117 + 115 + 116 + 97 + 114) // magic "ustar"
            + 2 * 48; // version "00"
        assert_eq!(by_hand, 3537);
        assert_eq!(by_hand, field_sum(&expected));
        assert_eq!(by_hand, 0o6721);
        expected[148..156].copy_from_slice(b"006721\0 ");
        assert_eq!(header("a.txt", 5).unwrap(), expected);
    }

    #[test]
    fn header_puts_split_paths_in_name_and_prefix() {
        let p = long_split();
        let h = header(&p, 0).unwrap();
        let (name, prefix) = split_path(&p).unwrap();
        assert_eq!(&h[..name.len()], name.as_bytes());
        assert_eq!(h[name.len()], 0);
        assert_eq!(&h[345..345 + prefix.len()], prefix.as_bytes());
        let parsed = tar::Header::from_byte_slice(&h);
        assert_eq!(parsed.path_bytes().as_ref(), p.as_bytes());
    }

    #[test]
    fn header_size_limit_is_8_gib() {
        assert_eq!(MAX_SIZE, 8u64.pow(11));
        assert!(matches!(
            header("big", MAX_SIZE),
            Err(Error::EntryTooLarge { ref path, size }) if path == "big" && size == MAX_SIZE
        ));
        assert!(matches!(
            header("big", u64::MAX),
            Err(Error::EntryTooLarge { .. })
        ));
        let h = header("ok", MAX_SIZE - 1).unwrap();
        assert_eq!(&h[124..136], b"77777777777\0");
        assert_eq!(octal_field(&h, 124..135), MAX_SIZE - 1);
    }

    #[test]
    fn header_rejects_unsplittable_paths() {
        let p = format!("zzz{}", "q".repeat(200));
        assert!(matches!(header(&p, 3), Err(Error::PathTooLong(ref s)) if *s == p));
        assert!(matches!(validate(&p, 3), Err(Error::PathTooLong(_))));
        // Size is checked first, like TarFS.jl.
        assert!(matches!(
            validate(&p, MAX_SIZE),
            Err(Error::EntryTooLarge { .. })
        ));
    }

    #[test]
    fn padding_reaches_block_boundaries() {
        assert_eq!(padding(0), 0);
        assert_eq!(padding(1), 511);
        assert_eq!(padding(511), 1);
        assert_eq!(padding(512), 0);
        assert_eq!(padding(513), 511);
        assert_eq!(padding(MAX_SIZE - 1), 1);
    }
}
