//! Inline SVG icons used by the page chrome.
//!
//! Path data are Primer Octicons (MIT, © GitHub Inc., see THIRD_PARTY_NOTICES.md) as emitted
//! by github.com (GH §5, `.dev/research/github/octicons.html`).

/// `d` attribute values of the 16px octicons the chrome uses.
mod path {
    pub const BOOK: &str = "M0 1.75A.75.75 0 0 1 .75 1h4.253c1.227 0 2.317.59 3 1.501A3.743 3.743 0 0 1 11.006 1h4.245a.75.75 0 0 1 .75.75v10.5a.75.75 0 0 1-.75.75h-4.507a2.25 2.25 0 0 0-1.591.659l-.622.621a.75.75 0 0 1-1.06 0l-.622-.621A2.25 2.25 0 0 0 5.258 13H.75a.75.75 0 0 1-.75-.75Zm7.251 10.324.004-5.073-.002-2.253A2.25 2.25 0 0 0 5.003 2.5H1.5v9h3.757a3.75 3.75 0 0 1 1.994.574ZM8.755 4.75l-.004 7.322a3.752 3.752 0 0 1 1.992-.572H14.5v-9h-3.495a2.25 2.25 0 0 0-2.25 2.25Z";
    pub const INFO: &str = "M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.5 7.75A.75.75 0 0 1 7.25 7h1a.75.75 0 0 1 .75.75v2.75h.25a.75.75 0 0 1 0 1.5h-2a.75.75 0 0 1 0-1.5h.25v-2h-.25a.75.75 0 0 1-.75-.75ZM8 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z";
    pub const ALERT: &str = "M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575Zm1.763.707a.25.25 0 0 0-.44 0L1.698 13.132a.25.25 0 0 0 .22.368h12.164a.25.25 0 0 0 .22-.368Zm.53 3.996v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z";
    pub const STOP: &str = "M4.47.22A.749.749 0 0 1 5 0h6c.199 0 .389.079.53.22l4.25 4.25c.141.14.22.331.22.53v6a.749.749 0 0 1-.22.53l-4.25 4.25A.749.749 0 0 1 11 16H5a.749.749 0 0 1-.53-.22L.22 11.53A.749.749 0 0 1 0 11V5c0-.199.079-.389.22-.53Zm.84 1.28L1.5 5.31v5.38l3.81 3.81h5.38l3.81-3.81V5.31L10.69 1.5ZM8 4a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 8 4Zm0 8a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z";
    pub const FILE: &str = "M2 1.75C2 .784 2.784 0 3.75 0h6.586c.464 0 .909.184 1.237.513l2.914 2.914c.329.328.513.773.513 1.237v9.586A1.75 1.75 0 0 1 13.25 16h-9.5A1.75 1.75 0 0 1 2 14.25Zm1.75-.25a.25.25 0 0 0-.25.25v12.5c0 .138.112.25.25.25h9.5a.25.25 0 0 0 .25-.25V6h-2.75A1.75 1.75 0 0 1 9 4.25V1.5Zm6.75.062V4.25c0 .138.112.25.25.25h2.688l-.011-.013-2.914-2.914-.013-.011Z";
    pub const FILE_DIRECTORY_FILL: &str = "M1.75 1A1.75 1.75 0 0 0 0 2.75v10.5C0 14.216.784 15 1.75 15h12.5A1.75 1.75 0 0 0 16 13.25v-8.5A1.75 1.75 0 0 0 14.25 3H7.5a.25.25 0 0 1-.2-.1l-.9-1.2C6.07 1.26 5.55 1 5 1H1.75Z";
    pub const COPY: [&str; 2] = [
        "M0 6.75C0 5.784.784 5 1.75 5h1.5a.75.75 0 0 1 0 1.5h-1.5a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h7.5a.25.25 0 0 0 .25-.25v-1.5a.75.75 0 0 1 1.5 0v1.5A1.75 1.75 0 0 1 9.25 16h-7.5A1.75 1.75 0 0 1 0 14.25Z",
        "M5 1.75C5 .784 5.784 0 6.75 0h7.5C15.216 0 16 .784 16 1.75v7.5A1.75 1.75 0 0 1 14.25 11h-7.5A1.75 1.75 0 0 1 5 9.25Zm1.75-.25a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h7.5a.25.25 0 0 0 .25-.25v-7.5a.25.25 0 0 0-.25-.25Z",
    ];
    pub const CHECK: &str = "M13.78 4.22a.75.75 0 0 1 0 1.06l-7.25 7.25a.75.75 0 0 1-1.06 0L2.22 9.28a.751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018L6 10.94l6.72-6.72a.75.75 0 0 1 1.06 0Z";
}

