//! Durable workspace ownership and admission fencing.

static CLEANUP_OPERATIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

use crate::store::Store;
use anyhow::{Result, ensure};
use hennery_kernel::json::ApiJson;
use rusqlite::params;

pub struct WorkspaceRegistration<'a> {
    pub profile_source: &'a str,
    pub id: &'a str,
    pub host_id: &'a str,
    pub root: &'a str,
    pub repository: &'a str,
    pub branch: &'a str,
    pub base_commit: &'a str,
}

impl Store {
    /// Called only after the host confirms creation and canonical paths.
    pub fn register_workspace(&self, workspace: WorkspaceRegistration<'_>) -> Result<()> {
        ensure!(
            hennery_kernel::hats::is_canonical(workspace.root),
            "invalid workspace root"
        );
        ensure!(
            hennery_kernel::hats::is_canonical(workspace.repository),
            "invalid repository root"
        );
        ensure!(workspace.root != "/", "workspace root cannot be the filesystem root");
        self.conn().execute(
            "INSERT INTO session_workspaces(owner_id,host_id,id,root,repository,branch,base_commit,profile_source)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(owner_id,id) DO NOTHING",
            params![
                self.owner_id(),
                workspace.host_id,
                workspace.id,
                workspace.root,
                workspace.repository,
                workspace.branch,
                workspace.base_commit,
                workspace.profile_source
            ],
        )?;
        Ok(())
    }

    /// Acquires the deletion fence atomically. A false result means either
    /// retained users or another worker already holds/completed the operation.
    pub fn claim_workspace_cleanup(&self, id: &str) -> Result<bool> {
        Ok(self.conn().execute(
            "UPDATE session_workspaces SET state='cleanup_running',last_attempt=unixepoch()
             WHERE owner_id = ?1 AND id = ?2 AND state IN ('cleanup_pending','blocked','failed','cleanup_running')
             AND NOT EXISTS(SELECT 1 FROM session_workspace_members m
                 WHERE m.owner_id = ?1 AND m.workspace_id = ?2)",
            params![self.owner_id(), id],
        )? == 1)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateRequest {
    pub id: Option<String>,
    pub host_id: String,
    pub cwd: String,
    pub base: String,
    #[serde(default)]
    pub files: Vec<String>,
    pub command: Option<String>,
}

pub(crate) async fn provision(state: &crate::AppState, mut request: CreateRequest) -> anyhow::Result<String> {
    use hennery_proto::frames::{CollectorFrame, HostFrame};
    let source = crate::resolve::resolve_on_host(state, &request.host_id, &request.cwd)
        .await
        .map_err(|e| anyhow::anyhow!("source resolution failed: {e:?}"))?;
    let profile_key = workspace_profile_key(&state.store, &request.host_id, &source.canonical)?;
    if request.files.is_empty() && request.command.is_none() {
        use rusqlite::OptionalExtension;
        let profile: Option<(String, Option<String>)> = state
            .store
            .conn()
            .query_row(
                "SELECT files,command FROM workspace_profiles WHERE owner_id = ?1 AND host_id = ?2 AND source = ?3",
                params![state.store.owner_id(), request.host_id, profile_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((files, command)) = profile {
            request.files = serde_json::from_str(&files)?;
            request.command = command;
        }
    }
    let hat = state.hosts.session_hat(&request.host_id, &source.canonical)?;
    ensure!(
        matches!(hat, Some(hennery_kernel::hats::SessionHat::Decided(_))),
        "ambiguous source hat"
    );
    let id = request.id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    state.store.conn().execute(
        "INSERT OR IGNORE INTO workspace_provisioning(owner_id,id,host_id,source) VALUES(?1,?2,?3,?4)",
        params![state.store.owner_id(), id, request.host_id, source.canonical],
    )?;
    let frame = CollectorFrame::Workspace {
        request_id: id.clone(),
        operation: serde_json::json!({
            "kind":"create", "id":id,"source":source.canonical,"base":request.base,"files":request.files,"command":request.command
        }),
    };
    let reply = state
        .hub
        .probe(&request.host_id, &id, frame, std::time::Duration::from_secs(120))
        .await
        .map_err(|e| anyhow::anyhow!("workspace delivery: {e:?}"))?;
    let HostFrame::WorkspaceResult { result, .. } = reply else {
        anyhow::bail!("invalid workspace response")
    };
    let field = |name: &str| {
        result[name]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing workspace {name}"))
    };
    let root = field("root")?;
    state.store.register_workspace(WorkspaceRegistration {
        profile_source: &profile_key,
        id: &id,
        host_id: &request.host_id,
        root,
        repository: field("repository")?,
        branch: field("branch")?,
        base_commit: field("base_commit")?,
    })?;
    state.store.conn().execute(
        "DELETE FROM workspace_provisioning WHERE owner_id = ?1 AND id = ?2",
        params![state.store.owner_id(), id],
    )?;
    if result.get("error").is_some() {
        state.store.conn().execute(
            "UPDATE session_workspaces SET state='failed' WHERE owner_id = ?1 AND id = ?2",
            params![state.store.owner_id(), id],
        )?;
    }
    ensure!(
        result.get("error").is_none(),
        "{}",
        result["error"].as_str().unwrap_or("workspace preparation failed")
    );
    // Generated paths do not grant access to a different hat.
    ensure!(
        state.hosts.session_hat(&request.host_id, root)? == hat,
        "workspace hat mismatch"
    );
    Ok(result["cwd"].as_str().unwrap_or(root).to_owned())
}

impl Store {
    pub fn archive_workspace_session(&self, id: &str) -> Result<Option<hennery_proto::rest::EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute("UPDATE sessions SET lifecycle='archived' WHERE owner_id = ?1 AND id = ?2
            AND lifecycle='closed' AND NOT EXISTS(SELECT 1 FROM purged_hats WHERE owner_id = ?1 AND hat_id=sessions.hat_id)",params![self.owner_id(),id])?;
        if changed == 0 {
            return Ok(None);
        }
        let event = crate::store::collector_event(
            &tx,
            self.owner_id(),
            id,
            "session_archived",
            serde_json::json!({}),
            &crate::store::now(),
        )?;
        tx.commit()?;
        Ok(Some(event))
    }
}

pub(crate) async fn archive(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match state.store.archive_workspace_session(&id) {
        Ok(Some(event)) => {
            state.hub.publish(event);
            axum::Json(serde_json::json!({"lifecycle":"archived"})).into_response()
        }
        Ok(None) => (
            axum::http::StatusCode::CONFLICT,
            "Only a closed session can be archived",
        )
            .into_response(),
        Err(_) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Could not archive session",
        )
            .into_response(),
    }
}

pub fn after_startup(state: &crate::AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            tokio::select! {biased; ()=state.shutdown.cancelled()=>break,_=tick.tick()=>{}}
            if let Err(err) = cleanup_page(&state).await {
                tracing::warn!(error=%err,"workspace cleanup deferred");
            }
        }
    });
}

