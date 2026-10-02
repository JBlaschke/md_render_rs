//! CSS reference handling for exports: find every `url(…)` and `@import` in a stylesheet,
//! resolve it against the stylesheet's bundle path (vendored CSS uses bundle-relative URLs,
//! DESIGN D10) and let the caller substitute a replacement.
//!
//! Finding and rewriting references is done by the styles module's CSS tokenizer (the one
//! vendoring uses, CR §7), so export and vendor agree on what a reference is: text inside
//! comments and strings (`content: "url(x)"`) is never touched, CSS escapes are resolved, and
//! replacements are escaped as the reference's quoting requires. Resolution is
//! [`crate::styles::resolve_bundle_ref`], plus the bundle route that rendered content uses.

use crate::styles::resolve_bundle_ref;
use crate::styles::vendor::css as tokenizer;

pub(crate) use crate::styles::vendor::css::RefKind;

/// URL route under which a server mounts the bundle (`RenderOptions::new` default
/// `bundle_url`). Content rendered for export uses it, and export rewrites it.
pub(crate) const BUNDLE_ROUTE: &str = "/__/md-render/bundle";

/// What a reference in a stylesheet points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// Leave the reference untouched (`data:` URI, `#fragment`, empty).
    Keep,
    /// A file inside the bundle, plus an optional `#fragment` to re-append.
    Bundle {
        path: String,
        fragment: Option<String>,
    },
    /// An absolute/external URL or a path that leaves the bundle.
    Unresolved,
}

/// Every reference in `css` (kind and value with CSS escapes resolved), in source order.
pub(crate) fn references(css: &str) -> Vec<(RefKind, String)> {
    tokenizer::scan(css)
        .into_iter()
        .map(|r| (r.kind, r.value))
        .collect()
}

/// Replace references: `replace(kind, value)` returns the new (unescaped) value, or `None` to
/// keep the reference as written. Only the value changes; `url(`, quotes, `@import` and
/// everything outside references are kept verbatim.
pub(crate) fn rewrite(
    css: &str,
    mut replace: impl FnMut(RefKind, &str) -> Option<String>,
) -> String {
    let refs = tokenizer::scan(css);
    tokenizer::rewrite(css, &refs, |r| replace(r.kind, &r.value))
}

/// The bundle path addressed by a URL under [`BUNDLE_ROUTE`] (`/__/md-render/bundle/a/b.png?v=1`
/// → `a/b.png`), as rendered content references bundle files; `None` for any other URL.
pub(crate) fn bundle_route_path(url: &str) -> Option<String> {
    let rel = url.strip_prefix(BUNDLE_ROUTE)?.strip_prefix('/')?;
    // Resolved from the bundle root (`from` has no directory).
    resolve_bundle_ref("", rel)
}

