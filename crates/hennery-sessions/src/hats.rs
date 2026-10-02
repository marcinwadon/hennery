//! Hat endpoints (kernel spec §5, §8): the hats, each host's path rules,
//! and resolving a path to its hat. They sit beside the host endpoints, in
//! the collector's one router; the model is the kernel's
//! (`hennery_kernel::hats`), and paths are resolved by their host
//! (`resolve`).

use crate::AppState;
use crate::api::Unplaceable;
use crate::api::{close_deleted_on_host, error, internal};
use crate::resolve::{NotResolved, resolve_on_host};
use crate::store::{Deletion, HatSession, Unattached};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::handler::Handler;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router, middleware};
use futures::stream::{self, StreamExt};
use hennery_kernel::hats::{HatChange, HatRecord, MAX_RULES, NewRule, PathRule, PurgeStart, RulesChange, SessionHat};
use hennery_kernel::json::ApiJson;
use hennery_kernel::logo::{self, Refusal};
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{
    CreateHatRequest, HatItem, HatResolution, HatResolveRequest, PathRuleInput, PathRuleItem, PathRulesRequest,
    PurgePreview, PurgeResult, SetHatLogoRequest, UpdateHatRequest,
};

/// Rule prefixes resolved through the host at once, at most: matches the
/// host's own resolution semaphore (the review's P6).
const RESOLVING_AT_ONCE: usize = 4;

/// The sessions of no hat a purge's preview lists, at most (plan 9c
/// decision 11).
const UNASSIGNED_SHOWN: u32 = 100;

/// The most `PUT /api/hats/{id}/logo` reads of a body: a 64 KiB logo is
/// 87,384 bytes of base64, and its JSON a few more (plan 4d-B2).
pub const LOGO_BODY_LIMIT: usize = 96 * 1024;

/// Every route needs the operator's session. Changing a hat and replacing a
/// host's path rules need a fresh step-up too (plan 5a decision 7): the
/// rules move the boundary of every later session on that host, a new
/// default for new hosts that of every later host, and a swapped name or
/// colour would point the next stepped-up change at the wrong hat. So does
/// a purge (plan 9c decision 10), which deletes what it cannot give back;
/// its preview only reads.
pub fn router(state: AppState) -> Router {
    let routes = Router::new()
        .route("/api/hats", get(list_hats).post(create_hat))
        .route(
            "/api/hats/{id}",
            patch(update_hat).route_layer(middleware::from_fn(hennery_kernel::auth::require_step_up)),
        )
        .route(
            "/api/hats/{id}/purge",
            // As for the path rules: step-up on `post` alone.
            get(purge_preview).post(purge_hat.layer(middleware::from_fn(hennery_kernel::auth::require_step_up))),
        )
        .route(
            "/api/hats/{id}/logo",
            // Step-up on `put` and `delete` alone (`Handler::layer`, the
            // review's A5): `get` is every `<img>` of the hat.
            get(hat_logo)
                .put(
                    put_logo
                        .layer(middleware::from_fn(hennery_kernel::auth::require_step_up))
                        .layer(DefaultBodyLimit::max(LOGO_BODY_LIMIT)),
                )
                .delete(delete_logo.layer(middleware::from_fn(hennery_kernel::auth::require_step_up))),
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
        purging: record.purging,
        logo: record.logo,
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
        HatChange::Purging => error(StatusCode::CONFLICT, "hat_purging", "the hat is being purged"),
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

/// What a refused logo is answered with: a fixed `ApiError` per refusal,
/// never the decoder's words or the upload (the review's A8).
fn logo_refused(refusal: Refusal) -> Response {
    tracing::debug!(?refusal, "logo refused");
    match refusal {
        Refusal::NotBase64 => error(
            StatusCode::BAD_REQUEST,
            "invalid_logo",
            "the logo must be standard base64, padded, with nothing around it",
        ),
        Refusal::TooLarge => error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "logo_too_large",
            "a logo is at most 64 KiB, and at most 256 KiB once re-encoded",
        ),
        Refusal::Unsupported => error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_logo",
            "a logo must be a PNG: turn other images into one first",
        ),
        Refusal::Dimensions => error(
            StatusCode::BAD_REQUEST,
            "invalid_logo",
            "a logo is at most 1024 × 1024 pixels",
        ),
        Refusal::Damaged => error(
            StatusCode::BAD_REQUEST,
            "invalid_logo",
            "the logo is not a PNG that can be read",
        ),
    }
}

