//! HTTP routes (DESIGN §6): documents, directories, raw files, embedded assets, bundle
//! files, `--css` files, the JSON resync endpoint, and the Host-header guard.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::events::{self, DocQuery};
use super::files;
use super::hub::DocStatus;
use super::paths::{self, RequestPath, ResolveError, Resolved};
use super::{App, RootTarget};
use crate::PREFIX;
use crate::markdown_files;
use crate::page::{self, AssetMode, Banner, BannerKind, LiveInfo, PageInput};
use crate::render::Rendered;

const TEXT_UTF8: &str = "text/plain; charset=utf-8";

/// Title prefix of a document page while the document has an error (must match live.js).
const ERROR_TITLE_PREFIX: &str = "(Error) ";

/// Build the router for `app`.
pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route(&format!("{PREFIX}/static/{{name}}"), get(static_asset))
        .route(&format!("{PREFIX}/bundle/{{*path}}"), get(bundle_file))
        .route(&format!("{PREFIX}/css/{{index}}/{{name}}"), get(extra_css))
        .route(&format!("{PREFIX}/events"), get(events::events))
        .route(&format!("{PREFIX}/render"), get(render_json))
        .route("/", get(document))
        .route("/{*path}", get(document))
        .layer(middleware::from_fn_with_state(app.clone(), host_guard))
        .layer(middleware::from_fn(access_log))
        .with_state(app)
}

/// Log target of the access log.
pub const ACCESS_LOG: &str = "md_render::access";

/// One line per request (GRIP C24): method, path and query, status and the time to the
/// response head (SSE streams: until the stream starts). Failed requests (4xx, 5xx: a
/// missing image, a broken link) at INFO, which is shown by default; the rest at DEBUG,
/// shown from `-v` on (CLI-7).
async fn access_log(req: Request, next: Next) -> Response {
    let started = std::time::Instant::now();
    let method = req.method().clone();
    let target = req
        .uri()
        .path_and_query()
        .map_or_else(|| req.uri().path().to_owned(), |p| p.as_str().to_owned());
    let resp = next.run(req).await;
    let status = resp.status();
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if status.is_client_error() || status.is_server_error() {
        tracing::info!(target: ACCESS_LOG, "{method} {target} -> {status} ({ms:.1} ms)");
    } else {
        tracing::debug!(target: ACCESS_LOG, "{method} {target} -> {status} ({ms:.1} ms)");
    }
    resp
}

/// Host-header allowlist against DNS rebinding when bound to loopback (D14, GRIP C39).
async fn host_guard(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    if let Some(allow) = &app.hosts {
        // Both the authority of an absolute-form request line and the Host header must name
        // this server: either alone could otherwise smuggle a foreign host past the check
        // (SEC-5). HTTP/1.0 clients may send neither; browsers (the rebinding vector) always
        // send Host.
        let authority = req.uri().authority().map(|a| a.as_str().to_owned());
        let host = req
            .headers()
            .get_all(header::HOST)
            .iter()
            .map(|h| String::from_utf8_lossy(h.as_bytes()).into_owned());
        if let Some(h) = authority.into_iter().chain(host).find(|h| !allow.allows(h)) {
            return (
                StatusCode::FORBIDDEN,
                [(header::CONTENT_TYPE, TEXT_UTF8)],
                format!("403 Forbidden: unexpected Host header {h:?} (md-render only answers to loopback names)\n"),
            )
                .into_response();
        }
    }
    next.run(req).await
}

/// `GET /__/md-render/static/{name}`: embedded assets.
async fn static_asset(UrlPath(name): UrlPath<String>, headers: HeaderMap) -> Response {
    let Some((bytes, mime)) = crate::assets::get(&name) else {
        return not_found_text();
    };
    // Content-based, so a rebuilt binary with changed client code is never served from a
    // stale browser cache (`no-cache` revalidates on every load).
    let etag = static_etag(bytes);
    if if_none_match(&headers, &etag) {
        return not_modified(&etag);
    }
    let mut resp = Response::new(Body::from(bytes));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    resp
}

/// `GET /__/md-render/bundle/{*path}`: a file of the current style bundle, read in place.
async fn bundle_file(
    State(app): State<Arc<App>>,
    UrlPath(path): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    let bundle = app.styles.bundle();
    let Some(bytes) = bundle.read(&path) else {
        return not_found_text();
    };
    let etag = format!("\"{}\"", bundle.etag().replace('"', ""));
    if if_none_match(&headers, &etag) {
        return not_modified(&etag);
    }
    let ctype = files::content_type(std::path::Path::new(&path), || None);
    let mut resp = Response::new(Body::from(bytes));
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&ctype) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    resp
}

