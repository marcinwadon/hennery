//! Kernel spec §7.2: every HTML response carries the
//! `Content-Security-Policy`. One layer over the whole router sets it, so a
//! page cannot be added without it.

use axum::http::{HeaderValue, header};
use axum::response::Response;

/// Kernel spec §7.2's policy. No page has an inline script, so it names no
/// hash: the web UI's theme bootstrap is a file of its own (`/theme.js`).
/// Everything else comes from this origin: scripts, styles, fonts, API
/// calls and form targets (plan 4b).
pub const POLICY: &str = "script-src 'self'; default-src 'self'; connect-src 'self'; img-src 'self' data: blob:; \
     object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// Set `POLICY` on a `text/html` response. Use as
/// `axum::middleware::map_response(csp::on_html)`, outermost.
pub async fn on_html(mut response: Response) -> Response {
    let is_html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|t| t.to_str().ok())
        .is_some_and(|t| t.trim_start().to_ascii_lowercase().starts_with("text/html"));
    if is_html {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(POLICY));
    }
    response
}
