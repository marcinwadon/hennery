//! Request auth for operator routes (kernel spec §3.2, §3.3): the session
//! cookie, behind the browser rules. It replaces the walking skeleton's
//! development bearer token.

use crate::auth_api::{error, internal, secure_cookies, with_cookie};
use crate::operator::{Authenticated, Operator, session_cookie};
use crate::secret::unix_now;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use std::sync::Arc;
use std::time::Duration;

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
/// cookie is sent again so the browser's copy lives as long. Not when the
/// handler set a cookie of its own, nor once the session has ended (a
/// request that revokes its own session): the browser keeps the last
/// `Set-Cookie`, and that must not be the dead token.
pub async fn require_operator(State(operator): State<Arc<Operator>>, mut req: Request, next: Next) -> Response {
    let (token, session) = match operator.authenticate_cookies(req.headers(), unix_now()) {
        Ok(Some(found)) => found,
        Ok(None) => return unauthenticated(),
        Err(err) => return internal(err),
    };
    let slid = session.slid;
    let session_id = session.session_id.clone();
    req.extensions_mut().insert(session);
    let response = next.run(req).await;
    let resend = slid
        && !response.headers().contains_key(header::SET_COOKIE)
        && matches!(operator.session_expires_at(&session_id, unix_now()), Ok(Some(_)));
    if resend {
        with_cookie(response, &session_cookie(&token, secure_cookies(&operator)))
    } else {
        response
    }
}

fn unauthenticated() -> Response {
    error(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first")
}

/// Axum middleware, inside `require_operator`: the session's last password
/// check must be within the last five minutes (kernel spec §3.4), or the
/// answer is 403 `step_up_required` and the client asks again.
pub async fn require_step_up(req: Request, next: Next) -> Response {
    let fresh = req
        .extensions()
        .get::<Authenticated>()
        .is_some_and(|session| session.stepped_up(unix_now()));
    if !fresh {
        return error(
            StatusCode::FORBIDDEN,
            "step_up_required",
            "confirm your password again (POST /api/auth/step-up/password)",
        );
    }
    next.run(req).await
}

/// Resolves once `session` has ended: revoked, signed out, or expired. A
/// stream a session holds open must not outlive it (3b decision 7), so
/// long-lived responses end with this. It re-checks the session whenever
/// the operator announces an ending, and at its expiry (which may have
/// slid since, through the session's other requests).
pub async fn session_ended(operator: Arc<Operator>, session: Authenticated) {
    let mut ends = operator.session_ends();
    // Whatever ended before this subscription is checked here.
    let Ok(Some(mut expires_at)) = operator.session_expires_at(&session.session_id, unix_now()) else {
        return;
    };
    loop {
        let left = Duration::from_secs(expires_at.saturating_sub(unix_now()).max(0) as u64);
        tokio::select! {
            changed = ends.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = tokio::time::sleep(left) => {}
        }
        match operator.session_expires_at(&session.session_id, unix_now()) {
            Ok(Some(later)) => expires_at = later,
            Ok(None) | Err(_) => return,
        }
    }
}