/// `GET /__/md-render/css/{index}/{name}`: a local `--css` file.
async fn extra_css(
    State(app): State<Arc<App>>,
    UrlPath((index, _name)): UrlPath<(usize, String)>,
) -> Response {
    let Some(file) = app.styles.css_file(index) else {
        return not_found_text();
    };
    match tokio::fs::read(file).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            format!("cannot read {}: {e}\n", file.display()),
        )
            .into_response(),
    }
}

/// `GET /__/md-render/render?path=<doc>`: JSON resync (`{version, html, title, warnings}`).
async fn render_json(State(app): State<Arc<App>>, Query(q): Query<DocQuery>) -> Response {
    let key = match app.validate_doc_param(q.path.as_deref()).await {
        Ok(k) => k,
        Err((status, msg)) => return json_response(status, serde_json::json!({ "error": msg })),
    };
    let Some(snap) = app.hub.current(&key).await else {
        let body = serde_json::json!({ "error": "timed out waiting for the render", "server_id": app.server_id });
        return json_response(StatusCode::SERVICE_UNAVAILABLE, body);
    };
    let mut body = match &snap.content {
        Some(c) => events::content_json(snap.version, &c.rendered),
        None => serde_json::json!({ "version": snap.version }),
    };
    body["server_id"] = app.server_id.clone().into();
    body["version"] = snap.version.into();
    let status = match &snap.status {
        DocStatus::Ready => StatusCode::OK,
        DocStatus::Missing => {
            body["deleted"] = true.into();
            body["path"] = key.clone().into();
            StatusCode::NOT_FOUND
        }
        DocStatus::Failed { kind, message } => {
            body["error"] = serde_json::json!({ "kind": kind.as_str(), "message": message });
            StatusCode::INTERNAL_SERVER_ERROR
        }
        DocStatus::Pending => StatusCode::SERVICE_UNAVAILABLE,
    };
    json_response(status, body)
}

/// `GET /` and `GET /{*path}`: documents, directories and raw files.
async fn document(State(app): State<Arc<App>>, req: Request) -> Response {
    let uri = req.uri().clone();
    let raw = uri.path();
    if raw == PREFIX || raw.starts_with(&format!("{PREFIX}/")) {
        return not_found_text(); // unknown internal route; never shadowed by user files
    }
    let query = uri.query();
    let rp = match RequestPath::from_uri_path(raw) {
        Ok(p) => p,
        Err(e) => return app.error_page(status(e.status()), &e.to_string()).await,
    };
    if rp.is_root() {
        if rp.collapsed {
            return redirect(StatusCode::MOVED_PERMANENTLY, "/", query);
        }
        return app.root_page(req).await;
    }
    let rel = rp.rel();
    let raw_requested = wants_raw(query);
    match app.resolve(&rel).await {
        Err(ResolveError::Forbidden(m)) => app.error_page(StatusCode::FORBIDDEN, &m).await,
        Err(ResolveError::Io(e)) => {
            let msg = format!("cannot access /{rel}: {e}");
            app.error_page(StatusCode::INTERNAL_SERVER_ERROR, &msg)
                .await
        }
        Ok(Resolved::Dir(abs)) => {
            if !rp.trailing_slash || rp.collapsed {
                // `/dir` -> `/dir/` so relative links resolve like on the sites (GRIP C35).
                return redirect(
                    StatusCode::MOVED_PERMANENTLY,
                    &paths::url_for(&rel, true),
                    query,
                );
            }
            app.dir_page(&rel, &abs).await
        }
        Ok(Resolved::File(abs)) => {
            if rp.trailing_slash || rp.collapsed {
                return redirect(
                    StatusCode::MOVED_PERMANENTLY,
                    &paths::url_for(&rel, false),
                    query,
                );
            }
            if app.is_document(&rel) && !raw_requested {
                app.doc_page(&rel).await
            } else {
                let force = app.is_document(&rel).then_some(TEXT_UTF8);
                files::serve_raw(&abs, req, force).await
            }
        }
        Ok(Resolved::Missing) => {
            if app.is_document(&rel) && !raw_requested && !rp.trailing_slash {
                if rp.collapsed {
                    return redirect(
                        StatusCode::MOVED_PERMANENTLY,
                        &paths::url_for(&rel, false),
                        query,
                    );
                }
                // Rendered as a 404 page that includes the live client, so it turns into the
                // document as soon as the file is created (GRIP C58).
                app.doc_page(&rel).await
            } else {
                let msg = format!("/{rel} does not exist");
                app.error_page(StatusCode::NOT_FOUND, &msg).await
            }
        }
    }
}

