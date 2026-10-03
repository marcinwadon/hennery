//! The OAuth routes behind the operator's session (gateway spec §4, §9;
//! api-8e-8f B2, B3, B6): the redirect URI, a pre-registered client
//! (step-up), authorize (step-up: kernel spec §3.4 lists gateway
//! credentials and pre-registered clients; api-8e-8f F2), and a
//! probe now. The callback is `callback.rs`, outside the session.
//!
//! Each authorize failure after the connection was found is also kept in
//! the connection's `oauth_error`, and the next authorize clears it. A
//! failed Connect never touches the status: a working grant stays `ok`
//! (G-7: it is never touched until a new consent completes).

use crate::api::{GatewayState, error, internal, item, not_found};
use crate::flows::{FLOW_TTL, MAX_FLOWS, Registration, Snapshot, cookie_name};
use crate::model::{CredKind, MAX_URL, TokenClient, url_for_logs};
use crate::oauth::{self, Consent, DiscoveryError, RegisterError};
use crate::probe;
use crate::store::{ClientChange, ClientSource, SecretInput, Slot};
use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hennery_kernel::egress::is_https_or_loopback;
use hennery_kernel::json::ApiJson;
use hennery_kernel::operator::Authenticated;
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{McpAuthorizeRequest, McpAuthorizeResponse, McpOauthClientRequest, McpOauthRedirect};
use url::Url;
use zeroize::Zeroizing;

/// `GET /api/mcp/oauth/redirect-uri`.
pub(crate) async fn redirect_uri(State(state): State<GatewayState>) -> Response {
    match state.redirect_uri() {
        Some(redirect_uri) => Json(McpOauthRedirect { redirect_uri }).into_response(),
        None => internal(anyhow::anyhow!("no public_url")),
    }
}

