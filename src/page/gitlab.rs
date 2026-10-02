//! GitLab page chrome: `<html>` classes and colour-mode `<head>` bits (GL §11.3), the blob
//! viewer template (GL §12), the approximate note wrapper for `--mode comment` (DESIGN D16),
//! banners (`.gl-alert`, CQ §3.7) and listing / error fragments.

use std::sync::OnceLock;

use regex::Regex;

use super::octicons::{self, Icon};
use super::{Banner, BannerKind, Layout, ListingEntry, crumbs_html, html, human_size, reason};
use crate::flavor::{Mode, Theme};

/// The `<html>` start tag: `gl-system` / `gl-light` / `gl-dark` plus `ui-neutral` (the
/// neutral palette tokens are scoped to `:root.ui-neutral`, GL §11.3). `icon_sprite` is the
/// URL of GitLab's icon sprite for the copy button, `""` when its icon is in the page itself
/// ([`inline_copy_icon`]).
pub(crate) fn html_open(theme: Theme, icon_sprite: Option<&str>) -> String {
    let mode = match theme {
        Theme::Auto => "gl-system",
        Theme::Light => "gl-light",
        Theme::Dark => "gl-dark",
    };
    let sprite = icon_sprite
        .map(|s| format!(" data-icon-sprite=\"{}\"", html::attr(s)))
        .unwrap_or_default();
    format!("<html class=\"{mode} ui-neutral\" lang=\"en\"{sprite}>\n")
}

/// Id of the copy-button icon in GitLab's icon sprite (GL §10.4, `copy_code.js`).
pub(crate) const COPY_ICON_ID: &str = "copy-to-clipboard";

/// A self-contained page cannot reference the vendored sprite file, so the copy-button icon is
/// copied into the page: the `copy-to-clipboard` `<symbol>` of `sprite`, rebuilt from its
/// `viewBox` and the geometry of its `<path>`s only (the sprite is downloaded content), inside
/// a hidden `<svg>`. With `data-icon-sprite=""` on `<html>`, gitlab.js then draws the button
/// with `<use href="#copy-to-clipboard">` like a served page. `None` when the sprite has no
/// such symbol.
pub(crate) fn inline_copy_icon(sprite: &[u8]) -> Option<String> {
    static SYMBOL: OnceLock<Regex> = OnceLock::new();
    static PATH: OnceLock<Regex> = OnceLock::new();
    static ATTR: OnceLock<Regex> = OnceLock::new();
    let symbol = SYMBOL.get_or_init(|| {
        Regex::new(r#"(?s)<symbol\b([^>]*\bid="copy-to-clipboard"[^>]*)>(.*?)</symbol>"#)
            .expect("valid regex")
    });
    let path = PATH.get_or_init(|| Regex::new(r"<path\b([^>]*?)/?>").expect("valid regex"));
    let attr = ATTR.get_or_init(|| {
        Regex::new(r#"(?:^|\s)(viewBox|d|fill-rule|clip-rule)="([^"]*)""#).expect("valid regex")
    });
    let text = std::str::from_utf8(sprite).ok()?;
    let caps = symbol.captures(text)?;
    let geometry = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || " .,-+\t\n\r".contains(c))
    };
    let view_box = attr
        .captures_iter(&caps[1])
        .find(|a| &a[1] == "viewBox")
        .map(|a| a[2].to_owned())
        .filter(|v| geometry(v))
        .unwrap_or_else(|| "0 0 16 16".to_owned());
    let mut paths = String::new();
    for p in path.captures_iter(&caps[2]) {
        let mut out = String::from("<path");
        let mut has_d = false;
        for a in attr.captures_iter(&p[1]) {
            let ok = match &a[1] {
                "d" => geometry(&a[2]),
                "fill-rule" | "clip-rule" => matches!(&a[2], "nonzero" | "evenodd"),
                _ => false,
            };
            if ok {
                has_d |= &a[1] == "d";
                out.push_str(&format!(" {}=\"{}\"", &a[1], &a[2]));
            }
        }
        if has_d {
            out.push_str("/>");
            paths.push_str(&out);
        }
    }
    (!paths.is_empty()).then(|| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" aria-hidden=\"true\" width=\"0\" height=\"0\" \
             style=\"position:absolute;width:0;height:0;overflow:hidden\">\
             <symbol id=\"{COPY_ICON_ID}\" viewBox=\"{view_box}\">{paths}</symbol></svg>\n"
        )
    })
}

