//! Mapping downloaded URLs to bundle paths (DESIGN D10, §4) and the relative-path math used to
//! rewrite `url()` references.
//!
//! Bundle paths mirror the site's URL paths below a per-flavor prefix (`github/assets/x.css`,
//! `github/images/spinners/y.gif`, `gitlab/assets/gitlab-sans/z.woff2`) so the CSS keeps working
//! when the bundle is mounted under any URL prefix or exported to a directory. Every path is
//! sanitized so it is a valid [`tarfs::check_path`] path, needs no percent-encoding in a URL,
//! survives the tarfs reader (no `._*` AppleDouble names) and fits a USTAR header.

use std::collections::HashMap;

use url::Url;

pub(crate) use crate::styles::sha256_hex;

/// Whether `path` fits a USTAR header: name ≤ 100 bytes, or a split at a `/` into a prefix of
/// ≤ 155 bytes and a name of ≤ 100 bytes (the tarfs default writer's rule).
pub(crate) fn fits_ustar(path: &str) -> bool {
    let b = path.as_bytes();
    if b.len() <= 100 {
        return true;
    }
    b.iter()
        .enumerate()
        .any(|(i, &c)| c == b'/' && i <= 155 && b.len() - i - 1 <= 100 && i + 1 < b.len())
}

/// Characters kept verbatim in a path segment; everything else becomes `_`. All of these are
/// valid unencoded in a URL path and in a CSS unquoted `url()`.
fn is_safe_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '~' | '@' | '+' | '=' | ',')
}

/// Sanitize one (percent-encoded) URL path segment into a bundle path segment.
pub(crate) fn sanitize_segment(seg: &str) -> String {
    let decoded = percent_encoding::percent_decode_str(seg).decode_utf8_lossy();
    let mut s: String = decoded.chars().map(|c| if is_safe_char(c) { c } else { '_' }).collect();
    if s.is_empty() || s == "." || s == ".." {
        s = "_".into();
    }
    if s.starts_with("._") {
        // tarfs skips macOS AppleDouble companions (`._*`) when reading.
        s.insert(0, '_');
    }
    s
}

/// Split a file name into stem and extension (`a.b.css` -> (`a.b`, `.css`)).
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 12 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Insert `-<tag>` before the extension of the last segment of `path`.
fn with_tag(path: &str, tag: &str) -> String {
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let (stem, ext) = split_ext(name);
    format!("{dir}{stem}-{tag}{ext}")
}

/// Relative URL path from the file `from` to the file `to` (both bundle paths), e.g.
/// `github/assets/global.css` → `github/images/a.gif` gives `../images/a.gif`.
pub(crate) fn relative_path(from: &str, to: &str) -> String {
    let from_dir: Vec<&str> = match from.rfind('/') {
        Some(i) => from[..i].split('/').collect(),
        None => Vec::new(),
    };
    let to_parts: Vec<&str> = to.split('/').collect();
    let (to_dir, to_name) = to_parts.split_at(to_parts.len().saturating_sub(1));
    let common = from_dir.iter().zip(to_dir).take_while(|(a, b)| a == b).count();
    let mut out = String::new();
    for _ in common..from_dir.len() {
        out.push_str("../");
    }
    for d in &to_dir[common..] {
        out.push_str(d);
        out.push('/');
    }
    out.push_str(to_name.first().copied().unwrap_or_default());
    out
}

/// One mount: URLs below `base` (same origin, path prefix) map below `prefix`.
#[derive(Clone, Debug)]
struct Mount {
    base: Url,
    prefix: String,
}

/// Assigns bundle paths to URLs for one flavor: deterministic, collision-free, idempotent.
#[derive(Debug)]
pub(crate) struct Mapper {
    flavor_prefix: String,
    mounts: Vec<Mount>,
    by_url: HashMap<String, String>,
    by_path: HashMap<String, String>,
}

impl Mapper {
    /// A mapper whose paths all start with `flavor_prefix/` (`github` or `gitlab`).
    pub(crate) fn new(flavor_prefix: &str) -> Self {
        Mapper {
            flavor_prefix: flavor_prefix.to_owned(),
            mounts: Vec::new(),
            by_url: HashMap::new(),
            by_path: HashMap::new(),
        }
    }

    /// Map URLs below `base` (its path should end with `/`) to paths below `prefix`. The most
    /// specific (longest) matching mount wins.
    pub(crate) fn mount(&mut self, base: &Url, prefix: &str) {
        let mut base = base.clone();
        base.set_query(None);
        base.set_fragment(None);
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        self.mounts.push(Mount { base, prefix: prefix.trim_end_matches('/').to_owned() });
        self.mounts.sort_by_key(|m| std::cmp::Reverse(m.base.path().len()));
    }

    /// Dedupe key: the URL without its fragment.
    fn key(url: &Url) -> String {
        let mut u = url.clone();
        u.set_fragment(None);
        u.into()
    }

    /// Claim `path` for `url` (e.g. a library at a fixed location). If `path` is taken by another
    /// URL a tagged variant is used; the path actually assigned is returned.
    pub(crate) fn reserve(&mut self, url: &Url, path: &str) -> String {
        let key = Self::key(url);
        if let Some(p) = self.by_url.get(&key) {
            return p.clone();
        }
        let path = self.unique(path, &key);
        self.insert(key, path.clone());
        path
    }