pub async fn cleanup_page(state: &crate::AppState) -> Result<()> {
    let _cleanup_guard = CLEANUP_OPERATIONS.lock().await;
    recover_provisioning(state).await?;
    use hennery_proto::frames::{CollectorFrame, HostFrame};
    let jobs: Vec<(String, String, String, String)> = {
        let conn = state.store.conn();
        let mut stmt=conn.prepare("SELECT id,host_id,repository,root FROM session_workspaces w WHERE owner_id = ?1
            AND state IN ('cleanup_pending','blocked','failed','cleanup_running') AND NOT EXISTS(SELECT 1 FROM session_workspace_members m WHERE m.owner_id=w.owner_id AND m.workspace_id=w.id) ORDER BY last_attempt,id LIMIT 16")?;
        stmt.query_map([state.store.owner_id()], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    for (id, host, repository, root) in jobs {
        state.store.conn().execute(
            "UPDATE session_workspaces SET last_attempt=unixepoch() WHERE owner_id = ?1 AND id = ?2",
            params![state.store.owner_id(), id],
        )?;
        if !state.hub.is_ready(&host) || !state.store.claim_workspace_cleanup(&id)? {
            continue;
        }
        let request_id = uuid::Uuid::now_v7().to_string();
        let reply = state
            .hub
            .probe(
                &host,
                &request_id,
                CollectorFrame::Workspace {
                    request_id: request_id.clone(),
                    operation: serde_json::json!({"kind":"cleanup","id":id,"repository":repository,"root":root}),
                },
                std::time::Duration::from_secs(30),
            )
            .await;
        let removed = matches!(reply,Ok(HostFrame::WorkspaceResult {ref result,..}) if result["removed"]==true);
        // An uncertain response does not prove that host deletion stopped.
        // Keep admission fenced until an idempotent retry receives a reply.
        if !matches!(reply, Ok(HostFrame::WorkspaceResult { .. })) {
            continue;
        }
        state.store.conn().execute(
            "UPDATE session_workspaces SET state = ?3 WHERE owner_id = ?1 AND id = ?2 AND state='cleanup_running'",
            params![state.store.owner_id(), id, if removed { "removed" } else { "blocked" }],
        )?;
    }
    Ok(())
}

pub(crate) async fn detail(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use rusqlite::OptionalExtension;
    let result=state.store.conn().query_row("SELECT w.root,w.branch,w.base_commit,w.state,
        (SELECT count(*) FROM session_workspace_members m WHERE m.owner_id=w.owner_id AND m.workspace_id=w.id)
        FROM sessions s JOIN session_workspaces w ON w.owner_id=s.owner_id AND w.id=s.managed_workspace_id
        WHERE s.owner_id = ?1 AND s.id = ?2 AND s.lifecycle<>'deleted'
        AND NOT EXISTS(SELECT 1 FROM purged_hats WHERE owner_id=s.owner_id AND hat_id=s.hat_id)",params![state.store.owner_id(),id],|r|Ok(serde_json::json!({
            "root":r.get::<_,String>(0)?,"branch":r.get::<_,String>(1)?,"base_commit":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"users":r.get::<_,i64>(4)?
        }))).optional();
    match result {
        Ok(value) => axum::Json(value).into_response(),
        Err(_) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Could not read workspace",
        )
            .into_response(),
    }
}

