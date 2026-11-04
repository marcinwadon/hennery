//! External control identities are separate from operator and delegated sessions.
//! Metadata, transcript and session-control authority are separately granted.
use crate::store::Store;
use anyhow::{Result, ensure};
use hennery_kernel::{
    hats::is_canonical,
    secret::{random_bytes, sha256_hex, unix_now},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootGrant {
    pub host_id: String,
    pub root: String,
}
/// Selections intersect: an allowed hat AND an allowed host/root are required.
/// Wildcards include future objects only after explicit operator consent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantScope {
    #[serde(default)]
    pub hat_ids: Vec<String>,
    #[serde(default)]
    pub all_hats: bool,
    #[serde(default)]
    pub roots: Vec<RootGrant>,
    #[serde(default)]
    pub all_hosts: bool,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewClient {
    pub label: String,
    #[serde(default)]
    pub hat_id: String,
    #[serde(default)]
    pub roots: Vec<RootGrant>,
    #[serde(default)]
    pub scope: Option<GrantScope>,
    #[serde(default)]
    pub read_metadata: bool,
    #[serde(default)]
    pub read_transcript: bool,
    #[serde(default)]
    pub control_sessions: bool,
    #[serde(default)]
    pub existing_sessions: bool,
    /// Credentials are finite-lived. Renewal/rotation is a later explicit action.
    pub expires_in_days: u32,
}
#[derive(Debug, Clone, Serialize)]
pub struct ClientRecord {
    pub client_id: String,
    pub label: String,
    pub hat_id: Option<String>,
    pub roots: Vec<RootGrant>,
    pub scope: GrantScope,
    pub read_metadata: bool,
    pub read_transcript: bool,
    pub control_sessions: bool,
    pub existing_sessions: bool,
    pub revision: i64,
    pub expires_at: i64,
    pub revoked: bool,
}
impl ClientRecord {
    pub(crate) fn allows_hat(&self, hat: &str) -> bool {
        self.scope.all_hats || self.scope.hat_ids.iter().any(|id| id == hat)
    }
}
/// Only authenticate can construct a principal; callers cannot inject an owner.
#[derive(Clone)]
pub struct ExternalIdentity {
    owner: String,
    hash: String,
    oauth_hash: Option<String>,
    pub(crate) client: ClientRecord,
}
#[derive(Debug, Clone, Serialize)]
pub struct SessionMetadata {
    pub agent_session_id: Option<String>,
    pub sidebar_pinned: bool,
    pub config: hennery_proto::frames::SessionConfig,
    pub created_at: String,
    pub last_event_at: String,
    pub session_id: String,
    pub host_id: String,
    pub hat_id: String,
    pub agent: String,
    pub cwd: String,
    pub title: Option<String>,
    pub lifecycle: String,
    pub activity: Option<String>,
    pub open_turn_id: Option<String>,
}
#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionFilters {
    pub hat_id: Option<String>,
    pub host_id: Option<String>,
    pub agent: Option<String>,
    pub cwd: Option<String>,
    pub lifecycle: Option<String>,
    pub activity: Option<String>,
    pub query: Option<String>,
    pub agent_session_id: Option<String>,
    pub sidebar_pinned: Option<bool>,
}
pub(crate) const TOKEN_PREFIX: &str = "hnry_external_";
pub fn valid_token(token: &str) -> bool {
    token
        .strip_prefix(TOKEN_PREFIX)
        .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}
