//! Which bundle stylesheets a page links, in which order, with which `media` (GH §4.4-4.5,
//! GL §11.3, GL §12).

use crate::flavor::{Theme, ThemeSel};
use crate::styles::{FlavorManifest, Stylesheet};

/// One bundle stylesheet to emit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sheet<'a> {
    /// Bundle path (`github/assets/light-….css`).
    pub path: &'a str,
    /// `media` attribute, if any.
    pub media: Option<String>,
}

/// The stylesheets of one page plus the theme ids actually in effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection<'a> {
    pub sheets: Vec<Sheet<'a>>,
    /// Light-slot theme id (GitHub `data-light-theme`).
    pub light: String,
    /// Dark-slot theme id (GitHub `data-dark-theme`).
    pub dark: String,
}

/// GitHub core roles after the theme files, in cascade order (GH §4.5).
const GITHUB_ORDER: [&str; 6] = ["primitives", "primer", "global", "github", "repository", "code"];

/// Media queries GitLab uses for "system" (auto) mode (GL §11.3).
const MEDIA_LIGHT: &str = "(prefers-color-scheme: light)";
const MEDIA_DARK: &str = "(prefers-color-scheme: dark)";

/// Pick the requested theme id if a file for it is vendored, else `default` if that one is,
/// else keep the requested id (nothing to link for it).
fn resolve_theme(available: &[&str], requested: &str, default: &str) -> String {
    if available.contains(&requested) {
        requested.to_owned()
    } else if available.contains(&default) {
        default.to_owned()
    } else {
        requested.to_owned()
    }
}

/// True for the embedded fallback (or any manifest that only has `fallback` sheets).
fn is_fallback(m: &FlavorManifest) -> bool {
    m.fallback || (!m.stylesheets.is_empty() && m.stylesheets.iter().all(|s| s.role == "fallback"))
}

fn sheet(s: &Stylesheet, media: Option<String>) -> Sheet<'_> {
    Sheet { path: &s.path, media }
}

/// Fallback bundles: every sheet in manifest order.
fn fallback_selection<'a>(m: &'a FlavorManifest, theme: &ThemeSel) -> Selection<'a> {
    Selection {
        sheets: m.stylesheets.iter().map(|s| sheet(s, s.media.clone())).collect(),
        light: theme.light.clone(),
        dark: theme.dark.clone(),
    }
}

/// GitHub: the theme files for the light/dark slots the color mode needs, then
/// primitives → primer → global → github → repository → code, then any unknown roles.
pub(crate) fn github<'a>(m: &'a FlavorManifest, theme: &ThemeSel) -> Selection<'a> {
    if is_fallback(m) {
        return fallback_selection(m, theme);
    }
    let theme_sheets: Vec<&Stylesheet> = m.stylesheets.iter().filter(|s| s.role == "theme").collect();
    let available: Vec<&str> = theme_sheets.iter().filter_map(|s| s.theme.as_deref()).collect();
    let light = resolve_theme(&available, &theme.light, "light");
    let dark = resolve_theme(&available, &theme.dark, "dark");
    let wanted: Vec<&str> = match theme.theme {
        Theme::Light => vec![light.as_str()],
        Theme::Dark => vec![dark.as_str()],
        Theme::Auto => vec![light.as_str(), dark.as_str()],
    };

    let mut sheets: Vec<Sheet<'a>> = theme_sheets
        .iter()
        .filter(|s| s.theme.as_deref().is_some_and(|id| wanted.contains(&id)))
        .map(|s| sheet(s, s.media.clone()))
        .collect();
    for role in GITHUB_ORDER {
        sheets.extend(m.stylesheets.iter().filter(|s| s.role == role).map(|s| sheet(s, s.media.clone())));
    }
    sheets.extend(
        m.stylesheets
            .iter()
            .filter(|s| s.role != "theme" && s.role != "fallback" && !GITHUB_ORDER.contains(&s.role.as_str()))
            .map(|s| sheet(s, s.media.clone())),
    );
    Selection { sheets, light, dark }
}

