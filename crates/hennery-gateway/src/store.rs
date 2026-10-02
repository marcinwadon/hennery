//! The gateway's store (gateway spec §2): connections, the hosts they are
//! mounted on, and their credentials, sealed (`crypto`). It keeps its own
//! connection to `hennery.db`, beside the kernel's and the sessions
//! store's, and every query names the database's owner (kernel spec §1).
//! Every SQL statement of the gateway is in this file, which the owner
//! audit reads (`hennery-testkit/tests/owner_filter.rs`).
//!
//! What lists a connection reads of `gw_credentials` only whether a row is
//! there (plan 8a decision 3): never the ciphertext, nor its version. Of
//! `gw_oauth_clients` it reads the client ids and whether a secret is
//! stored, a column of its own, never a ciphertext (plan 8f).
//!
//! Every write of a credential or an OAuth client is in this file, on this
//! one connection. A refresh's write is a compare-and-swap on the sealed
//! blob it read (plan 8f decision 9), so a writer that cannot take the
//! connection's refresh lock (a hat purge from the kernel's hook) cannot be
//! overwritten by a late refresh either.

use crate::crypto::{self, CLIENT_SECRET, OAUTH_TOKENS, PENDING_CLIENT_SECRET, STATIC_TOKEN};
use crate::key::MasterKey;
use crate::model::{
    AuthMethod, Change, ConnectionPatch, ConnectionRecord, CredKind, CredentialChange, DEFAULT_HEADER, DEFAULT_PREFIX,
    GrantTokens, MAX_CONNECTIONS, MAX_HOST_ID, MAX_MOUNTS, NewConnection, OauthClientView, OauthCredential, OauthError,
    StaticCredential, Status, StatusChange, TokenClient, client_id_problem, client_secret_problem, header_problem,
    label_problem, parse_url, prefix_problem, slug_problem, token_problem, tools,
};
use crate::schema::{COMPONENT, MIGRATIONS};
use anyhow::{Context, Result, anyhow};
use hennery_kernel::db;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use zeroize::Zeroizing;

/// A connection's columns, then whether it has a credential, then its OAuth
/// client's ids and whether each has a secret: `?1` is the owner, `?2` one
/// connection's id or `NULL` for all of them.
const SELECT_CONNECTIONS: &str = "SELECT c.id, c.slug, c.label, c.url, c.hat_id, c.cred_kind, c.static_header,
            c.static_prefix, c.tool_allowlist, c.internal_network, c.status, c.status_note, c.account_label,
            c.status_at, c.created_at, c.updated_at,
            EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = c.id AND k.owner_id = ?1),
            c.checked_at, c.oauth_error_code, c.oauth_error_message, c.oauth_error_at, c.resource_mismatch,
            c.accepted_resource, o.client_id, o.has_client_secret, o.pending_client_id, o.pending_has_secret
     FROM gw_connections c
     LEFT JOIN gw_oauth_clients o ON o.connection_id = c.id AND o.owner_id = ?1
     WHERE c.owner_id = ?1 AND (?2 IS NULL OR c.id = ?2)
     ORDER BY c.created_at, c.id";

/// The mounts on hosts that are not revoked (plan 8a decision 12): `?1`
/// is the owner, `?2` one connection's id or `NULL` for all of them.
const SELECT_MOUNTS: &str = "SELECT m.connection_id, m.host_id
     FROM gw_mounts m JOIN hosts h ON h.id = m.host_id AND h.owner_id = ?1
     WHERE m.owner_id = ?1 AND h.revoked_at IS NULL AND (?2 IS NULL OR m.connection_id = ?2)
     ORDER BY m.connection_id, m.host_id";

pub struct GatewayStore {
    conn: Mutex<Connection>,
    /// The database's owner (`db::kernel_owner`), whom every query names.
    owner: String,
}

impl GatewayStore {
    /// Open the gateway's tables in `hennery.db`, migrating the kernel's
    /// first (the gateway's reference them) and then its own.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(db::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(db::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        let owner = db::kernel_owner(&mut conn)?;
        db::migrate_component(&mut conn, COMPONENT, MIGRATIONS)?;
        Ok(Self {
            conn: Mutex::new(conn),
            owner,
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("gateway store lock")
    }

    /// The owner whose connections these are.
    pub fn owner_id(&self) -> &str {
        &self.owner
    }

    /// Every connection, oldest first.
    pub fn list(&self) -> Result<Vec<ConnectionRecord>> {
        read(&self.conn(), &self.owner, None)
    }

    pub fn connection(&self, id: &str) -> Result<Option<ConnectionRecord>> {
        Ok(read(&self.conn(), &self.owner, Some(id))?.pop())
    }

    /// A new connection (gateway spec §1), not connected, mounted nowhere.
    pub fn create(&self, new: &NewConnection, now: i64) -> Result<Change> {
        if let Some(problem) = slug_problem(&new.slug).or_else(|| label_problem(&new.label)) {
            return Ok(Change::Invalid(problem));
        }
        let url = match parse_url(&new.url, new.internal_network) {
            Ok(url) => url,
            Err(why) => return Ok(Change::Invalid(why)),
        };
        let header = new.static_header.as_deref().unwrap_or(DEFAULT_HEADER);
        let prefix = new.static_prefix.as_deref().unwrap_or(DEFAULT_PREFIX);
        if let Some(problem) = header_problem(header).or_else(|| prefix_problem(prefix)) {
            return Ok(Change::Invalid(problem));
        }
        let allowlist = match new.tool_allowlist.as_deref().map(tools).transpose() {
            Ok(list) => list,
            Err(why) => return Ok(Change::Invalid(why)),
        };
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let hat = tx
            .query_row(
                "SELECT 1 FROM hats WHERE id = ?1 AND owner_id = ?2",
                [&new.hat_id, &self.owner],
                |_| Ok(()),
            )
            .optional()?;
        if hat.is_none() {
            return Ok(Change::Invalid(format!("no hat {:?}", new.hat_id)));
        }
        let taken = tx
            .query_row(
                "SELECT 1 FROM gw_connections WHERE slug = ?1 AND owner_id = ?2",
                [&new.slug, &self.owner],
                |_| Ok(()),
            )
            .optional()?;
        if taken.is_some() {
            return Ok(Change::SlugTaken);
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM gw_connections WHERE owner_id = ?1",
            [&self.owner],
            |r| r.get(0),
        )?;
        if count >= MAX_CONNECTIONS as i64 {
            return Ok(Change::TooMany);
        }
        let id = format!("conn-{}", hex::encode(hennery_kernel::secret::random_bytes::<8>()));
        tx.execute(
            "INSERT INTO gw_connections(id, owner_id, slug, label, url, hat_id, cred_kind, static_header,
                                        static_prefix, tool_allowlist, internal_network, status_at, created_at,
                                        updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?12)",
            params![
                id,
                self.owner,
                new.slug,
                new.label.trim(),
                url.as_str(),
                new.hat_id,
                new.cred_kind.as_str(),
                header,
                prefix,
                allowlist.map(|list| serde_json::to_string(&list)).transpose()?,
                new.internal_network,
                now,
            ],
        )?;
        let created = read(&tx, &self.owner, Some(&id))?.pop();
        tx.commit()?;
        Ok(created.map_or(Change::NotFound, |record| Change::Done(Box::new(record))))
    }

