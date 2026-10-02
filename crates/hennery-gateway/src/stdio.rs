//! Local stdio servers (gateway spec §3.4; plan 8e decisions E1–E6): one
//! set per (host, hat), replaced whole, passed to that hat's sessions on
//! that host as ACP stdio `mcpServers` entries named `hennery-<name>`. The
//! agent runs them on the host; the gateway never proxies them.
//!
//! Only the environment's values are sealed (§2, §6): one JSON object per
//! row, under `crypto::seal_stdio`, bound to the row, its host and its hat
//! (the API review's R5). Their names are kept beside them, in order, so a
//! `GET` reads no secret. Every SQL statement here names the owner, and the
//! owner audit reads this file (`hennery-testkit/tests/owner_filter.rs`).

use crate::crypto;
use crate::key::MasterKey;
use crate::store::GatewayStore;
use anyhow::{Context, Result, anyhow};
use hennery_proto::frames::{McpServer, NameValue};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::{BTreeMap, HashMap, HashSet};
use zeroize::Zeroizing;

/// The most servers in one (host, hat)'s set.
pub const MAX_SET: usize = 32;
/// The most servers one owner has, across sets.
pub const MAX_OWNER: usize = 1024;
/// A command's most bytes.
pub const MAX_COMMAND: usize = 1024;
/// The most arguments, each one's most bytes, and all of them together.
pub const MAX_ARGS: usize = 64;
pub const MAX_ARG: usize = 4096;
pub const MAX_ARGS_TOTAL: usize = 16 * 1024;
/// The most environment variables per server, and a value's most bytes.
pub const MAX_ENV: usize = 64;
pub const MAX_ENV_VALUE: usize = 8192;
/// The longest host or hat id a query names, checked before any read.
pub const MAX_ID: usize = 64;

/// One server as stored, without its values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// The environment's names, in order; each has a value stored.
    pub env: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One server of a `PUT`. A `value` of `None` keeps the stored one.
#[derive(Clone, PartialEq, Eq)]
pub struct StdioInput {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, Option<String>)>,
}

// By hand: the args and the values may be secrets.
impl std::fmt::Debug for StdioInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioInput")
            .field("name", &self.name)
            .field("command", &self.command)
            .finish_non_exhaustive()
    }
}

/// The outcome of `GatewayStore::stdio_set` and `replace_stdio_set`.
#[derive(Debug, PartialEq, Eq)]
pub enum StdioChange {
    /// The set as stored now, oldest first.
    Done(Vec<StdioServer>),
    /// Not one of the owner's hosts, or hats (a hat being purged is none).
    NotFound,
    /// The host is revoked: its set can be read, not changed (E5).
    HostRevoked,
    /// A server or field refused; the message names which, never a value.
    Invalid(String),
    /// A kept value that is not stored, or whose server's command changed.
    EnvValueMissing(String),
    /// A name one of the owner's connections has as its slug (E2).
    SlugTaken(String),
    /// More than `MAX_SET` in the set, or `MAX_OWNER` for the owner.
    TooMany(String),
}

/// `^[a-z0-9][a-z0-9-]{0,47}$`, as a connection's slug (E2).
fn name_ok(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=48).contains(&bytes.len())
        && bytes[0] != b'-'
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// `^[A-Za-z_][A-Za-z0-9_]{0,127}$`.
fn env_name_ok(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=128).contains(&bytes.len())
        && !bytes[0].is_ascii_digit()
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// The first rule `servers` breaks, if any, before anything is read: what
/// is refused 400 `invalid` or 409 `too_many_stdio_servers`.
fn problem(servers: &[StdioInput]) -> Option<StdioChange> {
    if servers.len() > MAX_SET {
        return Some(StdioChange::TooMany(format!(
            "a host and hat have at most {MAX_SET} stdio servers"
        )));
    }
    let mut names = HashSet::new();
    for (i, server) in servers.iter().enumerate() {
        if !name_ok(&server.name) {
            return Some(StdioChange::Invalid(format!(
                "server {}: a name is 1 to 48 of a-z, 0-9 and -, not starting with -",
                i + 1
            )));
        }
        let at = &server.name;
        if !names.insert(at.as_str()) {
            return Some(StdioChange::Invalid(format!("server {at:?} is named twice")));
        }
        if server.command.is_empty() || server.command.len() > MAX_COMMAND || server.command.chars().any(char::is_control)
        {
            return Some(StdioChange::Invalid(format!(
                "server {at:?}: the command is 1 to {MAX_COMMAND} bytes, with no control characters"
            )));
        }
        if server.args.len() > MAX_ARGS
            || server.args.iter().any(|a| a.len() > MAX_ARG || a.contains('\0'))
            || server.args.iter().map(String::len).sum::<usize>() > MAX_ARGS_TOTAL
        {
            return Some(StdioChange::Invalid(format!(
                "server {at:?}: the args are at most {MAX_ARGS}, each at most {MAX_ARG} bytes with no NUL, \
                 {MAX_ARGS_TOTAL} bytes in all"
            )));
        }
        if server.env.len() > MAX_ENV {
            return Some(StdioChange::Invalid(format!(
                "server {at:?}: the env has at most {MAX_ENV} variables"
            )));
        }
        let mut env_names = HashSet::new();
        for (j, (name, value)) in server.env.iter().enumerate() {
            if !env_name_ok(name) {
                return Some(StdioChange::Invalid(format!(
                    "server {at:?}: env variable {} is not named as ^[A-Za-z_][A-Za-z0-9_]{{0,127}}$",
                    j + 1
                )));
            }
            if !env_names.insert(name.as_str()) {
                return Some(StdioChange::Invalid(format!(
                    "server {at:?}: env variable {name} is named twice"
                )));
            }
            if value
                .as_ref()
                .is_some_and(|v| v.len() > MAX_ENV_VALUE || v.contains('\0'))
            {
                return Some(StdioChange::Invalid(format!(
                    "server {at:?}: the value of {name} is at most {MAX_ENV_VALUE} bytes, with no NUL"
                )));
            }
        }
    }
    None
}

