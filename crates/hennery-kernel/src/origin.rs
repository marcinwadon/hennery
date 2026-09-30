//! The browser rules (kernel spec §3.3): what every browser route checks
//! before its handler, cookie or not.
//!
//! - **State-changing methods** (every method but `GET` and `HEAD`): `Origin`
//!   must be `public_url`'s, and a missing `Origin` is refused. Before setup
//!   there is no `public_url`, so they are refused outright. The body must be
//!   JSON: a `Content-Type` other than `application/json` is refused, and so
//!   is a body without one. A body-less request (park, logout, revoke) needs
//!   no `Content-Type`.
//! - **`GET` and `HEAD`**: browsers send no `Origin` on a same-origin `GET`,
//!   so `Sec-Fetch-Site` is checked instead: `same-origin` or `none`, or
//!   absent (a client that is not a browser, or one too old to send it;
//!   the `SameSite=Strict` cookie still keeps cross-site requests out). A
//!   present `Origin` must match.
//!
//! No `GET` or `HEAD` route changes state; one that must is a `POST`.
//! Accepting a missing `Sec-Fetch-Site` relies on this.
//!
//! Host enrollment, the host WebSocket and setup are not browser routes in
//! this sense and sit outside this layer (setup has its own rule).

use crate::auth_api::error;
use crate::operator::Operator;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

pub async fn browser_rules(State(operator): State<Arc<Operator>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    // A header that is not visible ASCII matches nothing.
    let origin = headers.get(header::ORIGIN).map(|v| v.to_str().unwrap_or("\u{0}"));
    let public_url = operator.public_url();
    let expected = public_url.as_ref().map(|u| u.origin());
    if is_state_changing(req.method()) {
        if expected.is_none() {
            return error(StatusCode::FORBIDDEN, "setup_required", "hennery is not set up yet");
        }
        if origin != expected {
            tracing::debug!(?expected, received = ?origin, "request refused: origin_mismatch");
            return error(
                StatusCode::FORBIDDEN,
                "origin_mismatch",
                "state-changing requests must come from the public_url's origin",
            );
        }
        if !is_json_or_empty(headers) {
            return error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "the body must be application/json",
            );
        }
    } else {
        let site = headers.get("sec-fetch-site").map(|v| v.to_str().unwrap_or(""));
        if site.is_some_and(|s| s != "same-origin" && s != "none") {
            return error(StatusCode::FORBIDDEN, "cross_site", "cross-site requests are refused");
        }
        if origin.is_some() && origin != expected {
            tracing::debug!(?expected, received = ?origin, "request refused: origin_mismatch");
            return error(
                StatusCode::FORBIDDEN,
                "origin_mismatch",
                "a request's Origin must be the public_url's",
            );
        }
    }
    next.run(req).await
}

fn is_state_changing(method: &Method) -> bool {
    method != Method::GET && method != Method::HEAD
}

/// `application/json` (any parameters), or no body at all.
fn is_json_or_empty(headers: &HeaderMap) -> bool {
    match headers.get(header::CONTENT_TYPE) {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|v| v.split(';').next())
            .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/json")),
        None => {
            !headers.contains_key(header::TRANSFER_ENCODING)
                && headers
                    .get(header::CONTENT_LENGTH)
                    .is_none_or(|len| len.to_str().is_ok_and(|len| len.trim() == "0"))
        }
    }
}
