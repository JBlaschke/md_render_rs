//! Scraping a site page for its stylesheets (GH §4.1, GL §11.1 / §13 step 2) and classifying them
//! into the roles of DESIGN §3.4 by file-name stem. Hashed names are never hard-coded.

use std::collections::HashSet;
use std::sync::OnceLock;

use dom_query::Document;
use regex::Regex;
use url::Url;

use crate::styles::manifest::role;

/// A `<link rel=stylesheet>` found on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkRef {
    /// `href`, or `data-href` (GitHub's lazily enabled themes), resolved against the page.
    pub url: Url,
    /// The `media` attribute, if any.
    pub media: Option<String>,
    /// GitHub's `data-color-theme` attribute, if any.
    pub color_theme: Option<String>,
}

/// What the page offers.
#[derive(Clone, Debug, Default)]
pub(crate) struct Page {
    /// Stylesheet links in document order, deduplicated by URL (first occurrence wins).
    pub links: Vec<LinkRef>,
    /// GitLab `icons-<hash>.svg` sprite URLs referenced anywhere on the page (fragment removed).
    pub sprites: Vec<Url>,
}

fn resolve(base: &Url, href: &str) -> Option<Url> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let mut u = base.join(href).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    u.set_fragment(None);
    Some(u)
}

fn sprite_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^icons-[0-9a-f]{16,}\.svg$").expect("valid regex"))
}

/// The last path segment of `url`.
pub(crate) fn file_name(url: &Url) -> &str {
    url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("")
}

/// Parse `html` (fetched from `page_url`) and collect its stylesheets and icon sprites.
pub(crate) fn scrape_page(html: &str, page_url: &Url) -> Page {
    let doc = Document::from(html);
    // <base href> changes the resolution base (rare, but cheap to honour).
    let base = doc
        .select("base[href]")
        .nodes()
        .first()
        .and_then(|n| n.attr("href"))
        .and_then(|h| page_url.join(h.trim()).ok())
        .unwrap_or_else(|| page_url.clone());

    let mut page = Page::default();
    let mut seen = HashSet::new();
    for node in doc.select("link[rel]").nodes() {
        let rel = node.attr("rel").map(|r| r.to_ascii_lowercase()).unwrap_or_default();
        let tokens: Vec<&str> = rel.split_ascii_whitespace().collect();
        if !tokens.contains(&"stylesheet") || tokens.contains(&"alternate") {
            continue;
        }
        let href = node.attr("href").or_else(|| node.attr("data-href"));
        let Some(url) = href.and_then(|h| resolve(&base, &h)) else { continue };
        if !seen.insert(url.as_str().to_owned()) {
            continue;
        }
        let media = node.attr("media").map(|m| m.trim().to_owned()).filter(|m| !m.is_empty());
        let color_theme = node.attr("data-color-theme").map(|t| t.trim().to_owned());
        page.links.push(LinkRef { url, media, color_theme });
    }

    let mut seen = HashSet::new();
    for node in doc.select("*").nodes() {
        for attr in node.attrs() {
            let value: &str = &attr.value;
            if !value.contains("icons-") {
                continue;
            }
            let Some(url) = resolve(&base, value) else { continue };
            if sprite_re().is_match(file_name(&url)) && seen.insert(url.as_str().to_owned()) {
                page.sprites.push(url);
            }
        }
    }
    page
}

/// A stylesheet selected for vendoring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SheetSpec {
    pub url: Url,
    /// One of [`role`].
    pub role: &'static str,
    pub theme: Option<String>,
    pub media: Option<String>,
}

