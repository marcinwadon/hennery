//! Where an agent keeps its own data for a session (plan 9d decision 1),
//! and the host's registry of it (B1).
//!
//! On every start and resume the host resolves the agent's data roots from
//! the environment the adapter runs with, and registers `(agent, agent
//! session id, roots)` durably before it sends `session_started`. A forget
//! acts only on an exact match of that registry, so a collector naming any
//! other root is refused (`unknown_to_host`). Entries are kept after a
//! forget: a retry after a lost answer checks again and answers the same.

use anyhow::{Context, Result};
use hennery_proto::frames::AgentHome;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The registry's file in the host's data directory.
pub const FILE: &str = "agent-homes.db";

/// The agents whose data the host knows how to find.
pub const CLAUDE: &str = "claude";
pub const CODEX: &str = "codex";

/// `name`'s value for the adapter: the agent's own configuration
/// (`agent.env`, the last setting wins), then the host's environment,
/// which the adapter inherits. Empty counts as unset, as both agents read
/// it.
fn lookup(env: &[(String, String)], name: &str) -> Option<String> {
    env.iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var(name).ok())
        .filter(|value| !value.is_empty())
}

/// `path`, canonical and UTF-8, if it exists.
fn canonical(path: &Path) -> Option<String> {
    match std::fs::canonicalize(path) {
        Ok(path) => path.into_os_string().into_string().ok(),
        Err(err) => {
            tracing::info!(path = %path.display(), error = %err, "an agent data root does not resolve; not recorded");
            None
        }
    }
}

/// The data roots `agent` uses with `env` (plan 9d decision 1): Claude's
/// `CLAUDE_CONFIG_DIR`, else `$HOME/.claude`; Codex's `CODEX_HOME`, else
/// `$HOME/.codex`, and `CODEX_SQLITE_HOME` if set. Canonical, as the
/// filesystem gives the bytes (O11: no NFC). `None` for another agent, or
/// a root that does not resolve (not made yet): such a session records no
/// home, and its forget is final (decision 11).
pub fn resolve(agent: &str, env: &[(String, String)]) -> Option<AgentHome> {
    let (var, default) = match agent {
        CLAUDE => ("CLAUDE_CONFIG_DIR", ".claude"),
        CODEX => ("CODEX_HOME", ".codex"),
        _ => return None,
    };
    let root = match lookup(env, var) {
        Some(root) => PathBuf::from(root),
        None => PathBuf::from(lookup(env, "HOME")?).join(default),
    };
    let sqlite_root = match (agent, lookup(env, "CODEX_SQLITE_HOME")) {
        (CODEX, Some(sqlite)) => Some(canonical(Path::new(&sqlite))?),
        _ => None,
    };
    Some(AgentHome {
        root: canonical(&root)?,
        sqlite_root,
    })
}

/// The host's registry of agent homes (B1), in its data directory.
pub struct Registry {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Registry")
    }
}

impl Registry {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path).with_context(|| format!("open {}", path.display()))?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Each entry is on disk before `session_started` goes out (B1):
        // every commit is synced, whatever the build's default.
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_homes (
                 agent TEXT NOT NULL,
                 agent_session_id TEXT NOT NULL,
                 root TEXT NOT NULL,
                 sqlite_root TEXT NOT NULL DEFAULT '',
                 recorded_at INTEGER NOT NULL,
                 PRIMARY KEY (agent, agent_session_id, root, sqlite_root));",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Register `home` for `agent`'s session `agent_session_id`, durably.
    /// `true` if that session was registered with other roots before (a
    /// moved home: both stay registered, B8).
    pub fn record(&self, agent: &str, agent_session_id: &str, home: &AgentHome) -> Result<bool> {
        let conn = self.conn.lock().expect("registry lock");
        let sqlite = home.sqlite_root.as_deref().unwrap_or("");
        let moved: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM agent_homes WHERE agent = ?1 AND agent_session_id = ?2
                     AND NOT (root = ?3 AND sqlite_root = ?4) LIMIT 1",
                params![agent, agent_session_id, home.root, sqlite],
                |r| r.get(0),
            )
            .optional()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        conn.execute(
            "INSERT INTO agent_homes(agent, agent_session_id, root, sqlite_root, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING",
            params![agent, agent_session_id, home.root, sqlite, now],
        )?;
        Ok(moved.is_some())
    }

    /// Whether exactly this `(agent, agent_session_id, home)` is registered
    /// (B1): byte for byte.
    pub fn contains(&self, agent: &str, agent_session_id: &str, home: &AgentHome) -> Result<bool> {
        let conn = self.conn.lock().expect("registry lock");
        Ok(conn
            .query_row(
                "SELECT 1 FROM agent_homes WHERE agent = ?1 AND agent_session_id = ?2 AND root = ?3
                     AND sqlite_root = ?4",
                params![
                    agent,
                    agent_session_id,
                    home.root,
                    home.sqlite_root.as_deref().unwrap_or("")
                ],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }
}
