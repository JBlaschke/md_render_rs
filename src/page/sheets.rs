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
    /// GitLab: the bundle has a dark application stylesheet (`app` role, dark slot). Without
    /// one the page must render as `gl-light`: GitLab's light CSS under `gl-dark` tokens mixes
    /// light Bootstrap colours with dark text tokens. Always true for GitHub and fallbacks.
    pub has_dark_app: bool,
}

/// GitHub core roles after the theme files, in cascade order (GH §4.5).
const GITHUB_ORDER: [&str; 6] = [
    "primitives",
    "primer",
    "global",
    "github",
    "repository",
    "code",
];

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
    Sheet {
        path: &s.path,
        media,
    }
}

/// Fallback bundles: every sheet in manifest order.
fn fallback_selection<'a>(m: &'a FlavorManifest, theme: &ThemeSel) -> Selection<'a> {
    Selection {
        sheets: m
            .stylesheets
            .iter()
            .map(|s| sheet(s, s.media.clone()))
            .collect(),
        light: theme.light.clone(),
        dark: theme.dark.clone(),
        has_dark_app: true,
    }
}

/// GitHub: the theme files for the light/dark slots the color mode needs, then
/// primitives → primer → global → github → repository → code, then any unknown roles.
pub(crate) fn github<'a>(m: &'a FlavorManifest, theme: &ThemeSel) -> Selection<'a> {
    if is_fallback(m) {
        return fallback_selection(m, theme);
    }
    let theme_sheets: Vec<&Stylesheet> =
        m.stylesheets.iter().filter(|s| s.role == "theme").collect();
    let available: Vec<&str> = theme_sheets
        .iter()
        .filter_map(|s| s.theme.as_deref())
        .collect();
    let light = resolve_theme(&available, &theme.light, "light");
    let dark = resolve_theme(&available, &theme.dark, "dark");
    let wanted: Vec<&str> = match theme.theme {
        Theme::Light => vec![light.as_str()],
        Theme::Dark => vec![dark.as_str()],
        Theme::Auto => vec![light.as_str(), dark.as_str()],
    };

    // Light slot first, then dark (github.com's order, GH §4.1), whatever the manifest order;
    // a theme used by both slots is linked once.
    let mut sheets: Vec<Sheet<'a>> = Vec::new();
    for (i, id) in wanted.iter().enumerate() {
        if wanted[..i].contains(id) {
            continue;
        }
        sheets.extend(
            theme_sheets
                .iter()
                .filter(|s| s.theme.as_deref() == Some(*id))
                .map(|s| sheet(s, s.media.clone())),
        );
    }
    for role in GITHUB_ORDER {
        sheets.extend(
            m.stylesheets
                .iter()
                .filter(|s| s.role == role)
                .map(|s| sheet(s, s.media.clone())),
        );
    }
    sheets.extend(
        m.stylesheets
            .iter()
            .filter(|s| {
                s.role != "theme"
                    && s.role != "fallback"
                    && !GITHUB_ORDER.contains(&s.role.as_str())
            })
            .map(|s| sheet(s, s.media.clone())),
    );
    Selection {
        sheets,
        light,
        dark,
        has_dark_app: true,
    }
}