impl App {
    /// `/`: the configured target.
    async fn root_page(&self, req: Request) -> Response {
        let query = req.uri().query().map(str::to_owned);
        match &self.target {
            RootTarget::Stdin { name, content } => {
                let title = self.title.clone().unwrap_or_else(|| name.clone());
                self.page(
                    StatusCode::OK,
                    content.clone(),
                    title,
                    Some(name.clone()),
                    None,
                    Vec::new(),
                )
                .await
            }
            RootTarget::File(rel) if !rel.contains('/') => self.checked_doc_page(rel).await,
            // A target below the root is shown at its own URL so relative links resolve.
            RootTarget::File(rel) => redirect(
                StatusCode::FOUND,
                &paths::url_for(rel, false),
                query.as_deref(),
            ),
            RootTarget::Dir(rel) if !rel.is_empty() => redirect(
                StatusCode::FOUND,
                &paths::url_for(rel, true),
                query.as_deref(),
            ),
            RootTarget::Dir(_) => match self.find_readme(None).await {
                Some(readme) => match readme.rsplit_once('/') {
                    Some((dir, _)) => redirect(
                        StatusCode::FOUND,
                        &paths::url_for(dir, true),
                        query.as_deref(),
                    ),
                    None => self.checked_doc_page(&readme).await,
                },
                None => self.listing_page("", &self.root.clone()).await,
            },
        }
    }

    /// A directory URL (with trailing slash): its README or a listing.
    async fn dir_page(&self, rel: &str, abs: &std::path::Path) -> Response {
        match self.find_readme(Some(abs)).await {
            Some(name) => self.checked_doc_page(&format!("{rel}/{name}")).await,
            None => self.listing_page(rel, abs).await,
        }
    }

    /// The README of directory `dir` ([`markdown_files::find_readme`]), or of the root with its
    /// fallbacks ([`markdown_files::find_root_readme`]) for None; off the reactor (Q-ASYNC-1).
    async fn find_readme(&self, dir: Option<&std::path::Path>) -> Option<String> {
        let (root, dir, flavor) = (self.root.clone(), dir.map(ToOwned::to_owned), self.flavor);
        super::blocking(move || match dir {
            Some(d) => markdown_files::find_readme(&d, flavor),
            None => markdown_files::find_root_readme(&root, flavor),
        })
        .await
        .ok()
        .flatten()
    }

    async fn listing_page(&self, rel: &str, abs: &std::path::Path) -> Response {
        let shown = if rel.is_empty() {
            "/".to_owned()
        } else {
            format!("/{rel}/")
        };
        let title = self
            .title
            .clone()
            .unwrap_or_else(|| format!("Index of {shown}"));
        if self.live {
            // A live listing (LIVE-9): the hub keeps it current and pushes changes.
            let key = super::hub::listing_key(rel);
            if let Some(snap) = self.hub.current(&key).await
                && snap.status == DocStatus::Ready
                && let Some(c) = &snap.content
            {
                let live = self.live_info(&key, snap.version);
                return self
                    .page(
                        StatusCode::OK,
                        c.rendered.clone(),
                        title,
                        Some(shown),
                        Some(live),
                        Vec::new(),
                    )
                    .await;
            }
        }
        let (root, dir, follow) = (self.root.clone(), abs.to_path_buf(), self.follow_symlinks);
        let rel_owned = rel.to_owned();
        let listed =
            tokio::task::spawn_blocking(move || paths::list_dir(&root, &dir, &rel_owned, follow))
                .await;
        let entries = match listed {
            Ok(Ok(e)) => e,
            Ok(Err(e)) => {
                let msg = format!("cannot list /{rel}: {e}");
                return self
                    .error_page(StatusCode::INTERNAL_SERVER_ERROR, &msg)
                    .await;
            }
            Err(e) => {
                let msg = format!("cannot list /{rel}: {e}");
                return self
                    .error_page(StatusCode::INTERNAL_SERVER_ERROR, &msg)
                    .await;
            }
        };
        let html = page::listing_html(self.flavor, &shown, &entries);
        let content = Arc::new(Rendered {
            html,
            ..Default::default()
        });
        self.page(
            StatusCode::OK,
            content,
            title,
            Some(shown),
            None,
            Vec::new(),
        )
        .await
    }

