//! Hat endpoints (kernel spec §5, §8): the hats, each host's path rules,
//! and resolving a path to its hat. They sit beside the host endpoints, in
//! the collector's one router; the model is the kernel's
//! (`hennery_kernel::hats`), and paths are resolved by their host
//! (`resolve`).

use crate::AppState;
use crate::api::{error, internal};
use crate::resolve::{NotResolved, resolve_on_host};
use axum::extract::{Path, State};
use axum::handler::Handler;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router, middleware};
use futures::stream::{self, StreamExt};
use hennery_kernel::hats::{HatChange, HatRecord, MAX_RULES, NewRule, PathRule, RulesChange};
use hennery_kernel::json::ApiJson;
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{
    CreateHatRequest, HatItem, HatResolution, HatResolveRequest, PathRuleInput, PathRuleItem, PathRulesRequest,
    UpdateHatRequest,
};

/// Rule prefixes resolved through the host at once, at most: matches the
/// host's own resolution semaphore (the review's P6).
const RESOLVING_AT_ONCE: usize = 4;

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
        .route("/api/hats/resolve", post(resolve_hat))
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
/// §8, the full set), 200 with the stored set. Each prefix is resolved
/// through the host (kernel spec §5.2), and verified if it exists there; a
/// prefix that does not exist keeps its deepest existing ancestor resolved
/// (plan 5b decision 3). With the host away, a set with any rule is refused
/// (409 `host_offline`, the review's B1): by its text alone a prefix under
/// a symlinked parent would miss every session under it, silently. The set's
/// size and each rule's `hat_id` are checked before anything reaches the
/// host (the review's Important 1): a set that will be refused anyway must
/// not cost the host a round trip per prefix.
async fn replace_path_rules(
    State(state): State<AppState>,
    Path(host_id): Path<String>,
    ApiJson(req): ApiJson<PathRulesRequest>,
) -> Response {
    match state.hosts.path_rules(&host_id) {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => return internal(err),
    }
    if req.rules.len() > MAX_RULES {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("a host has at most {MAX_RULES} path rules"),
        );
    }
    for rule in &req.rules {
        match state.hosts.hat(&rule.hat_id) {
            Ok(Some(_)) => {}
            Ok(None) => return error(StatusCode::BAD_REQUEST, "invalid", format!("no hat {:?}", rule.hat_id)),
            Err(err) => return internal(err),
        }
    }
    let rules = match rules_through_host(&state, &host_id, req.rules).await {
        Ok(rules) => rules,
        Err(why) => return why.into_response(),
    };
    match state.hosts.replace_path_rules(&host_id, &rules) {
        Ok(RulesChange::Done(stored)) => Json(stored.into_iter().map(rule_item).collect::<Vec<_>>()).into_response(),
        Ok(RulesChange::HostNotFound) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Ok(RulesChange::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}

/// Every rule's prefix resolved through its connected host, a few at once,
/// in the order given. The first refusal is the answer, and nothing is
/// stored.
async fn rules_through_host(
    state: &AppState,
    host_id: &str,
    rules: Vec<PathRuleInput>,
) -> Result<Vec<NewRule>, NotResolved> {
    let resolved: Vec<_> = stream::iter(rules)
        .map(|rule| async move {
            let on_host = resolve_on_host(state, host_id, &rule.prefix).await;
            (rule, on_host)
        })
        .buffered(RESOLVING_AT_ONCE)
        .collect()
        .await;
    let mut out = Vec::with_capacity(resolved.len());
    for (rule, on_host) in resolved {
        let on_host = on_host?;
        // A rule's prefix names a directory (kernel spec §5.2): a file
        // that exists there can never hold a session's cwd.
        if on_host.exists && !on_host.is_dir {
            return Err(NotResolved::Refused {
                code: "invalid".into(),
                message: format!("{} is not a directory on that host", on_host.canonical),
            });
        }
        out.push(NewRule {
            prefix: on_host.canonical,
            hat_id: rule.hat_id,
            verified: on_host.exists,
        });
    }
    Ok(out)
}

/// `POST /api/hats/resolve` (kernel spec §8): the path resolved by its host,
/// and the hat it resolves to there.
async fn resolve_hat(State(state): State<AppState>, ApiJson(req): ApiJson<HatResolveRequest>) -> Response {
    let on_host = match resolve_on_host(&state, &req.host_id, &req.path).await {
        Ok(on_host) => on_host,
        Err(why) => return why.into_response(),
    };
    match state.hosts.resolve_hat(&req.host_id, &on_host.canonical) {
        Ok(Some(resolution)) => Json(HatResolution {
            canonical: on_host.canonical,
            exists: on_host.exists,
            is_dir: on_host.is_dir,
            hat_id: resolution.hat_id,
            rule_id: resolution.rule_id,
        })
        .into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => internal(err),
    }
}
