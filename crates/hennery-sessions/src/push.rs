//! Push endpoints (kernel spec §6, §8; plan 10a): the VAPID public key, the
//! owner's push subscriptions, each hat's push policy, and the settings
//! that carry the push contact. The model is the kernel's
//! (`hennery_kernel::push`); delivery is plan 10b's.

use crate::AppState;
use crate::api::{error, internal};
use axum::extract::{DefaultBodyLimit, Extension, Path, State};
use axum::handler::Handler;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router, middleware};
use hennery_kernel::json::ApiJson;
use hennery_kernel::operator::Authenticated;
use hennery_kernel::push::{NewSubscription, PushPolicy, Subscribed, Subscription};
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{
    PushPolicyItem, PushPolicyRequest, PushRotateRequest, PushSubscribeRequest, PushSubscriptionItem,
    PushUnsubscribeRequest, SettingsResponse, SettingsUpdateRequest, VapidKeyResponse,
};

/// Every route needs the operator's session. Subscribing a browser needs a
/// fresh step-up too (plan 10a decision 4): a subscription outlives the
/// request, and one added with a stolen cookie would go on receiving
/// notifications. A rotation needs none (the review's A1): it only replaces
/// a subscription the owner has, by the endpoint only that browser knows.
/// Bodies are small: a subscription is well under 16 KiB (the review's A8).
pub fn router(state: AppState) -> Router {
    let routes = Router::new()
        .route("/api/push/vapid", get(vapid_key))
        .route(
            "/api/push/subscriptions",
            get(list_subscriptions)
                .post(subscribe.layer(middleware::from_fn(hennery_kernel::auth::require_step_up)))
                .delete(unsubscribe_endpoint),
        )
        .route("/api/push/subscriptions/rotate", post(rotate))
        .route("/api/push/subscriptions/{id}", delete(unsubscribe))
        .route("/api/push/policies", get(list_policies))
        .route("/api/push/policies/{hat_id}", put(set_policy))
        .route("/api/settings", get(settings).patch(update_settings))
        .layer(DefaultBodyLimit::max(hennery_kernel::auth_api::MAX_BODY_BYTES));
    hennery_kernel::auth::operator_only(routes, state.operator.clone()).with_state(state)
}

fn subscription_item(state: &AppState, sub: Subscription, session_id: &str) -> anyhow::Result<PushSubscriptionItem> {
    let signed_out = state
        .operator
        .session_expires_at(&sub.auth_session, unix_now())?
        .is_none();
    Ok(PushSubscriptionItem {
        endpoint_host: sub.endpoint_host(),
        this_device: sub.auth_session == session_id,
        signed_out,
        id: sub.id,
        device_label: sub.device_label,
        created_at: rfc3339(sub.created_at),
        last_success_at: sub.last_success_at.map(rfc3339),
        last_error: sub.last_error,
    })
}

fn policy_item(hat_id: String, policy: PushPolicy) -> PushPolicyItem {
    PushPolicyItem {
        hat_id,
        muted: policy.muted,
        details: policy.details,
        generic_title: policy.generic_title,
    }
}

/// `GET /api/push/vapid`: the key a browser subscribes with.
async fn vapid_key(State(state): State<AppState>) -> Response {
    Json(VapidKeyResponse {
        public_key: state.vapid.public_key().to_string(),
    })
    .into_response()
}

/// `GET /api/push/subscriptions`: every subscription, oldest first.
async fn list_subscriptions(State(state): State<AppState>, Extension(session): Extension<Authenticated>) -> Response {
    let items = state.hosts.subscriptions().and_then(|subs| {
        subs.into_iter()
            .map(|sub| subscription_item(&state, sub, &session.session_id))
            .collect::<anyhow::Result<Vec<_>>>()
    });
    match items {
        Ok(items) => Json(items).into_response(),
        Err(err) => internal(err),
    }
}

/// `POST /api/push/subscriptions`: 201 with a new subscription, 200 when
/// the endpoint's earlier one is replaced.
async fn subscribe(
    State(state): State<AppState>,
    Extension(session): Extension<Authenticated>,
    ApiJson(req): ApiJson<PushSubscribeRequest>,
) -> Response {
    let outcome = state
        .hosts
        .subscribe(&new_subscription(&req), &session.session_id, unix_now());
    subscribed(&state, outcome, &session)
}

/// `POST /api/push/subscriptions/rotate`: 200 with the subscription as
/// replaced, or 404 when the old endpoint is not the owner's.
async fn rotate(
    State(state): State<AppState>,
    Extension(session): Extension<Authenticated>,
    ApiJson(req): ApiJson<PushRotateRequest>,
) -> Response {
    let outcome = state
        .hosts
        .rotate(&req.old_endpoint, &new_subscription(&req.subscription), unix_now());
    if let Ok(Subscribed::Replaced(sub)) = &outcome {
        tracing::info!(id = %sub.id, host = %sub.endpoint_host(), "push subscription rotated");
    }
    subscribed(&state, outcome, &session)
}

