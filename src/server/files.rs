//! Raw (non-Markdown) file responses and Content-Type selection (DESIGN §6, GRIP C36/C37).

use std::io::Read as _;
use std::path::Path;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

/// How many leading bytes are inspected to classify a file without a known extension.
const SNIFF_LEN: usize = 8192;

const TEXT_UTF8: &str = "text/plain; charset=utf-8";
const OCTET: &str = "application/octet-stream";

/// `application/*` types that are really text and should display in the browser with an
/// explicit UTF-8 charset.
const TEXTUAL_APPLICATION: &[&str] = &[
    "javascript",
    "ecmascript",
    "json",
    "ld+json",
    "manifest+json",
    "xml",
    "xhtml+xml",
    "toml",
    "yaml",
    "x-yaml",
];

/// `application/*` script types browsers would download; shown as plain text instead.
const SOURCE_AS_PLAIN: &[&str] = &[
    "x-sh",
    "x-csh",
    "x-python",
    "x-perl",
    "x-ruby",
    "x-tcl",
    "x-httpd-php",
    "x-tex",
];

/// Content-Type for a file served raw.
///
/// Known extensions use `mime_guess`; textual types get `charset=utf-8` so `.txt`, `.rs` or
/// `.js` display in the browser. Unknown extensions are sniffed: a NUL byte or invalid UTF-8
/// in the first 8 KiB means `application/octet-stream`, otherwise `text/plain`.
pub fn content_type(path: &Path, sniff: impl FnOnce() -> Option<Vec<u8>>) -> String {
    match mime_guess::from_path(path).first() {
        Some(m) => {
            let (ty, sub) = (m.type_().as_str(), m.subtype().as_str());
            let suffix = m
                .suffix()
                .map(|s| format!("+{}", s.as_str()))
                .unwrap_or_default();
            let full_sub = format!("{sub}{suffix}");
            let textual = ty == "text"
                || (ty == "application" && TEXTUAL_APPLICATION.contains(&full_sub.as_str()))
                || (ty == "image" && full_sub == "svg+xml");
            if ty == "application" && SOURCE_AS_PLAIN.contains(&full_sub.as_str()) {
                TEXT_UTF8.to_owned()
            } else if textual && m.get_param(mime_guess::mime::CHARSET).is_none() {
                format!("{}; charset=utf-8", m.essence_str())
            } else {
                m.to_string()
            }
        }
        None => match sniff() {
            Some(head) if looks_like_text(&head, head.len() >= SNIFF_LEN) => TEXT_UTF8.to_owned(),
            Some(_) => OCTET.to_owned(),
            None => OCTET.to_owned(),
        },
    }
}

/// Text heuristic for extension-less files: no NUL byte and valid UTF-8. When `cut` is set
/// (the sniffing window ended before the file did), a multi-byte sequence cut off at the end
/// of the window is allowed.
pub fn looks_like_text(head: &[u8], cut: bool) -> bool {
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        // `error_len() == None` means "incomplete sequence at the end of the input".
        Err(e) => cut && e.error_len().is_none(),
    }
}

/// Read up to [`SNIFF_LEN`] bytes of a file (None on any I/O error).
pub fn sniff_file(path: &Path) -> Option<Vec<u8>> {
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(SNIFF_LEN);
    f.take(SNIFF_LEN as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Serve a validated, canonical file path raw: Range / If-Modified-Since / Last-Modified via
/// `tower-http`'s `ServeFile`, our own Content-Type, and `Cache-Control: no-cache` so edited
/// images are revalidated on reload.
pub async fn serve_raw(path: &Path, req: Request, force_type: Option<&str>) -> Response {
    let ctype = match force_type {
        Some(t) => t.to_owned(),
        None => {
            let p = path.to_path_buf();
            let guess_path = p.clone();
            tokio::task::spawn_blocking(move || content_type(&guess_path, || sniff_file(&p)))
                .await
                .unwrap_or_else(|_| OCTET.to_owned())
        }
    };
    let resp = match ServeFile::new(path).oneshot(req).await {
        Ok(r) => r,
        Err(never) => match never {},
    };
    let mut resp = resp.map(Body::new);
    let status = resp.status();
    if status.is_success() {
        if let Ok(v) = HeaderValue::from_str(&ctype) {
            resp.headers_mut().insert(header::CONTENT_TYPE, v);
        }
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        resp.headers_mut().insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    } else if status == StatusCode::NOT_FOUND {
        // The file vanished between resolution and open.
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ct(name: &str) -> String {
        content_type(Path::new(name), || None)
    }

    #[test]
    fn known_types() {
        assert_eq!(ct("a.txt"), "text/plain; charset=utf-8");
        assert_eq!(ct("a.css"), "text/css; charset=utf-8");
        assert_eq!(ct("a.png"), "image/png");
        assert_eq!(ct("a.svg"), "image/svg+xml; charset=utf-8");
        assert_eq!(ct("a.json"), "application/json; charset=utf-8");
        assert!(
            ct("a.js").ends_with("javascript; charset=utf-8"),
            "{}",
            ct("a.js")
        );
        assert_eq!(ct("a.sh"), "text/plain; charset=utf-8");
        assert_eq!(ct("a.woff2"), "font/woff2");
        assert_eq!(ct("a.pdf"), "application/pdf");
    }

    #[test]
    fn sniffing_unknown_extensions() {
        let text = content_type(Path::new("Makefile"), || {
            Some(b"all:\n\techo hi\n".to_vec())
        });
        assert_eq!(text, "text/plain; charset=utf-8");
        let bin = content_type(Path::new("blob"), || {
            Some(vec![0x7f, b'E', b'L', b'F', 0, 1])
        });
        assert_eq!(bin, "application/octet-stream");
        let latin1 = content_type(Path::new("blob"), || Some(b"caf\xe9 au lait".to_vec()));
        assert_eq!(latin1, "application/octet-stream");
        assert_eq!(
            content_type(Path::new("blob"), || None),
            "application/octet-stream"
        );
    }

    #[test]
    fn truncated_utf8_is_still_text() {
        let mut head = "ü".repeat(10).into_bytes();
        head.pop(); // cut the last two-byte sequence in half
        assert!(looks_like_text(&head, true));
        // A file that really ends mid-sequence is not UTF-8.
        assert!(!looks_like_text(&head, false));
        assert!(!looks_like_text(b"a\0b", true));
        assert!(!looks_like_text(b"\xff\xfe", true));
        let mut window = vec![b'a'; SNIFF_LEN - 1];
        window.push(0xc3);
        let ct = content_type(Path::new("noext"), || Some(window));
        assert_eq!(ct, "text/plain; charset=utf-8");
    }
}