/// Resolve a stylesheet reference against the bundle path of the stylesheet containing it.
pub(crate) fn resolve(base_path: &str, reference: &str) -> Target {
    let r = reference.trim();
    if r.is_empty()
        || r.starts_with('#')
        || r.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("data:"))
    {
        return Target::Keep;
    }
    let fragment = r.split_once('#').map(|(_, f)| f.to_owned());
    // Root-relative references are only meaningful when they address the bundle route itself.
    let path = if r.starts_with('/') {
        bundle_route_path(r)
    } else {
        resolve_bundle_ref(base_path, r)
    };
    match path {
        Some(path) => Target::Bundle { path, fragment },
        None => Target::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(css: &str) -> Vec<String> {
        references(css).into_iter().map(|(_, r)| r).collect()
    }

    #[test]
    fn route_matches_render_default() {
        assert_eq!(BUNDLE_ROUTE, format!("{}/bundle", crate::PREFIX));
        assert_eq!(
            crate::render::RenderOptions::new(crate::Flavor::Github).bundle_url,
            BUNDLE_ROUTE
        );
    }

    #[test]
    fn finds_all_reference_forms() {
        let css = r#"a{background:url(img/a.png)} b{background:URL( "b c.svg#x" )}
            @import 'x.css'; @import "y.css"; @import url(z.css);
            c{src:url('f.woff2') format("woff2")} d{e:url(data:image/png;base64,AAA=)}"#;
        assert_eq!(
            values(css),
            [
                "img/a.png",
                "b c.svg#x",
                "x.css",
                "y.css",
                "z.css",
                "f.woff2",
                "data:image/png;base64,AAA="
            ]
        );
        let kinds: Vec<RefKind> = references(css).into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            kinds[..5],
            [
                RefKind::Url,
                RefKind::Url,
                RefKind::Import,
                RefKind::Import,
                RefKind::Import
            ]
        );
    }

    /// EXP-3: text in comments and strings is not a reference, and escapes are resolved (the
    /// former regex scanner rewrote `content:"url(x.png)"`, closing the string early, and cut
    /// `url(a\)b.png)` at the escaped parenthesis).
    #[test]
    fn comments_strings_and_escapes() {
        let css = r#"a::after{content:"url(x.png)"}/* url(y.png) */b{background:url("q\"uote.png")}c{background:url(a\)b.png)}"#;
        assert_eq!(values(css), ["q\"uote.png", "a)b.png"]);
        let out = rewrite(css, |_, r| Some(format!("data:,{}", r.len())));
        assert_eq!(
            out,
            r#"a::after{content:"url(x.png)"}/* url(y.png) */b{background:url("data:,10")}c{background:url(data:,7)}"#
        );
    }

    #[test]
    fn rewrite_keeps_quoting_and_unchanged_references() {
        let css = "a{b:url(x.png)}@import 'y.css';c{d:url(keep.png)}e{f:url(\"z.png\")}";
        let out = rewrite(css, |_, r| (r != "keep.png").then(|| format!("data:,{r}")));
        assert_eq!(
            out,
            "a{b:url(data:,x.png)}@import 'data:,y.css';c{d:url(keep.png)}e{f:url(\"data:,z.png\")}"
        );
        // A replacement with characters special in its quoting is escaped.
        assert_eq!(
            rewrite("a{b:url(x)}", |_, _| Some("p(a) r".into())),
            r"a{b:url(p\(a\)\20 r)}"
        );
    }

    #[test]
    fn resolution() {
        let base = "github/assets/global-1.css";
        let bundle = |path: &str, fragment: Option<&str>| Target::Bundle {
            path: path.into(),
            fragment: fragment.map(Into::into),
        };
        assert_eq!(
            resolve(base, "../images/spinners/a.gif"),
            bundle("github/images/spinners/a.gif", None)
        );
        assert_eq!(
            resolve(
                "gitlab/assets/application-1.css",
                "./icons-stacked-2.svg#git"
            ),
            bundle("gitlab/assets/icons-stacked-2.svg", Some("git"))
        );
        assert_eq!(
            resolve(
                "gitlab/lib/katex/katex.min.css",
                "fonts/KaTeX_Main-Regular.woff2?v=1"
            ),
            bundle("gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff2", None)
        );
        assert_eq!(
            resolve(base, "a%20b.png"),
            bundle("github/assets/a b.png", None)
        );
        assert_eq!(
            resolve(base, "/__/md-render/bundle/github/emoji/x.png"),
            bundle("github/emoji/x.png", None)
        );
        assert_eq!(resolve(base, "data:image/png;base64,AA"), Target::Keep);
        assert_eq!(resolve(base, "DATA:,x"), Target::Keep);
        assert_eq!(resolve(base, "#filter"), Target::Keep);
        assert_eq!(resolve(base, ""), Target::Keep);
        assert_eq!(
            resolve(base, "https://example.com/x.png"),
            Target::Unresolved
        );
        assert_eq!(resolve(base, "//cdn.example.com/x.png"), Target::Unresolved);
        assert_eq!(resolve(base, "/images/x.png"), Target::Unresolved);
        assert_eq!(
            resolve(base, "/__/md-render/bundlex/a.png"),
            Target::Unresolved
        );
        assert_eq!(resolve(base, "../../../etc/passwd"), Target::Unresolved);
    }

    #[test]
    fn bundle_route_paths() {
        let p = |u: &str| bundle_route_path(u);
        assert_eq!(
            p("/__/md-render/bundle/github/emoji/a%20b.png?v=2#x").as_deref(),
            Some("github/emoji/a b.png")
        );
        assert_eq!(
            p("/__/md-render/bundle/a/./b/../c.png").as_deref(),
            Some("a/c.png")
        );
        assert_eq!(p("/__/md-render/bundle/../x.png"), None);
        assert_eq!(p("/__/md-render/bundle/"), None);
        assert_eq!(p("/__/md-render/bundle"), None);
        assert_eq!(p("img/a.png"), None);
    }
}
