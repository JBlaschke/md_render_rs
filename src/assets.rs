//! Static files embedded in the binary, served under `/__/md-render/static/<name>`
//! (docs/DESIGN.md §3.7). Owners fill in the files; the registry itself is append-only.

/// Look up an embedded asset by name: `(bytes, mime type)`.
pub fn get(name: &str) -> Option<(&'static [u8], &'static str)> {
    let found: (&'static [u8], &'static str) = match name {
        "live.js" => (include_bytes!("../assets/client/live.js"), "text/javascript; charset=utf-8"),
        "github.js" => (include_bytes!("../assets/client/github.js"), "text/javascript; charset=utf-8"),
        "gitlab.js" => (include_bytes!("../assets/client/gitlab.js"), "text/javascript; charset=utf-8"),
        "favicon.svg" => (include_bytes!("../assets/favicon.svg"), "image/svg+xml"),
        "fallback-github.css" => (include_bytes!("../assets/fallback/github.css"), "text/css; charset=utf-8"),
        "fallback-gitlab.css" => (include_bytes!("../assets/fallback/gitlab.css"), "text/css; charset=utf-8"),
        _ => return None,
    };
    Some(found)
}

/// Names of all embedded assets.
pub const NAMES: &[&str] =
    &["live.js", "github.js", "gitlab.js", "favicon.svg", "fallback-github.css", "fallback-gitlab.css"];

#[cfg(test)]
mod tests {
    #[test]
    fn all_names_resolve() {
        for n in super::NAMES {
            assert!(super::get(n).is_some(), "{n}");
        }
        assert!(super::get("nope").is_none());
    }
}