    /// A document found without resolving a request path (the target, a README): refused
    /// with 403 like a requested path when it escapes the root, not read and failed as a 500
    /// (CLI-18).
    async fn checked_doc_page(&self, rel: &str) -> Response {
        match self.resolve(rel).await {
            Err(ResolveError::Forbidden(m)) => {
                let msg = format!("cannot serve /{rel}: {m}");
                self.error_page(StatusCode::FORBIDDEN, &msg).await
            }
            _ => self.doc_page(rel).await,
        }
    }

    /// A Markdown document (existing or not).
    async fn doc_page(&self, rel: &str) -> Response {
        let Some(snap) = self.hub.current(rel).await else {
            let msg = format!("timed out rendering {rel}");
            return self.error_page(StatusCode::SERVICE_UNAVAILABLE, &msg).await;
        };
        let file_title = rel.rsplit('/').next().unwrap_or(rel).to_owned();
        let mut title = self.title.clone().unwrap_or(file_title);
        if matches!(snap.status, DocStatus::Failed { .. } | DocStatus::Missing) {
            // The `(Error) ` prefix live.js shows while an error is active (GRIP §11): a page
            // loaded in that state starts with it, and the client takes it as an active error
            // that the next good content clears (LIVE-7).
            title.insert_str(0, ERROR_TITLE_PREFIX);
        }
        let live = self.live.then(|| self.live_info(rel, snap.version));
        match (&snap.status, &snap.content) {
            (DocStatus::Ready, Some(c)) => {
                self.page(
                    StatusCode::OK,
                    c.rendered.clone(),
                    title,
                    Some(rel.to_owned()),
                    live,
                    Vec::new(),
                )
                .await
            }
            (DocStatus::Failed { kind, message }, Some(c)) => {
                // Keep showing the last good render under an error banner.
                let banner = Banner {
                    kind: BannerKind::Error,
                    message: format!("{} error: {message}", kind.as_str()),
                };
                let status = StatusCode::INTERNAL_SERVER_ERROR;
                self.page(
                    status,
                    c.rendered.clone(),
                    title,
                    Some(rel.to_owned()),
                    live,
                    vec![banner],
                )
                .await
            }
            (DocStatus::Failed { kind, message }, None) => {
                let msg = format!("{} error: {message}", kind.as_str());
                self.doc_error_page(StatusCode::INTERNAL_SERVER_ERROR, &msg, rel, title, live)
                    .await
            }
            (DocStatus::Missing, _) => {
                let msg = format!(
                    "/{rel} does not exist (yet); this page updates as soon as it is created"
                );
                self.doc_error_page(StatusCode::NOT_FOUND, &msg, rel, title, live)
                    .await
            }
            (DocStatus::Ready | DocStatus::Pending, _) => {
                let msg = format!("{rel} has not been rendered yet");
                self.doc_error_page(StatusCode::SERVICE_UNAVAILABLE, &msg, rel, title, live)
                    .await
            }
        }
    }

    /// The error page of a document: keeps the document's title and file-name header and
    /// includes the live client, so it turns into the document once the problem is fixed
    /// (GRIP C58).
    async fn doc_error_page(
        &self,
        status: StatusCode,
        message: &str,
        rel: &str,
        title: String,
        live: Option<LiveInfo>,
    ) -> Response {
        let html = page::error_html(self.flavor, status.as_u16(), message);
        let content = Arc::new(Rendered {
            html,
            ..Default::default()
        });
        self.page(
            status,
            content,
            title,
            Some(rel.to_owned()),
            live,
            Vec::new(),
        )
        .await
    }

    fn live_info(&self, rel: &str, version: u64) -> LiveInfo {
        let q = paths::encode_query_value(rel);
        LiveInfo {
            events_url: format!("{PREFIX}/events?path={q}"),
            render_url: format!("{PREFIX}/render?path={q}"),
            doc: rel.to_owned(),
            version,
            server_id: self.server_id.clone(),
        }
    }

