//! GitHub page chrome: `<html>` attributes (GH §3.1), README box (GH §3.2, §9.A), comment
//! wrapper (GH §3.3, §9.B), banners and listing / error fragments built from Primer classes that
//! the vendored CSS defines (CQ §3.7: `.Box`, `.Box-row`, `.flash`, `.flash-error`, …).

use super::octicons::{self, Icon};
use super::{Banner, BannerKind, Layout, ListingEntry, crumbs_html, human_size, reason};
use crate::flavor::{Mode, Theme};
use crate::html;

/// The `<html>` start tag. The theme attributes must be on the root element because several
/// rules use `:root[data-color-mode=…]` (GH §3.1); `light` / `dark` are the theme ids actually
/// linked (GH §4.4).
pub(crate) fn html_open(theme: Theme, light: &str, dark: &str) -> String {
    format!(
        "<html lang=\"en\" data-color-mode=\"{}\" data-light-theme=\"{}\" data-dark-theme=\"{}\" \
         data-a11y-animated-images=\"system\" data-a11y-link-underlines=\"true\">\n",
        theme.as_str(),
        html::attr_any_quote(light),
        html::attr_any_quote(dark)
    )
}

/// Page-level CSS emitted after the bundle stylesheets: github.com's inline tab-size rule
/// (GH §3.1), the `.color-fg-success` rule GH §5.3 asks for, the 2026 README box (GH §3.2,
/// validated against github.com) and md-render's own bits (banners, mermaid, listing).
pub(crate) const CHROME_CSS: &str = "\
:root{--tab-size-preference:4}pre,code{tab-size:var(--tab-size-preference)}
.color-fg-success{color:var(--fgColor-success)}
.mdr-box{border:var(--borderWidth-thin,1px) solid var(--borderColor-default,#d1d9e0);border-radius:var(--borderRadius-medium,6px);display:flex;flex-direction:column}
.mdr-box-header{display:flex;align-items:center;position:sticky;top:0;z-index:1;padding:0 var(--base-size-16,16px);min-height:48px;border-bottom:var(--borderWidth-thin,1px) solid var(--borderColor-default,#d1d9e0);background-color:var(--bgColor-default,#fff);border-top-left-radius:var(--borderRadius-medium,6px);border-top-right-radius:var(--borderRadius-medium,6px)}
.mdr-box-title{display:flex;align-items:center;gap:var(--base-size-8,8px);margin:0;min-width:0;font-size:var(--text-body-size-medium,14px);font-weight:var(--base-text-weight-semibold,600);line-height:1.5;overflow-wrap:anywhere}
.mdr-box-title .octicon{flex-shrink:0;color:var(--fgColor-muted,#59636e)}
.mdr-box-body{padding:var(--base-size-32,32px);overflow:auto}
@media (max-width:767.98px){.mdr-box-body{padding:var(--base-size-16,16px)}}
.markdown-body .highlight pre:has(+.zeroclipboard-container){min-height:52px}
.mdr-banners:empty{display:none}
.mdr-banners>.flash{display:flex;align-items:flex-start;margin-bottom:var(--base-size-16,16px);overflow-wrap:anywhere}
.mdr-banners>.flash>.octicon{flex-shrink:0;margin-top:2px}
.mdr-mermaid{display:flex;justify-content:center;overflow:auto;padding:var(--base-size-8,8px) 0}
.mdr-mermaid svg{max-width:100%;height:auto}
.mdr-listing{margin-bottom:var(--base-size-16,16px)}
.mdr-listing-title a{color:inherit}
.mdr-listing-row{display:flex;align-items:center;gap:var(--base-size-12,12px)}
.mdr-listing-row>.octicon{flex-shrink:0}
.mdr-listing .icon-directory{color:var(--treeViewItem-leadingVisual-iconColor-rest,var(--fgColor-muted,#59636e))}
.mdr-listing-name{flex:1 1 auto;min-width:0;overflow-wrap:anywhere}
.markdown-body .mdr-listing a.mdr-listing-name{text-decoration:none}
.markdown-body .mdr-listing a.mdr-listing-name:hover{text-decoration:underline}
.mdr-listing-size{font-size:var(--text-body-size-small,12px);white-space:nowrap}
.mdr-error>.octicon{margin-right:var(--base-size-8,8px)}
";

/// The `<body>` contents: wrapper markup per mode × layout around `content`.
pub(crate) fn body(
    mode: Mode,
    layout: Layout,
    file_name: Option<&str>,
    content: &str,
    banners: &str,
    root_attrs: &str,
) -> String {
    let banners = format!("<div id=\"mdr-banners\" class=\"mdr-banners\">{banners}</div>\n");
    let mut out = String::with_capacity(content.len() + 2048);
    match (mode, layout) {
        (Mode::Readme, Layout::Readme) => {
            out.push_str("<div class=\"container-xl px-3 px-md-4 px-lg-5 mt-4 mb-4 mdr-page\">\n");
            out.push_str(&banners);
            out.push_str("<div id=\"readme\" class=\"mdr-box md\">\n");
            if let Some(name) = file_name {
                out.push_str(&format!(
                    "<div class=\"mdr-box-header\"><h2 class=\"mdr-box-title\">{}<span>{}</span></h2></div>\n",
                    octicons::svg(Icon::Book, ""),
                    html::text(name)
                ));
            }
            out.push_str("<div class=\"js-snippet-clipboard-copy-unpositioned mdr-box-body\">\n");
            out.push_str(&format!(
                "<article class=\"markdown-body entry-content container-lg\" itemprop=\"text\" id=\"md-content\"{root_attrs}>{content}</article>\n"
            ));
            out.push_str("</div>\n</div>\n</div>\n");
        }
        (Mode::Readme, Layout::Bare) => {
            out.push_str("<div class=\"container-xl px-3 px-md-4 px-lg-5 mt-4 mb-4 mdr-page\">\n");
            out.push_str(&banners);
            out.push_str("<div class=\"js-snippet-clipboard-copy-unpositioned\">\n");
            out.push_str(&format!(
                "<article class=\"markdown-body entry-content container-lg\" itemprop=\"text\" id=\"md-content\"{root_attrs}>{content}</article>\n"
            ));
            out.push_str("</div>\n</div>\n");
        }
        (Mode::Comment, layout) => {
            out.push_str("<div class=\"container-lg px-3 mt-4 mb-4 mdr-page\">\n");
            out.push_str(&banners);
            let boxed = layout == Layout::Readme;
            if boxed {
                out.push_str("<div class=\"Box mdr-comment\">\n");
                if let Some(name) = file_name {
                    out.push_str(&format!(
                        "<div class=\"Box-header\"><strong>{}</strong></div>\n",
                        html::text(name)
                    ));
                }
            }
            out.push_str("<div class=\"js-snippet-clipboard-copy-unpositioned\">\n");
            out.push_str(&format!("<div class=\"comment-body markdown-body\" id=\"md-content\"{root_attrs}>{content}</div>\n"));
            out.push_str("</div>\n");
            if boxed {
                out.push_str("</div>\n");
            }
            out.push_str("</div>\n");
        }
    }
    out
}

/// A banner above the content (Primer `.flash`: blue info, `.flash-warn`, `.flash-error`).
pub(crate) fn banner(b: &Banner) -> String {
    let (class, icon, role) = match b.kind {
        BannerKind::Error => ("flash flash-error", Icon::Stop, "alert"),
        BannerKind::Warning => ("flash flash-warn", Icon::Alert, "alert"),
        BannerKind::Info => ("flash", Icon::Info, "status"),
    };
    format!(
        "<div class=\"{class} mdr-banner\" role=\"{role}\">{}<div>{}</div></div>",
        octicons::svg(icon, ""),
        html::text(&b.message)
    )
}

/// Directory listing fragment: a Primer `.Box` with one `.Box-row` per entry.
pub(crate) fn listing(dir: &str, entries: &[ListingEntry]) -> String {
    let mut out = String::from("<div class=\"Box mdr-listing\" data-mdr-listing=\"\">\n");
    out.push_str(&format!(
        "<div class=\"Box-header\"><div class=\"Box-title mdr-listing-title\" role=\"heading\" aria-level=\"2\">Index of {}</div></div>\n",
        crumbs_html(dir)
    ));
    if entries.is_empty() {
        out.push_str("<div class=\"Box-row color-fg-muted\">This directory is empty.</div>\n");
    }
    for e in entries {
        let icon = if e.is_dir {
            octicons::svg(Icon::FileDirectoryFill, "icon-directory")
        } else {
            octicons::svg(Icon::File, "color-fg-muted")
        };
        let label = if e.is_dir { "Directory" } else { "File" };
        let size = match (e.is_dir, e.size) {
            (false, Some(n)) => format!(
                "<span class=\"color-fg-muted mdr-listing-size\">{}</span>",
                human_size(n)
            ),
            _ => String::new(),
        };
        out.push_str(&format!(
            "<div class=\"Box-row Box-row--hover-gray py-2 mdr-listing-row\">{icon}<span class=\"sr-only\">{label}</span>\
             <a class=\"Link--primary mdr-listing-name\" href=\"{}\">{}</a>{size}</div>\n",
            html::attr_any_quote(&e.href),
            html::text(&e.name)
        ));
    }
    out.push_str("</div>\n");
    out
}

/// Error fragment: a Primer `.flash-error` box with status line and message.
pub(crate) fn error(status: u16, message: &str) -> String {
    format!(
        "<div class=\"flash flash-error mdr-error\" role=\"alert\">{}<strong>{status} {}</strong>\n<p>{}</p></div>\n",
        octicons::svg(Icon::Stop, ""),
        html::text(reason(status)),
        html::text(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_attributes() {
        let h = html_open(Theme::Auto, "light", "dark_dimmed");
        assert!(h.contains("data-color-mode=\"auto\""));
        assert!(h.contains("data-dark-theme=\"dark_dimmed\""));
        assert!(h.contains("data-a11y-link-underlines=\"true\""));
        assert!(html_open(Theme::Dark, "light", "x\"y").contains("data-dark-theme=\"x&quot;y\""));
    }

    #[test]
    fn banner_kinds() {
        let b = banner(&Banner {
            kind: BannerKind::Warning,
            message: "<b>".into(),
        });
        assert!(b.contains("flash flash-warn"));
        assert!(b.contains("&lt;b&gt;"));
        assert!(
            banner(&Banner {
                kind: BannerKind::Info,
                message: String::new()
            })
            .contains("role=\"status\"")
        );
    }
}
