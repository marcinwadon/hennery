//! Operator facade for the skills catalogue (skills spec §5): upload, list,
//! replace, delete, assign to hosts and read per-host delivery state. Every
//! write ends by pushing the new manifest to the connected hosts.
use crate::{
    AppState,
    api::{error, internal},
    skills_sync::broadcast_skills,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    handler::Handler,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, put},
};
use hennery_kernel::{
    json::ApiJson,
    secret::unix_now,
    skill_archive::{self, MAX_COMPRESSED, Refusal},
    skills::{SkillRecord, SkillWrite},
};
use hennery_proto::{
    frames::Capability,
    rest::{
        CreateSkillRequest, HatHostsResponse, ReplaceSkillArchiveRequest, SetSkillHostsRequest, SkillDetail,
        SkillHostState, SkillHostStatus, SkillHostsResponse, SkillItem,
    },
    skills::FrontmatterError,
};

/// A skill upload: the largest archive as base64, and room for the JSON around it.
const SKILL_BODY_LIMIT: usize = MAX_COMPRESSED.div_ceil(3) * 4 + 1024;

pub fn router(state: AppState) -> Router {
    let step_up = || middleware::from_fn(hennery_kernel::auth::require_step_up);
    let routes = Router::new()
        .route(
            "/api/skills",
            get(list).post(create.layer(step_up()).layer(DefaultBodyLimit::max(SKILL_BODY_LIMIT))),
        )
        .route("/api/skills/hat-hosts/{hat_id}", get(hat_hosts))
        .route("/api/skills/{id}", get(detail).delete(remove.layer(step_up())))
        .route(
            "/api/skills/{id}/archive",
            put(replace.layer(step_up()).layer(DefaultBodyLimit::max(SKILL_BODY_LIMIT))),
        )
        .route(
            "/api/skills/{id}/hosts",
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
    error(StatusCode::NOT_FOUND, "skill_not_found", "no such skill")
}

fn invalid(message: impl Into<String>) -> Response {
    error(StatusCode::BAD_REQUEST, "invalid_skill", message)
}

/// What a refused upload is answered with: the entry or the frontmatter
/// problem, never the archive's bytes.
fn refused(refusal: Refusal) -> Response {
    tracing::debug!(?refusal, "skill refused");
    match refusal {
        Refusal::NotBase64 => invalid("the archive must be standard base64, padded, with nothing around it"),
        Refusal::TooLarge => error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "skill_too_large",
            "a skill archive is at most 8 MiB compressed and 8 MiB unpacked",
        ),
        Refusal::NotTarGz => invalid("the archive is not a readable tar.gz"),
        Refusal::Entry(why) => invalid(format!("the archive has an entry that is not allowed: {why}")),
        Refusal::Trailing => invalid("the archive has bytes after its gzip stream"),
        Refusal::NoSkillMd => invalid(
            "the archive has no SKILL.md at its root; most often the skill is wrapped in a top-level \
             directory (pdf/SKILL.md): archive the directory's contents instead, as in tar czf pdf.tgz -C pdf .",
        ),
        Refusal::Frontmatter(why) => invalid(format!("SKILL.md frontmatter: {}", frontmatter_problem(why))),
    }
}

fn frontmatter_problem(why: FrontmatterError) -> &'static str {
    match why {
        FrontmatterError::Missing => "the file must start with a --- line",
        FrontmatterError::Unterminated => "the opening --- has no closing --- line",
        FrontmatterError::NoName => "no name",
        FrontmatterError::NoDescription => "no description",
        FrontmatterError::DescriptionTooLong => "the description is too long",
        FrontmatterError::BadName => "the name is not a valid skill name",
    }
}

fn written(write: SkillWrite) -> Response {
    match write {
        SkillWrite::Exists => error(StatusCode::CONFLICT, "skill_exists", "a skill with this name exists"),
        SkillWrite::TooMany => error(StatusCode::CONFLICT, "too_many_skills", "too many skills are stored"),
        SkillWrite::NameMismatch => error(
            StatusCode::CONFLICT,
            "skill_name_mismatch",
            "the new SKILL.md names a different skill",
        ),
        SkillWrite::Unknown => not_found(),
    }
}

