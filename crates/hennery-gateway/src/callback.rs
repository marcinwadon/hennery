//! The OAuth callback (gateway spec §4.3; api-8e-8f B5, R1, R3, R6, R7):
//! `GET /api/mcp/oauth/callback`, the vendor's redirect target, and
//! `GET /api/mcp/oauth/callback.js`, its page's one script.
//!
//! - **Outside the operator's session and the browser rules.** The session
//!   cookie is `SameSite=Strict`, so the browser does not send it on the
//!   vendor's cross-site redirect, and `Sec-Fetch-Site: cross-site` would be
//!   refused by the browser rules. `state` and the flow's own cookie
//!   authenticate it instead. It is the one `GET` that changes state (kernel
//!   spec §3.3's write-back).
//! - **Its own headers:** `Cache-Control: no-store`, `Referrer-Policy:
//!   no-referrer`, `X-Content-Type-Options: nosniff`, and the kernel's
//!   `Content-Security-Policy` (`csp::on_html`), layered here: the kernel's
//!   layer covers only the router it is put on, not one merged beside it.
//! - **The order of checks** (api-8e-8f B5): `state` (before any storage);
//!   the flow cookie, constant-time, the flow consumed either way; the
//!   operator's session; `iss` (RFC 9207, mix-up); the vendor's `error`;
//!   the exchange; under the refresh lock, the connection unchanged since the
//!   flow started; then the grant is stored. Every outcome from the cookie
//!   on clears it. Every failure from the flow on is kept in the
//!   connection's `oauth_error`; the first two name no connection, so they
//!   write nothing.
//! - **The page** is static HTML, no inline script, no external resource,
//!   no label, URL, account, client id, token or code: `<body
//!   data-outcome data-result data-connection>`, a heading and the message
//!   as escaped text. No redirect, no return URL: it navigates nowhere.

use crate::api::{CALLBACK_PATH, GatewayState};
use crate::flows::{MAX_FLOW_COOKIES, Taken, cookie_name};
use crate::model::vendor_text;
use crate::notify;
use crate::oauth::{self, Grant, TokenError};
use crate::store::{GrantStored, GrantToStore};
use axum::Router;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use hennery_kernel::secret::unix_now;
use std::collections::HashMap;

/// The script's path: `script-src 'self'` allows it.
pub const SCRIPT_PATH: &str = "/api/mcp/oauth/callback.js";

/// The query parameters the callback reads; each at most once (lane L16).
const PARAMETERS: &[&str] = &["code", "state", "error", "error_description", "iss"];

/// The most bytes of one parameter read.
const MAX_PARAMETER: usize = 4096;

/// The callback's routes, merged beside the operator's, never under them.
pub fn router(state: GatewayState) -> Router {
    Router::new()
        .route(CALLBACK_PATH, get(callback))
        .route(SCRIPT_PATH, get(script))
        .layer(middleware::map_response(private_headers))
        .layer(middleware::map_response(hennery_kernel::csp::on_html))
        .with_state(state)
}

async fn private_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

/// The page's script: static, data-free, served like the kernel's
/// `/setup.js`. It strips `code` and `state` from the URL and the history,
/// tells the opener's window over `BroadcastChannel` (the popup has no
/// opener), and closes the window on success after a second.
pub const SCRIPT: &str = r#"// hennery: the OAuth callback page's script (plan 8f). Static; it holds no data.
history.replaceState(null, "", "/api/mcp/oauth/callback");
const page = document.body;
const outcome = page.dataset.outcome;
try {
  const channel = new BroadcastChannel("hennery-mcp-oauth");
  channel.postMessage({
    type: "done",
    connection_id: page.dataset.connection,
    outcome,
    result: page.dataset.result,
  });
  channel.close();
} catch (e) {
  // No BroadcastChannel: the opener's window refetches on focus.
}
const close = document.getElementById("close");
if (outcome === "connected") {
  setTimeout(() => window.close(), 1000);
} else if (close) {
  close.hidden = false;
  close.addEventListener("click", () => window.close());
}
"#;

async fn script() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], SCRIPT).into_response()
}

/// Escaped for HTML text and attribute values.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// One answer of the callback.
struct Outcome {
    status: StatusCode,
    /// `connected`, or the failure's code.
    result: &'static str,
    connection_id: Option<String>,
    message: String,
}

impl Outcome {
    fn failed(status: StatusCode, result: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            result,
            connection_id: None,
            message: message.into(),
        }
    }

    fn of(mut self, connection_id: &str) -> Self {
        self.connection_id = Some(connection_id.to_string());
        self
    }
}