    /// Change what `patch` names (gateway spec §4.6). Another origin or
    /// another kind deletes the connection's credential and its OAuth
    /// client first, in the same transaction, and its status starts over:
    /// the token was granted for the old upstream and the old kind (G-14).
    /// Any change of the URL, a path's too, clears a `resource` found or
    /// accepted for the old one (plan 8f decision F1). The caller holds the
    /// connection's refresh lock (gateway spec §4.5).
    pub fn update(&self, id: &str, patch: &ConnectionPatch, now: i64) -> Result<Change> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let Some(current) = read(&tx, &self.owner, Some(id))?.pop() else {
            return Ok(Change::NotFound);
        };
        let label = patch.label.as_deref().unwrap_or(&current.label);
        let kind = patch.cred_kind.unwrap_or(current.cred_kind);
        let internal_network = patch.internal_network.unwrap_or(current.internal_network);
        let header = patch.static_header.as_deref().unwrap_or(&current.static_header);
        let prefix = patch.static_prefix.as_deref().unwrap_or(&current.static_prefix);
        if let Some(problem) = label_problem(label) {
            return Ok(Change::Invalid(problem));
        }
        let url = match parse_url(patch.url.as_deref().unwrap_or(&current.url), internal_network) {
            Ok(url) => url,
            Err(why) => return Ok(Change::Invalid(why)),
        };
        if let Some(problem) = header_problem(header).or_else(|| prefix_problem(prefix)) {
            return Ok(Change::Invalid(problem));
        }
        let allowlist = match &patch.tool_allowlist {
            None => current.tool_allowlist.clone(),
            Some(None) => None,
            Some(Some(list)) => match tools(list) {
                Ok(list) => Some(list),
                Err(why) => return Ok(Change::Invalid(why)),
            },
        };
        let old_origin = url::Url::parse(&current.url).context("a stored url")?.origin();
        if url.origin() != old_origin || kind != current.cred_kind {
            tx.execute(
                "DELETE FROM gw_credentials WHERE connection_id = ?1 AND owner_id = ?2",
                [id, &self.owner],
            )?;
            tx.execute(
                "DELETE FROM gw_oauth_clients WHERE connection_id = ?1 AND owner_id = ?2",
                [id, &self.owner],
            )?;
            tx.execute(
                "UPDATE gw_connections SET status = 'not_connected', status_note = NULL, account_label = NULL,
                                           status_at = ?3, oauth_error_code = NULL, oauth_error_message = NULL,
                                           oauth_error_at = NULL
                 WHERE id = ?1 AND owner_id = ?2",
                params![id, self.owner, now],
            )?;
        }
        if url.as_str() != current.url {
            tx.execute(
                "UPDATE gw_connections SET resource_mismatch = NULL, accepted_resource = NULL
                 WHERE id = ?1 AND owner_id = ?2",
                [id, &self.owner],
            )?;
        }
        tx.execute(
            "UPDATE gw_connections SET label = ?3, url = ?4, cred_kind = ?5, static_header = ?6, static_prefix = ?7,
                                       tool_allowlist = ?8, internal_network = ?9, updated_at = ?10
             WHERE id = ?1 AND owner_id = ?2",
            params![
                id,
                self.owner,
                label.trim(),
                url.as_str(),
                kind.as_str(),
                header,
                prefix,
                allowlist.map(|list| serde_json::to_string(&list)).transpose()?,
                internal_network,
                now,
            ],
        )?;
        let updated = read(&tx, &self.owner, Some(id))?.pop();
        tx.commit()?;
        Ok(updated.map_or(Change::NotFound, |record| Change::Done(Box::new(record))))
    }

    /// Delete a connection with its credential, OAuth client and mounts.
    /// False if there was none.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM gw_credentials WHERE connection_id = ?1 AND owner_id = ?2",
            [id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM gw_oauth_clients WHERE connection_id = ?1 AND owner_id = ?2",
            [id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM gw_mounts WHERE connection_id = ?1 AND owner_id = ?2",
            [id, &self.owner],
        )?;
        let deleted = tx.execute(
            "DELETE FROM gw_connections WHERE id = ?1 AND owner_id = ?2",
            [id, &self.owner],
        )?;
        tx.commit()?;
        Ok(deleted == 1)
    }

    /// Replace the hosts a connection is mounted on (gateway spec §9: the
    /// full set, never a delta). Each must be the owner's and not revoked,
    /// or nothing changes.
    pub fn replace_mounts(&self, id: &str, host_ids: &[String]) -> Result<Change> {
        // Bounded before the transaction: each id is a query under the lock.
        if host_ids.len() > MAX_MOUNTS {
            return Ok(Change::Invalid(format!(
                "a connection is mounted on at most {MAX_MOUNTS} hosts"
            )));
        }
        if host_ids.iter().any(|host| host.len() > MAX_HOST_ID) {
            return Ok(Change::Invalid(format!("a host id is at most {MAX_HOST_ID} bytes")));
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if read(&tx, &self.owner, Some(id))?.is_empty() {
            return Ok(Change::NotFound);
        }
        let mut hosts: Vec<&String> = host_ids.iter().collect();
        hosts.sort();
        hosts.dedup();
        for host in &hosts {
            let live = tx
                .query_row(
                    "SELECT 1 FROM hosts WHERE id = ?1 AND owner_id = ?2 AND revoked_at IS NULL",
                    [host.as_str(), &self.owner],
                    |_| Ok(()),
                )
                .optional()?;
            if live.is_none() {
                return Ok(Change::Invalid(format!("no host {host:?}, or it is revoked")));
            }
        }
        tx.execute(
            "DELETE FROM gw_mounts WHERE connection_id = ?1 AND owner_id = ?2",
            [id, &self.owner],
        )?;
        for host in hosts {
            tx.execute(
                "INSERT INTO gw_mounts(connection_id, host_id, owner_id) VALUES (?1, ?2, ?3)",
                [id, host.as_str(), &self.owner],
            )?;
        }
        let mounted = read(&tx, &self.owner, Some(id))?.pop();
        tx.commit()?;
        Ok(mounted.map_or(Change::NotFound, |record| Change::Done(Box::new(record))))
    }

    /// Store `token` as the connection's static credential, sealed under
    /// `key`, replacing any before it. Only a `static` connection takes one.
    pub fn set_static_credential(&self, id: &str, token: &str, key: &MasterKey, now: i64) -> Result<CredentialChange> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let kind: Option<String> = tx
            .query_row(
                "SELECT cred_kind FROM gw_connections WHERE id = ?1 AND owner_id = ?2",
                [id, &self.owner],
                |r| r.get(0),
            )
            .optional()?;
        let Some(kind) = kind else {
            return Ok(CredentialChange::NotFound);
        };
        let kind = CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?;
        if kind != CredKind::Static {
            return Ok(CredentialChange::WrongKind(kind));
        }
        if let Some(problem) = token_problem(token) {
            return Ok(CredentialChange::Invalid(problem));
        }
        let sealed = crypto::seal(key, id, STATIC_TOKEN, token.as_bytes());
        tx.execute(
            "INSERT INTO gw_credentials(connection_id, owner_id, key_version, ciphertext, expires_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)
             ON CONFLICT(connection_id) DO UPDATE SET key_version = excluded.key_version,
                 ciphertext = excluded.ciphertext, expires_at = NULL, updated_at = excluded.updated_at
                 WHERE gw_credentials.owner_id = excluded.owner_id",
            params![id, self.owner, key.version(), sealed, now],
        )?;
        tx.commit()?;
        Ok(CredentialChange::Done)
    }

    /// A `static` connection's token, opened with `key`, with where it goes,
    /// all read in one statement (`StaticCredential`); `None` without one.
    /// An error if it does not open: another key, or a blob moved from
    /// another row.
    pub fn static_credential(&self, id: &str, key: &MasterKey) -> Result<Option<StaticCredential>> {
        type Row = (u32, Vec<u8>, String, String, String, String, bool);
        let row: Option<Row> = self
            .conn()
            .query_row(
                "SELECT k.key_version, k.ciphertext, c.cred_kind, c.url, c.static_header, c.static_prefix,
                        c.internal_network
                 FROM gw_credentials k JOIN gw_connections c ON c.id = k.connection_id AND c.owner_id = k.owner_id
                 WHERE k.connection_id = ?1 AND k.owner_id = ?2",
                [id, &self.owner],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((version, blob, kind, url, static_header, static_prefix, internal_network)) = row else {
            return Ok(None);
        };
        if kind != CredKind::Static.as_str() {
            return Ok(None);
        }
        let mut opened =
            crypto::open(key, id, STATIC_TOKEN, version, &blob).with_context(|| format!("connection {id}"))?;
        // Moved out of the zeroizing buffer, not copied; wiped on the error
        // path too.
        let token = match String::from_utf8(std::mem::take(&mut *opened)) {
            Ok(token) => Zeroizing::new(token),
            Err(err) => {
                drop(Zeroizing::new(err.into_bytes()));
                return Err(anyhow!("connection {id}: not a token"));
            }
        };
        Ok(Some(StaticCredential {
            token,
            url,
            static_header,
            static_prefix,
            internal_network,
        }))
    }

    /// An `oauth_client` connection's pre-registered client (gateway spec
    /// §4.2, api-8e-8f B2), replacing the one before. With a live grant it
    /// is saved as pending and replaces the grant's client only when a
    /// Connect with it completes (G-7); without one it is the client at
    /// once, its pins cleared (the review's R2: the next authorize pins it
    /// again). `Keep` keeps the stored secret only while `client_id` is
    /// that of the client this replaces (the pending one if any, else the
    /// active one): a secret never moves to another client. The caller
    /// holds the connection's refresh lock and drops its live flows.
    pub fn set_oauth_client(
        &self,
        id: &str,
        client_id: &str,
        secret: SecretInput,
        key: &MasterKey,
        now: i64,
    ) -> Result<ClientChange> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let kind: Option<String> = tx
            .query_row(
                "SELECT cred_kind FROM gw_connections WHERE id = ?1 AND owner_id = ?2",
                [id, &self.owner],
                |r| r.get(0),
            )
            .optional()?;
        let Some(kind) = kind else {
            return Ok(ClientChange::NotFound);
        };
        let kind = CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?;
        if kind != CredKind::OauthClient {
            return Ok(ClientChange::WrongKind(kind));
        }
        if let Some(problem) = client_id_problem(client_id) {
            return Ok(ClientChange::Invalid(problem));
        }
        if let SecretInput::Set(secret) = &secret
            && let Some(problem) = client_secret_problem(secret)
        {
            return Ok(ClientChange::Invalid(problem));
        }
        let has_grant: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM gw_credentials WHERE connection_id = ?1 AND owner_id = ?2)",
            [id, &self.owner],
            |r| r.get(0),
        )?;
        type Row = (String, Option<u32>, Option<Vec<u8>>, Option<String>, Option<Vec<u8>>);
        let stored: Option<Row> = tx
            .query_row(
                "SELECT client_id, key_version, client_secret_ciphertext, pending_client_id, pending_secret_ciphertext
                 FROM gw_oauth_clients WHERE connection_id = ?1 AND owner_id = ?2",
                [id, &self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        // The client this replaces, and its secret, opened.
        let replaced: Option<(String, Option<Zeroizing<String>>)> = match &stored {
            None => None,
            Some((_, version, _, Some(pending), pending_blob)) => Some((
                pending.clone(),
                open_secret(key, id, PENDING_CLIENT_SECRET, *version, pending_blob)?,
            )),
            Some((active, version, blob, None, _)) => {
                Some((active.clone(), open_secret(key, id, CLIENT_SECRET, *version, blob)?))
            }
        };
        let secret: Option<Zeroizing<String>> = match secret {
            SecretInput::Set(secret) => Some(secret),
            SecretInput::Clear => None,
            SecretInput::Keep => match replaced {
                Some((replaced_id, kept)) if replaced_id == client_id => kept,
                _ => {
                    return Ok(ClientChange::Invalid(
                        "a new client_id needs client_secret: send the secret, or null for a public client".into(),
                    ));
                }
            },
        };
        let has_secret = secret.is_some();
        if has_grant && stored.is_some() {
            let sealed = secret
                .as_ref()
                .map(|s| crypto::seal(key, id, PENDING_CLIENT_SECRET, s.as_bytes()));
            tx.execute(
                "UPDATE gw_oauth_clients SET pending_client_id = ?3, pending_has_secret = ?4,
                     pending_secret_ciphertext = ?5, pending_issuer = NULL, pending_token_endpoint = NULL,
                     key_version = ?6
                 WHERE connection_id = ?1 AND owner_id = ?2",
                params![id, self.owner, client_id, has_secret, sealed, key.version()],
            )?;
        } else {
            let sealed = secret
                .as_ref()
                .map(|s| crypto::seal(key, id, CLIENT_SECRET, s.as_bytes()));
            tx.execute(
                "INSERT INTO gw_oauth_clients(connection_id, owner_id, client_id, has_client_secret,
                                              client_secret_ciphertext, key_version, registered_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(connection_id) DO UPDATE SET client_id = excluded.client_id,
                     has_client_secret = excluded.has_client_secret,
                     client_secret_ciphertext = excluded.client_secret_ciphertext,
                     key_version = excluded.key_version, token_endpoint_auth_method = NULL, issuer = NULL,
                     authorization_endpoint = NULL, token_endpoint = NULL, redirect_uri = NULL, scopes = NULL,
                     resource = NULL, resource_param_accepted = 1, registered_at = excluded.registered_at,
                     pending_client_id = NULL, pending_has_secret = 0, pending_secret_ciphertext = NULL,
                     pending_issuer = NULL, pending_token_endpoint = NULL
                     WHERE gw_oauth_clients.owner_id = excluded.owner_id",
                params![id, self.owner, client_id, has_secret, sealed, key.version(), now],
            )?;
        }
        let record = read(&tx, &self.owner, Some(id))?.pop();
        tx.commit()?;
        Ok(record.map_or(ClientChange::NotFound, |record| ClientChange::Done(Box::new(record))))
    }

    /// The connection's OAuth clients, their secrets opened, for an
    /// authorize (gateway spec §4.2): the one in use and a pending one, and
    /// whether a grant is live. `None`: no such connection.
    pub fn oauth_clients(&self, id: &str, key: &MasterKey) -> Result<Option<ClientSlots>> {
        let conn = self.conn();
        let has_grant: Option<bool> = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = c.id AND k.owner_id = ?2)
                 FROM gw_connections c WHERE c.id = ?1 AND c.owner_id = ?2",
                [id, &self.owner],
                |r| r.get(0),
            )
            .optional()?;
        let Some(has_grant) = has_grant else {
            return Ok(None);
        };
        type Row = (
            String,
            Option<u32>,
            Option<Vec<u8>>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<Vec<u8>>,
            Option<String>,
            Option<String>,
        );
        let row: Option<Row> = conn
            .query_row(
                "SELECT client_id, key_version, client_secret_ciphertext, token_endpoint_auth_method, issuer,
                        token_endpoint, redirect_uri, pending_client_id, pending_secret_ciphertext, pending_issuer,
                        pending_token_endpoint
                 FROM gw_oauth_clients WHERE connection_id = ?1 AND owner_id = ?2",
                [id, &self.owner],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                    ))
                },
            )
            .optional()?;
        drop(conn);
        let Some((
            client_id,
            version,
            blob,
            method,
            issuer,
            token_endpoint,
            redirect_uri,
            pending_id,
            pending_blob,
            pending_issuer,
            pending_token_endpoint,
        )) = row
        else {
            return Ok(Some(ClientSlots {
                active: None,
                pending: None,
                has_grant,
            }));
        };
        let active = ClientSlot {
            secret: open_secret(key, id, CLIENT_SECRET, version, &blob)?,
            client_id,
            auth_method: method.as_deref().and_then(AuthMethod::parse),
            issuer,
            token_endpoint,
            redirect_uri,
        };
        let pending = match pending_id {
            Some(client_id) => Some(ClientSlot {
                secret: open_secret(key, id, PENDING_CLIENT_SECRET, version, &pending_blob)?,
                client_id,
                auth_method: None,
                issuer: pending_issuer,
                token_endpoint: pending_token_endpoint,
                redirect_uri: None,
            }),
            None => None,
        };
        Ok(Some(ClientSlots {
            active: Some(active),
            pending,
            has_grant,
        }))
    }

    /// Pin a pre-registered client to the authorization server its first
    /// authorize found (the review's R2): its secret only ever goes to that
    /// token endpoint. Only while `client_id` is still that slot's client.
    pub fn pin_client(
        &self,
        id: &str,
        slot: Slot,
        client_id: &str,
        issuer: &str,
        token_endpoint: &str,
    ) -> Result<bool> {
        let sql = match slot {
            Slot::Active => {
                "UPDATE gw_oauth_clients SET issuer = ?4, token_endpoint = ?5
                 WHERE connection_id = ?1 AND owner_id = ?2 AND client_id = ?3 AND issuer IS NULL"
            }
            Slot::Pending => {
                "UPDATE gw_oauth_clients SET pending_issuer = ?4, pending_token_endpoint = ?5
                 WHERE connection_id = ?1 AND owner_id = ?2 AND pending_client_id = ?3 AND pending_issuer IS NULL"
            }
        };
        Ok(self
            .conn()
            .execute(sql, params![id, self.owner, client_id, issuer, token_endpoint])?
            == 1)
    }

    /// An OAuth connection's grant with where it goes and how it is
    /// refreshed, all read in one statement (`OauthCredential`); `None`
    /// without one. An error if it does not open.
    pub fn oauth_credential(&self, id: &str, key: &MasterKey) -> Result<Option<OauthCredential>> {
        type Row = (
            (u32, Vec<u8>, Option<i64>, String, String, bool),
            (String, Option<u32>, Option<Vec<u8>>, Option<String>, Option<String>),
            (Option<String>, Option<String>, bool),
        );
        let row: Option<Row> = self
            .conn()
            .query_row(
                "SELECT k.key_version, k.ciphertext, k.expires_at, c.cred_kind, c.url, c.internal_network,
                        o.client_id, o.key_version, o.client_secret_ciphertext, o.token_endpoint_auth_method,
                        o.token_endpoint, o.scopes, o.resource, o.resource_param_accepted
                 FROM gw_credentials k
                 JOIN gw_connections c ON c.id = k.connection_id AND c.owner_id = k.owner_id
                 JOIN gw_oauth_clients o ON o.connection_id = k.connection_id AND o.owner_id = k.owner_id
                 WHERE k.connection_id = ?1 AND k.owner_id = ?2",
                [id, &self.owner],
                |r| {
                    Ok((
                        (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?),
                        (r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?),
                        (r.get(11)?, r.get(12)?, r.get(13)?),
                    ))
                },
            )
            .optional()?;
        let Some((
            (version, sealed, expires_at, kind, url, internal_network),
            (client_id, secret_version, secret_blob, method, token_endpoint),
            (scopes, resource, resource_param_accepted),
        )) = row
        else {
            return Ok(None);
        };
        let cred_kind = CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?;
        if !cred_kind.is_oauth() {
            return Ok(None);
        }
        let opened =
            crypto::open(key, id, OAUTH_TOKENS, version, &sealed).with_context(|| format!("connection {id}"))?;
        let tokens = GrantTokens::from_opened(&opened).ok_or_else(|| anyhow!("connection {id}: not a grant"))?;
        Ok(Some(OauthCredential {
            tokens,
            expires_at,
            url,
            internal_network,
            cred_kind,
            client: TokenClient {
                secret: open_secret(key, id, CLIENT_SECRET, secret_version, &secret_blob)?,
                client_id,
                auth_method: method
                    .as_deref()
                    .and_then(AuthMethod::parse)
                    .ok_or_else(|| anyhow!("connection {id}: a grant without its client's method"))?,
                token_endpoint: token_endpoint
                    .ok_or_else(|| anyhow!("connection {id}: a grant without a token endpoint"))?,
            },
            scopes: scopes
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .context("stored scopes")?
                .unwrap_or_default(),
            resource,
            resource_param_accepted,
            sealed,
        }))
    }

    /// Store what a refresh gave (gateway spec §4.4, §4.5): only while the
    /// credential is still the very blob `read` holds, else nothing (false).
    /// Every write of a credential seals it with a fresh nonce, so a grant
    /// deleted (another origin or kind, a delete, a purge) or replaced (a
    /// completed Connect) meanwhile never matches; a path-only edit, which
    /// keeps the grant (§4.6), does, and keeps the rotated token. A
    /// response without a refresh token keeps the old one (the caller
    /// passes it on).
    pub fn store_refreshed(
        &self,
        id: &str,
        read: &OauthCredential,
        refreshed: &RefreshedGrant<'_>,
        key: &MasterKey,
        now: i64,
    ) -> Result<bool> {
        let RefreshedGrant {
            tokens,
            expires_at,
            resource_param_accepted,
        } = *refreshed;
        let sealed = crypto::seal(key, id, OAUTH_TOKENS, &tokens.to_sealable());
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE gw_credentials SET key_version = ?3, ciphertext = ?4, expires_at = ?5, updated_at = ?6
             WHERE connection_id = ?1 AND owner_id = ?2 AND ciphertext = ?7",
            params![id, self.owner, key.version(), sealed, expires_at, now, read.sealed],
        )?;
        if changed == 1 && resource_param_accepted != read.resource_param_accepted {
            tx.execute(
                "UPDATE gw_oauth_clients SET resource_param_accepted = ?3 WHERE connection_id = ?1 AND owner_id = ?2",
                params![id, self.owner, resource_param_accepted],
            )?;
        }
        tx.commit()?;
        Ok(changed == 1)
    }

    /// Store a completed Connect's grant with the client and authorization
    /// server it was made with (gateway spec §4.3; api-8e-8f B5 step 8),
    /// under the connection's refresh lock, which the caller holds: only if
    /// the connection is still what the flow started from (its URL, kind,
    /// internal marking, and for a pre-registered client the same client),
    /// else nothing (the review's R1). A pending client becomes the client
    /// (G-7). The status is `ok` as of `now`, `status_at` too even when it
    /// was `ok` already (the UI waits for a newer one), and the latest
    /// Connect's failure is cleared.
    pub fn store_grant(&self, grant: &GrantToStore<'_>, key: &MasterKey, now: i64) -> Result<GrantStored> {
        let id = grant.connection_id;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        type Row = (String, String, bool, String, String, String);
        let current: Option<Row> = tx
            .query_row(
                "SELECT url, cred_kind, internal_network, status, label, hat_id FROM gw_connections
                 WHERE id = ?1 AND owner_id = ?2",
                [id, &self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        let Some((url, kind, internal_network, status, label, hat_id)) = current else {
            return Ok(GrantStored::ConnectionChanged);
        };
        if url != grant.url || kind != grant.cred_kind.as_str() || internal_network != grant.internal_network {
            return Ok(GrantStored::ConnectionChanged);
        }
        let clients: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT client_id, pending_client_id FROM gw_oauth_clients WHERE connection_id = ?1 AND owner_id = ?2",
                [id, &self.owner],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let same_client = match grant.source {
            ClientSource::Registered => true,
            ClientSource::Active => {
                matches!(&clients, Some((active, None)) if *active == grant.client.client_id)
            }
            ClientSource::Pending => {
                matches!(&clients, Some((_, Some(pending))) if *pending == grant.client.client_id)
            }
        };
        if !same_client {
            return Ok(GrantStored::ConnectionChanged);
        }
        let secret = grant
            .client
            .secret
            .as_ref()
            .map(|s| crypto::seal(key, id, CLIENT_SECRET, s.as_bytes()));
        tx.execute(
            "INSERT INTO gw_oauth_clients(connection_id, owner_id, client_id, has_client_secret, client_secret_ciphertext,
                                          key_version, token_endpoint_auth_method, issuer, authorization_endpoint,
                                          token_endpoint, redirect_uri, scopes, resource, resource_param_accepted,
                                          registered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(connection_id) DO UPDATE SET client_id = excluded.client_id,
                 has_client_secret = excluded.has_client_secret,
                 client_secret_ciphertext = excluded.client_secret_ciphertext, key_version = excluded.key_version,
                 token_endpoint_auth_method = excluded.token_endpoint_auth_method, issuer = excluded.issuer,
                 authorization_endpoint = excluded.authorization_endpoint, token_endpoint = excluded.token_endpoint,
                 redirect_uri = excluded.redirect_uri, scopes = excluded.scopes, resource = excluded.resource,
                 resource_param_accepted = excluded.resource_param_accepted, registered_at = excluded.registered_at,
                 pending_client_id = NULL, pending_has_secret = 0, pending_secret_ciphertext = NULL,
                 pending_issuer = NULL, pending_token_endpoint = NULL
                 WHERE gw_oauth_clients.owner_id = excluded.owner_id",
            params![
                id,
                self.owner,
                grant.client.client_id,
                grant.client.secret.is_some(),
                secret,
                key.version(),
                grant.client.auth_method.as_str(),
                grant.issuer,
                grant.authorization_endpoint,
                grant.client.token_endpoint,
                grant.redirect_uri,
                serde_json::to_string(grant.scopes)?,
                grant.resource,
                grant.resource_param_accepted,
                grant.registered_at,
            ],
        )?;
        let sealed = crypto::seal(key, id, OAUTH_TOKENS, &grant.tokens.to_sealable());
        tx.execute(
            "INSERT INTO gw_credentials(connection_id, owner_id, key_version, ciphertext, expires_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(connection_id) DO UPDATE SET key_version = excluded.key_version,
                 ciphertext = excluded.ciphertext, expires_at = excluded.expires_at, updated_at = excluded.updated_at
                 WHERE gw_credentials.owner_id = excluded.owner_id",
            params![id, self.owner, key.version(), sealed, grant.expires_at, now],
        )?;
        tx.execute(
            "UPDATE gw_connections SET status = 'ok', status_note = NULL, account_label = NULL, status_at = ?3,
                                       checked_at = ?3, oauth_error_code = NULL, oauth_error_message = NULL,
                                       oauth_error_at = NULL, resource_mismatch = NULL
             WHERE id = ?1 AND owner_id = ?2",
            params![id, self.owner, now],
        )?;
        tx.commit()?;
        let from = Status::parse(&status).ok_or_else(|| anyhow!("a stored status"))?;
        Ok(GrantStored::Stored(StatusChange {
            connection_id: id.to_string(),
            hat_id,
            label,
            from,
            to: Status::Ok,
        }))
    }

    /// Why the latest Connect of `id` did not complete (api-8e-8f B3, B5);
    /// `None` clears it. The status is not touched: a failed reconnect of a
    /// working grant leaves it `ok`.
    pub fn set_oauth_error(&self, id: &str, error: Option<(&str, &str)>, now: i64) -> Result<()> {
        let (code, message) = error.unzip();
        self.conn().execute(
            "UPDATE gw_connections SET oauth_error_code = ?3, oauth_error_message = ?4,
                                       oauth_error_at = CASE WHEN ?3 IS NULL THEN NULL ELSE ?5 END
             WHERE id = ?1 AND owner_id = ?2",
            params![id, self.owner, code, message, now],
        )?;
        Ok(())
    }

    /// The `resource` the latest authorize found that is not `url`, or
    /// `None` when it matched (gateway spec §4.1, G-4). Only while the
    /// connection still has `url`.
    pub fn set_resource_mismatch(&self, id: &str, url: &str, found: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE gw_connections SET resource_mismatch = ?4 WHERE id = ?1 AND owner_id = ?2 AND url = ?3",
            params![id, self.owner, url, found],
        )?;
        Ok(())
    }

    /// The operator accepted `resource` for the connection at `url`
    /// (api-8e-8f F3): kept until the URL changes.
    pub fn accept_resource(&self, id: &str, url: &str, resource: &str) -> Result<bool> {
        Ok(self.conn().execute(
            "UPDATE gw_connections SET accepted_resource = ?4, resource_mismatch = NULL
             WHERE id = ?1 AND owner_id = ?2 AND url = ?3",
            params![id, self.owner, url, resource],
        )? == 1)
    }

    /// Whether any credential is stored: then a missing master key is an
    /// error, never a new key (`key::load_or_create`).
    pub fn has_ciphertext(&self) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM gw_credentials WHERE owner_id = ?1)
                 OR EXISTS (SELECT 1 FROM gw_oauth_clients WHERE owner_id = ?1
                                AND (client_secret_ciphertext IS NOT NULL OR pending_secret_ciphertext IS NOT NULL))",
            [&self.owner],
            |r| r.get(0),
        )?)
    }

    /// That `key` opens the newest stored credential, so a wrong key stops
    /// the start instead of sealing new rows beside ones it cannot open
    /// (plan 8a decision 8). Only the newest: a damaged older row shows when
    /// it is used, and does not stop the start.
    pub fn check_key(&self, key: &MasterKey) -> Result<()> {
        let conn = self.conn();
        let row: Option<(String, u32, Vec<u8>, String)> = conn
            .query_row(
                "SELECT k.connection_id, k.key_version, k.ciphertext, c.cred_kind
                 FROM gw_credentials k JOIN gw_connections c ON c.id = k.connection_id AND c.owner_id = k.owner_id
                 WHERE k.owner_id = ?1
                 ORDER BY k.updated_at DESC, k.connection_id LIMIT 1",
                [&self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        // And the newest client secret: a key that opens no grant may still
        // be asked to open one (plan 8f).
        let secret: Option<(String, u32, Vec<u8>)> = conn
            .query_row(
                "SELECT connection_id, key_version, client_secret_ciphertext FROM gw_oauth_clients
                 WHERE owner_id = ?1 AND client_secret_ciphertext IS NOT NULL
                 ORDER BY registered_at DESC, connection_id LIMIT 1",
                [&self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        drop(conn);
        let refused = |id: &str| {
            format!(
                "the master key does not open the stored credential of connection {id}: restore the key it was \
                 sealed with. {}",
                crate::key::GIVE_UP
            )
        };
        if let Some((id, version, blob, kind)) = row {
            let kind =
                CredKind::parse(&kind).ok_or_else(|| anyhow!("connection {id} has a credential of kind {kind}"))?;
            anyhow::ensure!(kind != CredKind::None, "connection {id} has a credential of kind none");
            crypto::open(key, &id, crypto::credential_field(kind), version, &blob)
                .map(drop)
                .with_context(|| refused(&id))?;
        }
        if let Some((id, version, blob)) = secret {
            crypto::open(key, &id, CLIENT_SECRET, version, &blob)
                .map(drop)
                .with_context(|| refused(&id))?;
        }
        Ok(())
    }

    /// The gateway's part of purging a hat (kernel spec §5.5, lane L6): the
    /// hat's connections, with their credentials, OAuth clients and mounts,
    /// in one transaction. Idempotent: a hat with nothing left, or gone, is
    /// done.
    pub fn purge_hat(&self, hat_id: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM gw_credentials WHERE owner_id = ?2
                 AND connection_id IN (SELECT id FROM gw_connections WHERE hat_id = ?1 AND owner_id = ?2)",
            [hat_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM gw_oauth_clients WHERE owner_id = ?2
                 AND connection_id IN (SELECT id FROM gw_connections WHERE hat_id = ?1 AND owner_id = ?2)",
            [hat_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM gw_mounts WHERE owner_id = ?2
                 AND connection_id IN (SELECT id FROM gw_connections WHERE hat_id = ?1 AND owner_id = ?2)",
            [hat_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM gw_connections WHERE hat_id = ?1 AND owner_id = ?2",
            [hat_id, &self.owner],
        )?;
        tx.commit()?;
        Ok(())
    }
}

/// What a `PUT …/oauth-client` does with the secret (api-8e-8f B2).
pub enum SecretInput {
    /// Absent: keep the stored one, for the same client id only.
    Keep,
    /// `null`: a public client.
    Clear,
    /// A string: this one.
    Set(Zeroizing<String>),
}

/// The outcome of setting a pre-registered client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientChange {
    Done(Box<ConnectionRecord>),
    NotFound,
    /// The connection is not `oauth_client`.
    WrongKind(CredKind),
    Invalid(String),
}

/// Which of a connection's two client places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// The client the grant was made with, or the next Connect's when there
    /// is no grant.
    Active,
    /// A pre-registered client saved while a grant is live (G-7).
    Pending,
}

/// One client, its secret opened. Its `Debug` never shows the secret.
#[derive(Clone)]
pub struct ClientSlot {
    pub client_id: String,
    pub secret: Option<Zeroizing<String>>,
    /// `None` until a Connect with it completes.
    pub auth_method: Option<AuthMethod>,
    /// The authorization server it is pinned to (the review's R2); `None`
    /// until its first authorize.
    pub issuer: Option<String>,
    pub token_endpoint: Option<String>,
    /// The redirect URI it was registered or used with.
    pub redirect_uri: Option<String>,
}

impl std::fmt::Debug for ClientSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSlot")
            .field("client_id", &self.client_id)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("auth_method", &self.auth_method)
            .field("issuer", &self.issuer.as_deref().map(crate::model::url_for_logs))
            .finish_non_exhaustive()
    }
}

/// A connection's OAuth clients, for an authorize.
#[derive(Debug, Clone)]
pub struct ClientSlots {
    pub active: Option<ClientSlot>,
    pub pending: Option<ClientSlot>,
    /// A grant is stored (live, even when `needs_auth`: G-7).
    pub has_grant: bool,
}

/// Where a flow's client came from (api-8e-8f S3): the callback stores the
/// grant only if that place still holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientSource {
    Active,
    Pending,
    /// Registered dynamically by the authorize (`oauth_dcr`), held by the
    /// flow until the callback stores it with the grant.
    Registered,
}