/// `<head>` elements that must precede the stylesheets: the `color-scheme` meta and, in
/// system mode, gitlab.com's `gl-dark` bootstrap (`_application_color_mode_js.html.haml`) so
/// the first paint already uses the right tokens. `gitlab.js` keeps the class in sync later.
pub(crate) fn head_pre(theme: Theme) -> String {
    match theme {
        Theme::Auto => "<meta name=\"color-scheme\" content=\"light dark\">\n\
             <script>(function(){try{if(window.matchMedia('(prefers-color-scheme: dark)').matches)\
             {document.documentElement.classList.add('gl-dark');}}catch(e){}})();</script>\n"
            .to_owned(),
        Theme::Dark => "<meta name=\"color-scheme\" content=\"dark light\">\n".to_owned(),
        Theme::Light => String::new(),
    }
}

/// Page-level CSS after the bundle stylesheets (md-render's own elements only; everything
/// else comes from GitLab's CSS).
pub(crate) const CHROME_CSS: &str = "\
.mdr-banners:empty{display:none}
.mdr-banners>.gl-alert{margin-bottom:16px}
.gl-alert .mdr-alert-icon{fill:currentColor}
.mdr-note{border:1px solid var(--gl-border-color-default,#dcdcde);border-radius:.25rem;background-color:var(--gl-background-color-default,#fff)}
.mdr-note-header{padding:8px 16px;border-bottom:1px solid var(--gl-border-color-default,#dcdcde);background-color:var(--gl-background-color-subtle,#fbfafd);border-top-left-radius:.25rem;border-top-right-radius:.25rem}
.mdr-note .note-body{padding:16px}
.md-render-mermaid{overflow:auto;text-align:center}
.md-render-mermaid svg{max-width:100%;height:auto}
.mdr-listing{border:1px solid var(--gl-border-color-default,#dcdcde);border-radius:.25rem;overflow:hidden;margin-bottom:16px}
.mdr-listing-header{padding:12px 16px;border-bottom:1px solid var(--gl-border-color-default,#dcdcde);background-color:var(--gl-background-color-subtle,#fbfafd);font-weight:600}
.mdr-listing-header a{color:inherit}
.mdr-listing-row{display:flex;align-items:center;gap:8px;padding:8px 16px;border-bottom:1px solid var(--gl-border-color-default,#dcdcde)}
.mdr-listing-row:last-child{border-bottom:0}
.mdr-listing-row:hover{background-color:var(--gl-background-color-subtle,#fbfafd)}
.mdr-listing-icon{flex-shrink:0;fill:currentColor;color:var(--gl-text-color-subtle,#626168)}
.mdr-listing-name{flex:1 1 auto;min-width:0;overflow-wrap:anywhere}
.mdr-listing-size{color:var(--gl-text-color-subtle,#626168);font-size:12px;white-space:nowrap}
.mdr-listing-empty{padding:8px 16px;color:var(--gl-text-color-subtle,#626168)}
";

/// `<body>` start tag (`tab-width-8` = gitlab.com's default `tab-size:8`, GL §11.4).
pub(crate) const BODY_OPEN: &str = "<body class=\"tab-width-8\">\n";

/// The `<body>` contents: wrapper markup per mode × layout around `content`.
pub(crate) fn body(
    mode: Mode,
    layout: Layout,
    file_name: Option<&str>,
    content: &str,
    banners: &str,
    root_attrs: &str,
) -> String {
    let mut out = String::with_capacity(content.len() + 2048);
    out.push_str("<div class=\"container-fluid container-limited gl-pt-5 mdr-page\">\n");
    out.push_str(&format!(
        "<div id=\"mdr-banners\" class=\"mdr-banners\">{banners}</div>\n"
    ));
    match (mode, layout) {
        (Mode::Readme, Layout::Readme) => {
            out.push_str("<div class=\"file-holder\" id=\"fileHolder\">\n");
            if let Some(name) = file_name {
                out.push_str(&format!(
                    "<div class=\"js-file-title file-title-flex-parent gl-px-4 gl-py-3\"><div class=\"file-header-content\"><strong>{}</strong></div></div>\n",
                    html::text(name)
                ));
            }
            out.push_str("<div class=\"blob-viewer\" data-type=\"rich\" data-loaded=\"true\">\n");
            out.push_str(&format!(
                "<div class=\"blob-viewer\" data-path=\"{}\" data-rich-type=\"markup\" data-type=\"rich\">\n",
                html::attr(file_name.unwrap_or(""))
            ));
            out.push_str(&format!("<div class=\"file-content js-markup-content md\" id=\"md-content\"{root_attrs}>{content}</div>\n"));
            out.push_str("</div>\n</div>\n</div>\n");
        }
        (Mode::Readme, Layout::Bare) => {
            out.push_str(&format!("<div class=\"file-content js-markup-content md\" id=\"md-content\"{root_attrs}>{content}</div>\n"));
        }
        (Mode::Comment, Layout::Readme) => {
            out.push_str("<div class=\"mdr-note\">\n");
            if let Some(name) = file_name {
                out.push_str(&format!(
                    "<div class=\"mdr-note-header\"><strong>{}</strong></div>\n",
                    html::text(name)
                ));
            }
            out.push_str(&format!(
                "<div class=\"note-body\"><div class=\"note-text md js-markup-content\" id=\"md-content\"{root_attrs}>{content}</div></div>\n"
            ));
            out.push_str("</div>\n");
        }
        (Mode::Comment, Layout::Bare) => {
            out.push_str(&format!("<div class=\"note-text md js-markup-content\" id=\"md-content\"{root_attrs}>{content}</div>\n"));
        }
    }
    out.push_str("</div>\n");
    out
}

fn alert_icon(icon: Icon) -> String {
    octicons::svg(
        icon,
        "gl-icon s16 gl-alert-icon gl-alert-icon-no-title mdr-alert-icon",
    )
}

/// A banner above the content (GitLab UI `GlAlert` markup).
pub(crate) fn banner(b: &Banner) -> String {
    let (variant, icon, role) = match b.kind {
        BannerKind::Error => ("danger", Icon::Stop, "alert"),
        BannerKind::Warning => ("warning", Icon::Alert, "alert"),
        BannerKind::Info => ("info", Icon::Info, "status"),
    };
    format!(
        "<div class=\"gl-alert gl-alert-{variant} gl-alert-not-dismissible mdr-banner\" role=\"{role}\">{}\
         <div class=\"gl-alert-content\"><div class=\"gl-alert-body\">{}</div></div></div>",
        alert_icon(icon),
        html::text(&b.message)
    )
}

/// Directory listing fragment (tree-like rows; GitLab has no standalone `.tree-table` rule,
/// CQ §3.7, so the box itself is md-render's own CSS on GitLab tokens).
pub(crate) fn listing(dir: &str, entries: &[ListingEntry]) -> String {
    let mut out = String::from("<div class=\"mdr-listing\" data-mdr-listing=\"\">\n");
    out.push_str(&format!(
        "<div class=\"mdr-listing-header\" role=\"heading\" aria-level=\"2\">Index of {}</div>\n",
        crumbs_html(dir)
    ));
    if entries.is_empty() {
        out.push_str("<div class=\"mdr-listing-empty\">This directory is empty.</div>\n");
    }
    for e in entries {
        let (icon, label) = if e.is_dir {
            (
                octicons::svg(Icon::FileDirectoryFill, "s16 mdr-listing-icon folder-icon"),
                "Directory",
            )
        } else {
            (
                octicons::svg(Icon::File, "s16 mdr-listing-icon file-icon"),
                "File",
            )
        };
        let size = match (e.is_dir, e.size) {
            (false, Some(n)) => {
                format!("<span class=\"mdr-listing-size\">{}</span>", human_size(n))
            }
            _ => String::new(),
        };
        out.push_str(&format!(
            "<div class=\"tree-item mdr-listing-row\">{icon}<span class=\"gl-sr-only\">{label}</span>\
             <a class=\"tree-item-link mdr-listing-name\" href=\"{}\">{}</a>{size}</div>\n",
            html::attr(&e.href),
            html::text(&e.name)
        ));
    }
    out.push_str("</div>\n");
    out
}

/// Error fragment: a danger `.gl-alert` with title and message.
pub(crate) fn error(status: u16, message: &str) -> String {
    format!(
        "<div class=\"gl-alert gl-alert-danger gl-alert-not-dismissible mdr-error\" role=\"alert\">{}\
         <div class=\"gl-alert-content\"><div class=\"gl-alert-title\"><strong>{status} {}</strong></div>\
         <div class=\"gl-alert-body\">{}</div></div></div>\n",
        alert_icon(Icon::Stop),
        html::text(reason(status)),
        html::text(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_classes() {
        assert!(html_open(Theme::Auto, None).starts_with("<html class=\"gl-system ui-neutral\""));
        assert!(html_open(Theme::Light, None).contains("gl-light ui-neutral"));
        assert!(html_open(Theme::Dark, Some("/s.svg")).contains("data-icon-sprite=\"/s.svg\""));
    }

    #[test]
    fn copy_icon_is_rebuilt_from_the_sprite() {
        let sprite = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"><symbol viewBox="0 0 16 16" id="abuse"><path d="M1 1"/></symbol><symbol viewBox="0 0 16 16" id="copy-to-clipboard" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" clip-rule="evenodd" d="M4 9.5H2.5v-7h7V4H11V2a1 1 0 0 0-1-1H2z"/></symbol><symbol id="copy-to-clipboard-2"><path d="M2 2"/></symbol></svg>"#;
        let svg = inline_copy_icon(sprite).unwrap();
        assert_eq!(
            svg,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" aria-hidden=\"true\" width=\"0\" height=\"0\" \
             style=\"position:absolute;width:0;height:0;overflow:hidden\"><symbol id=\"copy-to-clipboard\" \
             viewBox=\"0 0 16 16\"><path fill-rule=\"evenodd\" clip-rule=\"evenodd\" \
             d=\"M4 9.5H2.5v-7h7V4H11V2a1 1 0 0 0-1-1H2z\"/></symbol></svg>\n"
        );
        // Only geometry survives.
        let hostile = br#"<svg><symbol id="copy-to-clipboard" viewBox="0 0 16 16" onload="x()"><script>alert(1)</script><path onclick="x()" d="M0 0h1" style="fill:url(javascript:x)"/><path d="M0 0&quot;/><script>"/><foreignObject><path d="M1 1"/></foreignObject></symbol></svg>"#;
        let svg = inline_copy_icon(hostile).unwrap();
        assert!(!svg.contains("script") && !svg.contains("onload") && !svg.contains("onclick"));
        assert!(!svg.contains("style=\"fill") && !svg.contains("&quot;"));
        assert!(svg.contains("<path d=\"M0 0h1\"/>"), "{svg}");
        assert_eq!(
            inline_copy_icon(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            None
        );
        assert_eq!(
            inline_copy_icon(br#"<svg><symbol id="copy-to-clipboard"></symbol></svg>"#),
            None
        );
    }

    #[test]
    fn head_bits() {
        assert!(head_pre(Theme::Auto).contains("content=\"light dark\""));
        assert!(head_pre(Theme::Auto).contains("classList.add('gl-dark')"));
        assert!(head_pre(Theme::Dark).contains("content=\"dark light\""));
        assert!(head_pre(Theme::Light).is_empty());
    }

    #[test]
    fn banner_variants() {
        let b = banner(&Banner {
            kind: BannerKind::Error,
            message: "x & y".into(),
        });
        assert!(b.contains("gl-alert-danger"));
        assert!(b.contains("x &amp; y"));
    }
}