/// Result of classification.
#[derive(Clone, Debug, Default)]
pub(crate) struct Classified {
    /// Selected stylesheets in cascade order.
    pub sheets: Vec<SheetSpec>,
    /// Links that were not selected, with the reason.
    pub skipped: Vec<(Url, &'static str)>,
}

impl Classified {
    /// Whether a sheet of `role` (and `theme`, if given) was selected.
    pub(crate) fn has(&self, role: &str, theme: Option<&str>) -> bool {
        self.sheets.iter().any(|s| s.role == role && (theme.is_none() || s.theme.as_deref() == theme))
    }
}

fn github_res() -> &'static (Regex, Regex, Regex) {
    static RES: OnceLock<(Regex, Regex, Regex)> = OnceLock::new();
    RES.get_or_init(|| {
        (
            // GH §4.1 THEME_RE / CORE_RE / MODULE_RE, with the hash separator and length relaxed
            // (and the hash optional) so a cosmetic change in GitHub's naming does not break us.
            Regex::new(r"^((?:light|dark)(?:_[a-z]+)*)(?:[-.][0-9a-f]{8,})?\.css$").expect("valid regex"),
            Regex::new(
                r"^(primer-primitives|primer|global|github|repository|code)(?:[-.][0-9a-f]{8,})?\.css$",
            )
            .expect("valid regex"),
            Regex::new(r"\.module\.css$").expect("valid regex"),
        )
    })
}

/// Cascade rank of a role (GH §4.5 order; GitLab page order).
fn rank(role: &str) -> usize {
    [
        role::THEME,
        role::PRIMITIVES,
        role::PRIMER,
        role::GLOBAL,
        role::GITHUB,
        role::REPOSITORY,
        role::CODE,
        role::APP,
        role::TAILWIND,
        role::FONTS,
        role::HIGHLIGHT,
    ]
    .iter()
    .position(|r| *r == role)
    .unwrap_or(usize::MAX)
}

fn sort_cascade(sheets: &mut [SheetSpec]) {
    // Stable: page order within a role.
    sheets.sort_by_key(|s| rank(s.role));
}

/// `media="all"` is the default; record it as absent (GH §4.1: GitHub's links all carry it).
fn normalize_media(media: &Option<String>) -> Option<String> {
    media.clone().filter(|m| !m.eq_ignore_ascii_case("all"))
}

/// GitHub: themes (`light` + `dark`, or all with `all_themes`), primer-primitives, primer,
/// global, github, repository, code; every `*.module.css` and anything unknown is dropped.
pub(crate) fn classify_github(links: &[LinkRef], all_themes: bool) -> Classified {
    let (theme_re, core_re, module_re) = github_res();
    let mut out = Classified::default();
    let mut themes_seen = HashSet::new();
    for link in links {
        let name = file_name(&link.url);
        if module_re.is_match(name) {
            out.skipped.push((link.url.clone(), "React component CSS (*.module.css)"));
        } else if let Some(c) = theme_re.captures(name) {
            let id = c[1].to_owned();
            if !all_themes && id != "light" && id != "dark" {
                out.skipped.push((link.url.clone(), "theme not requested (use all themes)"));
            } else if !themes_seen.insert(id.clone()) {
                out.skipped.push((link.url.clone(), "duplicate theme"));
            } else {
                out.sheets.push(SheetSpec {
                    url: link.url.clone(),
                    role: role::THEME,
                    theme: Some(id),
                    media: normalize_media(&link.media),
                });
            }
        } else if let Some(c) = core_re.captures(name) {
            let role = match &c[1] {
                "primer-primitives" => role::PRIMITIVES,
                "primer" => role::PRIMER,
                "global" => role::GLOBAL,
                "github" => role::GITHUB,
                "repository" => role::REPOSITORY,
                _ => role::CODE,
            };
            if out.has(role, None) {
                out.skipped.push((link.url.clone(), "duplicate role"));
            } else {
                out.sheets.push(SheetSpec {
                    url: link.url.clone(),
                    role,
                    theme: None,
                    media: normalize_media(&link.media),
                });
            }
        } else {
            out.skipped.push((link.url.clone(), "not a markdown-relevant stylesheet"));
        }
    }
    sort_cascade(&mut out.sheets);
    out
}