/// The page for `outcome`, clearing the flow's cookie if `clear` names it.
fn page(outcome: &Outcome, clear: Option<(&str, bool)>) -> Response {
    let connected = outcome.result == "connected";
    let heading = if connected { "Connected" } else { "Not connected" };
    let body = format!(
        "<!doctype html>\n<html lang=\"en\">\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>hennery</title>\n<body data-outcome=\"{}\" data-result=\"{}\" data-connection=\"{}\">\n<h1>{heading}</h1>\n<p id=\"message\">{}</p>\n<p><button id=\"close\" type=\"button\" hidden>Close</button></p>\n<script src=\"{SCRIPT_PATH}\"></script>\n</body>\n</html>\n",
        if connected { "connected" } else { "failed" },
        escape(outcome.result),
        escape(outcome.connection_id.as_deref().unwrap_or("")),
        escape(&outcome.message),
    );
    let mut response = (
        outcome.status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response();
    if let Some((name, secure)) = clear {
        let secure = if secure { "; Secure" } else { "" };
        let cookie = format!("{name}=; HttpOnly; SameSite=Lax; Path={CALLBACK_PATH}; Max-Age=0{secure}");
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

/// The query's parameters the callback reads, or `None` if one is there
/// twice, in another case too, too long, or holds a control character:
/// whichever of two a vendor or another parser would read, the callback
/// reads neither (lane L16).
fn parameters(query: Option<&str>) -> Option<HashMap<&'static str, String>> {
    let mut out: HashMap<&'static str, String> = HashMap::new();
    for (name, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        let Some(known) = PARAMETERS.iter().copied().find(|p| *p == name) else {
            // `STATE` or `Code` beside the real one: a reader matching
            // names whatever their case would take it.
            if PARAMETERS.iter().any(|p| p.eq_ignore_ascii_case(&name)) {
                return None;
            }
            continue;
        };
        if value.len() > MAX_PARAMETER || value.chars().any(char::is_control) {
            return None;
        }
        if out.insert(known, value.into_owned()).is_some() {
            return None;
        }
    }
    Some(out)
}

/// The values of every cookie named `name`, at most `MAX_FLOW_COOKIES`: a
/// sibling subdomain may set one of the same name first (as
/// `operator::session_tokens`).
fn cookies<'a>(headers: &'a HeaderMap, name: &str) -> Vec<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then_some(value)
        })
        .take(MAX_FLOW_COOKIES)
        .collect()
}

/// `GET /api/mcp/oauth/callback`.
async fn callback(State(state): State<GatewayState>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    let runtime = &state.runtime;
    let now = unix_now();
    let unknown = || {
        page(
            &Outcome::failed(
                StatusCode::BAD_REQUEST,
                "flow_unknown",
                "This sign-in is unknown or has expired. Start it again from hennery.",
            ),
            None,
        )
    };
    // 1. `state`, before anything is stored or read from storage.
    let Some(params) = parameters(query.as_deref()) else {
        return unknown();
    };
    let Some(flow_state) = params.get("state") else {
        return unknown();
    };
    // 2. The flow's cookie: the flow is consumed either way.
    let name = cookie_name(flow_state);
    let clear = Some((name.as_str(), state.secure_cookies()));
    let snapshot = match runtime.flows.take(flow_state, &cookies(&headers, &name), now) {
        Taken::Unknown => return unknown(),
        Taken::Mismatch(_) => {
            tracing::warn!("gateway: an OAuth callback without its flow's cookie refused");
            return page(
                &Outcome::failed(
                    StatusCode::BAD_REQUEST,
                    "flow_mismatch",
                    "This sign-in was not started in this browser. Start it again from hennery.",
                ),
                clear,
            );
        }
        Taken::Taken(snapshot) => snapshot,
    };
    // 3. From here on, only the snapshot (G-8).
    let id = snapshot.connection_id.clone();
    let outcome = finish(&state, &snapshot, &params, now).await.of(&id);
    let code = outcome.result;
    if code == "connected" {
        tracing::info!(connection_id = %id, "gateway: a Connect completed");
    } else {
        tracing::info!(connection_id = %id, code, "gateway: a Connect failed");
        if let Err(err) = runtime.store.set_oauth_error(&id, Some((code, &outcome.message)), now) {
            tracing::error!(connection_id = %id, error = %err, "gateway: a Connect's failure not recorded");
        }
    }
    page(&outcome, clear)
}