/// A completed Connect, to be stored (`GatewayStore::store_grant`).
pub struct GrantToStore<'a> {
    pub connection_id: &'a str,
    /// What the connection was when the flow started (the review's R1).
    pub url: &'a str,
    pub cred_kind: CredKind,
    pub internal_network: bool,
    pub source: ClientSource,
    pub client: &'a TokenClient,
    pub issuer: &'a str,
    pub authorization_endpoint: &'a str,
    pub redirect_uri: &'a str,
    pub scopes: &'a [String],
    pub resource: &'a str,
    pub resource_param_accepted: bool,
    pub registered_at: i64,
    pub tokens: &'a GrantTokens,
    pub expires_at: Option<i64>,
}

/// What a refresh gave, to be stored (`GatewayStore::store_refreshed`).
#[derive(Clone, Copy)]
pub struct RefreshedGrant<'a> {
    pub tokens: &'a GrantTokens,
    pub expires_at: Option<i64>,
    /// Whether the token endpoint took `resource` this time.
    pub resource_param_accepted: bool,
}

/// The outcome of storing a grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantStored {
    /// Stored; the status moved from what it was to `ok`.
    Stored(StatusChange),
    /// The connection was edited, deleted or given another client since
    /// the flow started: nothing was stored.
    ConnectionChanged,
}

