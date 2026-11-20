//! Operator facade for the hook sets (skills spec §8, §9): create, list,
//! replace, delete, assign to hosts and read per-host state. Nothing is
//! pushed: a hook set takes effect at the next session start on its hosts.
use crate::{
    AppState,
    api::{error, internal},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    handler::Handler,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use hennery_kernel::{
    hook_sets::{HookSetRecord, HookSetWrite},
    json::ApiJson,
    secret::unix_now,
};
use hennery_proto::{
    frames::Capability,
    hooks::{CLAUDE_EVENTS, CODEX_EVENTS},
    rest::{
        CreateHookSetRequest, HookEventSupport, HookEventsResponse, HookSetDetail, HookSetHostState, HookSetHostStatus,
        HookSetHostsResponse, HookSetItem, ReplaceHookSetRequest, SetHookSetHostsRequest,
    },
};

/// A hook set's JSON: the largest config, and room around it.
const HOOK_SET_BODY_LIMIT: usize = 64 * 1024 + 4 * 1024;

pub fn router(state: AppState) -> Router {
    let step_up = || middleware::from_fn(hennery_kernel::auth::require_step_up);
    let routes = Router::new()
        .route(
            "/api/hook-sets",
            get(list).post(
                create
                    .layer(step_up())
                    .layer(DefaultBodyLimit::max(HOOK_SET_BODY_LIMIT)),
            ),
        )
        .route("/api/hook-sets/events", get(events))
        .route(
            "/api/hook-sets/{id}",
            get(detail)
                .put(
                    replace
                        .layer(step_up())
                        .layer(DefaultBodyLimit::max(HOOK_SET_BODY_LIMIT)),
                )
                .delete(remove.layer(step_up())),
        )
        .route(
            "/api/hook-sets/{id}/hosts",
            get(hosts).put(set_hosts.layer(step_up()).layer(DefaultBodyLimit::max(16 * 1024))),
        );
    hennery_kernel::auth::operator_only(routes, state.operator.clone())
        .layer(middleware::map_response(|mut response: Response| async move {
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("private, no-store"),
            );
            response
        }))
        .with_state(state)
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "hook_set_not_found", "no such hook set")
}

fn invalid(message: impl Into<String>) -> Response {
    error(StatusCode::BAD_REQUEST, "invalid_hook_set", message)
}

/// The two JSON writes read their body as plain JSON first, then as the
/// typed request: a config the types refuse (a `type: "prompt"` handler, an
/// unknown field) is `invalid_hook_set` with serde's words, which name the
/// problem, rather than `invalid_body`.
fn typed<T: serde::de::DeserializeOwned>(body: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(body).map_err(|err| err.to_string())
}

fn written(write: HookSetWrite) -> Response {
    match write {
        HookSetWrite::Exists => error(
            StatusCode::CONFLICT,
            "hook_set_exists",
            "a hook set with this name exists",
        ),
        HookSetWrite::TooMany => error(
            StatusCode::CONFLICT,
            "too_many_hook_sets",
            "too many hook sets are stored",
        ),
        HookSetWrite::Invalid(why) => invalid(why.to_string()),
        HookSetWrite::BadName => invalid("the name is not a valid hook set name"),
        HookSetWrite::Unknown => not_found(),
    }
}

