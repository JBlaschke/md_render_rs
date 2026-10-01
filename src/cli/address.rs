//! grip's positional `[<path>] [<address>]` rules, ported from path-and-address 2.0.1
//! (`parsing.py`, GRIP §2.3) and extended with bracketed IPv6 literals (GRIP §10 C2).
//!
//! The key rule: **a lone positional is an address only if it contains a valid port**
//! (`80`, `:80`, `host:80`, `0`, `[::1]:80`). A lone host name such as `localhost` is a path.

use std::net::Ipv6Addr;

/// Default listen host (grip `settings.HOST`, GRIP §2.3).
pub const DEFAULT_HOST: &str = "localhost";
/// Default listen port (grip `settings.PORT`, GRIP §2.3).
pub const DEFAULT_PORT: u16 = 6419;

/// A successfully split address. Either part may be missing (`80` has no host, `localhost` has
/// no port). An empty host (`:80`) is kept as `Some("")`, exactly like grip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPort {
    /// Host name or IP literal. IPv6 literals are stored **without** brackets (`::1`).
    pub host: Option<String>,
    pub port: Option<u16>,
}

/// `path_and_address.resolve()`: decide which positional is the path and which the address.
///
/// With two positionals (or none) they are taken as given. With exactly one, it is the address
/// iff [`split_address`] finds a valid port in it, otherwise it is the path.
pub fn resolve<'a>(
    path_or_address: Option<&'a str>,
    address: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    match (path_or_address, address) {
        (None, a) => (None, a),
        (p, Some(a)) => (p, Some(a)),
        (Some(single), None) => {
            if split_address(single).is_some_and(|hp| hp.port.is_some()) {
                (None, Some(single))
            } else {
                (Some(single), None)
            }
        }
    }
}

/// `path_and_address.split_address()`: `None` means "invalid address" (grip's `(None, None)`).
///
/// Ported exactly, including Python `int()` port parsing (surrounding whitespace, a sign and
/// `_` digit separators are accepted). Extension: `[v6]` and `[v6]:port` (GRIP §10 C2); a bare
/// IPv6 literal without brackets stays invalid (it has more than two `:`-separated parts).
pub fn split_address(address: &str) -> Option<HostPort> {
    if address.is_empty() {
        return None;
    }
    if let Some(rest) = address.strip_prefix('[') {
        return split_bracketed_ipv6(rest);
    }
    let components: Vec<&str> = address.split(':').collect();
    if components.len() > 2 {
        return None;
    }
    if !components[0].is_empty() && !valid_hostname(components[0]) {
        return None;
    }
    if components.len() == 2 && !valid_port(components[1]) {
        return None;
    }
    let (host, port) = if components.len() == 1 {
        // `components.insert(0 if valid_port(c) else 1, None)`
        if valid_port(components[0]) {
            (None, Some(components[0]))
        } else {
            (Some(components[0]), None)
        }
    } else {
        (Some(components[0]), Some(components[1]))
    };
    // `port = int(port) if port else None`; validity was checked above.
    let port = match port {
        Some(p) if !p.is_empty() => Some(port_number(p)?),
        _ => None,
    };
    Some(HostPort {
        host: host.map(str::to_owned),
        port,
    })
}

/// `[addr]` or `[addr]:port` (the leading `[` already stripped).
fn split_bracketed_ipv6(rest: &str) -> Option<HostPort> {
    let (inner, after) = rest.split_once(']')?;
    inner.parse::<Ipv6Addr>().ok()?;
    let port = if after.is_empty() {
        None
    } else {
        let p = after.strip_prefix(':')?;
        if !valid_port(p) {
            return None;
        }
        Some(port_number(p)?)
    };
    Some(HostPort {
        host: Some(inner.to_owned()),
        port,
    })
}

