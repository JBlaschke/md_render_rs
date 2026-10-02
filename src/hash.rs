//! SHA-256 and hex helpers shared by the renderers, the server and the style bundle.

use sha2::{Digest as _, Sha256};

/// SHA-256 of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(bytes));
    out
}

/// SHA-256 over `parts`, each part followed by a NUL separator (so `["a", "b"]` and `["ab"]`
/// differ).
pub(crate) fn sha256_parts(parts: &[&str]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0xf)]));
    }
    s
}

/// Lowercase hex SHA-256 of `bytes` (64 digits).
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

/// `data-mdr-key` value (DESIGN §5, §7): 16 hex digits of [`sha256_parts`] over a description
/// of the element's source, e.g. `["code", lang, text]` or `["math", class, tex]`.
///
/// The offline renderers and the `--api` post-processing build the parts the same way, so a
/// block keeps its key when a page switches between the two (an API fallback).
pub(crate) fn mdr_key(parts: &[&str]) -> String {
    hex(&sha256_parts(parts)[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hex(&sha256(b"abc")), sha256_hex(b"abc"));
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(sha256_parts(&["abc"]), sha256(b"abc\0"));
    }

    #[test]
    fn keys_separate_parts() {
        assert_eq!(mdr_key(&["a", "b"]), mdr_key(&["a", "b"]));
        assert_ne!(mdr_key(&["a", "b"]), mdr_key(&["ab"]));
        assert_eq!(mdr_key(&["x"]).len(), 16);
        assert_eq!(mdr_key(&["x"]), hex(&sha256(b"x\0")[..8]));
    }
}