fn item(state: &AppState, record: HookSetRecord) -> anyhow::Result<HookSetItem> {
    let host_ids = state.hosts.hook_set_hosts(&record.id)?;
    Ok(HookSetItem {
        id: record.id,
        name: record.name,
        content_hash: record.content_hash,
        events: record.events,
        host_ids,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

/// `GET /api/hook-sets`
async fn list(State(state): State<AppState>) -> Response {
    let items = state
        .hosts
        .list_hook_sets()
        .and_then(|records| records.into_iter().map(|record| item(&state, record)).collect());
    match items {
        Ok(items) => Json::<Vec<HookSetItem>>(items).into_response(),
        Err(err) => internal(err),
    }
}

/// `GET /api/hook-sets/events`: every accepted event, and whether Codex
/// runs it too.
async fn events() -> Response {
    Json(HookEventsResponse {
        events: CLAUDE_EVENTS
            .iter()
            .map(|event| HookEventSupport {
                event: (*event).to_owned(),
                claude: true,
                codex: CODEX_EVENTS.contains(event),
            })
            .collect(),
    })
    .into_response()
}

/// `POST /api/hook-sets`: 201 with the new hook set.
async fn create(State(state): State<AppState>, ApiJson(body): ApiJson<serde_json::Value>) -> Response {
    let req: CreateHookSetRequest = match typed(body) {
        Ok(req) => req,
        Err(why) => return invalid(why),
    };
    match state.hosts.create_hook_set(&req.name, &req.hooks, unix_now()) {
        Ok(Ok(record)) => match item(&state, record) {
            Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
            Err(err) => internal(err),
        },
        Ok(Err(write)) => written(write),
        Err(err) => internal(err),
    }
}

/// `GET /api/hook-sets/{id}`
async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let found = state.hosts.hook_set(&id).and_then(|found| {
        let Some((record, hooks)) = found else { return Ok(None) };
        Ok(Some(HookSetDetail {
            item: item(&state, record)?,
            hooks,
        }))
    });
    match found {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => not_found(),
        Err(err) => internal(err),
    }
}

/// `PUT /api/hook-sets/{id}`: the new `hooks` replace the old ones.
async fn replace(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<serde_json::Value>,
) -> Response {
    let req: ReplaceHookSetRequest = match typed(body) {
        Ok(req) => req,
        Err(why) => return invalid(why),
    };
    match state.hosts.replace_hook_set(&id, &req.hooks, unix_now()) {
        Ok(Ok(record)) => match item(&state, record) {
            Ok(item) => Json(item).into_response(),
            Err(err) => internal(err),
        },
        Ok(Err(write)) => written(write),
        Err(err) => internal(err),
    }
}

/// `DELETE /api/hook-sets/{id}`: 204.
async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.hosts.delete_hook_set(&id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
        Err(err) => internal(err),
    }
}

/// Each assigned host's state: `unsupported` when its record or its live
/// connection lacks `hook_sets`, `offline` with no ready connection, else
/// `assigned`.
fn host_states(state: &AppState, id: &str) -> anyhow::Result<Vec<HookSetHostState>> {
    let mut states = Vec::new();
    for host_id in state.hosts.hook_set_hosts(id)? {
        let supported = state
            .hosts
            .host(&host_id)?
            .is_some_and(|host| host.capabilities.has(Capability::HookSets));
        let live = state.hub.ready_capability(&host_id, Capability::HookSets);
        let status = if !supported || live == Some(false) {
            HookSetHostStatus::Unsupported
        } else if live.is_none() {
            HookSetHostStatus::Offline
        } else {
            HookSetHostStatus::Assigned
        };
        states.push(HookSetHostState { host_id, status });
    }
    Ok(states)
}

fn hosts_response(state: &AppState, id: &str) -> Response {
    match host_states(state, id) {
        Ok(hosts) => Json(HookSetHostsResponse {
            hook_set_id: id.to_owned(),
            hosts,
        })
        .into_response(),
        Err(err) => internal(err),
    }
}

/// `GET /api/hook-sets/{id}/hosts`
async fn hosts(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.hosts.hook_set(&id) {
        Ok(Some(_)) => hosts_response(&state, &id),
        Ok(None) => not_found(),
        Err(err) => internal(err),
    }
}

/// `PUT /api/hook-sets/{id}/hosts`: the set of hosts replaces the old one.
async fn set_hosts(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<SetHookSetHostsRequest>,
) -> Response {
    match state.hosts.hook_set(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    }
    match state.hosts.set_hook_set_hosts(&id, &req.host_ids, unix_now()) {
        Ok(Ok(())) => hosts_response(&state, &id),
        Ok(Err(HookSetWrite::Unknown)) => {
            error(StatusCode::BAD_REQUEST, "unknown_host", "a host is unknown or revoked")
        }
        Ok(Err(write)) => written(write),
        Err(err) => internal(err),
    }
}