fn item_of(record: SkillRecord, host_ids: Vec<String>) -> SkillItem {
    SkillItem {
        id: record.id,
        name: record.name,
        description: record.description,
        content_hash: record.content_hash,
        host_ids,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn item(state: &AppState, record: SkillRecord) -> anyhow::Result<SkillItem> {
    let host_ids = state.hosts.skill_hosts(&record.id)?;
    Ok(item_of(record, host_ids))
}

/// Push the manifest after a write; the write stands if the push fails.
fn broadcast(state: &AppState) {
    if let Err(err) = broadcast_skills(&state.hosts, &state.hub) {
        tracing::warn!(error = %err, "could not push the skills to the hosts");
    }
}

/// Decode and inspect off the async threads. The `Err` is the HTTP
/// response itself, returned once per request, so its size does not matter.
#[allow(clippy::result_large_err)]
async fn inspect(data: String) -> Result<(Vec<u8>, skill_archive::Inspected), Response> {
    match tokio::task::spawn_blocking(move || skill_archive::from_upload(&data)).await {
        Ok(Ok(done)) => Ok(done),
        Ok(Err(refusal)) => Err(refused(refusal)),
        Err(err) => Err(internal(err.into())),
    }
}

/// `GET /api/skills`
async fn list(State(state): State<AppState>) -> Response {
    let items = state
        .hosts
        .list_skills()
        .and_then(|records| records.into_iter().map(|record| item(&state, record)).collect());
    match items {
        Ok(items) => Json::<Vec<SkillItem>>(items).into_response(),
        Err(err) => internal(err),
    }
}

/// `POST /api/skills`: 201 with the new skill.
async fn create(State(state): State<AppState>, ApiJson(req): ApiJson<CreateSkillRequest>) -> Response {
    let (archive, inspected) = match inspect(req.data).await {
        Ok(done) => done,
        Err(response) => return response,
    };
    match state.hosts.create_skill(&inspected, &archive, unix_now()) {
        Ok(Ok(record)) => match item(&state, record) {
            Ok(item) => {
                broadcast(&state);
                (StatusCode::CREATED, Json(item)).into_response()
            }
            Err(err) => internal(err),
        },
        Ok(Err(write)) => written(write),
        Err(err) => internal(err),
    }
}

/// `GET /api/skills/{id}`
async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let found = state.hosts.skill(&id).and_then(|record| {
        let Some(record) = record else { return Ok(None) };
        let skill_md = state.hosts.skill_md(&id)?.unwrap_or_default();
        Ok(Some(SkillDetail {
            item: item(&state, record)?,
            skill_md,
        }))
    });
    match found {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => not_found(),
        Err(err) => internal(err),
    }
}

/// `PUT /api/skills/{id}/archive`: an unknown skill is 404 before the upload
/// is decoded.
async fn replace(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<ReplaceSkillArchiveRequest>,
) -> Response {
    match state.hosts.skill(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    }
    let (archive, inspected) = match inspect(req.data).await {
        Ok(done) => done,
        Err(response) => return response,
    };
    match state.hosts.replace_skill_archive(&id, &inspected, &archive, unix_now()) {
        Ok(Ok(record)) => match item(&state, record) {
            Ok(item) => {
                broadcast(&state);
                Json(item).into_response()
            }
            Err(err) => internal(err),
        },
        Ok(Err(write)) => written(write),
        Err(err) => internal(err),
    }
}

/// `DELETE /api/skills/{id}`: 204.
async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.hosts.delete_skill(&id) {
        Ok(true) => {
            broadcast(&state);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => not_found(),
        Err(err) => internal(err),
    }
}

/// Each assigned host's state from its latest report; a report covers the
/// host's whole set, so a skill it does not list was assigned after it. A
/// report older than the owner's current revision says nothing about the
/// current content (`pending`); a host with no ready connection is
/// `offline`, whatever it last reported.
fn host_states(state: &AppState, id: &str) -> anyhow::Result<Vec<SkillHostState>> {
    let revision = state.hosts.skills_revision()?;
    let mut states = Vec::new();
    for host_id in state.hosts.skill_hosts(id)? {
        let supported = state
            .hosts
            .host(&host_id)?
            .is_some_and(|host| host.capabilities.has(Capability::Skills));
        let live = state.hub.ready_capability(&host_id, Capability::Skills);
        let report = state.hosts.skill_state(&host_id)?;
        let mut entry = SkillHostState {
            host_id,
            status: SkillHostStatus::Unknown,
            failure: None,
            reported_at: None,
        };
        if let Some(report) = &report {
            entry.reported_at = Some(report.reported_at);
        }
        if !supported || live == Some(false) {
            entry.status = SkillHostStatus::Unsupported;
        } else if live.is_none() {
            entry.status = SkillHostStatus::Offline;
        } else if let Some(report) = report {
            if report.revision < revision {
                entry.status = SkillHostStatus::Pending;
            } else if report.applied.iter().any(|applied| applied == id) {
                entry.status = SkillHostStatus::Applied;
            } else if let Some(failed) = report.failed.into_iter().find(|failed| failed.id == id) {
                entry.status = SkillHostStatus::Failed;
                entry.failure = Some(failed);
            } else {
                entry.status = SkillHostStatus::Pending;
            }
        }
        states.push(entry);
    }
    Ok(states)
}

fn hosts_response(state: &AppState, id: &str) -> Response {
    match host_states(state, id) {
        Ok(hosts) => Json(SkillHostsResponse {
            skill_id: id.to_owned(),
            hosts,
        })
        .into_response(),
        Err(err) => internal(err),
    }
}

/// `GET /api/skills/{id}/hosts`
async fn hosts(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.hosts.skill(&id) {
        Ok(Some(_)) => hosts_response(&state, &id),
        Ok(None) => not_found(),
        Err(err) => internal(err),
    }
}

/// `PUT /api/skills/{id}/hosts`: the set of hosts replaces the old one.
async fn set_hosts(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<SetSkillHostsRequest>,
) -> Response {
    match state.hosts.skill(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    }
    match state.hosts.set_skill_hosts(&id, &req.host_ids, unix_now()) {
        Ok(Ok(())) => {
            broadcast(&state);
            hosts_response(&state, &id)
        }
        Ok(Err(SkillWrite::Unknown)) => error(StatusCode::BAD_REQUEST, "unknown_host", "a host is unknown or revoked"),
        Ok(Err(write)) => written(write),
        Err(err) => internal(err),
    }
}

/// `GET /api/skills/hat-hosts/{hat_id}`
async fn hat_hosts(State(state): State<AppState>, Path(hat_id): Path<String>) -> Response {
    match state.store.hosts_of_hat(&hat_id) {
        Ok(host_ids) => Json(HatHostsResponse { host_ids }).into_response(),
        Err(err) => internal(err),
    }
}
