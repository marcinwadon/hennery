//! Immutable session outputs, shared by operator UI and session-bound MCP.
use crate::{
    AppState,
    api::{error, internal},
    store::{Store, collector_event, now},
};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hennery_gateway::identity::SessionIdentity;
use hennery_kernel::json::ApiJson;
use hennery_proto::{
    files::{ArtifactRevision, MAX_ARTIFACT_BYTES, PublishArtifact},
    frames::{CollectorFrame, HostFrame},
    rest::{EventDto, StoredBlock},
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn read_revision(r: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRevision> {
    Ok(ArtifactRevision {
        artifact_id: r.get(0)?,
        session_id: r.get(1)?,
        revision: r.get(2)?,
        title: r.get(3)?,
        name: r.get(4)?,
        mime_type: r.get(5)?,
        sha256: r.get(6)?,
        size: r.get::<_, i64>(7)? as u64,
        created_at: r.get(8)?,
    })
}
const COLUMNS: &str = "artifact_id,session_id,revision,title,name,mime_type,sha256,size,created_at";
fn request_digest(q: &PublishArtifact) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(q).expect("typed request")))
}
fn scope(tx: &Transaction<'_>, owner: &str, session: &str, expected: Option<(&str, &str)>) -> Result<()> {
    let row: Option<(String,String)> = tx.query_row(
        "SELECT s.host_id,s.hat_id FROM sessions s JOIN hosts h ON h.owner_id = ?1 AND h.id=s.host_id
         WHERE s.owner_id = ?1 AND s.id=?2 AND s.lifecycle<>'deleted'
         AND h.revoked_at IS NULL AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id=s.hat_id)",
        params![owner,session], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (host, hat) = row.context("not_found")?;
    ensure!(expected.is_none_or(|(h, t)| h == host && t == hat), "session_changed");
    Ok(())
}
pub(crate) fn queue_cleanup_in(tx: &Transaction<'_>, owner: &str, session: &str, host: &str) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO file_cache_cleanup(owner_id,session_id,host_id)
        SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM turns t JOIN turn_attachments a ON a.owner_id = ?1 AND a.turn_id=t.turn_id
        WHERE t.owner_id = ?1 AND t.session_id=?2 AND json_extract(t.content,'$[' || a.position || '].type')='file')",
        params![owner, session, host],
    )?;
    Ok(())
}
impl Store {
    pub fn file_cleanup_pending(&self, host: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT session_id FROM file_cache_cleanup WHERE owner_id = ?1 AND host_id=?2 ORDER BY session_id LIMIT 32",
        )?;
        Ok(stmt
            .query_map(params![self.owner_id(), host], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn file_cleanup_done(&self, host: &str, session: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM file_cache_cleanup WHERE owner_id = ?1 AND host_id=?2 AND session_id=?3",
            params![self.owner_id(), host, session],
        )?;
        Ok(())
    }
    pub fn artifact_page(&self, session: &str, cursor: Option<&str>) -> Result<hennery_proto::files::ArtifactPage> {
        ensure!(
            cursor.is_none_or(|c| uuid::Uuid::parse_str(c).is_ok()),
            "invalid_arguments"
        );
        // Session access is checked even for an empty page.
        self.artifact_revisions(session, Some(""), 0)?;
        let conn = self.conn();
        let sql = format!(
            "SELECT {COLUMNS} FROM artifact_revisions a WHERE a.owner_id = ?1 AND a.session_id=?2 AND (?3 IS NULL OR a.artifact_id<?3) AND a.revision=(SELECT MAX(r.revision) FROM artifact_revisions r WHERE r.owner_id = ?1 AND r.artifact_id=a.artifact_id) ORDER BY a.artifact_id DESC LIMIT 51"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut artifacts = stmt
            .query_map(params![self.owner_id(), session, cursor], read_revision)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let more = artifacts.len() > 50;
        artifacts.truncate(50);
        let next_cursor = more.then(|| artifacts.last().unwrap().artifact_id.clone());
        Ok(hennery_proto::files::ArtifactPage { artifacts, next_cursor })
    }
    pub fn artifact_retry(&self, session: &str, q: &PublishArtifact) -> Result<Option<ArtifactRevision>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        scope(&tx, self.owner_id(), session, None)?;
        artifact_retry_in(&tx, self.owner_id(), session, q)
    }
    pub fn artifact_revisions(
        &self,
        session: &str,
        artifact: Option<&str>,
        before: u32,
    ) -> Result<Vec<ArtifactRevision>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        ensure!(tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE owner_id = ?1 AND id=?2 AND lifecycle<>'deleted' AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id=sessions.hat_id))",params![self.owner_id(),session],|r|r.get::<_,bool>(0))?,"not_found");
        let sql = format!(
            "SELECT {COLUMNS} FROM artifact_revisions a WHERE a.owner_id = ?1 AND a.session_id=?2 AND (?3 IS NULL OR a.artifact_id=?3) AND a.revision<?4 ORDER BY a.artifact_id DESC,a.revision DESC LIMIT 51"
        );
        let mut stmt = tx.prepare(&sql)?;
        Ok(stmt
            .query_map(params![self.owner_id(), session, artifact, before], read_revision)?
            .collect::<rusqlite::Result<_>>()?)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn publish_artifact(
        &self,
        session: &str,
        host: &str,
        hat: &str,
        q: &PublishArtifact,
        name: &str,
        bytes: &[u8],
        identity: Option<&SessionIdentity>,
    ) -> Result<(ArtifactRevision, Option<EventDto>)> {
        ensure!(
            q.valid() && hennery_proto::files::valid_name(name) && bytes.len() <= MAX_ARTIFACT_BYTES,
            "invalid_arguments"
        );
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        scope(&tx, self.owner_id(), session, Some((host, hat)))?;
        if let Some(identity) = identity {
            let caller = self.file_tool_context_in(&tx, identity, true)?.context("not_found")?;
            ensure!(
                caller.session_id() == session && caller.host_id() == host && caller.hat_id() == hat,
                "not_found"
            );
        }
        if let Some(old) = artifact_retry_in(&tx, self.owner_id(), session, q)? {
            return Ok((old, None));
        }
        let id = q
            .artifact_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let mut inherited_title = None;
        let revision = if let Some(expected) = q.expected_revision {
            let current:Option<(u32,String)>=tx.query_row("SELECT r.revision,r.title FROM artifact_revisions r JOIN session_artifacts a ON a.owner_id = ?1 AND a.id=r.artifact_id WHERE r.owner_id = ?1 AND a.session_id=?2 AND a.id=?3 ORDER BY r.revision DESC LIMIT 1",params![self.owner_id(),session,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            ensure!(
                current.as_ref().is_some_and(|(revision, _)| *revision == expected),
                "artifact_revision_conflict"
            );
            inherited_title = current.map(|(_, title)| title);
            expected + 1
        } else {
            tx.execute(
                "INSERT INTO session_artifacts(owner_id,session_id,id) VALUES(?1,?2,?3)",
                params![self.owner_id(), session, id],
            )?;
            1
        };
        let mime = q.mime_type.clone().unwrap_or_else(|| mime_for(name).into());
        let digest = hex::encode(Sha256::digest(bytes));
        let at = now();
        let dir = self.attachments.as_deref().context("file_storage_unavailable")?;
        crate::attachments::write(dir, &digest, bytes)?;
        tx.execute("INSERT INTO attachments(owner_id,sha256,mime,size,created_at)VALUES(?1,?2,?3,?4,?5) ON CONFLICT(owner_id,sha256) DO NOTHING",params![self.owner_id(),digest,mime,bytes.len() as i64,at])?;
        let result = ArtifactRevision {
            artifact_id: id,
            session_id: session.into(),
            revision,
            title: q.title.clone().or(inherited_title).unwrap_or_else(|| name.into()),
            name: name.into(),
            mime_type: mime,
            sha256: digest,
            size: bytes.len() as u64,
            created_at: at.clone(),
        };
        tx.execute("INSERT INTO artifact_revisions(owner_id,session_id,artifact_id,revision,title,name,mime_type,sha256,size,created_at,request_key,request_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![self.owner_id(),session,result.artifact_id,revision,result.title,name,result.mime_type,result.sha256,bytes.len() as i64,at,q.request_key,request_digest(q)])?;
        let event = collector_event(
            &tx,
            self.owner_id(),
            session,
            "artifact_published",
            serde_json::to_value(&result)?,
            &at,
        )?;
        tx.commit()?;
        Ok((result, Some(event)))
    }
    pub fn session_file_inputs(&self, session: &str) -> Result<Vec<StoredBlock>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT DISTINCT json_extract(t.content,'$[' || a.position || ']') FROM turns t JOIN turn_attachments a ON a.owner_id = ?1 AND a.turn_id=t.turn_id WHERE t.owner_id = ?1 AND t.session_id=?2 LIMIT 501")?;
        let rows = stmt
            .query_map(params![self.owner_id(), session], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|s| Ok(serde_json::from_str(&s)?)).collect()
    }
}
pub fn start_cleanup(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            tokio::select! { ()=state.shutdown.cancelled()=>break, _=tick.tick()=>{} }
            for host in state.hub.connected_hosts() {
                if !state
                    .hub
                    .has_capability(&host, hennery_proto::frames::Capability::Files)
                {
                    continue;
                }
                let Ok(pending) = state.store.file_cleanup_pending(&host) else {
                    continue;
                };
                for session in pending {
                    let request_id = uuid::Uuid::now_v7().to_string();
                    let result = tokio::select! {
                        ()=state.shutdown.cancelled()=>return,
                        result=state.hub.probe(&host,&request_id,CollectorFrame::ForgetFiles{request_id:request_id.clone(),session_id:session.clone()},state.probe_timeout)=>result
                    };
                    if matches!(result, Ok(HostFrame::FilesForgotten { .. })) {
                        let _ = state.store.file_cleanup_done(&host, &session);
                    }
                }
            }
        }
    });
}
fn artifact_retry_in(
    tx: &Transaction<'_>,
    owner: &str,
    session: &str,
    q: &PublishArtifact,
) -> Result<Option<ArtifactRevision>> {
    let sql = format!(
        "SELECT {COLUMNS},request_digest FROM artifact_revisions WHERE owner_id = ?1 AND session_id=?2 AND request_key=?3"
    );
    let row = tx
        .query_row(&sql, params![owner, session, q.request_key], |r| {
            Ok((read_revision(r)?, r.get::<_, String>(9)?))
        })
        .optional()?;
    if let Some((revision, digest)) = row {
        ensure!(digest == request_digest(q), "request_conflict");
        Ok(Some(revision))
    } else {
        Ok(None)
    }
}
fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "md" | "markdown" => "text/markdown",
        "txt" | "log" | "rs" | "py" | "ts" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
}
pub fn tools() -> Value {
    json!({"tools":[
        {"name":"artifact_publish","description":"Publish a completed output file to this session so the operator can download it after the host disconnects. Paths are relative to this session working directory (absolute paths beneath it also work); regular files only, no symlinks, up to 16 MiB. Returns artifact_id and revision. To update the SAME artifact, supply its artifact_id and expected_revision from the last result. Each update creates an immutable revision; old links keep their original bytes. Reuse request_key for retries of exactly the same request. Do not publish intermediate or private files unless requested.","inputSchema":hennery_proto::files::publish_schema()},
        {"name":"artifact_list","description":"List the latest revision of this session's published artifacts, 50 per page. Follow next_cursor as cursor. Use returned artifact_id and revision when publishing an update.","inputSchema":hennery_proto::files::list_schema()}
    ]})
}
pub fn error_message(error: &anyhow::Error) -> &'static str {
    match crate::session_tools::error_code(error) {
        "host_offline" => {
            "The session host is offline; reconnect it before publishing. Existing artifact revisions remain available."
        }
        "files_unsupported" => "This host needs an update before publishing files.",
        "file_unavailable" | "file_too_large" => {
            "Use a regular file beneath this session's working directory, at most 16 MiB. Symlinks are not followed."
        }
        "artifact_revision_conflict" => {
            "The artifact changed. Read artifact_list, review its latest revision and publish with that expected_revision and a new request_key."
        }
        "request_conflict" => {
            "The request_key already identifies a different publication. Retry the original arguments, or use a new key for a new publication."
        }
        "invalid_arguments" => {
            "Provide path and request_key. For an update, provide artifact_id and expected_revision from artifact_list."
        }
        _ => "File publication is unavailable for this session.",
    }
}
pub async fn execute(state: &AppState, identity: &SessionIdentity, name: &str, args: Value) -> Result<Value> {
    let context = state.store.file_tool_context(identity, true)?.context("not_found")?;
    ensure!(
        state
            .hub
            .has_capability(context.host_id(), hennery_proto::frames::Capability::Files),
        "files_unsupported"
    );
    match name {
        "artifact_publish" => {
            let q: PublishArtifact = serde_json::from_value(args).context("invalid_arguments")?;
            Ok(serde_json::to_value(
                publish(state, context.session_id(), q, Some(identity)).await?,
            )?)
        }
        "artifact_list" => {
            let q: hennery_proto::files::ArtifactListArgs =
                serde_json::from_value(args).context("invalid_arguments")?;
            Ok(serde_json::to_value(
                state.store.artifact_page(context.session_id(), q.cursor.as_deref())?,
            )?)
        }
        _ => anyhow::bail!("invalid_arguments"),
    }
}
pub async fn publish(
    state: &AppState,
    session: &str,
    q: PublishArtifact,
    identity: Option<&SessionIdentity>,
) -> Result<ArtifactRevision> {
    ensure!(q.valid(), "invalid_arguments");
    let s = state.store.find_session(session)?.context("not_found")?;
    if let Some(old) = state.store.artifact_retry(session, &q)? {
        return Ok(old);
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ReadFile {
        request_id: request_id.clone(),
        cwd: s.cwd.clone(),
        path: q.path.clone(),
    };
    let reply = state
        .hub
        .probe(&s.host_id, &request_id, frame, state.probe_timeout)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                match e {
                    crate::hub::RequestError::NotConnected => "host_offline",
                    crate::hub::RequestError::Unsupported => "files_unsupported",
                    _ => "file_unavailable",
                }
            )
        })?;
    let HostFrame::File { name, data, .. } = reply else {
        anyhow::bail!("file_unavailable")
    };
    ensure!(data.len() <= MAX_ARTIFACT_BYTES.div_ceil(3) * 4, "file_too_large");
    let bytes = STANDARD.decode(data).context("file_unavailable")?;
    let (result, event) = state
        .store
        .publish_artifact(session, &s.host_id, &s.hat_id, &q, &name, &bytes, identity)?;
    if let Some(event) = event {
        state.hub.publish(event)
    }
    Ok(result)
}
pub fn router(state: AppState) -> Router {
    let routes = Router::new()
        .route("/api/sessions/{id}/files", get(list).post(publish_route))
        .route("/api/sessions/{id}/artifacts/{artifact}", get(history))
        .route("/api/sessions/{id}/artifacts/{artifact}/{revision}", get(download));
    hennery_kernel::auth::operator_only(routes, state.operator.clone()).with_state(state)
}
fn failure(err: anyhow::Error) -> Response {
    let code = err.to_string();
    let (status, message) = match code.as_str() {
        "not_found" => (StatusCode::NOT_FOUND, "No such session or artifact."),
        "invalid_arguments" => (
            StatusCode::BAD_REQUEST,
            "Give a path and retry key; updates need artifact_id and expected_revision.",
        ),
        "artifact_revision_conflict" => (
            StatusCode::CONFLICT,
            "This artifact changed. Refresh its latest revision before publishing.",
        ),
        "request_conflict" => (
            StatusCode::CONFLICT,
            "This retry key was used for a different publication.",
        ),
        "files_unsupported" => (
            StatusCode::CONFLICT,
            "This host needs an update before publishing files.",
        ),
        "host_offline" => (
            StatusCode::CONFLICT,
            "The host is offline. Existing published files remain downloadable.",
        ),
        "file_unavailable" | "file_too_large" => (
            StatusCode::BAD_REQUEST,
            "Read a regular file below the session working directory, up to 16 MiB. Symlinks are not followed.",
        ),
        "session_changed" => (
            StatusCode::CONFLICT,
            "The session moved while the file was being published. Retry after refreshing.",
        ),
        _ => return internal(err),
    };
    error(status, &code, message)
}
async fn publish_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(q): ApiJson<PublishArtifact>,
) -> Response {
    match publish(&state, &id, q, None).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => failure(e),
    }
}
async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<hennery_proto::files::ArtifactListArgs>,
) -> Response {
    match (
        state.store.artifact_page(&id, q.cursor.as_deref()),
        state.store.session_file_inputs(&id),
    ) {
        (Ok(page), Ok(mut attachments)) => {
            let attachments_truncated = attachments.len() > 500;
            attachments.truncate(500);
            Json(json!({"artifacts":page.artifacts,"next_cursor":page.next_cursor,"attachments":attachments,"attachments_truncated":attachments_truncated})).into_response()
        }
        (Err(e), _) | (_, Err(e)) => failure(e),
    }
}
#[derive(Deserialize)]
struct Before {
    before: Option<u32>,
}
async fn history(
    State(state): State<AppState>,
    Path((id, artifact)): Path<(String, String)>,
    Query(q): Query<Before>,
) -> Response {
    match state
        .store
        .artifact_revisions(&id, Some(&artifact), q.before.unwrap_or(u32::MAX))
    {
        Ok(mut r) => {
            let more = r.len() > 50;
            r.truncate(50);
            let next_before = more.then(|| r.last().unwrap().revision);
            Json(json!({"revisions":r,"next_before":next_before})).into_response()
        }
        Err(e) => failure(e),
    }
}
async fn download(
    State(state): State<AppState>,
    Path((id, artifact, revision)): Path<(String, String, u32)>,
) -> Response {
    let rows = match state
        .store
        .artifact_revisions(&id, Some(&artifact), revision.saturating_add(1))
    {
        Ok(r) => r,
        Err(e) => return failure(e),
    };
    let Some(r) = rows.into_iter().find(|r| r.revision == revision) else {
        return failure(anyhow::anyhow!("not_found"));
    };
    let bytes = match state.store.attachment(&r.sha256) {
        Ok(Some(b)) => b.bytes,
        Ok(None) => return failure(anyhow::anyhow!("not_found")),
        Err(e) => return internal(e),
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'"),
    );
    let encoded = r.name.bytes().map(|b| format!("%{b:02X}")).collect::<String>();
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"download\"; filename*=UTF-8''{encoded}"
        ))
        .expect("percent-encoded filename"),
    );
    response
}
