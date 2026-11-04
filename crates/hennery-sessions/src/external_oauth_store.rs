//! Durable authorization admission. OAuth clients are applications, never operators.
//! Every consent creates a new external grant; codes and tokens hold only hashes.
use crate::{
    external_clients::{self, NewClient, RootGrant},
    store::Store,
};
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hennery_kernel::{
    egress::Url,
    secret::{random_bytes, sha256_hex, unix_now},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const READ_SCOPE: &str = "hennery:sessions:read";
pub(crate) const DISCOVER_SCOPE: &str = "hennery:discover";
pub(crate) const SCOPE: &str = "hennery:discover hennery:sessions:read";
pub(crate) const TRANSCRIPT_SCOPE: &str = "hennery:sessions:transcript";
pub(crate) const CONTROL_SCOPE: &str = "hennery:sessions:control";
pub(crate) const DEFAULT_SCOPE: &str =
    "hennery:discover hennery:sessions:read hennery:sessions:transcript hennery:sessions:control";
pub(crate) fn secret(prefix: &str) -> String {
    format!("{prefix}{}", hex::encode(random_bytes::<32>()))
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Metadata {
    #[serde(default)]
    pub client_id: Option<String>,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    pub grant_types: Vec<String>,
    #[serde(default)]
    pub response_types: Vec<String>,
}
pub(crate) fn validate_metadata(m: &Metadata) -> Result<()> {
    ensure!(
        !m.client_name.trim().is_empty() && m.client_name.len() <= 128 && !m.client_name.chars().any(char::is_control),
        "invalid_client_metadata"
    );
    ensure!(
        m.token_endpoint_auth_method.as_deref() == Some("none"),
        "invalid_client_metadata"
    );
    ensure!(
        !m.redirect_uris.is_empty() && m.redirect_uris.len() <= 8,
        "invalid_client_metadata"
    );
    for uri in &m.redirect_uris {
        ensure!(redirect(uri).is_ok(), "invalid_redirect_uri");
    }
    ensure!(
        m.grant_types
            .iter()
            .all(|g| matches!(g.as_str(), "authorization_code" | "refresh_token")),
        "invalid_client_metadata"
    );
    ensure!(
        m.grant_types.is_empty() || m.grant_types.iter().any(|g| g == "authorization_code"),
        "invalid_client_metadata"
    );
    ensure!(
        m.response_types.is_empty() || m.response_types == ["code"],
        "invalid_client_metadata"
    );
    Ok(())
}
fn redirect(raw: &str) -> Result<Url> {
    ensure!(raw.len() <= 2048, "invalid_redirect_uri");
    let u = Url::parse(raw).map_err(|_| anyhow::anyhow!("invalid_redirect_uri"))?;
    ensure!(
        u.username().is_empty() && u.password().is_none() && u.fragment().is_none(),
        "invalid_redirect_uri"
    );
    ensure!(
        u.scheme() == "https"
            || u.scheme() == "http" && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        "invalid_redirect_uri"
    );
    ensure!(
        !u.query_pairs()
            .any(|(k, _)| matches!(k.as_ref(), "code" | "error" | "state" | "iss")),
        "invalid_redirect_uri"
    );
    Ok(u)
}
pub(crate) fn redirect_matches(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }
    let (Ok(mut a), Ok(mut b)) = (redirect(registered), redirect(requested)) else {
        return false;
    };
    if a.scheme() != "http" || b.scheme() != "http" {
        return false;
    }
    let _ = a.set_port(None);
    let _ = b.set_port(None);
    a == b // Only a validated loopback URI gets RFC8252 port flexibility.
}
#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Authorization {
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub resource: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}
#[derive(Clone, Serialize)]
pub(crate) struct Pending {
    pub request_id: String,
    pub client_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub resource: String,
    pub scope: String,
    pub state: Option<String>,
    pub expires_at: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Consent {
    pub request_id: String,
    pub allow: bool,
    #[serde(default)]
    pub hat_id: String,
    #[serde(default)]
    pub roots: Vec<RootGrant>,
    #[serde(default)]
    pub scope: Option<external_clients::GrantScope>,
    #[serde(default)]
    pub read_metadata: bool,
    #[serde(default)]
    pub read_transcript: bool,
    #[serde(default)]
    pub control_sessions: bool,
    #[serde(default)]
    pub existing_sessions: bool,
    #[serde(default = "days")]
    pub expires_in_days: u32,
}
fn days() -> u32 {
    7
}
#[derive(Deserialize)]
pub(crate) struct TokenRequest {
    pub grant_type: String,
    pub client_id: String,
    pub resource: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default)]
    pub code_verifier: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct TokenReply {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}