async fn recover_provisioning(state: &crate::AppState) -> Result<()> {
    use hennery_proto::frames::{CollectorFrame, HostFrame};
    let jobs: Vec<(String, String, String)> = {
        let conn = state.store.conn();
        let mut stmt=conn.prepare("SELECT id,host_id,source FROM workspace_provisioning WHERE owner_id = ?1 AND created_at<unixepoch()-180 ORDER BY last_attempt,id LIMIT 16")?;
        stmt.query_map([state.store.owner_id()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, host, source) in jobs {
        state.store.conn().execute(
            "UPDATE workspace_provisioning SET last_attempt=unixepoch() WHERE owner_id = ?1 AND id = ?2",
            params![state.store.owner_id(), id],
        )?;
        if !state.hub.is_ready(&host) {
            continue;
        }
        let request = uuid::Uuid::now_v7().to_string();
        let reply = state
            .hub
            .probe(
                &host,
                &request,
                CollectorFrame::Workspace {
                    request_id: request.clone(),
                    operation: serde_json::json!({"kind":"inspect","id":id,"source":source}),
                },
                std::time::Duration::from_secs(30),
            )
            .await;
        if let Ok(HostFrame::WorkspaceResult { result, .. }) = reply
            && let (Some(root), Some(repository), Some(branch), Some(base_commit)) = (
                result["root"].as_str(),
                result["repository"].as_str(),
                result["branch"].as_str(),
                result["base_commit"].as_str(),
            )
        {
            state.store.register_workspace(WorkspaceRegistration {
                profile_source: &workspace_profile_key(&state.store, &host, &source)?,
                id: &id,
                host_id: &host,
                root,
                repository,
                branch,
                base_commit,
            })?;
            state.store.conn().execute(
                "DELETE FROM workspace_provisioning WHERE owner_id = ?1 AND id = ?2",
                params![state.store.owner_id(), id],
            )?;
        }
    }
    // Provisioning can succeed before launch admission later fails. Give
    // admission ten minutes, then reclaim only unused registered workspaces.
    state.store.conn().execute("UPDATE session_workspaces SET state='cleanup_pending' WHERE owner_id = ?1 AND state='ready'
        AND created_at<unixepoch()-600 AND NOT EXISTS(SELECT 1 FROM session_workspace_members m WHERE m.owner_id=session_workspaces.owner_id AND m.workspace_id=session_workspaces.id)", [state.store.owner_id()])?;
    Ok(())
}

#[derive(serde::Deserialize)]
pub(crate) struct ProfileSource {
    host_id: Option<String>,
    cwd: Option<String>,
    session_id: Option<String>,
}
#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparationProfile {
    files: Vec<String>,
    command: Option<String>,
}
async fn profile_source(state: &crate::AppState, source: ProfileSource) -> Result<(String, String)> {
    let (host, cwd) = if let Some(id) = source.session_id {
        let session = state
            .store
            .find_session(&id)?
            .ok_or_else(|| anyhow::anyhow!("session unavailable"))?;
        (session.host_id, session.cwd)
    } else {
        (
            source.host_id.ok_or_else(|| anyhow::anyhow!("host missing"))?,
            source.cwd.ok_or_else(|| anyhow::anyhow!("cwd missing"))?,
        )
    };
    let path = crate::resolve::resolve_on_host(state, &host, &cwd)
        .await
        .map_err(|_| anyhow::anyhow!("source unavailable"))?;
    ensure!(path.is_dir, "source is not a directory");
    Ok((host, path.canonical))
}
pub(crate) async fn read_profile(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Query(source): axum::extract::Query<ProfileSource>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let result: Result<PreparationProfile> = async {
        use rusqlite::OptionalExtension;
        let (host, cwd) = profile_source(&state, source).await?;
        let row: Option<(String, Option<String>)> = state
            .store
            .conn()
            .query_row(
                "SELECT files,command FROM workspace_profiles WHERE owner_id = ?1 AND host_id = ?2 AND source = ?3",
                params![state.store.owner_id(), host, cwd],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((files, command)) => PreparationProfile {
                files: serde_json::from_str(&files)?,
                command,
            },
            None => PreparationProfile::default(),
        })
    }
    .await;
    match result {
        Ok(profile) => axum::Json(profile).into_response(),
        Err(_) => (axum::http::StatusCode::CONFLICT, "Preparation profile unavailable").into_response(),
    }
}
pub(crate) async fn save_profile(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Query(source): axum::extract::Query<ProfileSource>,
    ApiJson(profile): ApiJson<PreparationProfile>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let result: Result<()> = async {
        ensure!(
            profile.files.len() <= 64 && profile.command.as_ref().is_none_or(|s| s.len() <= 8192),
            "profile too large"
        );
        let (host, cwd) = profile_source(&state, source).await?;
        state.store.conn().execute(
            "INSERT INTO workspace_profiles(owner_id,host_id,source,files,command) VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(owner_id,host_id,source) DO UPDATE SET files=excluded.files,command=excluded.command",
            params![
                state.store.owner_id(),
                host,
                cwd,
                serde_json::to_string(&profile.files)?,
                profile.command
            ],
        )?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => axum::Json(serde_json::json!({"saved":true})).into_response(),
        Err(_) => (axum::http::StatusCode::CONFLICT, "Could not save preparation profile").into_response(),
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct PreviewRequest {
    #[serde(flatten)]
    source: ProfileSource,
    base: String,
}
pub(crate) async fn preview(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    ApiJson(request): ApiJson<PreviewRequest>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use hennery_proto::frames::{CollectorFrame, HostFrame};
    let result: Result<serde_json::Value> = async {
        let (host, cwd) = profile_source(&state, request.source).await?;
        let id = uuid::Uuid::now_v7().to_string();
        let answer = state
            .hub
            .probe(
                &host,
                &id,
                CollectorFrame::Workspace {
                    request_id: id.clone(),
                    operation: serde_json::json!({"kind":"preview","id":id,"source":cwd,"base":request.base}),
                },
                std::time::Duration::from_secs(15),
            )
            .await
            .map_err(|_| anyhow::anyhow!("preview unavailable"))?;
        let HostFrame::WorkspaceResult { result, .. } = answer else {
            anyhow::bail!("invalid preview")
        };
        ensure!(result["base_commit"].is_string(), "base revision unavailable");
        Ok(result)
    }
    .await;
    match result {
        Ok(value) => axum::Json(value).into_response(),
        Err(_) => (axum::http::StatusCode::CONFLICT, "Base revision unavailable").into_response(),
    }
}

pub(crate) async fn inventory(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let result: Result<Vec<serde_json::Value>> = (|| {
        let conn = state.store.conn();
        let mut stmt=conn.prepare("SELECT w.id,w.root,w.branch,w.state,(SELECT count(*) FROM session_workspace_members m WHERE m.owner_id=w.owner_id AND m.workspace_id=w.id) FROM session_workspaces w WHERE w.owner_id = ?1 AND w.state<>'removed' ORDER BY created_at DESC LIMIT 100")?;
        Ok(stmt.query_map([state.store.owner_id()],|r|Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"root":r.get::<_,String>(1)?,"branch":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"users":r.get::<_,i64>(4)?})))?.collect::<rusqlite::Result<_>>()?)
    })();
    match result {
        Ok(value) => axum::Json(value).into_response(),
        Err(_) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Workspace inventory unavailable",
        )
            .into_response(),
    }
}