/// GitLab: `app` light/dark, `tailwind`, `fonts`, `highlight` light/dark, then unknown roles.
/// Theme-specific files get `media` queries in auto mode and are filtered in light/dark mode,
/// exactly as gitlab.com's `_head.html.haml` does (GL §11.3).
pub(crate) fn gitlab<'a>(m: &'a FlavorManifest, theme: &ThemeSel) -> Selection<'a> {
    if is_fallback(m) {
        return fallback_selection(m, theme);
    }
    let themed = |s: &&Stylesheet| s.role == "app" || s.role == "highlight";
    let available: Vec<&str> =
        m.stylesheets.iter().filter(themed).map(|s| s.theme.as_deref().unwrap_or("light")).collect();
    let light = resolve_theme(&available, &theme.light, "light");
    let dark = resolve_theme(&available, &theme.dark, "dark");

    // For a themed sheet: Some(media) when it is emitted, None when this theme mode skips it.
    let themed_media = |s: &Stylesheet| -> Option<Option<String>> {
        let id = s.theme.as_deref().unwrap_or("light");
        let is_light = id == light;
        let is_dark = id == dark;
        match theme.theme {
            Theme::Light => is_light.then_some(None),
            Theme::Dark => is_dark.then_some(None),
            Theme::Auto if is_light => Some(Some(MEDIA_LIGHT.to_owned())),
            Theme::Auto if is_dark => Some(Some(MEDIA_DARK.to_owned())),
            Theme::Auto => None,
        }
    };
    let slot_order = |s: &Stylesheet| -> u8 {
        if s.theme.as_deref().unwrap_or("light") == light { 0 } else { 1 }
    };

    let mut sheets = Vec::new();
    for role in ["app", "tailwind", "fonts", "highlight"] {
        let mut of_role: Vec<&Stylesheet> = m.stylesheets.iter().filter(|s| s.role == role).collect();
        if role == "app" || role == "highlight" {
            // Light before dark regardless of manifest order (GL §12); stable otherwise.
            of_role.sort_by_key(|s| slot_order(s));
            sheets.extend(of_role.into_iter().filter_map(|s| themed_media(s).map(|media| sheet(s, media))));
        } else {
            sheets.extend(of_role.into_iter().map(|s| sheet(s, s.media.clone())));
        }
    }
    sheets.extend(
        m.stylesheets
            .iter()
            .filter(|s| !["app", "tailwind", "fonts", "highlight", "fallback"].contains(&s.role.as_str()))
            .map(|s| sheet(s, s.media.clone())),
    );
    Selection { sheets, light, dark }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ss(path: &str, role: &str, theme: Option<&str>) -> Stylesheet {
        Stylesheet {
            path: path.into(),
            role: role.into(),
            theme: theme.map(Into::into),
            media: Some("all".into()),
            ..Default::default()
        }
    }

    fn sel(theme: Theme, light: &str, dark: &str) -> ThemeSel {
        ThemeSel { theme, light: light.into(), dark: dark.into() }
    }

    fn github_manifest() -> FlavorManifest {
        FlavorManifest {
            // deliberately not in cascade order
            stylesheets: vec![
                ss("g/code.css", "code", None),
                ss("g/light.css", "theme", Some("light")),
                ss("g/dark.css", "theme", Some("dark")),
                ss("g/dark_dimmed.css", "theme", Some("dark_dimmed")),
                ss("g/global.css", "global", None),
                ss("g/primer.css", "primer", None),
                ss("g/prim.css", "primitives", None),
                ss("g/github.css", "github", None),
                ss("g/extra.css", "something-new", None),
                ss("g/repo.css", "repository", None),
            ],
            ..Default::default()
        }
    }

    fn paths<'a>(s: &'a Selection<'a>) -> Vec<&'a str> {
        s.sheets.iter().map(|s| s.path).collect()
    }

    #[test]
    fn github_auto_order() {
        let m = github_manifest();
        let s = github(&m, &sel(Theme::Auto, "light", "dark"));
        assert_eq!(
            paths(&s),
            [
                "g/light.css",
                "g/dark.css",
                "g/prim.css",
                "g/primer.css",
                "g/global.css",
                "g/github.css",
                "g/repo.css",
                "g/code.css",
                "g/extra.css"
            ]
        );
        assert_eq!((s.light.as_str(), s.dark.as_str()), ("light", "dark"));
        assert_eq!(s.sheets[0].media.as_deref(), Some("all"));
    }

    #[test]
    fn github_single_theme_and_variants() {
        let m = github_manifest();
        let s = github(&m, &sel(Theme::Light, "light", "dark"));
        assert_eq!(paths(&s)[..2], ["g/light.css", "g/prim.css"]);
        let s = github(&m, &sel(Theme::Dark, "light", "dark_dimmed"));
        assert_eq!(paths(&s)[..2], ["g/dark_dimmed.css", "g/prim.css"]);
        assert_eq!(s.dark, "dark_dimmed");
    }

    #[test]
    fn github_missing_theme_falls_back() {
        let m = github_manifest();
        let s = github(&m, &sel(Theme::Auto, "light_tritanopia", "dark_high_contrast"));
        assert_eq!((s.light.as_str(), s.dark.as_str()), ("light", "dark"));
        assert_eq!(paths(&s)[..2], ["g/light.css", "g/dark.css"]);
    }

    #[test]
    fn github_without_any_theme_files_keeps_ids() {
        let m = FlavorManifest { stylesheets: vec![ss("g/primer.css", "primer", None)], ..Default::default() };
        let s = github(&m, &sel(Theme::Auto, "light_colorblind", "dark"));
        assert_eq!(s.light, "light_colorblind");
        assert_eq!(paths(&s), ["g/primer.css"]);
    }

    #[test]
    fn fallback_manifest_links_everything() {
        let m = FlavorManifest {
            fallback: true,
            stylesheets: vec![Stylesheet { path: "github/fallback.css".into(), role: "fallback".into(), ..Default::default() }],
            ..Default::default()
        };
        let s = github(&m, &ThemeSel::default());
        assert_eq!(paths(&s), ["github/fallback.css"]);
        let s = gitlab(&m, &ThemeSel::default());
        assert_eq!(paths(&s), ["github/fallback.css"]);
    }

    fn gitlab_manifest() -> FlavorManifest {
        FlavorManifest {
            stylesheets: vec![
                ss("l/hl-dark.css", "highlight", Some("dark")),
                ss("l/app-dark.css", "app", Some("dark")),
                ss("l/app.css", "app", Some("light")),
                ss("l/fonts.css", "fonts", None),
                ss("l/tw.css", "tailwind", None),
                ss("l/hl-white.css", "highlight", Some("light")),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn gitlab_auto_media_and_order() {
        let m = gitlab_manifest();
        let s = gitlab(&m, &ThemeSel::default());
        let got: Vec<(&str, Option<&str>)> = s.sheets.iter().map(|s| (s.path, s.media.as_deref())).collect();
        assert_eq!(
            got,
            [
                ("l/app.css", Some(MEDIA_LIGHT)),
                ("l/app-dark.css", Some(MEDIA_DARK)),
                ("l/tw.css", Some("all")),
                ("l/fonts.css", Some("all")),
                ("l/hl-white.css", Some(MEDIA_LIGHT)),
                ("l/hl-dark.css", Some(MEDIA_DARK)),
            ]
        );
    }

    #[test]
    fn gitlab_light_and_dark_filter() {
        let m = gitlab_manifest();
        let s = gitlab(&m, &sel(Theme::Light, "light", "dark"));
        assert_eq!(paths(&s), ["l/app.css", "l/tw.css", "l/fonts.css", "l/hl-white.css"]);
        assert!(s.sheets[0].media.is_none());
        let s = gitlab(&m, &sel(Theme::Dark, "light", "dark_dimmed"));
        assert_eq!(paths(&s), ["l/app-dark.css", "l/tw.css", "l/fonts.css", "l/hl-dark.css"]);
        assert_eq!(s.dark, "dark");
    }
}