/// `PUT /api/mcp/connections/{id}/oauth-client` (step-up).
pub(crate) async fn oauth_client(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<McpOauthClientRequest>,
) -> Response {
    let runtime = &state.runtime;
    let secret = match req.client_secret {
        None => SecretInput::Keep,
        Some(None) => SecretInput::Clear,
        Some(Some(secret)) => SecretInput::Set(Zeroizing::new(secret)),
    };
    // A client secret is a credential: its write takes the refresh lock.
    let _lock = runtime.lock(&id).await;
    match runtime
        .store
        .set_oauth_client(&id, &req.client_id, secret, &runtime.key, unix_now())
    {
        Ok(ClientChange::Done(record)) => {
            // Its flows were for the client before (the review's R1).
            runtime.flows.drop_connection(&id);
            tracing::info!(connection_id = %id, "gateway connection's OAuth client set");
            Json(item(*record, &state.redirect_uri().unwrap_or_default())).into_response()
        }
        Ok(ClientChange::NotFound) => not_found(),
        Ok(ClientChange::WrongKind(kind)) => error(
            StatusCode::CONFLICT,
            "wrong_cred_kind",
            format!("a {} connection takes no pre-registered client", kind.as_str()),
        ),
        Ok(ClientChange::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}

/// How a protected-resource document's `resource` stands to the
/// connection's URL (gateway spec §4.1; api-8e-8f R4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceFit {
    /// Same origin, and the URL's path is the resource's or below it.
    Matches,
    /// Same origin, another path: shown to the operator, who may accept it.
    Mismatch,
    /// Another origin, not `https`, over 2048 bytes or not a URL: it cannot
    /// be accepted.
    Foreign,
}

/// Judge `resource` against `url`. The path is compared segment by
/// segment: `/mcp` covers `/mcp` and `/mcp/x`, never `/mcpx`.
pub fn resource_fit(resource: &str, url: &Url) -> ResourceFit {
    let Some(named) = (resource.len() <= MAX_URL)
        .then(|| Url::parse(resource).ok())
        .flatten()
        .filter(is_https_or_loopback)
    else {
        return ResourceFit::Foreign;
    };
    if named.origin() != url.origin() {
        return ResourceFit::Foreign;
    }
    let base = named.path().trim_end_matches('/');
    let path = url.path();
    if base.is_empty() || path == base || path.starts_with(&format!("{base}/")) {
        ResourceFit::Matches
    } else {
        ResourceFit::Mismatch
    }
}

/// `POST /api/mcp/connections/{id}/authorize` (step-up): discovery,
/// registration if needed, the flow and its cookie, and the consent URL
/// (gateway spec §4.1–§4.3).
pub(crate) async fn authorize(
    State(state): State<GatewayState>,
    Extension(session): Extension<Authenticated>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<McpAuthorizeRequest>,
) -> Response {
    let runtime = &state.runtime;
    let now = unix_now();
    let connection = match runtime.store.connection(&id) {
        Ok(Some(connection)) => connection,
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    };
    if !connection.cred_kind.is_oauth() {
        return error(
            StatusCode::CONFLICT,
            "wrong_cred_kind",
            format!(
                "a {} connection does not connect with OAuth",
                connection.cred_kind.as_str()
            ),
        );
    }
    let Some(redirect_uri) = state.redirect_uri() else {
        return internal(anyhow::anyhow!("no public_url"));
    };
    // The latest Connect's failure is cleared by the next authorize, and
    // each failure from here on is kept.
    if let Err(err) = runtime.store.set_oauth_error(&id, None, now) {
        return internal(err);
    }
    let fail = |status: StatusCode, code: &str, message: String| {
        if let Err(err) = runtime.store.set_oauth_error(&id, Some((code, &message)), now) {
            tracing::error!(connection_id = %id, error = %err, "gateway: a Connect's failure not recorded");
        }
        tracing::info!(connection_id = %id, code, "gateway: a Connect did not start");
        error(status, code, message)
    };
    let slots = match runtime.store.oauth_clients(&id, &runtime.key) {
        Ok(Some(slots)) => slots,
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    };
    if connection.cred_kind == CredKind::OauthClient && slots.active.is_none() {
        return fail(
            StatusCode::CONFLICT,
            "no_oauth_client",
            "enter the client registered at the vendor first (PUT …/oauth-client)".into(),
        );
    }
    if runtime.flows.live(&id, now) >= MAX_FLOWS {
        return fail(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_flows",
            format!("at most {MAX_FLOWS} Connects can be in flight"),
        );
    }
    let url = match Url::parse(&connection.url) {
        Ok(url) => url,
        Err(err) => return internal(err.into()),
    };
    let client = runtime.client(connection.internal_network);
    let discovered = match oauth::discover(&client, &url).await {
        Ok(discovered) => discovered,
        Err(DiscoveryError::Failed(why)) => return fail(StatusCode::BAD_GATEWAY, "discovery_failed", why),
        Err(DiscoveryError::Insecure(why)) => return fail(StatusCode::BAD_GATEWAY, "insecure_metadata", why),
        Err(DiscoveryError::Refused(why)) => return fail(StatusCode::BAD_GATEWAY, "egress_refused", why),
        Err(DiscoveryError::Unreachable(why)) => return fail(StatusCode::BAD_GATEWAY, "upstream_unreachable", why),
    };
    // The resource (gateway spec §4.1, G-4; api-8e-8f F3, R4).
    match &discovered.resource {
        Some(resource) => match resource_fit(resource, &url) {
            ResourceFit::Matches => {
                if let Err(err) = runtime.store.set_resource_mismatch(&id, &connection.url, None) {
                    return internal(err);
                }
            }
            ResourceFit::Foreign => {
                return fail(
                    StatusCode::BAD_GATEWAY,
                    "resource_foreign",
                    format!(
                        "the protected-resource document of {} names a resource on another origin, or not https: it cannot be accepted",
                        url_for_logs(&connection.url)
                    ),
                );
            }
            ResourceFit::Mismatch => {
                let accepted = connection.accepted_resource.as_deref() == Some(resource.as_str());
                if !accepted && req.accept_resource.as_deref() == Some(resource.as_str()) {
                    match runtime.store.accept_resource(&id, &connection.url, resource) {
                        Ok(true) => {}
                        Ok(false) => return not_found(),
                        Err(err) => return internal(err),
                    }
                } else if !accepted {
                    if let Err(err) = runtime
                        .store
                        .set_resource_mismatch(&id, &connection.url, Some(resource))
                    {
                        return internal(err);
                    }
                    // A stale acceptance is a mismatch again, with the value
                    // found now (api-8e-8f F3): the frontend refetches on 409.
                    let message = if req.accept_resource.is_some() {
                        "the accepted resource is not the one found now: compare them again"
                    } else {
                        "the protected-resource document names another resource than the URL: accept it, or fix the URL"
                    };
                    return fail(StatusCode::CONFLICT, "resource_mismatch", message.into());
                }
            }
        },
        None if req.accept_resource.is_some() => {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid",
                "no protected-resource document names a resource to accept".into(),
            );
        }
        None => {}
    }
    if !discovered.s256 {
        return fail(
            StatusCode::BAD_GATEWAY,
            "pkce_unsupported",
            format!(
                "the authorization server at {} does not advertise PKCE S256",
                url_for_logs(&discovered.issuer)
            ),
        );
    }
    let token_endpoint = discovered.token_endpoint.as_str().to_string();
    let (source, token_client, registered_at) = match connection.cred_kind {
        CredKind::OauthClient => {
            let (slot, held) = match (&slots.pending, &slots.active) {
                (Some(pending), _) => (Slot::Pending, pending),
                (None, Some(active)) => (Slot::Active, active),
                (None, None) => unreachable!("checked above"),
            };
            match &held.issuer {
                Some(pinned) => {
                    if *pinned != discovered.issuer || held.token_endpoint.as_deref() != Some(token_endpoint.as_str()) {
                        return fail(
                            StatusCode::CONFLICT,
                            "issuer_changed",
                            format!(
                                "the authorization server is now {}, not the one the client was first used with: enter the client again",
                                url_for_logs(&discovered.issuer)
                            ),
                        );
                    }
                }
                // The first authorize with this client pins it (the
                // review's R2).
                None => {
                    if let Err(err) =
                        runtime
                            .store
                            .pin_client(&id, slot, &held.client_id, &discovered.issuer, &token_endpoint)
                    {
                        return internal(err);
                    }
                }
            }
            let source = match slot {
                Slot::Active => ClientSource::Active,
                Slot::Pending => ClientSource::Pending,
            };
            (
                source,
                TokenClient {
                    client_id: held.client_id.clone(),
                    secret: held.secret.clone(),
                    auth_method: oauth::auth_method_for(held.secret.is_some(), &discovered.auth_methods),
                    token_endpoint: token_endpoint.clone(),
                },
                now,
            )
        }
        CredKind::OauthDcr => {
            let reusable = slots.active.as_ref().filter(|active| {
                active.redirect_uri.as_deref() == Some(redirect_uri.as_str())
                    && active.token_endpoint.as_deref() == Some(token_endpoint.as_str())
                    && active.issuer.as_deref() == Some(discovered.issuer.as_str())
            });
            if let (Some(active), Some(method)) = (reusable, reusable.and_then(|a| a.auth_method)) {
                (
                    ClientSource::Active,
                    TokenClient {
                        client_id: active.client_id.clone(),
                        secret: active.secret.clone(),
                        auth_method: method,
                        token_endpoint: token_endpoint.clone(),
                    },
                    now,
                )
            } else if let Some(held) = runtime.flows.registration(&id, &token_endpoint, &redirect_uri) {
                (ClientSource::Registered, held.client, held.registered_at)
            } else {
                let Some(endpoint) = &discovered.registration_endpoint else {
                    return fail(
                        StatusCode::CONFLICT,
                        "no_registration_endpoint",
                        "the authorization server offers no dynamic registration: switch to a pre-registered client"
                            .into(),
                    );
                };
                match oauth::register(&client, endpoint, &redirect_uri, &discovered.scopes).await {
                    Ok(registered) => {
                        let token_client = TokenClient {
                            client_id: registered.client_id,
                            secret: registered.secret,
                            auth_method: registered.auth_method,
                            token_endpoint: token_endpoint.clone(),
                        };
                        runtime.flows.hold_registration(
                            &id,
                            Registration {
                                client: token_client.clone(),
                                redirect_uri: redirect_uri.clone(),
                                registered_at: now,
                            },
                        );
                        (ClientSource::Registered, token_client, now)
                    }
                    Err(RegisterError::Refused(said)) => {
                        return fail(StatusCode::BAD_GATEWAY, "registration_refused", said);
                    }
                    Err(RegisterError::Invalid) => {
                        return fail(
                            StatusCode::BAD_GATEWAY,
                            "registration_refused",
                            "the registration's answer named no usable client".into(),
                        );
                    }
                    Err(RegisterError::EgressRefused(why)) => {
                        return fail(StatusCode::BAD_GATEWAY, "egress_refused", why);
                    }
                    Err(RegisterError::Unreachable) => {
                        return fail(
                            StatusCode::BAD_GATEWAY,
                            "upstream_unreachable",
                            format!(
                                "the registration endpoint at {} could not be reached",
                                url_for_logs(endpoint.as_str())
                            ),
                        );
                    }
                }
            }
        }
        CredKind::None | CredKind::Static => unreachable!("checked above"),
    };
    let resource_param = !runtime.flows.resource_refused(&id, &connection.url);
    let verifier = oauth::verifier();
    let challenge = oauth::challenge_for(&verifier);
    let client_id = token_client.client_id.clone();
    let snapshot = Snapshot {
        connection_id: id.clone(),
        url: connection.url.clone(),
        cred_kind: connection.cred_kind,
        internal_network: connection.internal_network,
        source,
        client: token_client,
        issuer: discovered.issuer.clone(),
        authorization_endpoint: discovered.authorization_endpoint.as_str().to_string(),
        iss_parameter: discovered.iss_parameter,
        redirect_uri: redirect_uri.clone(),
        scopes: discovered.scopes.clone(),
        resource: connection.url.clone(),
        resource_param,
        verifier,
        registered_at,
        auth_session: session.session_id.clone(),
    };
    let Some(started) = runtime.flows.start(snapshot, now) else {
        return fail(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_flows",
            format!("at most {MAX_FLOWS} Connects can be in flight"),
        );
    };
    let consent = oauth::consent_url(
        &discovered.authorization_endpoint,
        &Consent {
            client_id: &client_id,
            redirect_uri: &redirect_uri,
            state: &started.state,
            challenge: &challenge,
            scopes: &discovered.scopes,
            resource: resource_param.then_some(connection.url.as_str()),
        },
    );
    // `consent` is `https` (or loopback `http`): it is the authorization
    // endpoint with a query added, whose scheme discovery checked.
    let secure = if state.secure_cookies() { "; Secure" } else { "" };
    let cookie = format!(
        "{}={}; HttpOnly; SameSite=Lax; Path={}; Max-Age={FLOW_TTL}{secure}",
        cookie_name(&started.state),
        started.cookie.as_str(),
        crate::api::CALLBACK_PATH,
    );
    tracing::info!(connection_id = %id, issuer = %url_for_logs(&discovered.issuer), "gateway: a Connect started");
    let mut response = Json(McpAuthorizeResponse {
        consent_url: consent.to_string(),
        expires_at: rfc3339(started.expires_at),
    })
    .into_response();
    match HeaderValue::from_str(&cookie) {
        Ok(value) => {
            response.headers_mut().append(header::SET_COOKIE, value);
            response
        }
        Err(err) => internal(err.into()),
    }
}