fn new_subscription(req: &PushSubscribeRequest) -> NewSubscription<'_> {
    NewSubscription {
        endpoint: &req.endpoint,
        p256dh: &req.keys.p256dh,
        auth: &req.keys.auth,
        device_label: req.device_label.as_deref(),
        // Milliseconds, rounded down: a subscription is never kept past
        // the browser's expiry.
        expires_at: req.expiration_time.map(|ms| ms.div_euclid(1000)),
    }
}

fn subscribed(state: &AppState, outcome: anyhow::Result<Subscribed>, session: &Authenticated) -> Response {
    let (status, sub) = match outcome {
        Ok(Subscribed::Created(sub)) => {
            tracing::info!(id = %sub.id, host = %sub.endpoint_host(), "push subscription added");
            (StatusCode::CREATED, sub)
        }
        Ok(Subscribed::Replaced(sub)) => (StatusCode::OK, sub),
        Ok(Subscribed::Invalid(why)) => return error(StatusCode::BAD_REQUEST, "invalid", why),
        Ok(Subscribed::TooMany) => {
            return error(
                StatusCode::CONFLICT,
                "too_many_subscriptions",
                "remove a device before subscribing another",
            );
        }
        Ok(Subscribed::Conflict) => {
            return error(
                StatusCode::CONFLICT,
                "endpoint_taken",
                "this browser is subscribed for another account",
            );
        }
        Ok(Subscribed::NotFound) => return error(StatusCode::NOT_FOUND, "not_found", "no such subscription"),
        // The session ended while the request ran: as if it had not been
        // signed in.
        Ok(Subscribed::SignedOut) => return error(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first"),
        Err(err) => return internal(err),
    };
    match subscription_item(state, sub, &session.session_id) {
        Ok(item) => (status, Json(item)).into_response(),
        Err(err) => internal(err),
    }
}

/// `DELETE /api/push/subscriptions`: 204, this browser's subscription
/// removed; 404 when there was none.
async fn unsubscribe_endpoint(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<PushUnsubscribeRequest>,
) -> Response {
    removed(state.hosts.unsubscribe_endpoint(&req.endpoint))
}

/// `DELETE /api/push/subscriptions/{id}`: 204, or 404.
async fn unsubscribe(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    removed(state.hosts.unsubscribe(&id))
}

fn removed(result: anyhow::Result<bool>) -> Response {
    match result {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, "not_found", "no such subscription"),
        Err(err) => internal(err),
    }
}

/// `GET /api/push/policies`: every hat's policy, oldest hat first.
async fn list_policies(State(state): State<AppState>) -> Response {
    match state.hosts.push_policies() {
        Ok(policies) => Json(
            policies
                .into_iter()
                .map(|(hat_id, policy)| policy_item(hat_id, policy))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(err) => internal(err),
    }
}

/// `PUT /api/push/policies/{hat_id}`: 200 with the hat's policy as stored,
/// or 404. No step-up (plan 10a decision 6): the payload is encrypted to
/// the browser, so a policy changes what a lock screen shows, not what
/// leaves hennery in the clear.
async fn set_policy(
    State(state): State<AppState>,
    Path(hat_id): Path<String>,
    ApiJson(req): ApiJson<PushPolicyRequest>,
) -> Response {
    let policy = PushPolicy {
        muted: req.muted,
        details: req.details,
        generic_title: req.generic_title,
    };
    match state.hosts.set_push_policy(&hat_id, policy) {
        Ok(true) => Json(policy_item(hat_id, policy)).into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, "not_found", "no such hat"),
        Err(err) => internal(err),
    }
}

/// `GET /api/settings`: `public_url` and the push contact.
async fn settings(State(state): State<AppState>) -> Response {
    settings_now(&state)
}

/// `PATCH /api/settings`: 200 with the settings as they are now. An empty
/// `contact` clears it.
async fn update_settings(State(state): State<AppState>, ApiJson(req): ApiJson<SettingsUpdateRequest>) -> Response {
    if let Some(contact) = req.contact {
        let contact = contact.trim();
        match state.operator.set_contact((!contact.is_empty()).then_some(contact)) {
            Ok(Ok(())) => {}
            Ok(Err(why)) => return error(StatusCode::BAD_REQUEST, "invalid", why),
            Err(err) => return internal(err),
        }
    }
    settings_now(&state)
}

fn settings_now(state: &AppState) -> Response {
    let public_url = state
        .operator
        .public_url()
        .map(|url| url.origin().to_string())
        .unwrap_or_default();
    match state.operator.contact() {
        Ok(contact) => Json(SettingsResponse { public_url, contact }).into_response(),
        Err(err) => internal(err),
    }
}