    /// The bundle path for `url` (fragment ignored).
    pub(crate) fn map(&mut self, url: &Url) -> String {
        let key = Self::key(url);
        if let Some(p) = self.by_url.get(&key) {
            return p.clone();
        }
        let raw_path = url.path();
        let (prefix, rest) = match self
            .mounts
            .iter()
            .find(|m| m.base.origin() == url.origin() && raw_path.starts_with(m.base.path()))
        {
            Some(m) => (m.prefix.clone(), &raw_path[m.base.path().len()..]),
            None => {
                // Foreign host: keep it apart so it can never shadow a site path.
                let mut host = sanitize_segment(url.host_str().unwrap_or("host"));
                if let Some(port) = url.port() {
                    host.push_str(&format!("_{port}"));
                }
                (format!("{}/_ext/{host}", self.flavor_prefix), raw_path.trim_start_matches('/'))
            }
        };
        let mut segs: Vec<String> =
            rest.split('/').filter(|s| !s.is_empty()).map(sanitize_segment).collect();
        if segs.is_empty() || rest.ends_with('/') {
            segs.push("index".into());
        }
        let mut path = format!("{prefix}/{}", segs.join("/"));
        if let Some(q) = url.query().filter(|q| !q.is_empty()) {
            path = with_tag(&path, &format!("q{}", &sha256_hex(q.as_bytes())[..8]));
        }
        if !fits_ustar(&path) {
            let name = segs.last().map(String::as_str).unwrap_or("index");
            let (_, ext) = split_ext(name);
            path = format!("{}/_long/{}{ext}", self.flavor_prefix, &sha256_hex(key.as_bytes())[..32]);
        }
        let path = self.unique(&path, &key);
        self.insert(key, path.clone());
        path
    }

    /// `path`, or a variant tagged with a hash of `key` if another URL already has it.
    fn unique(&self, path: &str, key: &str) -> String {
        let free = |p: &str| self.by_path.get(p).is_none_or(|k| k == key);
        if free(path) {
            return path.to_owned();
        }
        let tag = &sha256_hex(key.as_bytes())[..8];
        let mut candidate = with_tag(path, tag);
        let mut n = 2;
        while !free(&candidate) {
            candidate = with_tag(path, &format!("{tag}{n}"));
            n += 1;
        }
        candidate
    }