/// `{}`: a body is required, and nothing may be in it.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Empty {}

/// `POST /api/mcp/connections/{id}/probe` (api-8e-8f B6, may change).
pub(crate) async fn probe(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    ApiJson(Empty {}): ApiJson<Empty>,
) -> Response {
    let runtime = &state.runtime;
    let connection = match runtime.statuses.connection_by_id(&id) {
        Ok(Some(connection)) => connection,
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    };
    if connection.cred_kind != CredKind::None && !connection.has_credential {
        return error(
            StatusCode::CONFLICT,
            "no_credential",
            "the connection has no credential yet: set the token or Connect first",
        );
    }
    let verdict = probe::probe_now(runtime, &connection).await;
    tracing::info!(connection_id = %id, verdict = ?verdict, "gateway: a probe the operator asked for");
    match runtime.store.connection(&id) {
        Ok(Some(record)) => Json(item(record, &state.redirect_uri().unwrap_or_default())).into_response(),
        Ok(None) => not_found(),
        Err(err) => internal(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gateway spec §4.1 (G-4) and api-8e-8f R4: every outcome of the fit.
    #[test]
    fn resource_fit_compares_origins_and_path_segments() {
        let url = Url::parse("https://mcp.vendor.example/v1/mcp?k=1").unwrap();
        for (resource, expected) in [
            ("https://mcp.vendor.example/v1/mcp", ResourceFit::Matches),
            ("https://mcp.vendor.example/v1/mcp/", ResourceFit::Matches),
            ("https://mcp.vendor.example/v1", ResourceFit::Matches),
            ("https://mcp.vendor.example", ResourceFit::Matches),
            ("https://mcp.vendor.example/v1/mc", ResourceFit::Mismatch),
            ("https://mcp.vendor.example/v2/mcp", ResourceFit::Mismatch),
            ("https://mcp.vendor.example/v1/mcp/deeper", ResourceFit::Mismatch),
            ("https://other.example/v1/mcp", ResourceFit::Foreign),
            ("https://mcp.vendor.example:8443/v1/mcp", ResourceFit::Foreign),
            ("http://mcp.vendor.example/v1/mcp", ResourceFit::Foreign),
            ("not a url", ResourceFit::Foreign),
        ] {
            assert_eq!(resource_fit(resource, &url), expected, "{resource}");
        }
        let long = format!("https://mcp.vendor.example/{}", "a".repeat(MAX_URL));
        assert_eq!(resource_fit(&long, &url), ResourceFit::Foreign);
        // Loopback `http` is the one plain `http` taken (the fakes).
        let local = Url::parse("http://127.0.0.1:8080/mcp").unwrap();
        assert_eq!(resource_fit("http://127.0.0.1:8080/mcp", &local), ResourceFit::Matches);
    }
}
