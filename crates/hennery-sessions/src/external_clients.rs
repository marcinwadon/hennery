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
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewClient {
    pub label: String,
    pub hat_id: String,
    pub roots: Vec<RootGrant>,
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
    pub hat_id: String,
    pub roots: Vec<RootGrant>,
    pub read_metadata: bool,
    pub read_transcript: bool,
    pub control_sessions: bool,
    pub existing_sessions: bool,
    pub revision: i64,
    pub expires_at: i64,
    pub revoked: bool,
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
pub(crate) const TOKEN_PREFIX: &str = "hnry_external_";
pub fn valid_token(token: &str) -> bool {
    token
        .strip_prefix(TOKEN_PREFIX)
        .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}
pub(crate) fn record(conn: &rusqlite::Connection, owner: &str, id: &str) -> Result<Option<ClientRecord>> {
    let row = conn
        .query_row(
            "SELECT id,label,hat_id,read_metadata,existing_sessions,revision,expires_at,revoked_at IS NOT NULL,read_transcript,control_sessions
        FROM external_clients WHERE owner_id = ?1 AND id = ?2",
            params![owner, id],
            |r| {
                Ok(ClientRecord {
                    client_id: r.get(0)?,
                    label: r.get(1)?,
                    hat_id: r.get(2)?,
                    roots: vec![],
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
        Ok(client)
    }).transpose()
}
pub(crate) fn live(conn: &rusqlite::Connection, owner: &str, identity: &ExternalIdentity) -> Result<bool> {
    if identity.owner != owner {
        return Ok(false);
    }
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM external_clients c JOIN hats h ON h.owner_id = c.owner_id AND h.id = c.hat_id
      WHERE c.owner_id = ?1 AND c.id = ?2 AND c.token_hash = ?3 AND c.revision = ?4 AND c.revoked_at IS NULL AND c.expires_at > ?5
      AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = c.owner_id AND p.hat_id = c.hat_id)
      AND (?6 IS NULL OR EXISTS(SELECT 1 FROM external_oauth_tokens t WHERE t.owner_id = c.owner_id AND t.grant_id=c.id AND t.access_hash=?6 AND t.access_expires_at>?5)))",
      params![owner,identity.client.client_id,identity.hash,identity.client.revision,unix_now(),identity.oauth_hash], |r|r.get(0))?)
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
    pub fn external_roots(&self, identity: &ExternalIdentity) -> Result<Vec<RootGrant>> {
        let conn = self.conn();
        ensure!(live(&conn, self.owner_id(), identity)?, "external_unauthenticated");
        let mut stmt=conn.prepare("SELECT r.host_id,r.root FROM external_client_roots r JOIN hosts h ON h.owner_id = r.owner_id AND h.id=r.host_id
          WHERE r.owner_id = ?1 AND r.client_id = ?2 AND h.revoked_at IS NULL ORDER BY r.host_id,r.root")?;
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
        let conn = self.conn();
        ensure!(live(&conn, self.owner_id(), identity)?, "external_unauthenticated");
        ensure!(identity.client.read_metadata, "external_forbidden");
        ensure!(
            (1..=50).contains(&limit) && after.len() <= 128 && id.is_none_or(|id| id.len() <= 128),
            "invalid_arguments"
        );
        let mut stmt=conn.prepare("SELECT s.id,s.host_id,s.hat_id,s.agent,s.cwd,s.title,s.lifecycle,s.activity,s.open_turn_id
          FROM sessions s JOIN external_clients c ON c.owner_id = s.owner_id AND c.id = ?2 AND c.hat_id=s.hat_id
          WHERE s.owner_id = ?1 AND s.lifecycle<>'deleted' AND s.id>?3 AND (?4 IS NULL OR s.id=?4)
          AND (c.existing_sessions=1 OR EXISTS(SELECT 1 FROM external_client_sessions b WHERE b.owner_id = s.owner_id AND b.client_id=c.id AND b.session_id=s.id))
          AND EXISTS(SELECT 1 FROM external_client_roots r JOIN hosts h ON h.owner_id = r.owner_id AND h.id=r.host_id
            WHERE r.owner_id = s.owner_id AND r.client_id=c.id AND r.host_id=s.host_id AND h.revoked_at IS NULL
            AND (s.cwd=r.root OR (r.root='/' AND substr(s.cwd,1,1)='/') OR substr(s.cwd,1,length(r.root)+1)=r.root||'/'))
          ORDER BY s.id LIMIT ?5")?;
        Ok(stmt
            .query_map(
                params![self.owner_id(), identity.client.client_id, after, id, limit],
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
    ensure!(
        (1..=90).contains(&input.expires_in_days) && !input.roots.is_empty() && input.roots.len() <= 32,
        "invalid_grant"
    );
    ensure!(!input.existing_sessions || input.read_metadata, "invalid_grant");
    ensure!(
        !(input.read_transcript || input.control_sessions) || input.read_metadata,
        "invalid_grant"
    );
    let allowed: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM hats h WHERE h.owner_id = ?1 AND h.id = ?2
          AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = h.owner_id AND p.hat_id = h.id))",
        params![owner, input.hat_id],
        |r| r.get(0),
    )?;
    ensure!(allowed, "invalid_grant");
    for root in &input.roots {
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
        let resolved = hennery_kernel::hats::session_hat_in(tx, owner, &root.host_id, &root.root)?;
        ensure!(
            matches!(resolved,Some(hennery_kernel::hats::SessionHat::Decided(hat)) if hat.hat_id==input.hat_id),
            "invalid_grant"
        );
    }
    let id = uuid::Uuid::now_v7().to_string();
    let token = format!("{TOKEN_PREFIX}{}", hex::encode(random_bytes::<32>()));
    let expires = unix_now() + i64::from(input.expires_in_days) * 86400;
    tx.execute("INSERT INTO external_clients(owner_id,id,label,hat_id,token_hash,read_metadata,existing_sessions,revision,expires_at,read_transcript,control_sessions)
          VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9,?10)",params![owner,id,input.label.trim(),input.hat_id,sha256_hex(token.as_bytes()),input.read_metadata,input.existing_sessions,expires,input.read_transcript,input.control_sessions])?;
    for root in &input.roots {
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