/// Steps 4 to 9 (api-8e-8f B5).
async fn finish(
    state: &GatewayState,
    snapshot: &crate::flows::Snapshot,
    params: &HashMap<&'static str, String>,
    now: i64,
) -> Outcome {
    let runtime = &state.runtime;
    // 4. The operator's session that started it is still live.
    match state.operator.session_expires_at(&snapshot.auth_session, now) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Outcome::failed(
                StatusCode::BAD_REQUEST,
                "session_ended",
                "The hennery session that started this sign-in has ended. Sign in and start again.",
            );
        }
        Err(err) => {
            tracing::error!(error = %err, "gateway: a session not read");
            return Outcome::failed(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Something went wrong in hennery.",
            );
        }
    }
    // 5. `iss` (RFC 9207): mix-up protection, since any MCP URL may be
    // hostile (the review's R6).
    let issuer_ok = match params.get("iss") {
        Some(iss) => *iss == snapshot.issuer,
        None => !snapshot.iss_parameter,
    };
    if !issuer_ok {
        return Outcome::failed(
            StatusCode::BAD_REQUEST,
            "issuer_mismatch",
            "The answer did not come from the authorization server this sign-in started at.",
        );
    }
    // 6. The vendor's own refusal, reached only after state, cookie and
    // session passed, so an outsider cannot put text on this page.
    if let Some(said) = params.get("error") {
        let text = match params.get("error_description") {
            Some(description) => format!("{said}: {description}"),
            None => said.clone(),
        };
        let mut message = vendor_text(&text);
        if said == "invalid_target" && snapshot.resource_param {
            // RFC 8707: the server does not take `resource`; the next
            // Connect omits it (plan 8f decision 11).
            runtime.flows.refuse_resource(&snapshot.connection_id, &snapshot.url);
            message.push_str(" Connect again: hennery will not send the resource parameter this time.");
        }
        return Outcome::failed(StatusCode::BAD_REQUEST, "consent_denied", message);
    }
    let Some(code) = params.get("code") else {
        return Outcome::failed(
            StatusCode::BAD_REQUEST,
            "consent_denied",
            "The authorization server answered with no code.",
        );
    };
    // 7. The exchange: the verifier and `resource` as recorded; the token
    // endpoint's body is never logged or echoed.
    let client = runtime.client(snapshot.internal_network);
    let grant = Grant::Code {
        code,
        redirect_uri: &snapshot.redirect_uri,
        verifier: &snapshot.verifier,
    };
    let resource = snapshot.resource_param.then_some(snapshot.resource.as_str());
    let (issued, resource_param_accepted) = match oauth::token(&client, &snapshot.client, &grant, resource).await {
        Err(TokenError::InvalidTarget) if resource.is_some() => {
            (oauth::token(&client, &snapshot.client, &grant, None).await, false)
        }
        other => (other, snapshot.resource_param),
    };
    let issued = match issued {
        Ok(issued) => issued,
        Err(TokenError::Unavailable) => {
            return Outcome::failed(
                StatusCode::BAD_GATEWAY,
                "upstream_unreachable",
                "The authorization server could not be reached.",
            );
        }
        Err(TokenError::EgressRefused(why)) => {
            return Outcome::failed(StatusCode::BAD_GATEWAY, "egress_refused", why);
        }
        Err(err) => {
            return Outcome::failed(StatusCode::BAD_GATEWAY, "exchange_failed", err.message());
        }
    };
    let tokens = crate::model::GrantTokens {
        access_token: issued.access_token,
        refresh_token: issued.refresh_token,
    };
    // 8. Under the refresh lock: the connection unchanged since the flow
    // started (the review's R1), then the grant.
    let _lock = runtime.lock(&snapshot.connection_id).await;
    let stored = runtime.store.store_grant(
        &GrantToStore {
            connection_id: &snapshot.connection_id,
            url: &snapshot.url,
            cred_kind: snapshot.cred_kind,
            internal_network: snapshot.internal_network,
            source: snapshot.source,
            client: &snapshot.client,
            issuer: &snapshot.issuer,
            authorization_endpoint: &snapshot.authorization_endpoint,
            redirect_uri: &snapshot.redirect_uri,
            scopes: &snapshot.scopes,
            resource: &snapshot.resource,
            resource_param_accepted,
            registered_at: snapshot.registered_at,
            tokens: &tokens,
            expires_at: oauth::expires_at(unix_now(), issued.expires_in),
        },
        &runtime.key,
        unix_now(),
    );
    match stored {
        Ok(GrantStored::Stored(change)) => {
            notify::announce(runtime.notifier.as_ref(), &change);
            // 9.
            Outcome {
                status: StatusCode::OK,
                result: "connected",
                connection_id: None,
                message: "Connected. You may close this window.".into(),
            }
        }
        Ok(GrantStored::ConnectionChanged) => Outcome::failed(
            StatusCode::BAD_REQUEST,
            "connection_changed",
            "The connection was changed while this sign-in ran. Start it again.",
        ),
        Err(err) => {
            tracing::error!(connection_id = %snapshot.connection_id, error = %err, "gateway: a grant not stored");
            Outcome::failed(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Something went wrong in hennery.",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lane L16: a parameter twice, or with a control character, is read
    /// as neither.
    #[test]
    fn a_parameter_twice_or_with_a_control_character_is_refused() {
        assert_eq!(parameters(Some("state=a&code=b")).unwrap()["state"], "a");
        assert!(parameters(Some("state=a&state=b")).is_none());
        assert!(parameters(Some("state=a&code=b&code=c")).is_none());
        assert!(parameters(Some("state=a&iss=x&iss=y")).is_none());
        assert!(parameters(Some("state=a%00b")).is_none());
        assert!(parameters(Some("state=a&code=b%0A")).is_none());
        // Parameters the callback does not read may repeat.
        assert!(parameters(Some("state=a&x=1&x=2")).is_some());
    }

    #[test]
    fn the_page_escapes_what_it_shows() {
        assert_eq!(escape("<a href=\"x\">'&"), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;");
    }
}
