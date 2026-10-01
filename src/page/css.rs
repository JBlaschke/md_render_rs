//! CSS reference handling for exports: find every `url(…)` and `@import "…"` in a stylesheet,
//! resolve it against the stylesheet's bundle path (vendored CSS uses bundle-relative URLs,
//! DESIGN D10) and let the caller substitute a replacement.

use std::borrow::Cow;
use std::sync::OnceLock;

use percent_encoding::percent_decode_str;
use regex::{Captures, Regex};

/// URL route under which a server mounts the bundle (`RenderOptions::new` default
/// `bundle_url`). Content rendered for export uses it, and export rewrites it.
pub(crate) const BUNDLE_ROUTE: &str = "/__/md-render/bundle";

/// What a reference in a stylesheet points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// Leave the reference untouched (`data:` URI, `#fragment`, empty).
    Keep,
    /// A file inside the bundle, plus an optional `#fragment` to re-append.
    Bundle { path: String, fragment: Option<String> },
    /// An absolute/external URL or a path that leaves the bundle.
    Unresolved,
}

/// How a reference was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefKind {
    /// `url(…)` (also inside `@import url(…)`).
    Url,
    /// `@import "…"` / `@import '…'`.
    ImportString,
}

fn ref_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Groups 1-3: url("…") / url('…') / url(…); groups 4-5: @import "…" / '…'.
        // (The regex crate has no backreferences, so the quote styles are alternatives; CR §7.)
        Regex::new(
            r#"(?i)url\(\s*(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|([^"')\s]*))\s*\)|@import\s+(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)')"#,
        )
        .expect("valid regex")
    })
}

fn split_capture<'h>(c: &Captures<'h>) -> (RefKind, &'h str) {
    for (group, kind) in [(1, RefKind::Url), (2, RefKind::Url), (3, RefKind::Url), (4, RefKind::ImportString), (5, RefKind::ImportString)] {
        if let Some(m) = c.get(group) {
            return (kind, m.as_str());
        }
    }
    // Unreachable for this regex (one alternative always matches), but stay total.
    (RefKind::Url, "")
}

/// Every reference in `css`, in source order.
pub(crate) fn references(css: &str) -> Vec<(RefKind, String)> {
    ref_regex()
        .captures_iter(css)
        .map(|c| {
            let (kind, raw) = split_capture(&c);
            (kind, raw.to_owned())
        })
        .collect()
}

/// Replace references: `replace(kind, raw)` returns the new **URL** (without `url(` wrapper),
/// or `None` to keep the original text. The result is always written as `url("…")`
/// (`@import url("…")` for import strings), so the new URL must not contain `"`, `\` or
/// newlines (data URIs and percent-encoded paths never do).
pub(crate) fn rewrite<'c>(css: &'c str, mut replace: impl FnMut(RefKind, &str) -> Option<String>) -> Cow<'c, str> {
    ref_regex().replace_all(css, |c: &Captures<'_>| {
        let (kind, raw) = split_capture(c);
        match replace(kind, raw) {
            Some(url) => match kind {
                RefKind::Url => format!("url(\"{url}\")"),
                RefKind::ImportString => format!("@import url(\"{url}\")"),
            },
            None => c[0].to_owned(),
        }
    })
}

/// Does `s` start with a URL scheme (`http:`, `data:`, `c:`…)?
fn has_scheme(s: &str) -> bool {
    let Some(colon) = s.find(':') else { return false };
    let scheme = &s[..colon];
    !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Normalize a `/`-separated relative path, resolving `.` and `..`. `None` if it climbs out
/// of the bundle root or is empty.
pub(crate) fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    if out.is_empty() { None } else { Some(out.join("/")) }
}