/// A stored row: its id and what a kept value needs.
struct Row {
    id: String,
    command: String,
    args: Vec<String>,
    env: Vec<String>,
    key_version: Option<u32>,
    ciphertext: Option<Vec<u8>>,
    created_at: i64,
    updated_at: i64,
}

const SELECT_SET: &str = "SELECT id, name, command, args, env_names, key_version, env_ciphertext, created_at,
            updated_at
     FROM gw_stdio_servers
     WHERE owner_id = ?1 AND host_id = ?2 AND hat_id = ?3
     ORDER BY created_at, position, id";

/// The set's rows, oldest first, by name.
fn rows(conn: &Connection, owner: &str, host_id: &str, hat_id: &str) -> Result<Vec<(String, Row)>> {
    let mut stmt = conn.prepare(SELECT_SET)?;
    let rows = stmt.query_map(params![owner, host_id, hat_id], |r| {
        Ok((
            r.get::<_, String>(1)?,
            Row {
                id: r.get(0)?,
                command: r.get(2)?,
                args: Vec::new(),
                env: Vec::new(),
                key_version: r.get(5)?,
                ciphertext: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
            },
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (name, mut row, args, env) = row?;
        row.args = serde_json::from_str(&args).context("a stored stdio server's args")?;
        row.env = serde_json::from_str(&env).context("a stored stdio server's env names")?;
        out.push((name, row));
    }
    Ok(out)
}

fn listed(rows: Vec<(String, Row)>) -> Vec<StdioServer> {
    rows.into_iter()
        .map(|(name, row)| StdioServer {
            name,
            command: row.command,
            args: row.args,
            env: row.env,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect()
}

/// A row's values, opened. An error if they do not open (another key, a
/// blob moved from another row, host or hat) or do not read.
fn values(
    key: &MasterKey,
    host_id: &str,
    hat_id: &str,
    row: &Row,
) -> Result<BTreeMap<String, Zeroizing<String>>> {
    let (Some(version), Some(blob)) = (row.key_version, row.ciphertext.as_deref()) else {
        return Ok(BTreeMap::new());
    };
    let opened = crypto::open_stdio(key, &row.id, host_id, hat_id, version, blob)
        .with_context(|| format!("stdio server {}", row.id))?;
    let parsed: BTreeMap<String, String> =
        serde_json::from_slice(&opened).map_err(|_| anyhow!("stdio server {}: its values do not read", row.id))?;
    Ok(parsed.into_iter().map(|(k, v)| (k, Zeroizing::new(v))).collect())
}

/// Whether the host and the hat are the owner's, and the host revoked.
/// A hat being purged (`purged_hats`) is no hat here: its sets go with
/// the purge.
fn place(conn: &Connection, owner: &str, host_id: &str, hat_id: &str) -> Result<Option<bool>> {
    let hat = conn
        .query_row(
            "SELECT 1 FROM hats WHERE id = ?1 AND owner_id = ?2
                 AND NOT EXISTS (SELECT 1 FROM purged_hats WHERE hat_id = ?1 AND owner_id = ?2)",
            [hat_id, owner],
            |_| Ok(()),
        )
        .optional()?;
    if hat.is_none() {
        return Ok(None);
    }
    let revoked: Option<Option<i64>> = conn
        .query_row(
            "SELECT revoked_at FROM hosts WHERE id = ?1 AND owner_id = ?2",
            [host_id, owner],
            |r| r.get(0),
        )
        .optional()?;
    Ok(revoked.map(|at| at.is_some()))
}

impl GatewayStore {
    /// The (host, hat)'s set, oldest first; a revoked host's too.
    pub fn stdio_set(&self, host_id: &str, hat_id: &str) -> Result<StdioChange> {
        let conn = self.conn();
        if place(&conn, self.owner_id(), host_id, hat_id)?.is_none() {
            return Ok(StdioChange::NotFound);
        }
        Ok(StdioChange::Done(listed(rows(&conn, self.owner_id(), host_id, hat_id)?)))
    }

    /// Replace the (host, hat)'s set with `servers` (gateway spec §3.4;
    /// decisions E1–E5), in one transaction: a server keeps its row while
    /// its name stays; a value left `None` is kept from the same row, and
    /// only while its command is unchanged (S1); a name that is one of the
    /// owner's connection slugs is refused. A refused set changes nothing.
    pub fn replace_stdio_set(
        &self,
        host_id: &str,
        hat_id: &str,
        servers: &[StdioInput],
        key: &MasterKey,
        now: i64,
    ) -> Result<StdioChange> {
        if let Some(refused) = problem(servers) {
            return Ok(refused);
        }
        let owner = self.owner_id().to_string();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        match place(&tx, &owner, host_id, hat_id)? {
            None => return Ok(StdioChange::NotFound),
            Some(true) => return Ok(StdioChange::HostRevoked),
            Some(false) => {}
        }
        let stored: HashMap<String, Row> = rows(&tx, &owner, host_id, hat_id)?.into_iter().collect();
        // Every value first: a refusal must change nothing.
        let mut sealed = Vec::with_capacity(servers.len());
        for server in servers {
            let before = stored.get(&server.name);
            let mut kept: Option<BTreeMap<String, Zeroizing<String>>> = None;
            let mut env: BTreeMap<String, Zeroizing<String>> = BTreeMap::new();
            for (name, value) in &server.env {
                let value = match value {
                    Some(value) => Zeroizing::new(value.clone()),
                    None => {
                        let missing = || {
                            StdioChange::EnvValueMissing(format!(
                                "server {:?}: no value of {name} is kept: send it",
                                server.name
                            ))
                        };
                        let Some(before) = before.filter(|b| b.command == server.command) else {
                            return Ok(missing());
                        };
                        if kept.is_none() {
                            kept = Some(values(key, host_id, hat_id, before)?);
                        }
                        match kept.as_ref().and_then(|values| values.get(name)) {
                            Some(value) => value.clone(),
                            None => return Ok(missing()),
                        }
                    }
                };
                env.insert(name.clone(), value);
            }
            sealed.push(env);
        }
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        for name in &names {
            let taken = tx
                .query_row(
                    "SELECT 1 FROM gw_connections WHERE slug = ?1 AND owner_id = ?2",
                    [name, &owner.as_str()],
                    |_| Ok(()),
                )
                .optional()?;
            if taken.is_some() {
                return Ok(StdioChange::SlugTaken(format!(
                    "{name:?} is one of the connections' slugs"
                )));
            }
        }
        let elsewhere: i64 = tx.query_row(
            "SELECT count(*) FROM gw_stdio_servers WHERE owner_id = ?1 AND NOT (host_id = ?2 AND hat_id = ?3)",
            params![owner, host_id, hat_id],
            |r| r.get(0),
        )?;
        if elsewhere as usize + servers.len() > MAX_OWNER {
            return Ok(StdioChange::TooMany(format!(
                "there are at most {MAX_OWNER} stdio servers"
            )));
        }
        for (name, row) in &stored {
            if !names.contains(&name.as_str()) {
                tx.execute(
                    "DELETE FROM gw_stdio_servers WHERE id = ?1 AND owner_id = ?2",
                    [&row.id, &owner],
                )?;
            }
        }
        for (position, (server, env)) in servers.iter().zip(sealed).enumerate() {
            let before = stored.get(&server.name);
            let id = before.map_or_else(
                || format!("stdio-{}", hex::encode(hennery_kernel::secret::random_bytes::<8>())),
                |row| row.id.clone(),
            );
            let env_names: Vec<&str> = server.env.iter().map(|(name, _)| name.as_str()).collect();
            let (version, blob) = if env.is_empty() {
                (None, None)
            } else {
                let plain: BTreeMap<&str, &str> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                let plain = Zeroizing::new(serde_json::to_vec(&plain)?);
                (Some(key.version()), Some(crypto::seal_stdio(key, &id, host_id, hat_id, &plain)))
            };
            let changed = before.is_none_or(|b| {
                b.command != server.command
                    || b.args != server.args
                    || b.env.iter().map(String::as_str).ne(env_names.iter().copied())
                    || server.env.iter().any(|(_, value)| value.is_some())
            });
            tx.execute(
                "INSERT INTO gw_stdio_servers(id, owner_id, host_id, hat_id, name, position, command, args,
                                              env_names, key_version, env_ciphertext, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
                 ON CONFLICT(id) DO UPDATE SET position = excluded.position, command = excluded.command,
                     args = excluded.args, env_names = excluded.env_names, key_version = excluded.key_version,
                     env_ciphertext = excluded.env_ciphertext,
                     updated_at = CASE WHEN ?13 THEN excluded.updated_at ELSE gw_stdio_servers.updated_at END
                     WHERE gw_stdio_servers.owner_id = excluded.owner_id",
                params![
                    id,
                    owner,
                    host_id,
                    hat_id,
                    server.name,
                    position as i64,
                    server.command,
                    serde_json::to_string(&server.args)?,
                    serde_json::to_string(&env_names)?,
                    version,
                    blob,
                    now,
                    changed,
                ],
            )?;
        }
        let done = listed(rows(&tx, &owner, host_id, hat_id)?);
        tx.commit()?;
        Ok(StdioChange::Done(done))
    }
}

/// The (host, hat)'s stdio servers as a session gets them, inside the
/// sessions store's transaction (lane L1): `hennery-<name>`, the command
/// and args as stored, the values opened. Oldest first.
pub(crate) fn delivered_in(
    tx: &Transaction<'_>,
    owner: &str,
    host_id: &str,
    hat_id: &str,
    key: &MasterKey,
) -> Result<Vec<McpServer>> {
    let mut out = Vec::new();
    for (name, row) in rows(tx, owner, host_id, hat_id)? {
        let values = values(key, host_id, hat_id, &row)?;
        let mut env = Vec::with_capacity(row.env.len());
        for var in &row.env {
            let value = values
                .get(var)
                .ok_or_else(|| anyhow!("stdio server {}: no value of {var} is stored", row.id))?;
            env.push(NameValue::new(var.as_str(), value.as_str()));
        }
        out.push(McpServer::Stdio {
            name: format!("hennery-{name}"),
            command: row.command,
            args: row.args,
            env,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str) -> StdioInput {
        StdioInput {
            name: name.into(),
            command: "files-mcp".into(),
            args: vec![],
            env: vec![],
        }
    }

    fn invalid(servers: &[StdioInput]) -> String {
        match problem(servers) {
            Some(StdioChange::Invalid(why)) => why,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_limit_is_refused_and_names_no_value() {
        assert!(problem(&[server("files")]).is_none());
        let secret = "s3cr3t-value";
        let mut bad = server("files");
        bad.command = String::new();
        assert!(invalid(&[bad]).contains("command"));
        let mut bad = server("files");
        bad.command = "a\u{7}b".into();
        assert!(invalid(&[bad]).contains("command"));
        let mut bad = server("files");
        bad.args = vec![format!("{secret}\0")];
        assert!(!invalid(&[bad]).contains(secret));
        let mut bad = server("files");
        bad.args = vec!["x".into(); MAX_ARGS + 1];
        assert!(invalid(&[bad]).contains("args"));
        let mut bad = server("files");
        bad.args = vec!["x".repeat(MAX_ARG); 5];
        assert!(invalid(&[bad]).contains("args"));
        let mut bad = server("files");
        bad.env = (0..=MAX_ENV).map(|i| (format!("V{i}"), Some(String::new()))).collect();
        assert!(invalid(&[bad]).contains("env"));
        let mut bad = server("files");
        bad.env = vec![("1BAD".into(), None)];
        assert!(invalid(&[bad]).contains("env variable 1"));
        let mut bad = server("files");
        bad.env = vec![("A".into(), None), ("A".into(), None)];
        assert!(invalid(&[bad]).contains("twice"));
        let mut bad = server("files");
        bad.env = vec![("A".into(), Some(format!("{secret}\0")))];
        let why = invalid(&[bad]);
        assert!(why.contains("value of A") && !why.contains(secret), "{why}");
        assert!(invalid(&[server("-files")]).contains("server 1"));
        assert!(invalid(&[server("Files")]).contains("server 1"));
        assert!(invalid(&[server("files"), server("files")]).contains("twice"));
        let many: Vec<StdioInput> = (0..=MAX_SET).map(|i| server(&format!("s{i}"))).collect();
        assert!(matches!(problem(&many), Some(StdioChange::TooMany(_))));
    }

    #[test]
    fn env_names_follow_the_shell_rule() {
        for good in ["A", "_", "a_1", &"A".repeat(128)] {
            assert!(env_name_ok(good), "{good}");
        }
        for bad in ["", "1A", "A-B", "A B", &"A".repeat(129)] {
            assert!(!env_name_ok(bad), "{bad}");
        }
    }
}