pub(crate) fn record(conn: &rusqlite::Connection, owner: &str, id: &str) -> Result<Option<ClientRecord>> {
    let row = conn
        .query_row(
            "SELECT id,label,hat_id,read_metadata,existing_sessions,revision,expires_at,revoked_at IS NOT NULL,read_transcript,control_sessions,all_hats,all_hosts
        FROM external_clients WHERE owner_id = ?1 AND id = ?2",
            params![owner, id],
            |r| {
                Ok(ClientRecord {
                    client_id: r.get(0)?,
                    label: r.get(1)?,
                    hat_id: r.get(2)?,
                    roots: vec![],
                    scope: GrantScope { hat_ids: vec![], all_hats: r.get(10)?, roots: vec![], all_hosts: r.get(11)? },
                    read_metadata: r.get(3)?,
                    read_transcript: r.get(8)?,
                    control_sessions: r.get(9)?,
                    existing_sessions: r.get(4)?,
                    revision: r.get(5)?,
                    expires_at: r.get(6)?,
                    revoked: r.get(7)?,
                })
            },
        )
        .optional()?;
    row.map(|mut client| {
        let mut stmt=conn.prepare("SELECT host_id,root FROM external_client_roots WHERE owner_id = ?1 AND client_id = ?2 ORDER BY host_id,root")?;
        client.roots=stmt.query_map(params![owner,id], |r|Ok(RootGrant{host_id:r.get(0)?,root:r.get(1)?}))?.collect::<rusqlite::Result<_>>()?;
        client.scope.roots = client.roots.clone();
        let mut hats = conn.prepare("SELECT hat_id FROM external_client_hats WHERE owner_id = ?1 AND client_id = ?2 ORDER BY hat_id")?;
        client.scope.hat_ids = hats.query_map(params![owner,id], |r|r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(client)
    }).transpose()
}
pub(crate) fn live(conn: &rusqlite::Connection, owner: &str, identity: &ExternalIdentity) -> Result<bool> {
    if identity.owner != owner {
        return Ok(false);
    }
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM external_clients c
      WHERE c.owner_id = ?1 AND c.id = ?2 AND c.token_hash = ?3 AND c.revision = ?4 AND c.revoked_at IS NULL AND c.expires_at > ?5
      AND EXISTS(SELECT 1 FROM hats h WHERE h.owner_id = c.owner_id
        AND (c.all_hats=1 OR EXISTS(SELECT 1 FROM external_client_hats ch WHERE ch.owner_id=c.owner_id AND ch.client_id=c.id AND ch.hat_id=h.id))
        AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = h.owner_id AND p.hat_id=h.id))
      AND (?6 IS NULL OR EXISTS(SELECT 1 FROM external_oauth_tokens t WHERE t.owner_id = c.owner_id AND t.grant_id=c.id AND t.access_hash=?6 AND t.access_expires_at>?5)))",
      params![owner,identity.client.client_id,identity.hash,identity.client.revision,unix_now(),identity.oauth_hash], |r|r.get(0))?)
}
pub(crate) fn active_grant(conn: &rusqlite::Connection, owner: &str, id: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM external_clients c WHERE c.owner_id = ?1 AND c.id=?2
      AND c.revoked_at IS NULL AND c.expires_at>?3
      AND EXISTS(SELECT 1 FROM hats h WHERE h.owner_id=c.owner_id
        AND (c.all_hats=1 OR EXISTS(SELECT 1 FROM external_client_hats ch WHERE ch.owner_id=c.owner_id AND ch.client_id=c.id AND ch.hat_id=h.id))
        AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id=h.owner_id AND p.hat_id=h.id)))",
      params![owner,id,unix_now()],|r|r.get(0))?)
}
impl Store {
    pub fn issue_external_client(&self, input: &NewClient) -> Result<(ClientRecord, String)> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let result = issue_in(&tx, self.owner_id(), input)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn external_clients(&self) -> Result<Vec<ClientRecord>> {
        self.external_clients_after("")
    }
    pub fn external_clients_after(&self, after: &str) -> Result<Vec<ClientRecord>> {
        ensure!(after.len() <= 128, "invalid_arguments");
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id FROM external_clients WHERE owner_id = ?1 AND id > ?2 ORDER BY id LIMIT 100")?;
        let ids = stmt
            .query_map(params![self.owner_id(), after], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| record(&conn, self.owner_id(), &id)?.ok_or_else(|| anyhow::anyhow!("missing client")))
            .collect()
    }
    pub fn revoke_external_client(&self, id: &str) -> Result<bool> {
        Ok(self.conn().execute("UPDATE external_clients SET revoked_at=COALESCE(revoked_at,?3),revision=revision+CASE WHEN revoked_at IS NULL THEN 1 ELSE 0 END WHERE owner_id = ?1 AND id = ?2",params![self.owner_id(),id,unix_now()])?>0)
    }
    pub fn authenticate_external(&self, token: &str) -> Result<Option<ExternalIdentity>> {
        if !valid_token(token) {
            return Ok(None);
        }
        let conn = self.conn();
        let hash = sha256_hex(token.as_bytes());
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM external_clients WHERE owner_id = ?1 AND token_hash = ?2",
                params![self.owner_id(), hash],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        let identity = ExternalIdentity {
            owner: self.owner_id().into(),
            hash,
            oauth_hash: None,
            client: record(&conn, self.owner_id(), &id)?.expect("found client"),
        };
        Ok(live(&conn, self.owner_id(), &identity)?.then_some(identity))
    }
    pub fn authenticate_external_resource(&self, token: &str, resource: &str) -> Result<Option<ExternalIdentity>> {
        if let Some(identity) = self.authenticate_external(token)? {
            return Ok(Some(identity));
        }
        if !valid_token(token) {
            return Ok(None);
        }
        let conn = self.conn();
        let hash = sha256_hex(token.as_bytes());
        let row:Option<(String,String)>=conn.query_row("SELECT t.grant_id,c.token_hash FROM external_oauth_tokens t JOIN external_clients c ON c.owner_id = t.owner_id AND c.id=t.grant_id WHERE t.owner_id = ?1 AND t.access_hash=?2 AND t.resource=?3 AND t.access_expires_at>?4",params![self.owner_id(),hash,resource,unix_now()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((id, grant_hash)) = row else { return Ok(None) };
        let identity = ExternalIdentity {
            owner: self.owner_id().into(),
            hash: grant_hash,
            oauth_hash: Some(hash),
            client: record(&conn, self.owner_id(), &id)?.expect("joined grant"),
        };
        Ok(live(&conn, self.owner_id(), &identity)?.then_some(identity))
    }
    /// Recheck under the same DB gate as the actual metadata query. A cached
    /// principal is never authority after revocation or hat purge.
    pub fn external_hats(&self, identity: &ExternalIdentity) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn();
        ensure!(live(&conn, self.owner_id(), identity)?, "external_unauthenticated");
        let mut stmt = conn.prepare("SELECT h.id,h.name FROM hats h JOIN external_clients c ON c.owner_id=h.owner_id AND c.id=?2
          WHERE h.owner_id = ?1 AND (c.all_hats=1 OR EXISTS(SELECT 1 FROM external_client_hats ch WHERE ch.owner_id=h.owner_id AND ch.client_id=c.id AND ch.hat_id=h.id))
          AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id=h.owner_id AND p.hat_id=h.id) ORDER BY h.id")?;
        Ok(stmt
            .query_map(params![self.owner_id(), identity.client.client_id], |r| {
                Ok(serde_json::json!({"hat_id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?}))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn external_roots(&self, identity: &ExternalIdentity) -> Result<Vec<RootGrant>> {
        let conn = self.conn();
        ensure!(live(&conn, self.owner_id(), identity)?, "external_unauthenticated");
        let mut stmt=conn.prepare("SELECT h.id,'/' FROM hosts h JOIN external_clients c ON c.owner_id=h.owner_id AND c.id=?2
          WHERE h.owner_id = ?1 AND c.all_hosts=1 AND h.revoked_at IS NULL
          UNION SELECT r.host_id,r.root FROM external_client_roots r JOIN hosts h ON h.owner_id = r.owner_id AND h.id=r.host_id
          WHERE r.owner_id = ?1 AND r.client_id = ?2 AND h.revoked_at IS NULL ORDER BY 1,2")?;
        Ok(stmt
            .query_map(params![self.owner_id(), identity.client.client_id], |r| {
                Ok(RootGrant {
                    host_id: r.get(0)?,
                    root: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// Authorization and collection share the coordinated database gate. The
    /// returned metadata is a snapshot at that point: a later reassignment or
    /// deletion does not retract bytes already collected. Execution must instead
    /// resolve current authority, host realpath, hat and source policy on admission
    /// and again at dispatch/resume; a metadata read never authorizes a command.
    pub fn external_sessions(
        &self,
        identity: &ExternalIdentity,
        after: &str,
        id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<SessionMetadata>> {
        self.external_sessions_filtered(identity, after, id, limit, &SessionFilters::default())
    }
    pub(crate) fn external_sessions_filtered(
        &self,
        identity: &ExternalIdentity,
        after: &str,
        id: Option<&str>,
        limit: u32,
        filters: &SessionFilters,
    ) -> Result<Vec<SessionMetadata>> {
        ensure!(
            [
                &filters.host_id,
                &filters.hat_id,
                &filters.agent,
                &filters.cwd,
                &filters.lifecycle,
                &filters.activity,
                &filters.query,
                &filters.agent_session_id
            ]
            .iter()
            .all(|s| s.as_ref().is_none_or(|s| s.len() <= 4096)),
            "invalid_arguments"
        );
        let conn = self.conn();
        ensure!(live(&conn, self.owner_id(), identity)?, "external_unauthenticated");
        ensure!(identity.client.read_metadata, "external_forbidden");
        ensure!(
            (1..=50).contains(&limit) && after.len() <= 128 && id.is_none_or(|id| id.len() <= 128),
            "invalid_arguments"
        );
        let mut stmt=conn.prepare("SELECT s.id,s.host_id,s.hat_id,s.agent,s.cwd,s.title,s.lifecycle,s.activity,s.open_turn_id,s.agent_session_id,s.sidebar_pinned,s.model,s.mode,s.config_axes,s.created_at,s.last_event_at
          FROM sessions s JOIN external_clients c ON c.owner_id = s.owner_id AND c.id = ?2
          WHERE s.owner_id = ?1 AND s.lifecycle<>'deleted' AND s.id>?3 AND (?4 IS NULL OR s.id=?4)
          AND EXISTS(SELECT 1 FROM hats h WHERE h.owner_id=s.owner_id AND h.id=s.hat_id
            AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id=h.owner_id AND p.hat_id=h.id))
          AND (c.all_hats=1 OR EXISTS(SELECT 1 FROM external_client_hats ch WHERE ch.owner_id=s.owner_id AND ch.client_id=c.id AND ch.hat_id=s.hat_id))
          AND (c.existing_sessions=1 OR EXISTS(SELECT 1 FROM external_client_sessions b WHERE b.owner_id = s.owner_id AND b.client_id=c.id AND b.session_id=s.id))
          AND EXISTS(SELECT 1 FROM hosts h WHERE h.owner_id=s.owner_id AND h.id=s.host_id AND h.revoked_at IS NULL)
          AND (c.all_hosts=1 OR EXISTS(SELECT 1 FROM external_client_roots r
            WHERE r.owner_id = s.owner_id AND r.client_id=c.id AND r.host_id=s.host_id
            AND (s.cwd=r.root OR (r.root='/' AND substr(s.cwd,1,1)='/') OR substr(s.cwd,1,length(r.root)+1)=r.root||'/')))
          AND (?6 IS NULL OR s.host_id=?6) AND (?7 IS NULL OR s.agent=?7) AND (?8 IS NULL OR s.cwd=?8)
          AND (?9 IS NULL OR s.lifecycle=?9) AND (?10 IS NULL OR s.activity=?10)
          AND (?11 IS NULL OR instr(lower(COALESCE(s.title,'')||' '||s.id||' '||COALESCE(s.agent_session_id,'')||' '||s.cwd),lower(?11))>0)
          AND (?12 IS NULL OR s.agent_session_id=?12) AND (?13 IS NULL OR s.sidebar_pinned=?13)
          AND (?14 IS NULL OR s.hat_id=?14)
          ORDER BY s.id LIMIT ?5")?;
        Ok(stmt
            .query_map(
                params![
                    self.owner_id(),
                    identity.client.client_id,
                    after,
                    id,
                    limit,
                    filters.host_id,
                    filters.agent,
                    filters.cwd,
                    filters.lifecycle,
                    filters.activity,
                    filters.query,
                    filters.agent_session_id,
                    filters.sidebar_pinned,
                    filters.hat_id
                ],
                |r| {
                    Ok(SessionMetadata {
                        session_id: r.get(0)?,
                        host_id: r.get(1)?,
                        hat_id: r.get(2)?,
                        agent: r.get(3)?,
                        cwd: r.get(4)?,
                        title: r.get(5)?,
                        lifecycle: r.get(6)?,
                        activity: r.get(7)?,
                        open_turn_id: r.get(8)?,
                        agent_session_id: r.get(9)?,
                        sidebar_pinned: r.get(10)?,
                        config: hennery_proto::frames::SessionConfig {
                            model: r.get(11)?,
                            mode: r.get(12)?,
                            axes: serde_json::from_str(&r.get::<_, Option<String>>(13)?.unwrap_or_else(|| "{}".into()))
                                .map_err(|e| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        13,
                                        rusqlite::types::Type::Text,
                                        Box::new(e),
                                    )
                                })?,
                            ..Default::default()
                        },
                        created_at: r.get(14)?,
                        last_event_at: r.get(15)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<_>>()?)
    }
}

pub(crate) fn issue_in(
    tx: &rusqlite::Transaction<'_>,
    owner: &str,
    input: &NewClient,
) -> Result<(ClientRecord, String)> {
    ensure!(
        !input.label.trim().is_empty() && input.label.len() <= 128 && !input.label.chars().any(char::is_control),
        "invalid_grant"
    );
    ensure!((1..=90).contains(&input.expires_in_days), "invalid_grant");
    ensure!(!input.existing_sessions || input.read_metadata, "invalid_grant");
    ensure!(
        !(input.read_transcript || input.control_sessions) || input.read_metadata,
        "invalid_grant"
    );
    let scope = match &input.scope {
        Some(scope) => {
            ensure!(input.hat_id.is_empty() && input.roots.is_empty(), "invalid_grant");
            scope.clone()
        }
        None => GrantScope {
            hat_ids: vec![input.hat_id.clone()],
            all_hats: false,
            roots: input.roots.clone(),
            all_hosts: false,
        },
    };
    ensure!(scope.hat_ids.len() <= 128 && scope.roots.len() <= 128, "invalid_grant");
    ensure!(
        if scope.all_hats {
            scope.hat_ids.is_empty()
        } else {
            !scope.hat_ids.is_empty()
        },
        "invalid_grant"
    );
    ensure!(
        if scope.all_hosts {
            scope.roots.is_empty()
        } else {
            !scope.roots.is_empty()
        },
        "invalid_grant"
    );
    for hat in &scope.hat_ids {
        let allowed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM hats h WHERE h.owner_id = ?1 AND h.id = ?2
              AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = h.owner_id AND p.hat_id = h.id))",
            params![owner, hat],
            |r| r.get(0),
        )?;
        ensure!(allowed, "invalid_grant");
    }
    for root in &scope.roots {
        ensure!(
            root.root.len() <= 4096 && !root.root.chars().any(char::is_control) && is_canonical(&root.root),
            "invalid_grant"
        );
        let known: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM hosts WHERE owner_id = ?1 AND id = ?2 AND revoked_at IS NULL)",
            params![owner, root.host_id],
            |r| r.get(0),
        )?;
        ensure!(known, "invalid_grant");
        // Legacy one-hat issuance keeps its original project-root check. Broad
        // grants authorize the intersection; a root can contain several hats.
        if input.scope.is_none() {
            let resolved = hennery_kernel::hats::session_hat_in(tx, owner, &root.host_id, &root.root)?;
            ensure!(
                matches!(resolved,Some(hennery_kernel::hats::SessionHat::Decided(hat)) if hat.hat_id==input.hat_id),
                "invalid_grant"
            );
        }
    }
    let id = uuid::Uuid::now_v7().to_string();
    let token = format!("{TOKEN_PREFIX}{}", hex::encode(random_bytes::<32>()));
    let expires = unix_now() + i64::from(input.expires_in_days) * 86400;
    tx.execute("INSERT INTO external_clients(owner_id,id,label,hat_id,token_hash,read_metadata,existing_sessions,revision,expires_at,read_transcript,control_sessions,all_hats,all_hosts)
          VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9,?10,?11,?12)",params![owner,id,input.label.trim(),input.scope.is_none().then_some(&input.hat_id),sha256_hex(token.as_bytes()),input.read_metadata,input.existing_sessions,expires,input.read_transcript,input.control_sessions,scope.all_hats,scope.all_hosts])?;
    for hat in &scope.hat_ids {
        tx.execute(
            "INSERT OR IGNORE INTO external_client_hats(owner_id,client_id,hat_id) VALUES(?1,?2,?3)",
            params![owner, id, hat],
        )?;
    }
    for root in &scope.roots {
        tx.execute(
            "INSERT OR IGNORE INTO external_client_roots(owner_id,client_id,host_id,root) VALUES(?1,?2,?3,?4)",
            params![owner, id, root.host_id, root.root],
        )?;
    }
    let client = record(tx, owner, &id)?.expect("inserted client");
    Ok((client, token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_kernel::{
        hats::{HatChange, NewRule},
        hosts::{Enrollment, Hosts},
    };
    struct World {
        dir: tempfile::TempDir,
        store: Store,
        hosts: Hosts,
        hat: String,
    }
    impl World {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("test.db");
            let hosts = Hosts::open(&db).unwrap();
            hosts
                .register(
                    "h1",
                    &Enrollment {
                        public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                        name: "Test".into(),
                        host_version: "test".into(),
                        platform: "test".into(),
                    },
                    0,
                )
                .unwrap();
            let hat = hosts.host("h1").unwrap().unwrap().default_hat_id;
            let store = Store::open(&db).unwrap();
            Self { dir, store, hosts, hat }
        }
        fn grant(&self, existing: bool) -> NewClient {
            NewClient {
                scope: None,
                label: "Desktop test".into(),
                hat_id: self.hat.clone(),
                roots: vec![RootGrant {
                    host_id: "h1".into(),
                    root: "/project".into(),
                }],
                read_metadata: true,
                read_transcript: false,
                control_sessions: false,
                existing_sessions: existing,
                expires_in_days: 1,
            }
        }
        fn session(&self, id: &str, cwd: &str, hat: &str) {
            self.store.create_session(id, "h1", "claude", cwd, hat, None).unwrap();
        }
    }
    fn add_host(w: &World, id: &str) {
        w.hosts
            .register(
                id,
                &Enrollment {
                    public_key: if id == "h3" {
                        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025"
                    } else {
                        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"
                    }
                    .into(),
                    name: id.into(),
                    host_version: "test".into(),
                    platform: "test".into(),
                },
                0,
            )
            .unwrap();
    }
    fn add_hat(w: &World, name: &str) -> String {
        let HatChange::Done(hat) = w.hosts.create_hat(name, None, 0).unwrap() else {
            panic!()
        };
        hat.id
    }
    fn broad(w: &World, scope: GrantScope) -> NewClient {
        NewClient {
            hat_id: String::new(),
            roots: vec![],
            scope: Some(scope),
            control_sessions: true,
            read_transcript: true,
            ..w.grant(true)
        }
    }
    #[test]
    fn selected_hats_and_hosts_intersect_for_metadata_transcripts_and_control() {
        use crate::external_control_store::{placement, session_scope};
        let w = World::new();
        let second = add_hat(&w, "Second");
        let excluded = add_hat(&w, "Excluded");
        add_host(&w, "h2");
        add_host(&w, "h3");
        w.hosts.update_host("h2", None, Some(&second)).unwrap();
        w.session("one", "/project", &w.hat);
        w.store
            .create_session("two", "h2", "claude", "/work", &second, None)
            .unwrap();
        w.session("hat-denied", "/project", &excluded);
        w.store
            .create_session("host-denied", "h3", "claude", "/project", &w.hat, None)
            .unwrap();
        w.session("prefix-denied", "/project-other", &w.hat);
        let input = broad(
            &w,
            GrantScope {
                hat_ids: vec![w.hat.clone(), second.clone()],
                all_hats: false,
                roots: vec![
                    RootGrant {
                        host_id: "h1".into(),
                        root: "/project".into(),
                    },
                    RootGrant {
                        host_id: "h2".into(),
                        root: "/work".into(),
                    },
                ],
                all_hosts: false,
            },
        );
        let (_, token) = w.store.issue_external_client(&input).unwrap();
        let id = w.store.authenticate_external(&token).unwrap().unwrap();
        let rows = w.store.external_sessions(&id, "", None, 50).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.session_id.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        let page = w
            .store
            .external_sessions_filtered(
                &id,
                "",
                None,
                1,
                &SessionFilters {
                    hat_id: Some(second.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page[0].session_id, "two");
        let conn = w.store.conn();
        for name in ["one", "two"] {
            session_scope(&conn, w.store.owner_id(), &id.client, name, true).unwrap();
        }
        for name in ["hat-denied", "host-denied", "prefix-denied"] {
            assert!(session_scope(&conn, w.store.owner_id(), &id.client, name, false).is_err());
            assert!(session_scope(&conn, w.store.owner_id(), &id.client, name, true).is_err());
        }
        placement(&conn, w.store.owner_id(), &id.client, "h2", "/work/sub", &second).unwrap();
        assert!(placement(&conn, w.store.owner_id(), &id.client, "h2", "/work", &w.hat).is_err());
        assert!(placement(&conn, w.store.owner_id(), &id.client, "h2", "/work-other", &second).is_err());
        drop(conn);
        assert_eq!(w.store.external_hats(&id).unwrap().len(), 2);
        w.store.external_authorized_read(&id, "two", true).unwrap();
        assert!(w.store.external_authorized_read(&id, "hat-denied", true).is_err());
    }
    #[test]
    fn future_hats_and_hosts_require_independent_explicit_wildcards() {
        let w = World::new();
        let selected = GrantScope {
            hat_ids: vec![w.hat.clone()],
            all_hats: false,
            roots: vec![RootGrant {
                host_id: "h1".into(),
                root: "/".into(),
            }],
            all_hosts: false,
        };
        let scopes = [
            selected.clone(),
            GrantScope {
                all_hats: true,
                hat_ids: vec![],
                ..selected.clone()
            },
            GrantScope {
                all_hosts: true,
                roots: vec![],
                ..selected
            },
            GrantScope {
                hat_ids: vec![],
                all_hats: true,
                roots: vec![],
                all_hosts: true,
            },
        ];
        let identities = scopes
            .into_iter()
            .map(|scope| {
                let (_, token) = w.store.issue_external_client(&broad(&w, scope)).unwrap();
                w.store.authenticate_external(&token).unwrap().unwrap()
            })
            .collect::<Vec<_>>();
        let future = add_hat(&w, "Future");
        add_host(&w, "future-host");
        w.session("new-hat", "/project", &future);
        w.store
            .create_session("new-host", "future-host", "claude", "/project", &w.hat, None)
            .unwrap();
        w.store
            .create_session("both-new", "future-host", "claude", "/project", &future, None)
            .unwrap();
        for (identity, expected) in identities.iter().zip([
            vec![],
            vec!["new-hat"],
            vec!["new-host"],
            vec!["both-new", "new-hat", "new-host"],
        ]) {
            let rows = w.store.external_sessions(identity, "", None, 50).unwrap();
            assert_eq!(rows.iter().map(|r| r.session_id.as_str()).collect::<Vec<_>>(), expected);
        }
        assert_eq!(w.store.external_roots(&identities[0]).unwrap().len(), 1);
        assert_eq!(w.store.external_roots(&identities[3]).unwrap().len(), 2);
        w.store
            .conn()
            .execute(
                "UPDATE hosts SET revoked_at=1 WHERE owner_id = ?1 AND id='future-host'",
                [w.store.owner_id()],
            )
            .unwrap();
        assert_eq!(
            w.store.external_sessions(&identities[3], "", None, 50).unwrap().len(),
            1
        );
        assert_eq!(w.store.external_roots(&identities[3]).unwrap().len(), 1);
        assert!(
            crate::external_control_store::placement(
                &w.store.conn(),
                w.store.owner_id(),
                &identities[3].client,
                "future-host",
                "/project",
                &w.hat
            )
            .is_err()
        );
    }
    #[test]
    fn purging_one_hat_keeps_other_selected_hats_and_never_bypasses_cached_principals() {
        let w = World::new();
        let second = add_hat(&w, "Second");
        w.session("one", "/project", &w.hat);
        w.session("two", "/project", &second);
        let (_, token) = w
            .store
            .issue_external_client(&broad(
                &w,
                GrantScope {
                    hat_ids: vec![w.hat.clone(), second.clone()],
                    all_hats: false,
                    roots: vec![],
                    all_hosts: true,
                },
            ))
            .unwrap();
        let identity = w.store.authenticate_external(&token).unwrap().unwrap();
        w.store
            .conn()
            .execute(
                "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES(?1,?2,0)",
                params![w.store.owner_id(), w.hat],
            )
            .unwrap();
        assert!(w.store.authenticate_external(&token).unwrap().is_some());
        assert_eq!(
            w.store.external_sessions(&identity, "", None, 50).unwrap()[0].session_id,
            "two"
        );
        assert!(w.store.external_authorized_read(&identity, "one", true).is_err());
        assert!(
            crate::external_control_store::session_scope(
                &w.store.conn(),
                w.store.owner_id(),
                &identity.client,
                "one",
                false
            )
            .is_err()
        );
        w.store
            .conn()
            .execute(
                "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES(?1,?2,0)",
                params![w.store.owner_id(), second],
            )
            .unwrap();
        assert!(w.store.authenticate_external(&token).unwrap().is_none());
        assert!(w.store.external_roots(&identity).is_err());
    }
    #[test]
    fn broad_scope_rejects_ambiguous_empty_unknown_and_unbounded_selections() {
        let w = World::new();
        let valid = GrantScope {
            hat_ids: vec![w.hat.clone()],
            all_hats: false,
            roots: vec![RootGrant {
                host_id: "h1".into(),
                root: "/".into(),
            }],
            all_hosts: false,
        };
        let mut invalid = vec![];
        let mut s = valid.clone();
        s.hat_ids.clear();
        invalid.push(s);
        let mut s = valid.clone();
        s.roots.clear();
        invalid.push(s);
        let mut s = valid.clone();
        s.all_hats = true;
        invalid.push(s);
        let mut s = valid.clone();
        s.all_hosts = true;
        invalid.push(s);
        let mut s = valid.clone();
        s.hat_ids = vec!["foreign".into()];
        invalid.push(s);
        let mut s = valid.clone();
        s.roots[0].host_id = "foreign".into();
        invalid.push(s);
        let mut s = valid.clone();
        s.roots[0].root = "/project/../other".into();
        invalid.push(s);
        let mut s = valid.clone();
        s.hat_ids = vec![w.hat.clone(); 129];
        invalid.push(s);
        for scope in invalid {
            assert!(w.store.issue_external_client(&broad(&w, scope)).is_err());
        }
        let mut mixed = broad(&w, valid);
        mixed.hat_id = w.hat.clone();
        assert!(w.store.issue_external_client(&mixed).is_err());
        assert!(w.store.external_clients().unwrap().is_empty());
    }

    #[test]
    fn created_only_is_default_and_root_segment_and_hat_guards_are_enforced() {
        let w = World::new();
        w.session("allowed", "/project/child", &w.hat);
        w.session("prefix-trick", "/project-other", &w.hat);
        w.session("elsewhere", "/other", &w.hat);
        w.session("outside-host", "/project", &w.hat);
        w.store
            .conn()
            .execute(
                "UPDATE sessions SET host_id='other-host' WHERE owner_id = ?1 AND id='outside-host'",
                [w.store.owner_id()],
            )
            .unwrap();
        let HatChange::Done(other) = w.hosts.create_hat("Other", None, 0).unwrap() else {
            panic!()
        };
        w.session("other-hat", "/project/child", &other.id);
        let (_, token) = w.store.issue_external_client(&w.grant(false)).unwrap();
        let identity = w.store.authenticate_external(&token).unwrap().unwrap();
        assert!(w.store.external_sessions(&identity, "", None, 25).unwrap().is_empty());
        let (_, token) = w.store.issue_external_client(&w.grant(true)).unwrap();
        let identity = w.store.authenticate_external(&token).unwrap().unwrap();
        let rows = w.store.external_sessions(&identity, "", None, 25).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "allowed");
        assert!(
            w.store
                .external_sessions(&identity, "", Some("other-hat"), 1)
                .unwrap()
                .is_empty()
        );
        assert!(
            w.store
                .external_sessions(&identity, "", Some("prefix-trick"), 1)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn credential_survives_reopen_but_never_revocation_or_expiry() {
        let w = World::new();
        w.session("visible", "/project", &w.hat);
        let (client, token) = w.store.issue_external_client(&w.grant(true)).unwrap();
        let reopened = Store::open(&w.dir.path().join("test.db")).unwrap();
        let old = reopened.authenticate_external(&token).unwrap().unwrap();
        let stored: String = reopened
            .conn()
            .query_row("SELECT token_hash FROM external_clients", [], |r| r.get(0))
            .unwrap();
        assert_ne!(stored, token);
        assert_eq!(stored, sha256_hex(token.as_bytes()));
        assert!(reopened.revoke_external_client(&client.client_id).unwrap());
        assert!(reopened.authenticate_external(&token).unwrap().is_none());
        assert!(reopened.external_roots(&old).is_err());
        assert!(reopened.external_sessions(&old, "", None, 25).is_err());
        assert!(reopened.revoke_external_client(&client.client_id).unwrap());
        let (client, token) = w.store.issue_external_client(&w.grant(true)).unwrap();
        let cached = w.store.authenticate_external(&token).unwrap().unwrap();
        assert_eq!(w.store.external_sessions(&cached, "", None, 25).unwrap().len(), 1);
        reopened
            .conn()
            .execute(
                "UPDATE external_clients SET expires_at=0 WHERE id=?1",
                [client.client_id],
            )
            .unwrap();
        assert!(w.store.authenticate_external(&token).unwrap().is_none());
        assert!(w.store.external_roots(&cached).is_err());
        assert!(w.store.external_sessions(&cached, "", Some("visible"), 1).is_err());
    }
    #[test]
    fn metadata_uses_actual_hat_after_public_reassignment_and_keeps_authorized_snapshot() {
        let w = World::new();
        let HatChange::Done(other) = w.hosts.create_hat("Other", None, 0).unwrap() else {
            panic!()
        };
        w.session("moved", "/project", &w.hat);
        w.store.close_now("moved").unwrap();
        let (_, token) = w.store.issue_external_client(&w.grant(true)).unwrap();
        let identity = w.store.authenticate_external(&token).unwrap().unwrap();
        let snapshot = w.store.external_sessions(&identity, "", None, 25).unwrap();
        assert_eq!(snapshot.len(), 1);
        assert!(matches!(
            w.store.reassign_hat("moved", &other.id).unwrap(),
            crate::store::Reassign::Done(_)
        ));
        assert!(
            w.store
                .external_sessions(&identity, "", Some("moved"), 1)
                .unwrap()
                .is_empty()
        );
        // The previous authorized snapshot does not change after collection.
        assert_eq!(snapshot[0].hat_id, w.hat);
        assert_eq!(snapshot[0].session_id, "moved");
        assert!(matches!(
            w.store.reassign_hat("moved", &w.hat).unwrap(),
            crate::store::Reassign::Done(_)
        ));
        assert_eq!(w.store.external_sessions(&identity, "", None, 25).unwrap().len(), 1);
    }
    #[test]
    fn nested_hat_rules_do_not_widen_explicit_discovery_or_historical_metadata() {
        let w = World::new();
        let HatChange::Done(other) = w.hosts.create_hat("Other", None, 0).unwrap() else {
            panic!()
        };
        w.hosts
            .replace_path_rules(
                "h1",
                &[NewRule {
                    prefix: "/project/private".into(),
                    hat_id: other.id.clone(),
                    verified: true,
                }],
            )
            .unwrap();
        w.session("parent", "/project/public", &w.hat);
        w.session("nested", "/project/private/sub", &other.id);
        let (_, token) = w.store.issue_external_client(&w.grant(true)).unwrap();
        let identity = w.store.authenticate_external(&token).unwrap().unwrap();
        assert_eq!(
            w.store.external_sessions(&identity, "", None, 25).unwrap()[0].session_id,
            "parent"
        );
        assert!(
            w.store
                .external_sessions(&identity, "", Some("nested"), 1)
                .unwrap()
                .is_empty()
        );
        let mut nested = w.grant(true);
        nested.roots[0].root = "/project/private".into();
        assert!(w.store.issue_external_client(&nested).is_err());
        nested.hat_id = other.id.clone();
        let (_, nested_token) = w.store.issue_external_client(&nested).unwrap();
        let nested_identity = w.store.authenticate_external(&nested_token).unwrap().unwrap();
        assert_eq!(
            w.store.external_sessions(&nested_identity, "", None, 25).unwrap()[0].session_id,
            "nested"
        );

        w.hosts
            .replace_path_rules(
                "h1",
                &[NewRule {
                    prefix: "/project".into(),
                    hat_id: other.id.clone(),
                    verified: true,
                }],
            )
            .unwrap();
        w.hosts.update_host("h1", None, Some(&other.id)).unwrap();
        // Discovery retains the grant's explicit root; historical hats are not
        // rewritten by new path rules/defaults. Future execution must resolve anew.
        assert_eq!(w.store.external_roots(&identity).unwrap(), w.grant(true).roots);
        let rows = w.store.external_sessions(&identity, "", None, 25).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "parent");
        assert!(w.store.issue_external_client(&w.grant(true)).is_err());
    }
    #[test]
    fn purge_revoked_host_and_cached_foreign_principal_cannot_read() {
        let w = World::new();
        let (_, token) = w.store.issue_external_client(&w.grant(true)).unwrap();
        let mut identity = w.store.authenticate_external(&token).unwrap().unwrap();
        identity.owner = "foreign".into();
        assert!(w.store.external_roots(&identity).is_err());
        let identity = w.store.authenticate_external(&token).unwrap().unwrap();
        w.store
            .conn()
            .execute(
                "UPDATE hosts SET revoked_at=1 WHERE owner_id = ?1 AND id='h1'",
                [w.store.owner_id()],
            )
            .unwrap();
        assert!(w.store.external_roots(&identity).unwrap().is_empty());
        w.store
            .conn()
            .execute(
                "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES(?1,?2,0)",
                params![w.store.owner_id(), w.hat],
            )
            .unwrap();
        assert!(w.store.authenticate_external(&token).unwrap().is_none());
        assert!(w.store.external_roots(&identity).is_err());
    }
    #[test]
    fn grant_issuance_refuses_other_hat_path_unknown_host_and_invalid_inputs() {
        let w = World::new();
        let HatChange::Done(other) = w.hosts.create_hat("Other", None, 0).unwrap() else {
            panic!()
        };
        w.hosts
            .replace_path_rules(
                "h1",
                &[NewRule {
                    prefix: "/private".into(),
                    hat_id: other.id,
                    verified: true,
                }],
            )
            .unwrap();
        for mode in 0..6 {
            let mut grant = w.grant(true);
            match mode {
                0 => grant.roots[0].root = "/project/../private".into(),
                1 => grant.roots[0].root = "/private".into(),
                2 => grant.roots[0].host_id = "unknown".into(),
                3 => grant.hat_id = "unknown".into(),
                4 => grant.expires_in_days = 0,
                _ => grant.read_metadata = false,
            };
            assert!(w.store.issue_external_client(&grant).is_err(), "case {mode}");
        }
        assert!(w.store.external_clients().unwrap().is_empty());
    }
    #[test]
    fn permissions_and_pagination_are_bounded_without_seen_mutations() {
        let w = World::new();
        for id in ["a", "b", "c"] {
            w.session(id, "/project", &w.hat)
        }
        let (_, token) = w.store.issue_external_client(&w.grant(true)).unwrap();
        let identity = w.store.authenticate_external(&token).unwrap().unwrap();
        assert_eq!(
            w.store.external_sessions(&identity, "a", None, 1).unwrap()[0].session_id,
            "b"
        );
        assert!(w.store.external_sessions(&identity, "", None, 51).is_err());
        let before: i64 = w
            .store
            .conn()
            .query_row("SELECT SUM(seen_event_id) FROM sessions", [], |r| r.get(0))
            .unwrap();
        w.store.external_sessions(&identity, "", None, 25).unwrap();
        let after: i64 = w
            .store
            .conn()
            .query_row("SELECT SUM(seen_event_id) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
        let mut grant = w.grant(false);
        grant.read_metadata = false;
        let (_, token) = w.store.issue_external_client(&grant).unwrap();
        let identity = w.store.authenticate_external(&token).unwrap().unwrap();
        assert!(w.store.external_sessions(&identity, "", None, 25).is_err());
        for wrong in ["hnry_session_deadbeef", "Bearer operator", "hnry_external_short"] {
            assert!(w.store.authenticate_external(wrong).unwrap().is_none());
        }
        let grants = w.store.external_clients().unwrap();
        assert_eq!(grants.len(), 2);
        assert_eq!(w.store.external_clients_after(&grants[0].client_id).unwrap().len(), 1);
        assert!(w.store.external_clients_after(&"x".repeat(129)).is_err());
    }
}