/// Resolve a stylesheet reference against the bundle path of the stylesheet containing it.
pub(crate) fn resolve(base_path: &str, reference: &str) -> Target {
    let r = reference.trim();
    if r.is_empty() || r.starts_with('#') || r.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("data:")) {
        return Target::Keep;
    }
    if r.starts_with("//") || has_scheme(r) {
        return Target::Unresolved;
    }
    let (r, fragment) = match r.split_once('#') {
        Some((p, f)) => (p, Some(f.to_owned())),
        None => (r, None),
    };
    let r = r.split_once('?').map_or(r, |(p, _)| p);
    let decoded = percent_decode_str(r).decode_utf8_lossy();
    let joined = if let Some(abs) = decoded.strip_prefix('/') {
        // Root-relative: only meaningful when it addresses the bundle route itself.
        match abs.strip_prefix(&BUNDLE_ROUTE[1..]) {
            Some(rest) if rest.starts_with('/') => rest.to_owned(),
            _ => return Target::Unresolved,
        }
    } else {
        let dir = base_path.rsplit_once('/').map_or("", |(d, _)| d);
        format!("{dir}/{decoded}")
    };
    match normalize(&joined) {
        Some(path) => Target::Bundle { path, fragment },
        None => Target::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_matches_render_default() {
        assert_eq!(BUNDLE_ROUTE, format!("{}/bundle", crate::PREFIX));
        assert_eq!(crate::render::RenderOptions::new(crate::Flavor::Github).bundle_url, BUNDLE_ROUTE);
    }

    #[test]
    fn finds_all_reference_forms() {
        let css = r#"a{background:url(img/a.png)} b{background:URL( "b c.svg#x" )}
            @import 'x.css'; @import "y.css"; @import url(z.css);
            c{src:url('f.woff2') format("woff2")} d{e:url(data:image/png;base64,AAA=)}"#;
        let refs: Vec<String> = references(css).into_iter().map(|(_, r)| r).collect();
        assert_eq!(refs, ["img/a.png", "b c.svg#x", "x.css", "y.css", "z.css", "f.woff2", "data:image/png;base64,AAA="]);
        assert_eq!(references(css)[2].0, RefKind::ImportString);
    }

    #[test]
    fn rewrite_wraps_consistently() {
        let css = "a{b:url(x.png)}@import 'y.css';c{d:url(keep.png)}";
        let out = rewrite(css, |_, r| (r != "keep.png").then(|| format!("data:,{r}")));
        assert_eq!(out, "a{b:url(\"data:,x.png\")}@import url(\"data:,y.css\");c{d:url(keep.png)}");
    }

    #[test]
    fn resolution() {
        let base = "github/assets/global-1.css";
        assert_eq!(
            resolve(base, "../images/spinners/a.gif"),
            Target::Bundle { path: "github/images/spinners/a.gif".into(), fragment: None }
        );
        assert_eq!(
            resolve("gitlab/assets/application-1.css", "./icons-stacked-2.svg#git"),
            Target::Bundle { path: "gitlab/assets/icons-stacked-2.svg".into(), fragment: Some("git".into()) }
        );
        assert_eq!(
            resolve("gitlab/lib/katex/katex.min.css", "fonts/KaTeX_Main-Regular.woff2?v=1"),
            Target::Bundle { path: "gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff2".into(), fragment: None }
        );
        assert_eq!(
            resolve(base, "a%20b.png"),
            Target::Bundle { path: "github/assets/a b.png".into(), fragment: None }
        );
        assert_eq!(
            resolve(base, "/__/md-render/bundle/github/emoji/x.png"),
            Target::Bundle { path: "github/emoji/x.png".into(), fragment: None }
        );
        assert_eq!(resolve(base, "data:image/png;base64,AA"), Target::Keep);
        assert_eq!(resolve(base, "DATA:,x"), Target::Keep);
        assert_eq!(resolve(base, "#filter"), Target::Keep);
        assert_eq!(resolve(base, ""), Target::Keep);
        assert_eq!(resolve(base, "https://example.com/x.png"), Target::Unresolved);
        assert_eq!(resolve(base, "//cdn.example.com/x.png"), Target::Unresolved);
        assert_eq!(resolve(base, "/images/x.png"), Target::Unresolved);
        assert_eq!(resolve(base, "../../../etc/passwd"), Target::Unresolved);
    }

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize("a/./b/../c").as_deref(), Some("a/c"));
        assert_eq!(normalize("a//b/").as_deref(), Some("a/b"));
        assert_eq!(normalize("../a"), None);
        assert_eq!(normalize(""), None);
    }
}
