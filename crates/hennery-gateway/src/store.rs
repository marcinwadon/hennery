//! The gateway's store (gateway spec §2): connections, the hosts they are
//! mounted on, and their credentials, sealed (`crypto`). It keeps its own
//! connection to `hennery.db`, beside the kernel's and the sessions
//! store's, and every query names the database's owner (kernel spec §1).
//! Every SQL statement of the gateway is in this file, which the owner
//! audit reads (`hennery-testkit/tests/owner_filter.rs`).
//!
//! What lists a connection reads of `gw_credentials` only whether a row is
//! there (plan 8a decision 3): never the ciphertext, nor its version.

use crate::crypto::{self, STATIC_TOKEN};
use crate::key::MasterKey;
use crate::model::{
    Change, ConnectionPatch, ConnectionRecord, CredKind, CredentialChange, DEFAULT_HEADER, DEFAULT_PREFIX,
    MAX_CONNECTIONS, MAX_HOST_ID, MAX_MOUNTS, NewConnection, StaticCredential, header_problem, label_problem,
    parse_url, prefix_problem, slug_problem, token_problem, tools,
};
use crate::schema::{COMPONENT, MIGRATIONS};
use anyhow::{Context, Result, anyhow};
use hennery_kernel::db;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use zeroize::Zeroizing;

/// A connection's columns, then whether it has a credential: `?1` is the
/// owner, `?2` one connection's id or `NULL` for all of them.
const SELECT_CONNECTIONS: &str = "SELECT c.id, c.slug, c.label, c.url, c.hat_id, c.cred_kind, c.static_header,
            c.static_prefix, c.tool_allowlist, c.internal_network, c.status, c.status_note, c.account_label,
            c.status_at, c.created_at, c.updated_at,
            EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = c.id AND k.owner_id = ?1)
     FROM gw_connections c
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
        if !new.cred_kind.is_supported() {
            return Ok(Change::Unsupported(new.cred_kind));
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
    /// another kind deletes the connection's credential first, in the same
    /// transaction, and its status starts over: the token was granted for
    /// the old upstream and the old kind (G-14).
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
        if !kind.is_supported() {
            return Ok(Change::Unsupported(kind));
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
                "UPDATE gw_connections SET status = 'not_connected', status_note = NULL, account_label = NULL,
                                           status_at = ?3
                 WHERE id = ?1 AND owner_id = ?2",
                params![id, self.owner, now],
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

    /// Delete a connection with its credential and mounts. False if there
    /// was none.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM gw_credentials WHERE connection_id = ?1 AND owner_id = ?2",
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

    /// Whether any credential is stored: then a missing master key is an
    /// error, never a new key (`key::load_or_create`).
    pub fn has_ciphertext(&self) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM gw_credentials WHERE owner_id = ?1)",
            [&self.owner],
            |r| r.get(0),
        )?)
    }

    /// That `key` opens the newest stored credential, so a wrong key stops
    /// the start instead of sealing new rows beside ones it cannot open
    /// (plan 8a decision 8). Only the newest: a damaged older row shows when
    /// it is used, and does not stop the start.
    pub fn check_key(&self, key: &MasterKey) -> Result<()> {
        let row: Option<(String, u32, Vec<u8>, String)> = self
            .conn()
            .query_row(
                "SELECT k.connection_id, k.key_version, k.ciphertext, c.cred_kind
                 FROM gw_credentials k JOIN gw_connections c ON c.id = k.connection_id AND c.owner_id = k.owner_id
                 WHERE k.owner_id = ?1
                 ORDER BY k.updated_at DESC, k.connection_id LIMIT 1",
                [&self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((id, version, blob, kind)) = row else {
            return Ok(());
        };
        anyhow::ensure!(
            kind == CredKind::Static.as_str(),
            "connection {id} has a credential of kind {kind}"
        );
        crypto::open(key, &id, STATIC_TOKEN, version, &blob)
            .map(drop)
            .with_context(|| {
                format!(
                    "the master key does not open the stored credential of connection {id}: restore the key it was \
                     sealed with. {}",
                    crate::key::GIVE_UP
                )
            })
    }

    /// The gateway's part of purging a hat (kernel spec §5.5, lane L6): the
    /// hat's connections, with their credentials and mounts, in one
    /// transaction. Idempotent: a hat with nothing left, or gone, is done.
    pub fn purge_hat(&self, hat_id: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM gw_credentials WHERE owner_id = ?2
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
            },
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(8)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (mut record, kind, allowlist) = row?;
        record.cred_kind = CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?;
        record.tool_allowlist = allowlist
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .context("a stored tool allowlist")?;
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
}
