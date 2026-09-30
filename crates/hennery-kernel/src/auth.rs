//! Request auth for operator routes (kernel spec §3.2, §3.3): the session
//! cookie, behind the browser rules. It replaces the walking skeleton's
//! development bearer token.

use crate::auth_api::{error, internal, secure_cookies, with_cookie};
use crate::operator::{Operator, session_cookie, session_token};
use crate::secret::unix_now;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use std::sync::Arc;

/// Put `router`'s routes behind the browser rules (outermost) and the
/// session cookie. Only routes added to `router` before this call are
/// covered: add every operator route first.
pub fn operator_only<S: Clone + Send + Sync + 'static>(router: Router<S>, operator: Arc<Operator>) -> Router<S> {
    router
        .layer(middleware::from_fn_with_state(operator.clone(), require_operator))
        .layer(middleware::from_fn_with_state(operator, crate::origin::browser_rules))
}

/// Axum middleware: the request must carry a live session cookie (401
/// `unauthenticated` otherwise). The session goes into the request's
/// extensions as `Authenticated`; when this request slid its expiry, the
/// cookie is sent again so the browser's copy lives as long.
pub async fn require_operator(State(operator): State<Arc<Operator>>, mut req: Request, next: Next) -> Response {
    let Some(token) = session_token(req.headers()).map(str::to_string) else {
        return unauthenticated();
    };
    let session = match operator.authenticate(&token, unix_now()) {
        Ok(Some(session)) => session,
        Ok(None) => return unauthenticated(),
        Err(err) => return internal(err),
    };
    let slid = session.slid;
    req.extensions_mut().insert(session);
    let response = next.run(req).await;
    if slid {
        with_cookie(response, &session_cookie(&token, secure_cookies(&operator)))
    } else {
        response
    }
}

fn unauthenticated() -> Response {
    error(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first")
}
