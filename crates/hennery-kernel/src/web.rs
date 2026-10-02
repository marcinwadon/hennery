//! The web UI, embedded at build time (`build.rs`; kernel spec §7): the
//! router's fallback, so it answers only paths no route claims.
//!
//! - `/api` and `/api/…`, and `/mcp/…` (the gateway's proxy, umbrella §8):
//!   a JSON 404, whatever the method. Never the app.
//! - Any method but `GET` and `HEAD`: a JSON 404.
//! - A file of the build: the file. Hashed files (`/assets/…`) are cached
//!   for good; the rest revalidate with their ETag.
//! - A missing file under `/assets/`: a plain 404, so a stale chunk fails
//!   rather than running the page as a script.
//! - Anything else: `index.html`, whose router shows the route.
//!
//! The page holds no data, so the fallback sits outside the browser rules
//! and the cookie: a link opened from another site loads the app, and the
//! app's API calls are where those rules apply. Every answer is
//! `nosniff`; every file and page is `no-referrer`, `/index.html` asked for
//! by name included, and `/setup`'s `no-store` (kernel spec §3.1). The `Content-Security-Policy` comes from
//! `csp::on_html` over the whole router.

use crate::auth_api::error;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

/// One file of the build, as `build.rs` embeds it.
struct Asset {
    path: &'static str,
    body: &'static [u8],
    content_type: &'static str,
    etag: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

/// How long a hashed file may be cached: its name changes with its bytes.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The router's fallback.
pub async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path();
    if is_reserved(path) || (method != Method::GET && method != Method::HEAD) {
        let mut response = error(StatusCode::NOT_FOUND, "not_found", "no such route");
        response
            .headers_mut()
            .insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        return response;
    }
    if let Some(asset) = find(path) {
        let cache = if path.starts_with("/assets/") {
            IMMUTABLE
        } else {
            "no-cache"
        };
        return file(asset, cache, &headers);
    }
    if path.starts_with("/assets/") {
        return (
            StatusCode::NOT_FOUND,
            [(header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
            "not found",
        )
            .into_response();
    }
    let index = find("/index.html").expect("the build has an index.html");
    if is_setup(path) {
        file(index, "no-store", &HeaderMap::new())
    } else {
        file(index, "no-cache", &headers)
    }
}

/// Paths the app never answers: the API, and the gateway's proxy.
fn is_reserved(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path.starts_with("/mcp/")
}

/// The page a setup link opens: never cached (kernel spec §3.1).
fn is_setup(path: &str) -> bool {
    path == "/setup" || path.starts_with("/setup/")
}

fn find(path: &str) -> Option<&'static Asset> {
    FILES.iter().find(|a| a.path == path)
}

fn file(asset: &'static Asset, cache: &'static str, request: &HeaderMap) -> Response {
    let fresh = cache == "no-cache"
        && request
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|tags| tags.split(',').any(|t| t.trim() == asset.etag || t.trim() == "*"));
    let mut response = if fresh {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, asset.content_type)], asset.body).into_response()
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if cache != "no-store" {
        headers.insert(header::ETAG, HeaderValue::from_static(asset.etag));
    }
    if fresh {
        // A 304 names no type: the cached copy keeps its own.
        headers.remove(header::CONTENT_TYPE);
    }
    response
}

/// Whether the real UI is embedded, not the placeholder.
pub fn ui_embedded() -> bool {
    EMBEDDED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_and_the_gateways_proxy_are_reserved() {
        for path in ["/api", "/api/", "/api/nope", "/mcp/slug", "/mcp/"] {
            assert!(is_reserved(path), "{path}");
        }
        for path in ["/", "/apis", "/mcp", "/sessions/s-1", "/setup"] {
            assert!(!is_reserved(path), "{path}");
        }
    }

    #[test]
    fn the_build_has_an_index() {
        let index = find("/index.html").unwrap();
        assert_eq!(index.content_type, "text/html; charset=utf-8");
        assert!(index.etag.starts_with('"') && index.etag.len() == 18, "{}", index.etag);
    }
}