/// The 16px icons the chrome can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Book,
    Info,
    Alert,
    Stop,
    File,
    FileDirectoryFill,
}

impl Icon {
    /// Octicon name (`octicon-<name>` class).
    pub fn name(self) -> &'static str {
        match self {
            Icon::Book => "book",
            Icon::Info => "info",
            Icon::Alert => "alert",
            Icon::Stop => "stop",
            Icon::File => "file",
            Icon::FileDirectoryFill => "file-directory-fill",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Icon::Book => path::BOOK,
            Icon::Info => path::INFO,
            Icon::Alert => path::ALERT,
            Icon::Stop => path::STOP,
            Icon::File => path::FILE,
            Icon::FileDirectoryFill => path::FILE_DIRECTORY_FILL,
        }
    }
}

/// An `<svg>` element for `icon` with GitHub's attribute set plus `extra_class`
/// (space-separated, may be empty).
pub fn svg(icon: Icon, extra_class: &str) -> String {
    let sep = if extra_class.is_empty() { "" } else { " " };
    format!(
        "<svg aria-hidden=\"true\" focusable=\"false\" class=\"octicon octicon-{}{sep}{extra_class}\" \
         viewBox=\"0 0 16 16\" version=\"1.1\" width=\"16\" height=\"16\" fill=\"currentColor\">\
         <path d=\"{}\"></path></svg>",
        icon.name(),
        icon.path()
    )
}

/// GitHub's `<template id="snippet-clipboard-copy-button-unpositioned">` (GH §5.3,
/// `.dev/research/github/clipboard_templates.html`). `github.js` clones it into every code block
/// that carries `data-snippet-clipboard-copy-content`.
pub fn clipboard_template() -> String {
    format!(
        "<template id=\"snippet-clipboard-copy-button-unpositioned\">\
         <div class=\"zeroclipboard-container\">\
         <clipboard-copy aria-label=\"Copy code to clipboard\" class=\"ClipboardButton btn btn-invisible js-clipboard-copy m-2 p-0 d-flex flex-justify-center flex-items-center\" data-copy-feedback=\"Copied!\" data-tooltip-direction=\"w\">\
         <svg aria-hidden=\"true\" data-component=\"Octicon\" height=\"16\" viewBox=\"0 0 16 16\" version=\"1.1\" width=\"16\" data-view-component=\"true\" class=\"octicon octicon-copy js-clipboard-copy-icon\"><path d=\"{}\"></path><path d=\"{}\"></path></svg>\
         <svg aria-hidden=\"true\" data-component=\"Octicon\" height=\"16\" viewBox=\"0 0 16 16\" version=\"1.1\" width=\"16\" data-view-component=\"true\" class=\"octicon octicon-check js-clipboard-check-icon color-fg-success d-none\"><path d=\"{}\"></path></svg>\
         </clipboard-copy></div></template>",
        path::COPY[0],
        path::COPY[1],
        path::CHECK
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_markup() {
        let s = svg(Icon::Alert, "mr-2");
        assert!(s.starts_with("<svg "));
        assert!(s.contains("class=\"octicon octicon-alert mr-2\""));
        assert!(s.ends_with("</path></svg>"));
        assert!(svg(Icon::Book, "").contains("class=\"octicon octicon-book\""));
    }

    #[test]
    fn template_has_both_icons() {
        let t = clipboard_template();
        assert!(t.contains("id=\"snippet-clipboard-copy-button-unpositioned\""));
        assert!(t.contains("js-clipboard-copy-icon"));
        assert!(t.contains("js-clipboard-check-icon color-fg-success d-none"));
    }
}