/// `PUT /api/hats/{id}/logo` (kernel spec §5.1): the upload re-encoded
/// (`logo::from_upload`) and stored; 200 with the hat as it is now. An
/// unknown hat is 404 before the upload is read. A hat frozen for its purge
/// is 409 `hat_purging`, checked as the logo is stored.
async fn put_logo(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<SetHatLogoRequest>,
) -> Response {
    match state.hosts.hat(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such hat"),
        Err(err) => return internal(err),
    }
    // Decoding is work for a thread of its own (the review's O1).
    let logo = match tokio::task::spawn_blocking(move || logo::from_upload(&req.data)).await {
        Ok(Ok(logo)) => logo,
        Ok(Err(refusal)) => return logo_refused(refusal),
        Err(err) => return internal(err.into()),
    };
    match state.hosts.set_hat_logo(&id, &logo) {
        Ok(change) => hat_changed(change, StatusCode::OK),
        Err(err) => internal(err),
    }
}

/// `DELETE /api/hats/{id}/logo`: 200 with the hat, with no logo now, even a
/// frozen hat's (the review's A6), or 404.
async fn delete_logo(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.hosts.clear_hat_logo(&id) {
        Ok(change) => hat_changed(change, StatusCode::OK),
        Err(err) => internal(err),
    }
}

/// Whether `If-None-Match` names `etag` (quoted), by the weak comparison
/// RFC 9110 §13.1.2 asks of it, or is `*`.
fn not_modified(request: &HeaderMap, etag: &str) -> bool {
    request
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|tags| {
            tags.split(',')
                .map(str::trim)
                .any(|tag| tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == etag)
        })
}