    /// A styled error page for a request that is not a (live) document.
    pub(crate) async fn error_page(&self, status: StatusCode, message: &str) -> Response {
        let html = page::error_html(self.flavor, status.as_u16(), message);
        let content = Arc::new(Rendered {
            html,
            ..Default::default()
        });
        let reason = format!(
            "{} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("Error")
        );
        let title = self.title.clone().unwrap_or(reason);
        self.page(status, content, title, None, None, Vec::new())
            .await
    }

    /// Wrap content in the full page (crate::page) off the reactor.
    async fn page(
        &self,
        status: StatusCode,
        content: Arc<Rendered>,
        title: String,
        file_name: Option<String>,
        live: Option<LiveInfo>,
        banners: Vec<Banner>,
    ) -> Response {
        let bundle = self.styles.bundle();
        let (flavor, mode, theme, layout) =
            (self.flavor, self.mode, self.theme.clone(), self.layout);
        let extra_css = self.styles.extra_css_urls();
        let built = tokio::task::spawn_blocking(move || {
            let assets = AssetMode::Server {
                prefix: PREFIX.to_owned(),
            };
            let input = PageInput {
                flavor,
                mode,
                theme: &theme,
                layout,
                title: &title,
                file_name: file_name.as_deref(),
                content: &content,
                assets: &assets,
                live: live.as_ref(),
                banners: &banners,
                extra_css: &extra_css,
            };
            page::render_page(&input, &bundle)
        })
        .await;
        match built {
            Ok(Ok(html)) => (
                status,
                [
                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    (header::CACHE_CONTROL, "no-cache"),
                ],
                html,
            )
                .into_response(),
            Ok(Err(e)) => plain_error(&format!("building the page failed: {e:#}")),
            Err(e) => plain_error(&format!("building the page failed: {e}")),
        }
    }
}

/// `?raw`, `?raw=1`, `?raw=true`: serve a Markdown file's source instead of rendering it.
fn wants_raw(query: Option<&str>) -> bool {
    query.is_some_and(|q| {
        q.split('&').any(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            k == "raw" && !matches!(v, "0" | "false" | "no")
        })
    })
}

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST)
}

/// Redirect to `location` (an encoded path), preserving the query string.
fn redirect(status: StatusCode, location: &str, query: Option<&str>) -> Response {
    let target = match query {
        Some(q) if !q.is_empty() => format!("{location}?{q}"),
        _ => location.to_owned(),
    };
    match HeaderValue::from_str(&target) {
        Ok(v) => (status, [(header::LOCATION, v)]).into_response(),
        Err(_) => plain_error("invalid redirect target"),
    }
}

/// Strong ETag of an embedded asset: a truncated SHA-256 of its bytes.
fn static_etag(bytes: &[u8]) -> String {
    format!(
        "\"{}\"",
        crate::hash::hex(&crate::hash::sha256(bytes)[..12])
    )
}

fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().trim_start_matches("W/") == etag || t.trim() == "*")
        })
}

fn not_modified(etag: &str) -> Response {
    let mut resp = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(v) = HeaderValue::from_str(etag) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    resp
}

fn not_found_text() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, TEXT_UTF8)],
        "404 Not Found\n",
    )
        .into_response()
}

fn plain_error(msg: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, TEXT_UTF8)],
        format!("{msg}\n"),
    )
        .into_response()
}

fn json_response(status: StatusCode, body: serde_json::Value) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body.to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_query_detection() {
        assert!(wants_raw(Some("raw")));
        assert!(wants_raw(Some("a=1&raw=1")));
        assert!(!wants_raw(Some("raw=0")));
        assert!(!wants_raw(Some("rawr=1")));
        assert!(!wants_raw(None));
    }

    #[test]
    fn etag_matching() {
        let mut h = HeaderMap::new();
        assert!(!if_none_match(&h, "\"x\""));
        h.insert(header::IF_NONE_MATCH, "\"y\", W/\"x\"".parse().unwrap());
        assert!(if_none_match(&h, "\"x\""));
        h.insert(header::IF_NONE_MATCH, "*".parse().unwrap());
        assert!(if_none_match(&h, "\"z\""));
    }

    #[test]
    fn redirects_keep_query() {
        let r = redirect(StatusCode::MOVED_PERMANENTLY, "/docs/", Some("a=1"));
        assert_eq!(r.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(r.headers()[header::LOCATION], "/docs/?a=1");
        let r = redirect(StatusCode::FOUND, "/x.md", None);
        assert_eq!(r.headers()[header::LOCATION], "/x.md");
    }
}