fn gitlab_res() -> &'static (Regex, Regex) {
    static RES: OnceLock<(Regex, Regex)> = OnceLock::new();
    RES.get_or_init(|| {
        (
            // GL §13 step 2 (digest optional for self-hosted instances without asset digests).
            Regex::new(r"^(application|application_dark|tailwind|fonts)(?:-[0-9a-f]{8,})?\.css$")
                .expect("valid regex"),
            Regex::new(r"^([a-z_-]+?)(?:-[0-9a-f]{8,})?\.css$").expect("valid regex"),
        )
    })
}

/// GitLab: application (light) / application_dark (dark), tailwind, fonts, highlight
/// white (light) / dark (dark); `media` kept verbatim. Page bundles and other highlight themes
/// (only shown to logged-in users who picked them, GL §11.1) are dropped.
pub(crate) fn classify_gitlab(links: &[LinkRef]) -> Classified {
    let (core_re, highlight_re) = gitlab_res();
    let mut out = Classified::default();
    for link in links {
        let name = file_name(&link.url);
        let in_highlight = link.url.path().contains("/highlight/themes/");
        let spec = if in_highlight {
            match highlight_re.captures(name).as_ref().map(|c| &c[1]) {
                Some("white") => Some((role::HIGHLIGHT, Some("light"))),
                Some("dark") => Some((role::HIGHLIGHT, Some("dark"))),
                _ => None,
            }
        } else {
            match core_re.captures(name).as_ref().map(|c| &c[1]) {
                Some("application") => Some((role::APP, Some("light"))),
                Some("application_dark") => Some((role::APP, Some("dark"))),
                Some("tailwind") => Some((role::TAILWIND, None)),
                Some("fonts") => Some((role::FONTS, None)),
                _ => None,
            }
        };
        match spec {
            Some((role, theme)) if out.has(role, theme) => {
                out.skipped.push((link.url.clone(), "duplicate role"));
            }
            Some((role, theme)) => out.sheets.push(SheetSpec {
                url: link.url.clone(),
                role,
                theme: theme.map(str::to_owned),
                media: link.media.clone(),
            }),
            None if in_highlight => {
                out.skipped.push((link.url.clone(), "highlight theme other than white/dark"))
            }
            None => out.skipped.push((link.url.clone(), "not a markdown-relevant stylesheet")),
        }
    }
    sort_cascade(&mut out.sheets);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GH_PAGE: &str = r#"<!DOCTYPE html><html><head>
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/light-99f877e9ddfc0e51.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/light_high_contrast-48fdd0811afbab3c.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/dark-79ad2ace604703b3.css" />
<link data-color-theme="light" crossorigin="anonymous" media="all" rel="stylesheet" data-href="https://github.githubassets.com/assets/light-99f877e9ddfc0e51.css" />
<link data-color-theme="dark_dimmed" crossorigin="anonymous" media="all" rel="stylesheet" data-href="https://github.githubassets.com/assets/dark_dimmed-0de76f07cc035b10.css" />
<link data-color-theme="dark" crossorigin="anonymous" media="all" rel="stylesheet" data-href="https://github.githubassets.com/assets/dark-79ad2ace604703b3.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/primer-primitives-ed9ca172356fd545.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/primer-4136ede8b2650a2d.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/global-54ba76e934a49d7c.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/github-7338c07b3a07b8be.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/repository-11ee8a031c040c1a.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/code-2d56bdb0166c0238.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/primer-react-css.46296dcf75e7a0b0.module.css" />
<link crossorigin="anonymous" media="all" rel="stylesheet" href="https://github.githubassets.com/assets/code-view.ad7b18e8a715cd42.module.css" />
<link rel="preload" as="style" href="https://github.githubassets.com/assets/preload-0123456789abcdef.css" />
<link rel="alternate stylesheet" href="https://github.githubassets.com/assets/global-ffffffffffffffff.css" />
<link rel="icon" href="https://github.githubassets.com/favicons/favicon.svg" />
</head><body></body></html>"#;

    fn gh_page() -> Page {
        scrape_page(GH_PAGE, &Url::parse("https://github.com/joeyespo/grip").expect("url"))
    }

    #[test]
    fn scrapes_href_and_data_href_deduped() {
        let page = gh_page();
        let names: Vec<&str> = page.links.iter().map(|l| file_name(&l.url)).collect();
        assert_eq!(
            names,
            vec![
                "light-99f877e9ddfc0e51.css",
                "light_high_contrast-48fdd0811afbab3c.css",
                "dark-79ad2ace604703b3.css",
                "dark_dimmed-0de76f07cc035b10.css",
                "primer-primitives-ed9ca172356fd545.css",
                "primer-4136ede8b2650a2d.css",
                "global-54ba76e934a49d7c.css",
                "github-7338c07b3a07b8be.css",
                "repository-11ee8a031c040c1a.css",
                "code-2d56bdb0166c0238.css",
                "primer-react-css.46296dcf75e7a0b0.module.css",
                "code-view.ad7b18e8a715cd42.module.css",
            ]
        );
        assert_eq!(page.links[0].media.as_deref(), Some("all"));
        assert_eq!(page.links[3].color_theme.as_deref(), Some("dark_dimmed"));
    }

    #[test]
    fn github_default_themes() {
        let c = classify_github(&gh_page().links, false);
        let got: Vec<(&str, Option<&str>)> =
            c.sheets.iter().map(|s| (s.role, s.theme.as_deref())).collect();
        assert_eq!(
            got,
            vec![
                ("theme", Some("light")),
                ("theme", Some("dark")),
                ("primitives", None),
                ("primer", None),
                ("global", None),
                ("github", None),
                ("repository", None),
                ("code", None),
            ]
        );
        assert!(c.sheets.iter().all(|s| s.media.is_none()), "media=all is normalized away");
        assert!(c.skipped.iter().any(|(u, _)| file_name(u).ends_with(".module.css")));
    }

    #[test]
    fn github_all_themes_keep_page_order() {
        let c = classify_github(&gh_page().links, true);
        let themes: Vec<&str> = c.sheets.iter().filter_map(|s| s.theme.as_deref()).collect();
        assert_eq!(themes, vec!["light", "light_high_contrast", "dark", "dark_dimmed"]);
        assert_eq!(c.sheets[4].role, "primitives");
    }

    #[test]
    fn github_cascade_order_is_enforced() {
        // Shuffled page: code before primer, themes last.
        let base = Url::parse("https://h/assets/").expect("url");
        let links: Vec<LinkRef> = ["code-0123456789abcdef.css", "primer-0123456789abcdef.css", "dark-0123456789abcdef.css"]
            .iter()
            .map(|n| LinkRef { url: base.join(n).expect("join"), media: None, color_theme: None })
            .collect();
        let c = classify_github(&links, false);
        let roles: Vec<&str> = c.sheets.iter().map(|s| s.role).collect();
        assert_eq!(roles, vec!["theme", "primer", "code"]);
    }

    const GL_PAGE: &str = r#"<html class="gl-system ui-neutral"><head>
<link as="font" crossorigin="" href="/assets/gitlab-sans/GitLabSans-9892dc17af892e03de41625c0ee325117a3b8ee4ba6005f3a3eac68510030aed.woff2" rel="preload">
<link rel="stylesheet" href="/assets/application-dfd7063161c5166e934cb30dfb2873984c9e7644df019dfc87f516c90017fc17.css" media="(prefers-color-scheme: light)" />
<link rel="stylesheet" href="/assets/application_dark-eeaf891b87491eaea88e702df7c19303ed0d19e4f71eab7d60dfa6eba6a662e9.css" media="(prefers-color-scheme: dark)" />
<link rel="stylesheet" href="/assets/page_bundles/project-d8ddfdcca11af4ede140816f3befd2a538014d7403c28e0f3c62a50b61f93805.css" />
<link rel="stylesheet" href="/assets/tailwind-b6158d77dcda1a08914e5797673ae160111032ac3a269208fac14e807a078ff3.css" />
<link rel="stylesheet" href="/assets/fonts-deb7ad1d55ca77c0172d8538d53442af63604ff490c74acc2859db295c125bdb.css" />
<link rel="stylesheet" href="/assets/highlight/themes/white-9412a4ed9229ac65b95a034746ad1eb9460b35d3ff5a443489efed3fa788f4e4.css" media="(prefers-color-scheme: light)" />
<link rel="stylesheet" href="/assets/highlight/themes/dark-93b40c67c64b8df263614c6536b9b50ff93ed851711848703513c238f4abc8b5.css" media="(prefers-color-scheme: dark)" />
<link rel="stylesheet" href="/assets/highlight/themes/monokai-0123456789abcdef0123456789abcdef.css" />
</head><body>
<svg><use href="/assets/icons-5a3f88503a318f1eaf3b49d9d82c93cdde31fd5224ab8aeeb08534974b21f10c.svg#branch"></use></svg>
<svg><use xlink:href="/assets/icons-5a3f88503a318f1eaf3b49d9d82c93cdde31fd5224ab8aeeb08534974b21f10c.svg#commit"></use></svg>
<div data-icon-sprite="/assets/icons-stacked-8c179348aef46d5e4ea97c22f4483343a984d0362852a43c5bfdf1bd54601217.svg"></div>
</body></html>"#;

    fn gl_page() -> Page {
        scrape_page(GL_PAGE, &Url::parse("https://gitlab.com/gitlab-org/gitlab-foss").expect("url"))
    }

    #[test]
    fn gitlab_classification() {
        let page = gl_page();
        let c = classify_gitlab(&page.links);
        let got: Vec<(&str, Option<&str>, Option<&str>)> = c
            .sheets
            .iter()
            .map(|s| (s.role, s.theme.as_deref(), s.media.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("app", Some("light"), Some("(prefers-color-scheme: light)")),
                ("app", Some("dark"), Some("(prefers-color-scheme: dark)")),
                ("tailwind", None, None),
                ("fonts", None, None),
                ("highlight", Some("light"), Some("(prefers-color-scheme: light)")),
                ("highlight", Some("dark"), Some("(prefers-color-scheme: dark)")),
            ]
        );
        assert_eq!(
            c.sheets[0].url.as_str(),
            "https://gitlab.com/assets/application-dfd7063161c5166e934cb30dfb2873984c9e7644df019dfc87f516c90017fc17.css"
        );
        assert_eq!(c.skipped.len(), 2, "{:?}", c.skipped);
    }

    #[test]
    fn gitlab_sprites() {
        let page = gl_page();
        let sprites: Vec<&str> = page.sprites.iter().map(Url::as_str).collect();
        assert_eq!(
            sprites,
            vec!["https://gitlab.com/assets/icons-5a3f88503a318f1eaf3b49d9d82c93cdde31fd5224ab8aeeb08534974b21f10c.svg"]
        );
    }

    #[test]
    fn base_href_and_relative_links() {
        let html = r#"<head><base href="https://cdn.example/root/"><link rel=STYLESHEET href="x/primer-0123456789abcdef.css"><link rel="stylesheet" href="javascript:alert(1)"><link rel="stylesheet" href=""></head>"#;
        let page = scrape_page(html, &Url::parse("https://example/page").expect("url"));
        assert_eq!(page.links.len(), 1);
        assert_eq!(page.links[0].url.as_str(), "https://cdn.example/root/x/primer-0123456789abcdef.css");
    }

    #[test]
    fn garbage_html_does_not_panic() {
        for html in ["", "<link", "<<<>>>", "<link rel=stylesheet href=\"http://[::1\">", "\u{0}\u{1}"] {
            let p = scrape_page(html, &Url::parse("https://example/").expect("url"));
            let _ = classify_github(&p.links, true);
            let _ = classify_gitlab(&p.links);
        }
    }
}