/// `GET /api/hats/{id}/logo` (kernel spec §5.1, §3.3: a browser `GET`,
/// behind the cookie): the logo as stored, or 404 for a hat with none. Its
/// URL is on this origin, so the answer stays an image even opened as a
/// page (the parent's addition, the review's A9): a fixed type from the kind
/// stored, never sniffed; a sandbox that runs and loads nothing, framed
/// nowhere; inline under a fixed name; for this origin only. It is private
/// and revalidated each time by its strong `ETag`: 304 when it still
/// matches, with the same headers.
async fn hat_logo(State(state): State<AppState>, Path(id): Path<String>, request: HeaderMap) -> Response {
    let stored = match state.hosts.hat_logo(&id) {
        Ok(Some(stored)) => stored,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such logo"),
        Err(err) => return internal(err),
    };
    let content_type = match stored.mime.as_str() {
        logo::MIME_PNG => HeaderValue::from_static(logo::MIME_PNG),
        _ => return internal(anyhow::anyhow!("hat {id}'s logo is of a kind not served")),
    };
    let etag = format!("\"{}\"", stored.etag);
    let Ok(etag_value) = HeaderValue::from_str(&etag) else {
        return internal(anyhow::anyhow!("hat {id}'s logo has an ETag no header can carry"));
    };
    let mut response = if not_modified(&request, &etag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, content_type)], stored.bytes).into_response()
    };
    let headers = response.headers_mut();
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(hennery_kernel::csp::LOGO_POLICY),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline; filename=\"logo.png\""),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    headers.insert(header::ETAG, etag_value);
    response
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
            // A frozen hat is no hat for a rule (plan 9c A2); the registry
            // refuses it again in its own transaction.
            Ok(Some(hat)) if !hat.purging => {}
            Ok(_) => return error(StatusCode::BAD_REQUEST, "invalid", format!("no hat {:?}", rule.hat_id)),
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
/// and the hat a session there would get. Where a start would be refused
/// `hat_ambiguous`, so is this (the review's P1): the hat tester must agree
/// with the start.
async fn resolve_hat(State(state): State<AppState>, ApiJson(req): ApiJson<HatResolveRequest>) -> Response {
    let on_host = match resolve_on_host(&state, &req.host_id, &req.path).await {
        Ok(on_host) => on_host,
        Err(why) => return why.into_response(),
    };
    match state.hosts.session_hat(&req.host_id, &on_host.canonical) {
        Ok(Some(SessionHat::Ambiguous(rule))) => Unplaceable::Ambiguous(rule.prefix).into_response(),
        Ok(Some(SessionHat::Decided(resolution))) => Json(HatResolution {
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

/// What the sessions module's part of a purge deleted (plan 9c A13).
#[derive(Debug, Default, PartialEq)]
pub struct PurgedSessions {
    pub deleted: u64,
    /// Closed collector-side while their host may still run them.
    pub unconfirmed: Vec<String>,
    /// What each delete left for its host to remove (plan 9d decision 2),
    /// by session.
    pub forgets: Vec<(String, crate::store::HostForgets)>,
}

/// Sessions of a hat being purged that may have an adapter the collector
/// can reach, or that changed while the purge read them (plan 9c decision
/// 10d): the purge stops, the hat stays frozen, and a purge again resumes
/// once they are closed.
#[derive(Debug, PartialEq)]
pub struct SessionsRunning(pub Vec<String>);

impl std::fmt::Display for SessionsRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "close these sessions first: {}", self.0.join(", "))
    }
}

impl std::error::Error for SessionsRunning {}

/// Whether `session` may have an adapter the collector reaches (plan 9c
/// decision 10b): `starting` or `active` on a host connected and
/// reconciled. One presumed parked, or starting or active on a host away
/// or revoked, is closed collector-side instead, as a delete closes it
/// (plan 9a decision 5).
fn running(state: &AppState, session: &HatSession) -> bool {
    matches!(session.lifecycle.as_str(), "starting" | "active") && state.hub.is_ready(&session.host_id)
}

fn running_ids(state: &AppState, sessions: &[HatSession]) -> Vec<String> {
    sessions
        .iter()
        .filter(|session| running(state, session))
        .map(|session| session.id.clone())
        .collect()
}

/// Delete every kept session of `hat_id`, frozen for its purge (plan 9c
/// decision 10d): each as a delete does (plan 9a decision 5), in a
/// transaction of its own, so the store's lock is not held across hundreds;
/// its `session_deleted` goes to its streams, which end on it (A10). One not
/// closed is closed collector-side first, only while it is still what was
/// read here (A4). Any that may be running, or that changed since, stops it
/// with `SessionsRunning`: what was deleted stays deleted, and running it
/// again goes on from there. The deletes leave the WAL's checkpoint to the
/// caller, once after the last (`Store::owe_checkpoint`, `Store::checkpoint`).
pub fn purge_sessions(state: &AppState, hat_id: &str) -> anyhow::Result<PurgedSessions> {
    let sessions = state.store.hat_sessions(hat_id)?;
    let found = running_ids(state, &sessions);
    if !found.is_empty() {
        return Err(SessionsRunning(found).into());
    }
    let mut purged = PurgedSessions::default();
    for session in sessions {
        // Its host may have come back since the read above, with hundreds
        // of deletes in between: asked again, just before its own. Defence
        // in depth: one that returns between this and the delete is closed
        // by `delete_as_read`, or by its connection (decision 4).
        if running(state, &session) {
            return Err(SessionsRunning(vec![session.id]).into());
        }
        delete_as_read(state, session, &mut purged)?;
    }
    Ok(purged)
}

/// Delete one session of a purge, as `session` was read and judged not
/// running (plan 9c decision 10d; plan 9a decision 5, A4), counting it in
/// `purged`. Its host may have reconciled after that judgement and be
/// ready by the commit, as a delete's may: one deleted `unconfirmed` is
/// then sent `close_session` here, as `api::finish_delete` sends it; if the
/// commit came before its host was ready, its connection finds the
/// tombstone once it is (`ws::ready`, after `mark_ready`).
pub(crate) fn delete_as_read(state: &AppState, session: HatSession, purged: &mut PurgedSessions) -> anyhow::Result<()> {
    let unattached = (session.lifecycle != "closed").then(|| Unattached {
        lifecycle: session.lifecycle.clone(),
        presumed_parked: session.presumed_parked,
    });
    match state
        .store
        .delete_session_owing_checkpoint(&session.id, unattached.as_ref())?
    {
        Deletion::Done {
            event,
            unconfirmed,
            forgets,
        } => {
            state.hub.publish(event);
            purged.deleted += 1;
            purged.forgets.push((session.id.clone(), *forgets));
            if unconfirmed {
                close_deleted_on_host(state, &session.host_id, &session.id);
                purged.unconfirmed.push(session.id);
            }
        }
        // A resume, or its host back with it, since it was read.
        Deletion::Refused(_) => return Err(SessionsRunning(vec![session.id]).into()),
        // Deleted meanwhile, by a delete of its own.
        Deletion::NotFound => {}
    }
    Ok(())
}

fn sessions_running(ids: Vec<String>) -> Response {
    error(
        StatusCode::CONFLICT,
        "sessions_running",
        SessionsRunning(ids).to_string(),
    )
}

fn hat_is_default() -> Response {
    error(
        StatusCode::CONFLICT,
        "hat_is_default",
        "the hat is the default for new hosts or a host's default hat: make another hat that default first",
    )
}

/// `GET /api/hats/{id}/purge` (plan 9c decision 11): what a purge of the hat
/// would delete, and what stops it now. Reads only.
async fn purge_preview(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let hat = match state.hosts.hat(&id) {
        Ok(Some(hat)) => hat,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such hat"),
        Err(err) => return internal(err),
    };
    let preview = (|| -> anyhow::Result<PurgePreview> {
        let sessions = state.store.hat_sessions(&id)?;
        let (rules, recents) = state.hosts.purge_counts(&id)?;
        let (unassigned, unassigned_count) = state.store.unassigned(UNASSIGNED_SHOWN)?;
        Ok(PurgePreview {
            hat_id: id.clone(),
            purging: hat.purging,
            sessions: sessions.len() as u64,
            running: running_ids(&state, &sessions),
            rules,
            recents,
            unassigned,
            unassigned_count,
        })
    })();
    match preview {
        Ok(preview) => Json(preview).into_response(),
        Err(err) => internal(err),
    }
}

/// `POST /api/hats/{id}/purge` (kernel spec §5.5; plan 9c decision 10): 200
/// with what it deleted. 404 for an unknown hat, or one whose purge is done;
/// 409 `hat_is_default` for a default hat (a), and 409 `sessions_running`,
/// with their ids, while any of its sessions may run where the collector
/// can reach (b): nothing changed. Then the hat is frozen and its rules go
/// (c); the modules delete what they keep of it (d); and the hat's row goes,
/// its recents with it (e). Anything that stops it after the freeze leaves
/// the hat frozen, and a purge again resumes it (A12).
async fn purge_hat(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.hosts.hat(&id) {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such hat"),
        Err(err) => return internal(err),
    }
    // (a) before (b); the freeze checks it again in its transaction.
    match state.hosts.is_default_hat(&id) {
        Ok(false) => {}
        Ok(true) => return hat_is_default(),
        Err(err) => return internal(err),
    }
    let running = match state.store.hat_sessions(&id) {
        Ok(sessions) => running_ids(&state, &sessions),
        Err(err) => return internal(err),
    };
    if !running.is_empty() {
        return sessions_running(running);
    }
    let rules = match state.hosts.begin_purge(&id, unix_now()) {
        Ok(PurgeStart::Frozen { rules }) => rules,
        Ok(PurgeStart::NotFound) => return error(StatusCode::NOT_FOUND, "not_found", "no such hat"),
        Ok(PurgeStart::IsDefault) => return hat_is_default(),
        Err(err) => return internal(err),
    };
    tracing::info!(hat_id = %id, "hat frozen for its purge");
    // plan 8: the gateway's `on_hat_purged` runs here, before the sessions'
    // (A15), so a hat stuck frozen cannot reach MCP meanwhile.
    // One checkpoint for the whole purge, on every way out from here (plan
    // 9a A8): owed before the first delete, so a crash leaves it owed.
    state.store.owe_checkpoint();
    let purged = purge_sessions(&state, &id).and_then(|purged| {
        // `false`: a purge alongside got there first; it is done either way.
        state.hosts.finish_purge(&id)?;
        Ok(purged)
    });
    state.store.checkpoint();
    let purged = match purged {
        Ok(purged) => purged,
        Err(err) => {
            return match err.downcast::<SessionsRunning>() {
                Ok(SessionsRunning(ids)) => sessions_running(ids),
                Err(err) => internal(err),
            };
        }
    };
    tracing::info!(hat_id = %id, sessions = purged.deleted, rules, "hat purged");
    // The agents' own transcripts on the hosts, best effort, within one
    // wait for the whole purge (plan 9d decision 7, O10).
    let host_transcripts = crate::forget::after_purge(&state, &purged.forgets).await;
    Json(PurgeResult {
        sessions: purged.deleted,
        rules,
        unconfirmed: purged.unconfirmed,
        host_transcripts,
    })
    .into_response()
}