fn workspace_profile_key(store: &Store, host: &str, cwd: &str) -> Result<String> {
    use rusqlite::OptionalExtension;
    Ok(store.conn().query_row("SELECT profile_source FROM session_workspaces WHERE owner_id = ?1 AND host_id = ?2
        AND profile_source<>'' AND (root = ?3 OR substr(?3,1,length(root)+1)=root||'/') ORDER BY length(root) DESC LIMIT 1",params![store.owner_id(),host,cwd],|r|r.get::<_,String>(0)).optional()?.unwrap_or_else(||cwd.to_owned()))
}

pub(crate) async fn discard(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use hennery_proto::frames::{CollectorFrame, HostFrame};
    let result:Result<()>=async {
        let _cleanup_guard=CLEANUP_OPERATIONS.lock().await;
        let (host,repository,root):(String,String,String)=state.store.conn().query_row("SELECT host_id,repository,root FROM session_workspaces WHERE owner_id = ?1 AND id = ?2 AND state='blocked'",params![state.store.owner_id(),id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        ensure!(state.hub.is_ready(&host),"host offline");
        ensure!(state.store.conn().execute("UPDATE session_workspaces SET state='cleanup_running',last_attempt=unixepoch() WHERE owner_id = ?1 AND id = ?2 AND state='blocked' AND NOT EXISTS(SELECT 1 FROM session_workspace_members WHERE owner_id = ?1 AND workspace_id = ?2)",params![state.store.owner_id(),id])?==1,"workspace in use");
        let request=uuid::Uuid::now_v7().to_string();
        let answer=state.hub.probe(&host,&request,CollectorFrame::Workspace {request_id:request.clone(),operation:serde_json::json!({"kind":"cleanup","id":id,"repository":repository,"root":root,"discard":true})},std::time::Duration::from_secs(30)).await;
        match answer {
            Ok(HostFrame::WorkspaceResult {result,..})=>{
                let removed=result["removed"]==true;
                state.store.conn().execute("UPDATE session_workspaces SET state = ?3 WHERE owner_id = ?1 AND id = ?2",params![state.store.owner_id(),id,if removed {"removed"}else{"blocked"}])?;
                ensure!(removed,"host refused removal");
            }
            _=>anyhow::bail!("removal confirmation pending; admission remains fenced"),
        }
        Ok(())
    }.await;
    match result {
        Ok(()) => axum::Json(serde_json::json!({"removed":true})).into_response(),
        Err(err) => (axum::http::StatusCode::CONFLICT, err.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn archived_members_release_but_cannot_resume_and_request_cleanup() {
        let db = database();
        db.execute("UPDATE sessions SET lifecycle='archived' WHERE id='parent'", [])
            .unwrap();
        let state: String = db
            .query_row("SELECT state FROM session_workspaces", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "cleanup_pending");
        assert!(
            db.execute("UPDATE sessions SET lifecycle='starting' WHERE id='parent'", [])
                .is_err()
        );
        assert!(
            db.execute("UPDATE session_workspaces SET state='cleanup_running'", [])
                .is_err()
        );
        db.execute("UPDATE sessions SET lifecycle='archived' WHERE id='child'", [])
            .unwrap();
        db.execute("UPDATE session_workspaces SET state='cleanup_running'", [])
            .unwrap();
        db.execute("UPDATE sessions SET lifecycle='deleted' WHERE id='parent'", [])
            .unwrap();
    }

    #[test]
    fn migration_opens_with_the_real_session_schema() {
        let store = crate::store::Store::open_in_memory().unwrap();
        let count: i64 = store
            .conn()
            .query_row("SELECT count(*) FROM session_workspaces", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn registration_acquires_existing_resumable_users() {
        let db = database();
        db.execute_batch(
            "INSERT INTO sessions(id,owner_id,host_id,lifecycle,cwd) VALUES('existing','owner','host','failed','/repo/next/src');
            INSERT INTO session_workspaces(owner_id,host_id,id,root,repository,branch,base_commit)
            VALUES('owner','host','next','/repo/next','/repo','next','def');",
        )
        .unwrap();
        let member: String = db
            .query_row(
                "SELECT session_id FROM session_workspace_members WHERE workspace_id='next'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(member, "existing");
    }

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "PRAGMA foreign_keys=ON;
            CREATE TABLE sessions(id TEXT,owner_id TEXT,host_id TEXT,lifecycle TEXT,cwd TEXT,
                UNIQUE(id,owner_id));
            INSERT INTO sessions(id,owner_id,host_id,lifecycle,cwd) VALUES('parent','owner','host','closed','/repo/wt'),
                ('child','owner','host','parked','/repo/wt/sub');",
        )
        .unwrap();
        db.execute_batch(
            "CREATE TABLE delegation_notices(owner_id TEXT,parent_id TEXT,child_id TEXT,state TEXT,reason TEXT);",
        )
        .unwrap();
        db.execute_batch(include_str!("workspaces.sql")).unwrap();
        db.execute_batch(
            "INSERT INTO session_workspaces
            (owner_id,host_id,id,root,repository,branch,base_commit)
            VALUES('owner','host','workspace','/repo/wt','/repo','task','abc');",
        )
        .unwrap();
        db
    }

    #[test]
    fn closed_and_parked_users_block_cleanup_until_both_release() {
        let db = database();
        let cleanup = "UPDATE session_workspaces SET state='cleanup_running'";
        assert!(db.execute(cleanup, []).is_err());
        db.execute("UPDATE sessions SET lifecycle='archived' WHERE id='parent'", [])
            .unwrap();
        assert!(db.execute(cleanup, []).is_err());
        db.execute("UPDATE sessions SET lifecycle='deleted' WHERE id='child'", [])
            .unwrap();
        db.execute(cleanup, []).unwrap();
        assert!(
            db.execute(
                "INSERT INTO session_workspace_members VALUES('owner','workspace','parent')",
                []
            )
            .is_err()
        );
        let count: i64 = db
            .query_row("SELECT count(*) FROM session_workspaces", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "deleting the transcript must retain cleanup ownership");
    }

    #[test]
    fn explicit_subdirectory_admission_joins_and_cleanup_refuses_it() {
        let db = database();
        db.execute_batch(
            "DELETE FROM session_workspace_members;
            INSERT INTO sessions(id,owner_id,host_id,lifecycle,cwd) VALUES('reviewer','owner','host','starting','/repo/wt/src');",
        )
        .unwrap();
        let member: String = db
            .query_row("SELECT session_id FROM session_workspace_members", [], |r| r.get(0))
            .unwrap();
        assert_eq!(member, "reviewer");
        assert!(
            db.execute("UPDATE session_workspaces SET state='cleanup_running'", [])
                .is_err()
        );
        db.execute_batch(
            "DELETE FROM session_workspace_members;
            UPDATE session_workspaces SET state='cleanup_running';",
        )
        .unwrap();
        assert!(
            db.execute(
                "INSERT INTO sessions(id,owner_id,host_id,lifecycle,cwd) VALUES('late','owner','host','starting','/repo/wt/src')",
                []
            )
            .is_err()
        );
        let count: i64 = db
            .query_row("SELECT count(*) FROM sessions WHERE id='late'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "refused admission must roll back the session insert");
    }

    #[test]
    fn cleanup_fences_new_users_and_other_hosts_cannot_join() {
        let db = database();
        db.execute_batch(
            "DELETE FROM session_workspace_members;
            INSERT INTO sessions(id,owner_id,host_id,lifecycle,cwd) VALUES('new','owner','host','starting','/elsewhere'),
              ('foreign','owner','other-host','active','/elsewhere');",
        )
        .unwrap();
        assert!(
            db.execute(
                "INSERT INTO session_workspace_members VALUES('owner','workspace','foreign')",
                []
            )
            .is_err()
        );
        db.execute("UPDATE session_workspaces SET state='cleanup_running'", [])
            .unwrap();
        assert!(
            db.execute(
                "INSERT INTO session_workspace_members VALUES('owner','workspace','new')",
                []
            )
            .is_err()
        );
    }
}
