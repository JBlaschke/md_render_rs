//! Small shared helpers: HTML escaping and content hashing.

use sha2::{Digest, Sha256};

/// Appends `s` HTML-escaped for text content (`&`, `<`, `>`; GH §7.1 "GitHub escapes only
/// `<`, `>`, `&` in text").
pub(super) fn push_text(out: &mut String, s: &str) {
    let mut last = 0;
    for (i, b) in s.bytes().enumerate() {
        let rep = match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            _ => continue,
        };
        out.push_str(&s[last..i]);
        out.push_str(rep);
        last = i + 1;
    }
    out.push_str(&s[last..]);
}

/// Appends `s` escaped for a double-quoted attribute value (`&`, `"`, `<`, `>`; GH §7.1 RAW).
pub(super) fn push_attr(out: &mut String, s: &str) {
    let mut last = 0;
    for (i, b) in s.bytes().enumerate() {
        let rep = match b {
            b'&' => "&amp;",
            b'"' => "&quot;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            _ => continue,
        };
        out.push_str(&s[last..i]);
        out.push_str(rep);
        last = i + 1;
    }
    out.push_str(&s[last..]);
}

/// SHA-256 over `parts`, each part followed by a NUL separator (so `["a", "b"]` and `["ab"]`
/// differ).
pub(super) fn digest(parts: &[&str]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    let out = h.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&out);
    bytes
}

/// Lowercase hex of `bytes`.
pub(super) fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[usize::from(b >> 4)] as char);
        s.push(HEX[usize::from(b & 15)] as char);
    }
    s
}

/// `data-mdr-key` value: 16 hex digits of a hash of the element's source (DESIGN §5, §7).
pub(super) fn mdr_key(parts: &[&str]) -> String {
    hex(&digest(parts)[..8])
}

/// A deterministic UUIDv4-shaped string derived from `parts` (stands in for GitHub's random
/// enrichment `data-identity`, GH §8.7).
pub(super) fn uuid_like(parts: &[&str]) -> String {
    let mut b = digest(parts);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex(&b[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping() {
        let (mut t, mut a) = (String::new(), String::new());
        push_text(&mut t, r#"a<b>&"c'ü"#);
        push_attr(&mut a, r#"a<b>&"c'"#);
        assert_eq!(t, r#"a&lt;b&gt;&amp;"c'ü"#);
        assert_eq!(a, "a&lt;b&gt;&amp;&quot;c'");
    }

    #[test]
    fn hashing_is_stable() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(mdr_key(&["a", "b"]), mdr_key(&["a", "b"]));
        assert_ne!(mdr_key(&["a", "b"]), mdr_key(&["ab"]));
        assert_eq!(mdr_key(&["x"]).len(), 16);
        let u = uuid_like(&["x"]);
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
