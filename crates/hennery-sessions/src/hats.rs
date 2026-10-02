//! Hat endpoints (kernel spec §5, §8): the hats, and each host's path
//! rules. They sit beside the host endpoints, in the collector's one
//! router; the model is the kernel's (`hennery_kernel::hats`).

use crate::AppState;
use crate::api::{error, internal};
use axum::extract::{Path, State};
use axum::handler::Handler;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router, middleware};
use hennery_kernel::hats::{HatChange, HatRecord, NewRule, PathRule, RulesChange, normalize_lexically};
use hennery_kernel::json::ApiJson;
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{CreateHatRequest, HatItem, PathRuleItem, PathRulesRequest, UpdateHatRequest};

/// Every route needs the operator's session. Changing a hat and replacing a
/// host's path rules need a fresh step-up too (plan 5a decision 7): the
/// rules move the boundary of every later session on that host, a new
/// default for new hosts that of every later host, and a swapped name or
/// colour would point the next stepped-up change at the wrong hat.
pub fn router(state: AppState) -> Router {
    let routes = Router::new()
        .route("/api/hats", get(list_hats).post(create_hat))
        .route(
            "/api/hats/{id}",
            patch(update_hat).route_layer(middleware::from_fn(hennery_kernel::auth::require_step_up)),
        )
        .route(
            "/api/hosts/{id}/path-rules",
            // Step-up is layered on `put` alone (`Handler::layer`), so GET
            // stays free; a method added to this route later gets no
            // step-up unless it is layered too.
            get(path_rules).put(replace_path_rules.layer(middleware::from_fn(hennery_kernel::auth::require_step_up))),
        );
    hennery_kernel::auth::operator_only(routes, state.operator.clone()).with_state(state)
}

pub(crate) fn hat_item(record: HatRecord) -> HatItem {
    HatItem {
        id: record.id,
        name: record.name,
        colour: record.colour,
        created_at: rfc3339(record.created_at),
        default_for_new_hosts: record.default_for_new_hosts,
    }
}

fn rule_item(rule: PathRule) -> PathRuleItem {
    PathRuleItem {
        id: rule.id,
        prefix: rule.prefix,
        hat_id: rule.hat_id,
        verified: rule.verified,
    }
}

fn hat_changed(change: HatChange, status: StatusCode) -> Response {
    match change {
        HatChange::Done(record) => (status, Json(hat_item(record))).into_response(),
        HatChange::NotFound => error(StatusCode::NOT_FOUND, "not_found", "no such hat"),
        HatChange::NameTaken => error(StatusCode::CONFLICT, "name_taken", "another hat has this name"),
        HatChange::Invalid(why) => error(StatusCode::BAD_REQUEST, "invalid", why),
    }
}

/// `GET /api/hats`: every hat, oldest first.
async fn list_hats(State(state): State<AppState>) -> Response {
    match state.hosts.hats() {
        Ok(hats) => Json(hats.into_iter().map(hat_item).collect::<Vec<_>>()).into_response(),
        Err(err) => internal(err),
    }
}

/// `POST /api/hats`: 201 with the new hat.
async fn create_hat(State(state): State<AppState>, ApiJson(req): ApiJson<CreateHatRequest>) -> Response {
    match state.hosts.create_hat(&req.name, req.colour.as_deref(), unix_now()) {
        Ok(change) => hat_changed(change, StatusCode::CREATED),
        Err(err) => internal(err),
    }
}

/// `PATCH /api/hats/{id}`: 200 with the hat as it is now.
async fn update_hat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<UpdateHatRequest>,
) -> Response {
    match state.hosts.update_hat(
        &id,
        req.name.as_deref(),
        req.colour.as_deref(),
        req.default_for_new_hosts,
    ) {
        Ok(change) => hat_changed(change, StatusCode::OK),
        Err(err) => internal(err),
    }
}

/// `GET /api/hosts/{id}/path-rules`: the host's rules, longest prefix
/// first.
async fn path_rules(State(state): State<AppState>, Path(host_id): Path<String>) -> Response {
    match state.hosts.path_rules(&host_id) {
        Ok(Some(rules)) => Json(rules.into_iter().map(rule_item).collect::<Vec<_>>()).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => internal(err),
    }
}

/// `PUT /api/hosts/{id}/path-rules`: replace the host's rules (kernel spec
/// §8, the full set), 200 with the stored set. Each prefix is normalised by
/// its text and stored unverified (plan 5a decision 6); resolving it
/// through the host comes with `resolve_path`.
async fn replace_path_rules(
    State(state): State<AppState>,
    Path(host_id): Path<String>,
    ApiJson(req): ApiJson<PathRulesRequest>,
) -> Response {
    let mut rules = Vec::with_capacity(req.rules.len());
    for rule in req.rules {
        match normalize_lexically(&rule.prefix) {
            Ok(prefix) => rules.push(NewRule {
                prefix,
                hat_id: rule.hat_id,
                verified: false,
            }),
            Err(why) => return error(StatusCode::BAD_REQUEST, "invalid", format!("{:?}: {why}", rule.prefix)),
        }
    }
    match state.hosts.replace_path_rules(&host_id, &rules) {
        Ok(RulesChange::Done(stored)) => Json(stored.into_iter().map(rule_item).collect::<Vec<_>>()).into_response(),
        Ok(RulesChange::HostNotFound) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Ok(RulesChange::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}