/// `valid_hostname()`: at most 255 characters, one trailing `.` allowed, every `.`-separated
/// label matches `(?!-)[A-Z\d-]{1,63}(?<!-)$` (case-insensitive). `_` is rejected.
pub fn valid_hostname(host: &str) -> bool {
    if host.chars().count() > 255 {
        return false;
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    host.split('.').all(valid_label)
}

fn valid_label(label: &str) -> bool {
    (1..=63).contains(&label.len())
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `valid_port()`: `0 <= int(port) <= 65535` with Python `int()` parsing.
pub fn valid_port(port: &str) -> bool {
    python_int(port).is_some_and(|n| (0..=65535).contains(&n))
}

fn port_number(port: &str) -> Option<u16> {
    python_int(port).and_then(|n| u16::try_from(n).ok())
}

/// Python 3 `int(str)` for ASCII input: surrounding whitespace, an optional sign, decimal digits
/// with single `_` separators between digits. Non-ASCII digits (which Python also accepts) are
/// not supported. Values that do not fit an `i64` are rejected (they are never valid ports).
fn python_int(s: &str) -> Option<i64> {
    let s = s.trim();
    let (negative, digits) = match s.as_bytes().first()? {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let mut value: i64 = 0;
    for b in digits.bytes() {
        match b {
            b'0'..=b'9' => {
                value = value.checked_mul(10)?.checked_add(i64::from(b - b'0'))?;
            }
            b'_' => {}
            _ => return None,
        }
    }
    Some(if negative { -value } else { value })
}

/// Format `host:port` for URLs and messages, bracketing IPv6 literals.
pub fn display_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hp(host: Option<&str>, port: Option<u16>) -> Option<HostPort> {
        Some(HostPort {
            host: host.map(str::to_owned),
            port,
        })
    }

    /// GRIP §2.3 resolution table (single positional).
    #[test]
    fn resolve_single_positional_table() {
        let cases: &[(&str, Option<&str>, Option<&str>)] = &[
            ("80", None, Some("80")),
            ("0", None, Some("0")),
            (":80", None, Some(":80")),
            ("localhost:80", None, Some("localhost:80")),
            ("0.0.0.0:8080", None, Some("0.0.0.0:8080")),
            ("host.:80", None, Some("host.:80")),
            ("0.0.0.0", Some("0.0.0.0"), None),
            ("localhost", Some("localhost"), None),
            ("70000", Some("70000"), None),
            ("./80", Some("./80"), None),
            ("my_host:80", Some("my_host:80"), None),
            ("::1:80", Some("::1:80"), None),
            ("-", Some("-"), None),
            ("README.md", Some("README.md"), None),
            ("docs/", Some("docs/"), None),
            // IPv6 extension (C2): bracketed literal with a port is an address ...
            ("[::1]:8080", None, Some("[::1]:8080")),
            ("[::]:0", None, Some("[::]:0")),
            // ... without a port it is (like a lone host name) a path.
            ("[::1]", Some("[::1]"), None),
        ];
        for &(arg, path, addr) in cases {
            assert_eq!(resolve(Some(arg), None), (path, addr), "argv: {arg}");
        }
    }

    #[test]
    fn resolve_two_positionals_are_taken_verbatim() {
        assert_eq!(
            resolve(Some("README.md"), Some("8080")),
            (Some("README.md"), Some("8080"))
        );
        assert_eq!(resolve(Some("80"), Some("81")), (Some("80"), Some("81")));
        assert_eq!(resolve(None, None), (None, None));
    }

    /// GRIP §2.3 `split_address` results, including the two-positional rows.
    #[test]
    fn split_address_table() {
        let cases: &[(&str, Option<HostPort>)] = &[
            ("80", hp(None, Some(80))),
            ("0", hp(None, Some(0))),
            (":80", hp(Some(""), Some(80))),
            ("localhost:80", hp(Some("localhost"), Some(80))),
            ("0.0.0.0:8080", hp(Some("0.0.0.0"), Some(8080))),
            ("host.:80", hp(Some("host."), Some(80))),
            ("0.0.0.0", hp(Some("0.0.0.0"), None)),
            ("localhost", hp(Some("localhost"), None)),
            ("70000", hp(Some("70000"), None)),
            ("6419x", hp(Some("6419x"), None)),
            ("bad::addr", None),
            ("foo_bar", None),
            ("my_host:80", None),
            ("host:", None),
            ("host:70000", None),
            ("host:abc", None),
            ("-host:80", None),
            ("host-:80", None),
            ("a..b:80", None),
            ("", None),
            ("::1", None),
            // A lone component must be a valid host name first, so int() quirks such as a
            // sign, spaces or `_` only survive after the `:`.
            ("+80", None),
            (" 80", None),
            ("8_0", None),
            ("-0", None),
            ("host:+80", hp(Some("host"), Some(80))),
            (": 80", hp(Some(""), Some(80))),
            ("x:8_0", hp(Some("x"), Some(80))),
            ("x:-0", hp(Some("x"), Some(0))),
            // IPv6 extension.
            ("[::1]", hp(Some("::1"), None)),
            ("[::1]:8080", hp(Some("::1"), Some(8080))),
            ("[fe80::1:2]:1", hp(Some("fe80::1:2"), Some(1))),
            ("[::1]:", None),
            ("[::1]8080", None),
            ("[::1]:70000", None),
            ("[localhost]:80", None),
            ("[::1", None),
            ("[]:80", None),
        ];
        for (arg, want) in cases {
            assert_eq!(&split_address(arg), want, "address: {arg:?}");
        }
    }

    #[test]
    fn hostname_rules() {
        assert!(valid_hostname("localhost"));
        assert!(valid_hostname("example.com."));
        assert!(valid_hostname("a-b.c"));
        assert!(valid_hostname("80"));
        assert!(!valid_hostname("example.com.."));
        assert!(!valid_hostname("under_score"));
        assert!(!valid_hostname(&"a".repeat(64)));
        assert!(valid_hostname(&"a".repeat(63)));
        let long = vec!["a".repeat(50); 6].join(".");
        assert!(long.len() > 255);
        assert!(!valid_hostname(&long));
        assert!(!valid_hostname("héllo"));
    }

    #[test]
    fn port_rules() {
        for ok in ["0", "65535", "+1", "\t42 ", "1_0"] {
            assert!(valid_port(ok), "{ok:?}");
        }
        for bad in [
            "",
            "65536",
            "-1",
            "1.0",
            "_1",
            "1_",
            "0x10",
            "99999999999999999999999",
        ] {
            assert!(!valid_port(bad), "{bad:?}");
        }
    }

    #[test]
    fn display_brackets_ipv6() {
        assert_eq!(display_host_port("localhost", 6419), "localhost:6419");
        assert_eq!(display_host_port("::1", 80), "[::1]:80");
    }
}
