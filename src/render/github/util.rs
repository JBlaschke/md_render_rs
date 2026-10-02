//! Small shared helpers.

use crate::hash::{hex, sha256_parts};

/// A deterministic UUIDv4-shaped string derived from `parts` (stands in for GitHub's random
/// enrichment `data-identity`, GH §8.7).
pub(super) fn uuid_like(parts: &[&str]) -> String {
    let mut b = sha256_parts(parts);
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
    fn uuid_like_is_stable() {
        let u = uuid_like(&["x"]);
        assert_eq!(u, uuid_like(&["x"]));
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