    fn insert(&mut self, key: String, path: String) {
        self.by_path.insert(path.clone(), key.clone());
        self.by_url.insert(key, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("test url")
    }

    #[test]
    fn relative_paths() {
        let cases = [
            ("github/assets/global.css", "github/images/spinners/a.gif", "../images/spinners/a.gif"),
            ("github/assets/global.css", "github/assets/x.png", "x.png"),
            ("github/assets/global.css", "github/assets/images/x.svg", "images/x.svg"),
            (
                "gitlab/assets/highlight/themes/white.css",
                "gitlab/assets/icons-stacked.svg",
                "../../icons-stacked.svg",
            ),
            ("gitlab/assets/fonts.css", "gitlab/assets/gitlab-sans/S.woff2", "gitlab-sans/S.woff2"),
            ("gitlab/lib/katex/katex.min.css", "gitlab/lib/katex/fonts/K.woff2", "fonts/K.woff2"),
            ("a/b/c/d.css", "a/x/y/z.png", "../../x/y/z.png"),
            ("a/b.css", "c/d.png", "../c/d.png"),
            ("top.css", "a/b.png", "a/b.png"),
            ("a/top.css", "b.png", "../b.png"),
            ("same.css", "other.png", "other.png"),
            // Shared prefix of names must not count as a shared directory.
            ("gitlab/assets/a.css", "gitlab/assets2/b.png", "../assets2/b.png"),
        ];
        for (from, to, want) in cases {
            assert_eq!(relative_path(from, to), want, "{from} -> {to}");
        }
    }

    #[test]
    fn relative_path_resolves_back() {
        // Joining the relative path onto the CSS URL must give the asset URL.
        let base = url("http://x/__/md-render/bundle/");
        for (from, to) in [
            ("github/assets/global.css", "github/images/a/b.gif"),
            ("gitlab/assets/highlight/themes/dark.css", "gitlab/assets/icons.svg"),
            ("gitlab/lib/katex/katex.min.css", "gitlab/lib/katex/fonts/f.woff2"),
        ] {
            let css = base.join(from).expect("join");
            let got = css.join(&relative_path(from, to)).expect("join");
            assert_eq!(got, base.join(to).expect("join"));
        }
    }

    #[test]
    fn sanitizes_segments() {
        assert_eq!(sanitize_segment("a%20b.png"), "a_b.png");
        assert_eq!(sanitize_segment("image_comment_light_cursor@2x-1.svg"), "image_comment_light_cursor@2x-1.svg");
        assert_eq!(sanitize_segment("%C3%BC.css"), "_.css");
        assert_eq!(sanitize_segment("%2E%2E"), "_");
        assert_eq!(sanitize_segment(".."), "_");
        assert_eq!(sanitize_segment("._x"), "_._x");
        assert_eq!(sanitize_segment("a\"b(c).png"), "a_b_c_.png");
        for s in ["a%20b", "%00", "%2F", "x?y", "\\"] {
            let out = sanitize_segment(s);
            assert!(tarfs::check_path(&out).is_ok(), "{s} -> {out}");
            assert!(!out.contains('/'));
        }
    }

    #[test]
    fn ustar_fit() {
        assert!(fits_ustar(&"a".repeat(100)));
        assert!(!fits_ustar(&"a".repeat(101)));
        assert!(fits_ustar(&format!("{}/{}", "d".repeat(155), "n".repeat(100))));
        assert!(!fits_ustar(&format!("{}/{}", "d".repeat(156), "n".repeat(100))));
        assert!(!fits_ustar(&format!("{}/{}", "d".repeat(10), "n".repeat(101))));
        assert!(fits_ustar(&format!("{}/{}/{}", "a".repeat(100), "b".repeat(50), "n".repeat(99))));
    }

    #[test]
    fn mapper_mirrors_site_paths() {
        let mut m = Mapper::new("github");
        m.mount(&url("https://github.githubassets.com/"), "github");
        assert_eq!(
            m.map(&url("https://github.githubassets.com/assets/light-99f877e9ddfc0e51.css")),
            "github/assets/light-99f877e9ddfc0e51.css"
        );
        assert_eq!(
            m.map(&url("https://github.githubassets.com/images/spinners/octocat-spinner-32.gif")),
            "github/images/spinners/octocat-spinner-32.gif"
        );
        // Fragment ignored, same path; idempotent.
        assert_eq!(
            m.map(&url("https://github.githubassets.com/images/spinners/octocat-spinner-32.gif#x")),
            "github/images/spinners/octocat-spinner-32.gif"
        );
        // Foreign host.
        assert_eq!(m.map(&url("https://example.com:8443/a/b.png")), "github/_ext/example.com_8443/a/b.png");
        // Directory URL.
        assert_eq!(m.map(&url("https://github.githubassets.com/dir/")), "github/dir/index");
    }

    #[test]
    fn mapper_longest_mount_wins() {
        let mut m = Mapper::new("gitlab");
        m.mount(&url("https://cdn.jsdelivr.net/"), "gitlab/_cdn");
        m.mount(&url("https://cdn.jsdelivr.net/npm/katex@0.16.47/dist"), "gitlab/lib/katex");
        assert_eq!(
            m.map(&url("https://cdn.jsdelivr.net/npm/katex@0.16.47/dist/fonts/KaTeX_Main-Regular.woff2")),
            "gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff2"
        );
        assert_eq!(m.map(&url("https://cdn.jsdelivr.net/other.js")), "gitlab/_cdn/other.js");
    }

    #[test]
    fn mapper_queries_and_collisions() {
        let mut m = Mapper::new("gitlab");
        m.mount(&url("https://gitlab.com/"), "gitlab");
        let a = m.map(&url("https://gitlab.com/assets/a.png?v=1"));
        let b = m.map(&url("https://gitlab.com/assets/a.png?v=2"));
        let c = m.map(&url("https://gitlab.com/assets/a.png"));
        assert!(a.starts_with("gitlab/assets/a-q") && a.ends_with(".png"), "{a}");
        assert_ne!(a, b);
        assert_eq!(c, "gitlab/assets/a.png");
        // Two URLs that sanitize to the same path get distinct paths.
        let d = m.map(&url("https://gitlab.com/assets/x%20y.png"));
        let e = m.map(&url("https://gitlab.com/assets/x_y.png"));
        assert_eq!(d, "gitlab/assets/x_y.png");
        assert_ne!(d, e);
        assert!(e.starts_with("gitlab/assets/x_y-") && e.ends_with(".png"), "{e}");
        // Reserved paths take precedence and are stable.
        let lib = url("https://cdn.jsdelivr.net/npm/mermaid@11.16.1/dist/mermaid.min.js");
        assert_eq!(m.reserve(&lib, "gitlab/lib/mermaid/mermaid.min.js"), "gitlab/lib/mermaid/mermaid.min.js");
        assert_eq!(m.map(&lib), "gitlab/lib/mermaid/mermaid.min.js");
        let other = url("https://elsewhere.example/mermaid.min.js");
        assert_ne!(m.reserve(&other, "gitlab/lib/mermaid/mermaid.min.js"), "gitlab/lib/mermaid/mermaid.min.js");
    }

    #[test]
    fn mapper_long_paths_fit_ustar() {
        let mut m = Mapper::new("github");
        m.mount(&url("https://h/"), "github");
        let long = format!("https://h/{}/{}.css", "d".repeat(200), "n".repeat(150));
        let p = m.map(&url(&long));
        assert!(fits_ustar(&p), "{p}");
        assert!(p.starts_with("github/_long/") && p.ends_with(".css"), "{p}");
        assert!(tarfs::check_path(&p).is_ok());
    }

    #[test]
    fn sha() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
