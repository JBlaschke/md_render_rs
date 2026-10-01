//! Pinned client libraries and GitHub's custom emoji images (DESIGN D8, §4).
//!
//! * GitHub: MathJax 3.2.0 `es5/tex-chtml-full.js` + `es5/ui/safe.js` with the same relative
//!   layout (CQ §1.4: the plain `tex-mml-chtml.js` autoloads extensions at run time and breaks
//!   offline with the synchronous `tex2mml`), mermaid 11.17.2 (GH §8.7), and the 23 custom emoji
//!   PNGs (GH §8.5).
//! * GitLab: KaTeX 0.16.47 `katex.min.js` + `katex.min.css` and the `.woff2` fonts that CSS
//!   references (GL §13 step 4; the `.woff`/`.ttf` fallbacks are never requested by browsers
//!   that support woff2, so they are not vendored), mermaid 11.16.1 (GL §1). GitLab renders math
//!   with `katex.renderToString` (GL §10.2), so KaTeX's auto-render extension is not needed.
//!
//! All libraries come from jsDelivr's npm mirror at exact versions.

use crate::flavor::Flavor;
use crate::styles::manifest::script;

/// jsDelivr npm mirror.
pub const JSDELIVR_NPM: &str = "https://cdn.jsdelivr.net/npm/";
/// MathJax version used by github.com's `<math-renderer>` (GH §8.6).
pub const MATHJAX_VERSION: &str = "3.2.0";
/// Mermaid version used by GitHub's viewscreen renderer (GH §8.7).
pub const GITHUB_MERMAID_VERSION: &str = "11.17.2";
/// KaTeX version in GitLab's `yarn.lock` (GL §1).
pub const KATEX_VERSION: &str = "0.16.47";
/// Mermaid version used by GitLab's sandbox (GL §1).
pub const GITLAB_MERMAID_VERSION: &str = "11.16.1";

/// Where GitHub serves its custom emoji images.
pub const GITHUB_EMOJI_BASE: &str = "https://github.githubassets.com/images/icons/emoji/";

/// GitHub's 23 custom (non-Unicode) emoji, vendored as `github/emoji/<name>.png` (GH §8.5).
pub const GITHUB_CUSTOM_EMOJI: [&str; 23] = [
    "accessibility",
    "atom",
    "basecamp",
    "basecampy",
    "bowtie",
    "copilot",
    "dependabot",
    "electron",
    "feelsgood",
    "finnadie",
    "fishsticks",
    "goberserk",
    "godmode",
    "hurtrealbad",
    "neckbeard",
    "octocat",
    "rage1",
    "rage2",
    "rage3",
    "rage4",
    "shipit",
    "suspect",
    "trollface",
];

/// Bundle path of a GitHub custom emoji image.
pub fn github_emoji_path(name: &str) -> String {
    format!("github/emoji/{name}.png")
}

/// A pinned library package: everything below `base` maps below `prefix` in the bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Package {
    pub base: String,
    pub prefix: String,
    pub files: Vec<LibFile>,
}

/// One file of a [`Package`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LibFile {
    /// Path below the package base (and below the bundle prefix).
    pub rel: &'static str,
    /// Key in `FlavorManifest::scripts`.
    pub script: &'static str,
    /// Stylesheet whose `url()`s must be vendored and rewritten too.
    pub css: bool,
}

impl Package {
    pub(crate) fn url(&self, f: &LibFile) -> String {
        format!("{}{}", self.base, f.rel)
    }

    pub(crate) fn path(&self, f: &LibFile) -> String {
        format!("{}/{}", self.prefix, f.rel)
    }
}

fn npm(pkg: &str, version: &str, dir: &str) -> String {
    format!("{JSDELIVR_NPM}{pkg}@{version}/{dir}")
}

/// The libraries vendored for `flavor`.
pub(crate) fn packages(flavor: Flavor) -> Vec<Package> {
    match flavor {
        Flavor::Github => vec![
            Package {
                base: npm("mathjax", MATHJAX_VERSION, ""),
                prefix: "github/lib/mathjax".into(),
                files: vec![
                    LibFile { rel: "es5/tex-chtml-full.js", script: script::MATHJAX, css: false },
                    LibFile { rel: "es5/ui/safe.js", script: script::MATHJAX_SAFE, css: false },
                ],
            },
            Package {
                base: npm("mermaid", GITHUB_MERMAID_VERSION, "dist/"),
                prefix: "github/lib/mermaid".into(),
                files: vec![LibFile { rel: "mermaid.min.js", script: script::MERMAID, css: false }],
            },
        ],
        Flavor::Gitlab => vec![
            Package {
                base: npm("katex", KATEX_VERSION, "dist/"),
                prefix: "gitlab/lib/katex".into(),
                files: vec![
                    LibFile { rel: "katex.min.js", script: script::KATEX, css: false },
                    LibFile { rel: "katex.min.css", script: script::KATEX_CSS, css: true },
                ],
            },
            Package {
                base: npm("mermaid", GITLAB_MERMAID_VERSION, "dist/"),
                prefix: "gitlab/lib/mermaid".into(),
                files: vec![LibFile { rel: "mermaid.min.js", script: script::MERMAID, css: false }],
            },
        ],
    }
}

/// Whether a `url()` found in a library stylesheet should be downloaded. KaTeX lists
/// `woff2`, `woff` and `ttf` for every font; only `woff2` is ever used by current browsers.
pub(crate) fn want_lib_css_asset(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    !(lower.ends_with(".woff") || lower.ends_with(".ttf") || lower.ends_with(".eot"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_design() {
        let gh = packages(Flavor::Github);
        let paths: Vec<(String, String)> = gh
            .iter()
            .flat_map(|p| p.files.iter().map(move |f| (p.url(f), p.path(f))))
            .collect();
        assert_eq!(
            paths,
            vec![
                (
                    "https://cdn.jsdelivr.net/npm/mathjax@3.2.0/es5/tex-chtml-full.js".into(),
                    "github/lib/mathjax/es5/tex-chtml-full.js".into()
                ),
                (
                    "https://cdn.jsdelivr.net/npm/mathjax@3.2.0/es5/ui/safe.js".into(),
                    "github/lib/mathjax/es5/ui/safe.js".into()
                ),
                (
                    "https://cdn.jsdelivr.net/npm/mermaid@11.17.2/dist/mermaid.min.js".into(),
                    "github/lib/mermaid/mermaid.min.js".into()
                ),
            ]
        );
        let gl = packages(Flavor::Gitlab);
        assert_eq!(gl[0].url(&gl[0].files[1]), "https://cdn.jsdelivr.net/npm/katex@0.16.47/dist/katex.min.css");
        assert_eq!(gl[0].path(&gl[0].files[1]), "gitlab/lib/katex/katex.min.css");
        assert_eq!(gl[1].path(&gl[1].files[0]), "gitlab/lib/mermaid/mermaid.min.js");
        assert!(gl[1].base.contains("mermaid@11.16.1"));
    }

    #[test]
    fn emoji() {
        assert_eq!(GITHUB_CUSTOM_EMOJI.len(), 23);
        assert_eq!(github_emoji_path("octocat"), "github/emoji/octocat.png");
        let mut sorted = GITHUB_CUSTOM_EMOJI.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 23);
    }

    #[test]
    fn font_filter() {
        assert!(want_lib_css_asset("gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff2"));
        assert!(!want_lib_css_asset("gitlab/lib/katex/fonts/KaTeX_Main-Regular.woff"));
        assert!(!want_lib_css_asset("gitlab/lib/katex/fonts/KaTeX_Main-Regular.TTF"));
    }
}