struct CodeAdmission {
    grant: String,
    client: String,
    redirect: String,
    challenge: String,
    scope: String,
    expires: i64,
    used: i64,
    target: String,
}
fn pending(conn: &rusqlite::Connection, owner: &str, id: &str) -> Result<Option<Pending>> {
    Ok(conn.query_row("SELECT id,client_id,client_name,redirect_uri,resource,scope,state,expires_at FROM external_oauth_requests WHERE owner_id = ?1 AND id=?2 AND consumed=0 AND code_hash IS NULL AND expires_at>?3",
      params![owner,id,unix_now()],|r|Ok(Pending{request_id:r.get(0)?,client_id:r.get(1)?,client_name:r.get(2)?,redirect_uri:r.get(3)?,resource:r.get(4)?,scope:r.get(5)?,state:r.get(6)?,expires_at:r.get(7)?})).optional()?)
}
fn valid_grant(conn: &rusqlite::Connection, owner: &str, grant: &str) -> Result<bool> {
    external_clients::active_grant(conn, owner, grant)
}
fn revoke(conn: &rusqlite::Connection, owner: &str, grant: &str) -> Result<()> {
    conn.execute("UPDATE external_clients SET revoked_at=COALESCE(revoked_at,?3),revision=revision+CASE WHEN revoked_at IS NULL THEN 1 ELSE 0 END WHERE owner_id = ?1 AND id=?2",params![owner,grant,unix_now()])?;
    Ok(())
}
impl Store {
    pub(crate) fn register_oauth_client(&self, mut m: Metadata) -> Result<Metadata> {
        if m.grant_types.is_empty() {
            m.grant_types = vec!["authorization_code".into()];
        }
        if m.response_types.is_empty() {
            m.response_types = vec!["code".into()];
        }
        validate_metadata(&m)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let owner = self.owner_id();
        tx.execute(
            "DELETE FROM external_oauth_clients WHERE owner_id = ?1 AND expires_at<?2",
            params![owner, unix_now()],
        )?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM external_oauth_clients WHERE owner_id = ?1",
            [owner],
            |r| r.get(0),
        )?;
        ensure!(count < 1000, "temporarily_unavailable");
        let id = uuid::Uuid::now_v7().to_string();
        m.client_id = Some(id.clone());
        tx.execute(
            "INSERT INTO external_oauth_clients(owner_id,id,metadata,expires_at) VALUES(?1,?2,?3,?4)",
            params![owner, id, serde_json::to_string(&m)?, unix_now() + 600],
        )?;
        tx.commit()?;
        Ok(m)
    }
    pub(crate) fn oauth_client(&self, id: &str) -> Result<Option<Metadata>> {
        let value: Option<String> = self
            .conn()
            .query_row(
                "SELECT metadata FROM external_oauth_clients WHERE owner_id = ?1 AND id=?2 AND expires_at>?3",
                params![self.owner_id(), id, unix_now()],
                |r| r.get(0),
            )
            .optional()?;
        value.map(|s| serde_json::from_str(&s).map_err(Into::into)).transpose()
    }
    pub(crate) fn begin_oauth(&self, a: &Authorization, m: &Metadata, resource: &str) -> Result<String> {
        validate_metadata(m)?;
        ensure!(
            a.client_id.len() <= 2048 && m.client_id.as_deref() == Some(a.client_id.as_str()),
            "invalid_client"
        );
        ensure!(a.response_type == "code", "unsupported_response_type");
        ensure!(a.resource == resource, "invalid_target");
        ensure!(
            a.state
                .as_ref()
                .is_none_or(|s| s.len() <= 1024 && !s.chars().any(char::is_control)),
            "invalid_request"
        );
        ensure!(
            a.code_challenge_method == "S256"
                && a.code_challenge.len() == 43
                && a.code_challenge
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid_request"
        );
        ensure!(
            m.redirect_uris.iter().any(|r| redirect_matches(r, &a.redirect_uri)),
            "invalid_redirect_uri"
        );
        let scope = a.scope.as_deref().unwrap_or(SCOPE);
        ensure!(scope.len() <= 256, "invalid_scope");
        let mut scopes = std::collections::BTreeSet::new();
        for s in scope.split_ascii_whitespace() {
            ensure!(
                matches!(
                    s,
                    DISCOVER_SCOPE | READ_SCOPE | TRANSCRIPT_SCOPE | CONTROL_SCOPE | "offline_access"
                ),
                "invalid_scope"
            );
            scopes.insert(s);
        }
        ensure!(scopes.contains(DISCOVER_SCOPE), "invalid_scope");
        ensure!(
            !scopes.contains("offline_access") || m.grant_types.iter().any(|s| s == "refresh_token"),
            "invalid_scope"
        );
        let scope = scopes.into_iter().collect::<Vec<_>>().join(" ");
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let owner = self.owner_id();
        tx.execute(
            "DELETE FROM external_oauth_requests WHERE owner_id = ?1 AND expires_at<?2",
            params![owner, unix_now()],
        )?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM external_oauth_requests WHERE owner_id = ?1",
            [owner],
            |r| r.get(0),
        )?;
        ensure!(count < 1000, "temporarily_unavailable");
        let id = secret("auth_");
        tx.execute("INSERT INTO external_oauth_requests(owner_id,id,client_id,client_name,redirect_uri,resource,scope,state,challenge,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![owner,id,a.client_id,m.client_name,a.redirect_uri,resource,scope,a.state,a.code_challenge,unix_now()+600])?;
        tx.commit()?;
        Ok(id)
    }
    pub(crate) fn pending_oauth(&self, id: &str, resource: &str) -> Result<Pending> {
        let p = pending(&self.conn(), self.owner_id(), id)?.ok_or_else(|| anyhow::anyhow!("invalid_request"))?;
        ensure!(p.resource == resource, "invalid_target");
        Ok(p)
    }
    pub(crate) fn consent_oauth(&self, c: &Consent, resource: &str, issuer: &str) -> Result<String> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let owner = self.owner_id();
        let p = pending(&tx, owner, &c.request_id)?.ok_or_else(|| anyhow::anyhow!("invalid_request"))?;
        ensure!(p.resource == resource, "invalid_target");
        let mut url = Url::parse(&p.redirect_uri)?;
        {
            let mut q = url.query_pairs_mut();
            if let Some(state) = &p.state {
                q.append_pair("state", state);
            }
            q.append_pair("iss", issuer);
        }
        if !c.allow {
            tx.execute(
                "UPDATE external_oauth_requests SET consumed=1 WHERE owner_id = ?1 AND id=?2",
                params![owner, p.request_id],
            )?;
            url.query_pairs_mut().append_pair("error", "access_denied");
            tx.commit()?;
            return Ok(url.into());
        }
        ensure!(
            !c.read_metadata || p.scope.split_ascii_whitespace().any(|s| s == READ_SCOPE),
            "invalid_scope"
        );
        for (selected, scope) in [
            (c.read_transcript, TRANSCRIPT_SCOPE),
            (c.control_sessions, CONTROL_SCOPE),
        ] {
            ensure!(
                !selected || p.scope.split_ascii_whitespace().any(|s| s == scope),
                "invalid_scope"
            );
        }
        let input = NewClient {
            scope: c.scope.clone(),
            label: p.client_name,
            hat_id: c.hat_id.clone(),
            roots: c.roots.clone(),
            read_metadata: c.read_metadata,
            read_transcript: c.read_transcript,
            control_sessions: c.control_sessions,
            existing_sessions: c.existing_sessions,
            expires_in_days: c.expires_in_days,
        };
        let (grant, _unpublished_static_credential) = external_clients::issue_in(&tx, owner, &input)?;
        tx.execute(
            "UPDATE external_oauth_clients SET expires_at=?3 WHERE owner_id = ?1 AND id=?2",
            params![owner, p.client_id, unix_now() + 30 * 86400],
        )?;
        // The unused static credential never leaves this transaction's stack.
        let code = secret("code_");
        let mut scopes = vec![DISCOVER_SCOPE];
        if c.read_metadata {
            scopes.push(READ_SCOPE);
        }
        if c.read_transcript {
            scopes.push(TRANSCRIPT_SCOPE);
        }
        if c.control_sessions {
            scopes.push(CONTROL_SCOPE);
        }
        if p.scope.split_ascii_whitespace().any(|s| s == "offline_access") {
            scopes.push("offline_access");
        }
        tx.execute("UPDATE external_oauth_requests SET code_hash=?3,code_expires_at=?4,grant_id=?5,scope=?6 WHERE owner_id = ?1 AND id=?2",
          params![owner,p.request_id,sha256_hex(code.as_bytes()),unix_now()+60,grant.client_id,scopes.join(" ")])?;
        url.query_pairs_mut().append_pair("code", &code);
        tx.commit()?;
        Ok(url.into())
    }
    pub(crate) fn exchange_oauth(&self, r: &TokenRequest, resource: &str) -> Result<TokenReply> {
        ensure!(r.client_secret.is_none(), "invalid_client");
        ensure!(r.resource == resource, "invalid_target");
        ensure!(r.client_id.len() <= 2048, "invalid_client");
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let owner = self.owner_id();
        let now = unix_now();
        let (grant, scope, expiry) = if r.grant_type == "authorization_code" {
            let code = r
                .code
                .as_deref()
                .filter(|s| s.len() <= 128)
                .ok_or_else(|| anyhow::anyhow!("invalid_grant"))?;
            let row:Option<CodeAdmission>=tx.query_row("SELECT grant_id,client_id,redirect_uri,challenge,scope,code_expires_at,consumed,resource FROM external_oauth_requests WHERE owner_id = ?1 AND code_hash=?2 AND expires_at>?3",params![owner,sha256_hex(code.as_bytes()),now],|q|Ok(CodeAdmission{grant:q.get(0)?,client:q.get(1)?,redirect:q.get(2)?,challenge:q.get(3)?,scope:q.get(4)?,expires:q.get(5)?,used:q.get(6)?,target:q.get(7)?})).optional()?;
            let CodeAdmission {
                grant,
                client,
                redirect,
                challenge,
                scope,
                expires,
                used,
                target,
            } = row.ok_or_else(|| anyhow::anyhow!("invalid_grant"))?;
            let verifier = r
                .code_verifier
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("invalid_grant"))?;
            ensure!(
                (43..=128).contains(&verifier.len())
                    && verifier
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b)),
                "invalid_grant"
            );
            let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            ensure!(
                client == r.client_id
                    && r.redirect_uri.as_deref() == Some(redirect.as_str())
                    && challenge == actual
                    && target == resource,
                "invalid_grant"
            );
            if used != 0 {
                revoke(&tx, owner, &grant)?;
                tx.commit()?;
                anyhow::bail!("invalid_grant");
            }
            ensure!(expires > now && valid_grant(&tx, owner, &grant)?, "invalid_grant");
            tx.execute(
                "UPDATE external_oauth_requests SET consumed=1 WHERE owner_id = ?1 AND code_hash=?2",
                params![owner, sha256_hex(code.as_bytes())],
            )?;
            let expiry: i64 = tx.query_row(
                "SELECT expires_at FROM external_clients WHERE owner_id = ?1 AND id=?2",
                params![owner, grant],
                |q| q.get(0),
            )?;
            (grant, scope, expiry)
        } else if r.grant_type == "refresh_token" {
            let refresh = r
                .refresh_token
                .as_deref()
                .filter(|s| s.len() <= 128)
                .ok_or_else(|| anyhow::anyhow!("invalid_grant"))?;
            let hash = sha256_hex(refresh.as_bytes());
            let row:Option<(String,String,i64)>=tx.query_row("SELECT grant_id,scope,expires_at FROM external_oauth_tokens WHERE owner_id = ?1 AND refresh_hash=?2 AND client_id=?3 AND resource=?4",params![owner,hash,r.client_id,resource],|q|Ok((q.get(0)?,q.get(1)?,q.get(2)?))).optional()?;
            let Some((grant, scope, expiry)) = row else {
                let old:Option<String>=tx.query_row("SELECT h.grant_id FROM external_oauth_refresh_history h JOIN external_oauth_tokens t ON t.owner_id = h.owner_id AND t.grant_id=h.grant_id WHERE h.owner_id = ?1 AND h.token_hash=?2 AND t.client_id=?3 AND t.resource=?4 AND h.expires_at>?5",params![owner,hash,r.client_id,resource,now],|q|q.get(0)).optional()?;
                if let Some(grant) = old {
                    revoke(&tx, owner, &grant)?;
                    tx.commit()?;
                }
                anyhow::bail!("invalid_grant");
            };
            ensure!(expiry > now && valid_grant(&tx, owner, &grant)?, "invalid_grant");
            if let Some(requested) = r.scope.as_deref() {
                let requested = requested
                    .split_ascii_whitespace()
                    .collect::<std::collections::BTreeSet<_>>();
                let granted = scope
                    .split_ascii_whitespace()
                    .collect::<std::collections::BTreeSet<_>>();
                // Refresh may repeat the same set in any order. Expansion and
                // downscoping still require separate policy/explicit consent.
                ensure!(requested == granted, "invalid_scope");
            }
            tx.execute(
                "DELETE FROM external_oauth_refresh_history WHERE owner_id = ?1 AND expires_at<?2",
                params![owner, now],
            )?;
            tx.execute("INSERT INTO external_oauth_refresh_history(owner_id,token_hash,grant_id,expires_at) VALUES(?1,?2,?3,?4)",params![owner,hash,grant,expiry])?;
            (grant, scope, expiry)
        } else {
            anyhow::bail!("unsupported_grant_type")
        };
        let access = secret(external_clients::TOKEN_PREFIX);
        let access_hash = sha256_hex(access.as_bytes());
        let refresh = scope
            .split_ascii_whitespace()
            .any(|s| s == "offline_access")
            .then(|| secret("hnry_refresh_"));
        let refresh_hash = refresh.as_ref().map(|s| sha256_hex(s.as_bytes()));
        let access_expiry = (now + 900).min(expiry);
        tx.execute("INSERT INTO external_oauth_tokens(owner_id,grant_id,client_id,resource,scope,access_hash,access_expires_at,refresh_hash,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(owner_id,grant_id) DO UPDATE SET access_hash=excluded.access_hash,access_expires_at=excluded.access_expires_at,refresh_hash=excluded.refresh_hash",
          params![owner,grant,r.client_id,resource,scope,access_hash,access_expiry,refresh_hash,expiry])?;
        tx.commit()?;
        Ok(TokenReply {
            access_token: access,
            token_type: "Bearer",
            expires_in: access_expiry - now,
            scope,
            refresh_token: refresh,
        })
    }
    pub(crate) fn revoke_oauth_token(&self, token: &str, client: &str, resource: &str) -> Result<()> {
        ensure!(token.len() <= 128 && client.len() <= 2048, "invalid_request");
        let conn = self.conn();
        let hash = sha256_hex(token.as_bytes());
        let grant:Option<String>=conn.query_row("SELECT grant_id FROM external_oauth_tokens WHERE owner_id = ?1 AND client_id=?2 AND resource=?3 AND (access_hash=?4 OR refresh_hash=?4)",params![self.owner_id(),client,resource,hash],|q|q.get(0)).optional()?;
        if let Some(grant) = grant {
            revoke(&conn, self.owner_id(), &grant)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_kernel::hosts::{Enrollment, Hosts};
    struct World {
        dir: tempfile::TempDir,
        store: Store,
        host: Hosts,
        hat: String,
    }
    impl World {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("test.db");
            let host = Hosts::open(&db).unwrap();
            host.register(
                "host",
                &Enrollment {
                    public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                    name: "Test".into(),
                    platform: "test".into(),
                    host_version: "test".into(),
                },
                0,
            )
            .unwrap();
            let hat = host.host("host").unwrap().unwrap().default_hat_id;
            Self {
                store: Store::open(&db).unwrap(),
                dir,
                host,
                hat,
            }
        }
        fn client(&self) -> Metadata {
            self.store
                .register_oauth_client(Metadata {
                    client_id: None,
                    client_name: "Remote app".into(),
                    redirect_uris: vec!["https://client.example/callback".into()],
                    token_endpoint_auth_method: Some("none".into()),
                    grant_types: vec!["authorization_code".into(), "refresh_token".into()],
                    response_types: vec!["code".into()],
                })
                .unwrap()
        }
        fn authorization(&self, m: &Metadata) -> Authorization {
            Authorization {
                client_id: m.client_id.clone().unwrap(),
                redirect_uri: m.redirect_uris[0].clone(),
                response_type: "code".into(),
                code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest("a".repeat(43).as_bytes())),
                code_challenge_method: "S256".into(),
                resource: RESOURCE.into(),
                scope: Some(format!("{SCOPE} offline_access")),
                state: Some("client-state".into()),
            }
        }
        fn consent(&self, id: &str) -> Consent {
            Consent {
                scope: None,
                request_id: id.into(),
                allow: true,
                hat_id: self.hat.clone(),
                roots: vec![RootGrant {
                    host_id: "host".into(),
                    root: "/project".into(),
                }],
                read_metadata: true,
                read_transcript: false,
                control_sessions: false,
                existing_sessions: false,
                expires_in_days: 7,
            }
        }
        fn admitted(&self) -> TokenRequest {
            self.admitted_with_metadata(true)
        }
        fn admitted_with_metadata(&self, read_metadata: bool) -> TokenRequest {
            let m = self.client();
            let a = self.authorization(&m);
            let id = self.store.begin_oauth(&a, &m, RESOURCE).unwrap();
            let mut consent = self.consent(&id);
            consent.read_metadata = read_metadata;
            let redirect = self.store.consent_oauth(&consent, RESOURCE, ISSUER).unwrap();
            let u = Url::parse(&redirect).unwrap();
            assert_eq!(u.query_pairs().find(|(k, _)| k == "state").unwrap().1, "client-state");
            assert_eq!(u.query_pairs().find(|(k, _)| k == "iss").unwrap().1, ISSUER);
            TokenRequest {
                grant_type: "authorization_code".into(),
                client_id: a.client_id,
                resource: RESOURCE.into(),
                code: Some(u.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string()),
                redirect_uri: Some(a.redirect_uri),
                code_verifier: Some("a".repeat(43)),
                refresh_token: None,
                scope: None,
                client_secret: None,
            }
        }
    }
    const ISSUER: &str = "https://collector.example";
    const RESOURCE: &str = "https://collector.example/mcp/external/hennery";
    #[test]
    fn control_and_transcript_require_explicit_requested_consent_and_survive_token_refresh() {
        let w = World::new();
        let m = w.client();
        let a = w.authorization(&m);
        let id = w.store.begin_oauth(&a, &m, RESOURCE).unwrap();
        let mut c = w.consent(&id);
        c.control_sessions = true;
        assert!(w.store.consent_oauth(&c, RESOURCE, ISSUER).is_err());
        c.control_sessions = false;
        c.read_transcript = true;
        assert!(w.store.consent_oauth(&c, RESOURCE, ISSUER).is_err());

        let mut a = w.authorization(&m);
        a.scope = Some(format!("{DEFAULT_SCOPE} offline_access"));
        let id = w.store.begin_oauth(&a, &m, RESOURCE).unwrap();
        let mut c = w.consent(&id);
        c.control_sessions = true;
        c.read_transcript = true;
        c.read_metadata = false;
        assert!(w.store.consent_oauth(&c, RESOURCE, ISSUER).is_err());
        c.read_metadata = true;
        c.existing_sessions = true;
        let redirect = w.store.consent_oauth(&c, RESOURCE, ISSUER).unwrap();
        let u = Url::parse(&redirect).unwrap();
        let request = TokenRequest {
            grant_type: "authorization_code".into(),
            client_id: a.client_id.clone(),
            resource: RESOURCE.into(),
            code: Some(u.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string()),
            redirect_uri: Some(a.redirect_uri),
            code_verifier: Some("a".repeat(43)),
            refresh_token: None,
            scope: None,
            client_secret: None,
        };
        let token = w.store.exchange_oauth(&request, RESOURCE).unwrap();
        let identity = w
            .store
            .authenticate_external_resource(&token.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        assert!(identity.client.control_sessions && identity.client.read_transcript);
        w.store
            .create_session("session", "host", "claude", "/project", &w.hat, None)
            .unwrap();
        w.store
            .external_admit(
                &identity,
                "session_close",
                "key",
                &serde_json::json!({"request_key":"key","session_id":"session"}),
            )
            .unwrap();
        let (command, _) = w.store.external_pending().unwrap().remove(0);
        let refresh = TokenRequest {
            grant_type: "refresh_token".into(),
            client_id: a.client_id,
            resource: RESOURCE.into(),
            code: None,
            redirect_uri: None,
            code_verifier: None,
            refresh_token: token.refresh_token,
            scope: None,
            client_secret: None,
        };
        let rotated = w.store.exchange_oauth(&refresh, RESOURCE).unwrap();
        assert!(
            w.store
                .authenticate_external_resource(&token.access_token, RESOURCE)
                .unwrap()
                .is_none()
        );
        assert!(w.store.external_guard_in(&w.store.conn(), &command, None).is_ok());
        assert!(
            w.store
                .authenticate_external_resource(&rotated.access_token, RESOURCE)
                .unwrap()
                .unwrap()
                .client
                .control_sessions
        );
    }
    #[test]
    fn registration_pkce_redirect_resource_and_scope_are_validated_before_consent() {
        let w = World::new();
        let m = w.client();
        let a = w.authorization(&m);
        for mode in 0..6 {
            let mut bad = a.clone();
            match mode {
                0 => bad.code_challenge_method = "plain".into(),
                1 => bad.code_challenge = "short".into(),
                2 => bad.redirect_uri = "https://attacker.example/callback".into(),
                3 => bad.resource = "https://other.example/mcp".into(),
                4 => bad.scope = Some("operator:approve".into()),
                _ => bad.client_id = "other".into(),
            };
            assert!(w.store.begin_oauth(&bad, &m, RESOURCE).is_err(), "{mode}");
        }
        assert!(redirect_matches(
            "http://127.0.0.1/callback",
            "http://127.0.0.1:12345/callback"
        ));
        assert!(redirect_matches(
            "http://localhost/callback",
            "http://localhost:12345/callback"
        ));
        assert!(!redirect_matches(
            "https://client.example/callback",
            "https://client.example:12345/callback"
        ));
        assert!(!redirect_matches(
            "http://127.0.0.1/callback",
            "http://evil.example:12345/callback"
        ));
        let mut defaults = m.clone();
        defaults.grant_types.clear();
        defaults.response_types.clear();
        let defaults = w.store.register_oauth_client(defaults).unwrap();
        assert_eq!(defaults.grant_types, ["authorization_code"]);
        assert_eq!(defaults.response_types, ["code"]);
        let mut bad = m;
        bad.token_endpoint_auth_method = Some("client_secret_basic".into());
        assert!(w.store.register_oauth_client(bad).is_err());
    }
    #[test]
    fn wrong_exchange_does_not_consume_code_and_success_is_durable_and_scoped() {
        let w = World::new();
        w.store
            .create_session("old", "host", "claude", "/project", &w.hat, None)
            .unwrap();
        let r = w.admitted();
        for mode in 0..4 {
            let mut bad = TokenRequest {
                grant_type: r.grant_type.clone(),
                client_id: r.client_id.clone(),
                resource: r.resource.clone(),
                code: r.code.clone(),
                redirect_uri: r.redirect_uri.clone(),
                code_verifier: r.code_verifier.clone(),
                refresh_token: None,
                scope: None,
                client_secret: None,
            };
            match mode {
                0 => bad.client_id = "other".into(),
                1 => bad.redirect_uri = Some("https://attacker.example/callback".into()),
                2 => bad.code_verifier = Some("b".repeat(43)),
                _ => bad.resource = "https://other.example/mcp".into(),
            };
            assert!(w.store.exchange_oauth(&bad, RESOURCE).is_err());
        }
        let t = w.store.exchange_oauth(&r, RESOURCE).unwrap();
        assert!(t.refresh_token.is_some());
        assert_eq!(t.expires_in, 900);
        let reopened = Store::open(&w.dir.path().join("test.db")).unwrap();
        let id = reopened
            .authenticate_external_resource(&t.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        assert!(reopened.external_sessions(&id, "", None, 25).unwrap().is_empty());
        assert!(
            reopened
                .authenticate_external_resource(&t.access_token, "https://other.example/mcp")
                .unwrap()
                .is_none()
        );
        assert!(reopened.authenticate_external(&t.access_token).unwrap().is_none());
        let stored: String = reopened
            .conn()
            .query_row("SELECT access_hash FROM external_oauth_tokens", [], |q| q.get(0))
            .unwrap();
        assert_ne!(stored, t.access_token);
    }
    #[test]
    fn authorization_code_replay_revokes_issued_family_and_cached_principal() {
        let w = World::new();
        let r = w.admitted();
        let token = w.store.exchange_oauth(&r, RESOURCE).unwrap();
        let id = w
            .store
            .authenticate_external_resource(&token.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        assert!(w.store.exchange_oauth(&r, RESOURCE).is_err());
        assert!(w.store.external_roots(&id).is_err());
        assert!(
            w.store
                .authenticate_external_resource(&token.access_token, RESOURCE)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn refresh_rotates_and_reuse_revokes_family_without_cross_client_dos() {
        let w = World::new();
        let r = w.admitted();
        let first = w.store.exchange_oauth(&r, RESOURCE).unwrap();
        let old_id = w
            .store
            .authenticate_external_resource(&first.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        let mut refresh = TokenRequest {
            grant_type: "refresh_token".into(),
            client_id: "other".into(),
            resource: RESOURCE.into(),
            code: None,
            redirect_uri: None,
            code_verifier: None,
            refresh_token: first.refresh_token,
            scope: None,
            client_secret: None,
        };
        assert!(w.store.exchange_oauth(&refresh, RESOURCE).is_err());
        assert!(w.store.external_roots(&old_id).is_ok());
        refresh.client_id = r.client_id;
        let second = w.store.exchange_oauth(&refresh, RESOURCE).unwrap();
        assert!(w.store.external_roots(&old_id).is_err());
        assert_ne!(refresh.refresh_token, second.refresh_token);
        assert!(
            w.store
                .authenticate_external_resource(&second.access_token, RESOURCE)
                .unwrap()
                .is_some()
        );
        assert!(w.store.exchange_oauth(&refresh, RESOURCE).is_err());
        assert!(
            w.store
                .authenticate_external_resource(&second.access_token, RESOURCE)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn refresh_scope_is_an_order_independent_equal_set_and_rejections_do_not_rotate_or_expand() {
        let w = World::new();
        let admission = w.admitted_with_metadata(false);
        let first = w.store.exchange_oauth(&admission, RESOURCE).unwrap();
        assert_eq!(first.scope, format!("{DISCOVER_SCOPE} offline_access"));
        let first_identity = w
            .store
            .authenticate_external_resource(&first.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        let mut request = TokenRequest {
            grant_type: "refresh_token".into(),
            client_id: admission.client_id,
            resource: RESOURCE.into(),
            code: None,
            redirect_uri: None,
            code_verifier: None,
            refresh_token: first.refresh_token.clone(),
            scope: None,
            client_secret: None,
        };
        for invalid in [format!("{SCOPE} offline_access"), DISCOVER_SCOPE.into()] {
            request.scope = Some(invalid);
            assert_eq!(
                w.store
                    .exchange_oauth(&request, RESOURCE)
                    .err()
                    .expect("scope must be rejected")
                    .to_string(),
                "invalid_scope"
            );
            assert!(w.store.external_roots(&first_identity).is_ok());
            assert!(
                w.store
                    .authenticate_external_resource(&first.access_token, RESOURCE)
                    .unwrap()
                    .is_some()
            );
        }
        // Omitted scope succeeds on the same refresh token after both rejected
        // expansion and downscoping; neither rejection consumed or revoked it.
        request.scope = None;
        let second = w.store.exchange_oauth(&request, RESOURCE).unwrap();
        assert_eq!(second.scope, first.scope);
        assert_ne!(second.refresh_token, first.refresh_token);
        assert!(w.store.external_roots(&first_identity).is_err());
        let second_identity = w
            .store
            .authenticate_external_resource(&second.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        assert!(!second_identity.client.read_metadata);
        request.refresh_token = second.refresh_token.clone();
        request.scope = Some(format!("{SCOPE} offline_access"));
        assert_eq!(
            w.store
                .exchange_oauth(&request, RESOURCE)
                .err()
                .expect("scope must be rejected")
                .to_string(),
            "invalid_scope"
        );
        assert!(w.store.external_roots(&second_identity).is_ok());

        request.scope = Some(format!("offline_access {DISCOVER_SCOPE}"));
        let third = w.store.exchange_oauth(&request, RESOURCE).unwrap();
        assert_eq!(third.scope, first.scope);
        assert_ne!(third.refresh_token, second.refresh_token);
        assert!(w.store.external_roots(&second_identity).is_err());
        let third_identity = w
            .store
            .authenticate_external_resource(&third.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        assert!(!third_identity.client.read_metadata);
        // The actual successful rotation, unlike invalid_scope, spends the
        // previous token. Replaying it still revokes the family.
        assert_eq!(
            w.store
                .exchange_oauth(&request, RESOURCE)
                .err()
                .expect("spent refresh must fail")
                .to_string(),
            "invalid_grant"
        );
        assert!(w.store.external_roots(&third_identity).is_err());
        assert!(
            w.store
                .authenticate_external_resource(&third.access_token, RESOURCE)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn consent_is_one_use_denial_is_inert_and_requested_scope_cannot_be_expanded() {
        let w = World::new();
        let m = w.client();
        let mut a = w.authorization(&m);
        a.scope = Some(DISCOVER_SCOPE.into());
        let id = w.store.begin_oauth(&a, &m, RESOURCE).unwrap();
        let mut c = w.consent(&id);
        assert!(w.store.consent_oauth(&c, RESOURCE, ISSUER).is_err());
        assert!(w.store.external_clients().unwrap().is_empty());
        c.allow = false;
        let redirect = w.store.consent_oauth(&c, RESOURCE, ISSUER).unwrap();
        assert!(redirect.contains("error=access_denied"));
        assert!(!redirect.contains("code="));
        assert!(w.store.consent_oauth(&c, RESOURCE, ISSUER).is_err());
        assert!(w.store.external_clients().unwrap().is_empty());
    }
    #[test]
    fn expired_requests_codes_access_tokens_and_purged_hats_fail_closed() {
        let w = World::new();
        let m = w.client();
        let a = w.authorization(&m);
        let id = w.store.begin_oauth(&a, &m, RESOURCE).unwrap();
        w.store
            .conn()
            .execute("UPDATE external_oauth_requests SET expires_at=0", [])
            .unwrap();
        assert!(w.store.pending_oauth(&id, RESOURCE).is_err());
        let r = w.admitted();
        w.store
            .conn()
            .execute("UPDATE external_oauth_requests SET code_expires_at=0", [])
            .unwrap();
        assert!(w.store.exchange_oauth(&r, RESOURCE).is_err());
        let r = w.admitted();
        let t = w.store.exchange_oauth(&r, RESOURCE).unwrap();
        let identity = w
            .store
            .authenticate_external_resource(&t.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        w.store
            .conn()
            .execute("UPDATE external_oauth_tokens SET access_expires_at=0", [])
            .unwrap();
        assert!(w.store.external_roots(&identity).is_err());
        let r = w.admitted();
        let t = w.store.exchange_oauth(&r, RESOURCE).unwrap();
        w.store
            .conn()
            .execute(
                "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES(?1,?2,0)",
                params![w.store.owner_id(), w.hat],
            )
            .unwrap();
        assert!(
            w.store
                .authenticate_external_resource(&t.access_token, RESOURCE)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn revocation_ignores_foreign_client_and_invalidates_access_refresh_and_cached_identity() {
        let w = World::new();
        let r = w.admitted();
        let t = w.store.exchange_oauth(&r, RESOURCE).unwrap();
        let identity = w
            .store
            .authenticate_external_resource(&t.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        w.store.revoke_oauth_token(&t.access_token, "other", RESOURCE).unwrap();
        assert!(w.store.external_roots(&identity).is_ok());
        w.store
            .revoke_oauth_token(t.refresh_token.as_deref().unwrap(), &r.client_id, RESOURCE)
            .unwrap();
        assert!(w.store.external_roots(&identity).is_err());
    }
    #[test]
    fn multi_hat_oauth_consent_refresh_reopen_and_partial_purge_preserve_the_exact_selection() {
        use crate::external_clients::GrantScope;
        use hennery_kernel::hats::HatChange;
        let w = World::new();
        let HatChange::Done(second) = w.host.create_hat("Second", None, 0).unwrap() else {
            panic!()
        };
        let HatChange::Done(excluded) = w.host.create_hat("Excluded", None, 0).unwrap() else {
            panic!()
        };
        let m = w.client();
        let mut a = w.authorization(&m);
        a.scope = Some(format!("{SCOPE} {CONTROL_SCOPE} {TRANSCRIPT_SCOPE} offline_access"));
        let request = w.store.begin_oauth(&a, &m, RESOURCE).unwrap();
        let mut consent = w.consent(&request);
        consent.hat_id.clear();
        consent.roots.clear();
        consent.scope = Some(GrantScope {
            hat_ids: vec![w.hat.clone(), second.id.clone()],
            all_hats: false,
            roots: vec![RootGrant {
                host_id: "host".into(),
                root: "/".into(),
            }],
            all_hosts: false,
        });
        consent.read_transcript = true;
        consent.control_sessions = true;
        consent.existing_sessions = true;
        let callback = w.store.consent_oauth(&consent, RESOURCE, ISSUER).unwrap();
        let code = Url::parse(&callback)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap()
            .1
            .into_owned();
        let request = TokenRequest {
            grant_type: "authorization_code".into(),
            client_id: a.client_id.clone(),
            resource: RESOURCE.into(),
            code: Some(code),
            redirect_uri: Some(a.redirect_uri),
            code_verifier: Some("a".repeat(43)),
            refresh_token: None,
            scope: None,
            client_secret: None,
        };
        let first = w.store.exchange_oauth(&request, RESOURCE).unwrap();
        let identity = w
            .store
            .authenticate_external_resource(&first.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        assert_eq!(identity.client.scope.hat_ids.len(), 2);
        assert!(identity.client.hat_id.is_none());
        assert!(!identity.client.allows_hat(&excluded.id));
        w.store
            .create_session("second-session", "host", "fake", "/project", &second.id, None)
            .unwrap();
        w.store
            .create_session("excluded-session", "host", "fake", "/project", &excluded.id, None)
            .unwrap();
        assert_eq!(w.store.external_sessions(&identity, "", None, 50).unwrap().len(), 1);
        w.store
            .conn()
            .execute(
                "INSERT INTO purged_hats(owner_id,hat_id,purged_at) VALUES(?1,?2,0)",
                params![w.store.owner_id(), w.hat],
            )
            .unwrap();
        let refreshed = w
            .store
            .exchange_oauth(
                &TokenRequest {
                    grant_type: "refresh_token".into(),
                    client_id: a.client_id,
                    resource: RESOURCE.into(),
                    code: None,
                    redirect_uri: None,
                    code_verifier: None,
                    refresh_token: first.refresh_token,
                    scope: None,
                    client_secret: None,
                },
                RESOURCE,
            )
            .unwrap();
        let reopened = Store::open(&w.dir.path().join("test.db")).unwrap();
        let id = reopened
            .authenticate_external_resource(&refreshed.access_token, RESOURCE)
            .unwrap()
            .unwrap();
        assert_eq!(id.client.scope, identity.client.scope);
        assert!(id.client.control_sessions && id.client.read_transcript);
        assert_eq!(
            reopened.external_sessions(&id, "", None, 50).unwrap()[0].session_id,
            "second-session"
        );
        reopened.revoke_external_client(&id.client.client_id).unwrap();
        assert!(
            w.store
                .authenticate_external_resource(&refreshed.access_token, RESOURCE)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn current_hat_root_checks_apply_at_consent_and_existing_access_requires_opt_in() {
        let w = World::new();
        let m = w.client();
        let a = w.authorization(&m);
        let id = w.store.begin_oauth(&a, &m, RESOURCE).unwrap();
        let mut c = w.consent(&id);
        c.hat_id = "foreign".into();
        assert!(w.store.consent_oauth(&c, RESOURCE, ISSUER).is_err());
        assert!(w.store.external_clients().unwrap().is_empty());
        c.hat_id = w.hat.clone();
        c.roots[0].host_id = "foreign".into();
        assert!(w.store.consent_oauth(&c, RESOURCE, ISSUER).is_err());
        c.roots[0].host_id = "host".into();
        c.existing_sessions = true;
        w.store.consent_oauth(&c, RESOURCE, ISSUER).unwrap();
        assert!(w.store.external_clients().unwrap()[0].existing_sessions);
        // Keep the fixture's actual paired-host store alive throughout the checks.
        assert!(w.host.host("host").unwrap().is_some());
    }
}