/// GitLab: `app` light/dark, `tailwind`, `fonts`, `highlight` light/dark, then unknown roles.
/// Theme-specific files get `media` queries in auto mode and are filtered in light/dark mode,
/// exactly as gitlab.com's `_head.html.haml` does (GL §11.3). A themed role vendored for one
/// slot only (vendoring treats the dark sheets as optional, e.g. for a `--gitlab-source` that
/// links only `application.css`) is linked in every mode without `media` (the scraped
/// `(prefers-color-scheme: light)` would hide it on a dark OS), so the page never loses its
/// application or highlight CSS.
pub(crate) fn gitlab<'a>(m: &'a FlavorManifest, theme: &ThemeSel) -> Selection<'a> {
    if is_fallback(m) {
        return fallback_selection(m, theme);
    }
    let themed = |s: &&Stylesheet| s.role == "app" || s.role == "highlight";
    let slot_id = |s: &Stylesheet| s.theme.as_deref().unwrap_or("light").to_owned();
    let available: Vec<String> = m.stylesheets.iter().filter(themed).map(slot_id).collect();
    let available: Vec<&str> = available.iter().map(String::as_str).collect();
    let light = resolve_theme(&available, &theme.light, "light");
    let dark = resolve_theme(&available, &theme.dark, "dark");
    // (`--dark-theme light` puts the light file into the dark slot: no dark CSS either.)
    let has_dark_app = dark != "light"
        && m.stylesheets
            .iter()
            .any(|s| s.role == "app" && slot_id(s) == dark);

    let mut sheets = Vec::new();
    for role in ["app", "tailwind", "fonts", "highlight"] {
        let of_role: Vec<&'a Stylesheet> =
            m.stylesheets.iter().filter(|s| s.role == role).collect();
        if role != "app" && role != "highlight" {
            sheets.extend(of_role.into_iter().map(|s| sheet(s, s.media.clone())));
            continue;
        }
        let in_slot = |id: &str| -> Vec<&'a Stylesheet> {
            of_role
                .iter()
                .copied()
                .filter(|s| slot_id(s) == id)
                .collect()
        };
        let (light_sheets, dark_sheets) = (in_slot(&light), in_slot(&dark));
        // Light before dark regardless of manifest order (GL §12); stable otherwise.
        match (theme.theme, light_sheets.is_empty(), dark_sheets.is_empty()) {
            // Both slots use the same file (`--light-theme dark`): it applies in both modes.
            (Theme::Auto, false, false) if light == dark => {
                sheets.extend(light_sheets.iter().map(|s| sheet(s, None)));
            }
            (Theme::Auto, false, false) => {
                sheets.extend(
                    light_sheets
                        .iter()
                        .map(|s| sheet(s, Some(MEDIA_LIGHT.to_owned()))),
                );
                sheets.extend(
                    dark_sheets
                        .iter()
                        .map(|s| sheet(s, Some(MEDIA_DARK.to_owned()))),
                );
            }
            (Theme::Light, false, _) => sheets.extend(light_sheets.iter().map(|s| sheet(s, None))),
            (Theme::Dark, _, false) => sheets.extend(dark_sheets.iter().map(|s| sheet(s, None))),
            // Only one slot is vendored for this role: link it whatever the colour mode.
            (Theme::Auto | Theme::Light, true, false) => {
                sheets.extend(dark_sheets.iter().map(|s| sheet(s, None)));
            }
            (Theme::Auto | Theme::Dark, false, true) => {
                sheets.extend(light_sheets.iter().map(|s| sheet(s, None)));
            }
            (_, true, true) => {}
        }
    }
    sheets.extend(
        m.stylesheets
            .iter()
            .filter(|s| {
                !["app", "tailwind", "fonts", "highlight", "fallback"].contains(&s.role.as_str())
            })
            .map(|s| sheet(s, s.media.clone())),
    );
    Selection {
        sheets,
        light,
        dark,
        has_dark_app,
    }
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
        ThemeSel {
            theme,
            light: light.into(),
            dark: dark.into(),
        }
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
    fn github_theme_slots_order_and_dedup() {
        let mut m = github_manifest();
        m.stylesheets.swap(1, 2); // dark before light in the manifest
        let s = github(&m, &sel(Theme::Auto, "light", "dark"));
        assert_eq!(paths(&s)[..3], ["g/light.css", "g/dark.css", "g/prim.css"]);
        // The same theme in both slots is linked once.
        let s = github(&m, &sel(Theme::Auto, "dark", "dark"));
        assert_eq!(paths(&s)[..2], ["g/dark.css", "g/prim.css"]);
    }

    #[test]
    fn github_missing_theme_falls_back() {
        let m = github_manifest();
        let s = github(
            &m,
            &sel(Theme::Auto, "light_tritanopia", "dark_high_contrast"),
        );
        assert_eq!((s.light.as_str(), s.dark.as_str()), ("light", "dark"));
        assert_eq!(paths(&s)[..2], ["g/light.css", "g/dark.css"]);
    }

    #[test]
    fn github_without_any_theme_files_keeps_ids() {
        let m = FlavorManifest {
            stylesheets: vec![ss("g/primer.css", "primer", None)],
            ..Default::default()
        };
        let s = github(&m, &sel(Theme::Auto, "light_colorblind", "dark"));
        assert_eq!(s.light, "light_colorblind");
        assert_eq!(paths(&s), ["g/primer.css"]);
    }

    #[test]
    fn fallback_manifest_links_everything() {
        let m = FlavorManifest {
            fallback: true,
            stylesheets: vec![Stylesheet {
                path: "github/fallback.css".into(),
                role: "fallback".into(),
                ..Default::default()
            }],
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
        let got: Vec<(&str, Option<&str>)> = s
            .sheets
            .iter()
            .map(|s| (s.path, s.media.as_deref()))
            .collect();
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
        assert_eq!(
            paths(&s),
            ["l/app.css", "l/tw.css", "l/fonts.css", "l/hl-white.css"]
        );
        assert!(s.sheets[0].media.is_none());
        let s = gitlab(&m, &sel(Theme::Dark, "light", "dark_dimmed"));
        assert_eq!(
            paths(&s),
            ["l/app-dark.css", "l/tw.css", "l/fonts.css", "l/hl-dark.css"]
        );
        assert_eq!(s.dark, "dark");
        assert!(s.has_dark_app);
    }

    fn media<'a>(s: &'a Selection<'a>) -> Vec<(&'a str, Option<&'a str>)> {
        s.sheets
            .iter()
            .map(|s| (s.path, s.media.as_deref()))
            .collect()
    }

    /// A light sheet as vendoring records it from gitlab.com (with the light media query).
    fn scraped_light(path: &str, role: &str) -> Stylesheet {
        Stylesheet {
            media: Some(MEDIA_LIGHT.into()),
            ..ss(path, role, Some("light"))
        }
    }

    /// PAGE-3: a bundle without dark application/highlight CSS (both optional when vendoring)
    /// still links the light sheets on a dark OS (no colour-scheme query) and in dark mode.
    #[test]
    fn gitlab_light_only_bundle_links_the_light_sheets_in_every_mode() {
        let m = FlavorManifest {
            stylesheets: vec![
                scraped_light("l/app.css", "app"),
                ss("l/tw.css", "tailwind", None),
                ss("l/fonts.css", "fonts", None),
                scraped_light("l/hl-white.css", "highlight"),
            ],
            ..Default::default()
        };
        for mode in [Theme::Auto, Theme::Dark, Theme::Light] {
            let s = gitlab(&m, &sel(mode, "light", "dark"));
            assert_eq!(
                media(&s),
                [
                    ("l/app.css", None),
                    ("l/tw.css", Some("all")),
                    ("l/fonts.css", Some("all")),
                    ("l/hl-white.css", None),
                ],
                "{mode:?}"
            );
            assert!(!s.has_dark_app, "{mode:?}");
        }
    }

    #[test]
    fn gitlab_highlight_without_dark_theme_is_linked_unconditionally() {
        let m = FlavorManifest {
            stylesheets: vec![
                ss("l/app-dark.css", "app", Some("dark")),
                ss("l/app.css", "app", Some("light")),
                scraped_light("l/hl-white.css", "highlight"),
            ],
            ..Default::default()
        };
        let s = gitlab(&m, &ThemeSel::default());
        assert_eq!(
            media(&s),
            [
                ("l/app.css", Some(MEDIA_LIGHT)),
                ("l/app-dark.css", Some(MEDIA_DARK)),
                ("l/hl-white.css", None),
            ]
        );
        assert!(s.has_dark_app);
        let s = gitlab(&m, &sel(Theme::Dark, "light", "dark"));
        assert_eq!(
            media(&s),
            [("l/app-dark.css", None), ("l/hl-white.css", None)]
        );
    }

    #[test]
    fn gitlab_dark_only_role_is_used_in_light_mode() {
        let m = FlavorManifest {
            stylesheets: vec![
                ss("l/app.css", "app", Some("light")),
                ss("l/hl-dark.css", "highlight", Some("dark")),
            ],
            ..Default::default()
        };
        let s = gitlab(&m, &sel(Theme::Light, "light", "dark"));
        assert_eq!(paths(&s), ["l/app.css", "l/hl-dark.css"]);
        assert!(!s.has_dark_app);
    }

    #[test]
    fn gitlab_same_theme_in_both_slots_applies_in_both_modes() {
        let m = gitlab_manifest();
        let s = gitlab(&m, &sel(Theme::Auto, "dark", "dark"));
        assert_eq!(
            media(&s),
            [
                ("l/app-dark.css", None),
                ("l/tw.css", Some("all")),
                ("l/fonts.css", Some("all")),
                ("l/hl-dark.css", None),
            ]
        );
        assert!(s.has_dark_app);
        let s = gitlab(&m, &sel(Theme::Auto, "light", "light"));
        assert_eq!(
            paths(&s),
            ["l/app.css", "l/tw.css", "l/fonts.css", "l/hl-white.css"]
        );
        assert!(
            !s.has_dark_app,
            "light CSS in the dark slot is not dark CSS"
        );
    }
}