/// A secret column opened under `field`; `None` when there is none.
fn open_secret(
    key: &MasterKey,
    id: &str,
    field: &str,
    version: Option<u32>,
    blob: &Option<Vec<u8>>,
) -> Result<Option<Zeroizing<String>>> {
    let Some(blob) = blob else {
        return Ok(None);
    };
    let version = version.ok_or_else(|| anyhow!("connection {id}: a secret without its key version"))?;
    let mut opened = crypto::open(key, id, field, version, blob).with_context(|| format!("connection {id}"))?;
    match String::from_utf8(std::mem::take(&mut *opened)) {
        Ok(secret) => Ok(Some(Zeroizing::new(secret))),
        Err(err) => {
            drop(Zeroizing::new(err.into_bytes()));
            Err(anyhow!("connection {id}: not a secret"))
        }
    }
}

/// `owner`'s connections, or the one with `id`, with their live mounts.
fn read(conn: &Connection, owner: &str, id: Option<&str>) -> Result<Vec<ConnectionRecord>> {
    let mut stmt = conn.prepare(SELECT_CONNECTIONS)?;
    let rows = stmt.query_map(params![owner, id], |r| {
        Ok((
            ConnectionRecord {
                id: r.get(0)?,
                slug: r.get(1)?,
                label: r.get(2)?,
                url: r.get(3)?,
                hat_id: r.get(4)?,
                cred_kind: CredKind::None,
                static_header: r.get(6)?,
                static_prefix: r.get(7)?,
                tool_allowlist: None,
                internal_network: r.get(9)?,
                status: r.get(10)?,
                status_note: r.get(11)?,
                account_label: r.get(12)?,
                status_at: r.get(13)?,
                created_at: r.get(14)?,
                updated_at: r.get(15)?,
                has_credential: r.get(16)?,
                mounts: Vec::new(),
                checked_at: r.get(17)?,
                oauth_client: None,
                oauth_error: match (r.get::<_, Option<String>>(18)?, r.get(19)?, r.get(20)?) {
                    (Some(code), Some(message), Some(at)) => Some(OauthError { code, message, at }),
                    _ => None,
                },
                resource_mismatch: r.get(21)?,
                accepted_resource: r.get(22)?,
            },
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(8)?,
            (
                r.get::<_, Option<String>>(23)?,
                r.get::<_, Option<bool>>(24)?,
                r.get::<_, Option<String>>(25)?,
                r.get::<_, Option<bool>>(26)?,
            ),
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (mut record, kind, allowlist, client) = row?;
        record.cred_kind = CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?;
        record.tool_allowlist = allowlist
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .context("a stored tool allowlist")?;
        if record.cred_kind.is_oauth() {
            let (client_id, has_secret, pending, pending_secret) = client;
            record.oauth_client = Some(OauthClientView {
                has_client_secret: client_id.is_some() && has_secret.unwrap_or(false),
                client_id,
                pending: pending.map(|id| (id, pending_secret.unwrap_or(false))),
            });
        }
        out.push(record);
    }
    let mut stmt = conn.prepare(SELECT_MOUNTS)?;
    let mounts = stmt.query_map(params![owner, id], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?;
    for mount in mounts {
        let (connection, host) = mount?;
        if let Some(record) = out.iter_mut().find(|c| c.id == connection) {
            record.mounts.push(host);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    /// Every (table, column) `sql` reads, as SQLite's authorizer reports.
    fn reads(conn: &Connection, sql: &str) -> BTreeSet<(String, String)> {
        let seen: Arc<Mutex<BTreeSet<(String, String)>>> = Arc::default();
        let sink = seen.clone();
        conn.authorizer(Some(move |ctx: AuthContext<'_>| {
            if let AuthAction::Read {
                table_name,
                column_name,
            } = ctx.action
            {
                sink.lock()
                    .unwrap()
                    .insert((table_name.to_string(), column_name.to_string()));
            }
            Authorization::Allow
        }))
        .unwrap();
        conn.prepare(sql).unwrap();
        conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>).unwrap();
        Arc::try_unwrap(seen).unwrap().into_inner().unwrap()
    }

    /// Plan 8a decision 3 (gateway spec §2, "list endpoints never read
    /// those tables"): what lists and shows a connection reads of
    /// `gw_credentials` is whether a row is there, by its key and owner,
    /// and nothing of the secret.
    #[test]
    fn listing_connections_reads_no_secret() {
        let store = GatewayStore::open_in_memory().unwrap();
        let conn = store.conn();
        let mut credentials = BTreeSet::new();
        for sql in [SELECT_CONNECTIONS, SELECT_MOUNTS] {
            for (table, column) in reads(&conn, sql) {
                if table == "gw_credentials" {
                    credentials.insert(column);
                }
            }
        }
        let expected: BTreeSet<String> = ["connection_id", "owner_id"].map(String::from).into();
        assert_eq!(credentials, expected);
    }

    /// Plan 8f: what the list reads of `gw_oauth_clients` is the clients'
    /// ids and whether each has a secret, never a ciphertext.
    #[test]
    fn listing_connections_reads_no_client_secret() {
        let store = GatewayStore::open_in_memory().unwrap();
        let conn = store.conn();
        let clients: BTreeSet<String> = reads(&conn, SELECT_CONNECTIONS)
            .into_iter()
            .filter(|(table, _)| table == "gw_oauth_clients")
            .map(|(_, column)| column)
            .collect();
        let expected: BTreeSet<String> = [
            "client_id",
            "connection_id",
            "has_client_secret",
            "owner_id",
            "pending_client_id",
            "pending_has_secret",
        ]
        .map(String::from)
        .into();
        assert_eq!(clients, expected);
    }
}
