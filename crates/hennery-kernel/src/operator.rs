//! The operator (kernel spec §3): the one owner, their password, the
//! `public_url` setting, and the one-time setup link that creates them.
//!
//! - **One owner.** Setup creates it, once; a second setup is refused.
//! - **The setup token lives in memory only.** A restart before setup
//!   issues a new one, and the old one is dead by construction (kernel
//!   spec §3.1). Only its SHA-256 is kept.
//! - **Passwords are Argon2id PHC strings** (`password-auth`). Hashing and
//!   verifying are CPU- and memory-heavy (about 19 MiB each), so they run
//!   on a blocking thread and at most `MAX_CONCURRENT_HASHES` at once.
//!
//! Every time is seconds since the Unix epoch, passed in by the caller.

use crate::secret::{random_bytes, sha256_hex};
use crate::{db, schema};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// A setup link is valid this long (kernel spec §3.1).
pub const SETUP_TOKEN_TTL_SECS: i64 = 60 * 60;

/// The shortest password setup accepts, in characters.
pub const MIN_PASSWORD_CHARS: usize = 12;

/// The longest password accepted, in bytes: hashing is bounded by it.
pub const MAX_PASSWORD_BYTES: usize = 1024;

/// Argon2 runs at most this many at once, whatever the number of clients.
pub const MAX_CONCURRENT_HASHES: usize = 2;

/// The file in the data directory that holds the setup link until setup
/// (kernel spec §1, §3.1).
pub const SETUP_URL_FILE: &str = "setup-url";

/// The `settings` key of the public URL.
const PUBLIC_URL_KEY: &str = "public_url";

/// Where browsers reach the collector (umbrella spec §7.5): `https://`, or
/// `http://` to a loopback address, with no path, query or credentials.
/// Kept as its origin exactly as a browser serialises it in `Origin`
/// (lowercase scheme and host, no default port, no trailing slash), so the
/// two compare as plain strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicUrl {
    origin: String,
    https: bool,
}

impl PublicUrl {
    pub fn parse(input: &str) -> Result<Self, String> {
        let url = url::Url::parse(input.trim()).map_err(|err| format!("public_url is not a URL: {err}"))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("public_url must not hold credentials".into());
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err("public_url must be an origin only, with no path, query or fragment".into());
        }
        let https = match url.scheme() {
            "https" => true,
            "http" if is_loopback_host(&url) => false,
            "http" => return Err("public_url must be https://, or http:// to a loopback address".into()),
            _ => return Err("public_url must be https:// or http://".into()),
        };
        let origin = url.origin().ascii_serialization();
        Ok(Self { origin, https })
    }

    /// `scheme://host[:port]`, as a browser's `Origin` header has it.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Whether the session cookie is `Secure`: everywhere but loopback
    /// `http://` (kernel spec §3.2).
    pub fn is_https(&self) -> bool {
        self.https
    }
}

fn is_loopback_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// The outcome of `Operator::set_up`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOutcome {
    Done {
        owner_id: String,
    },
    /// There is an owner already.
    AlreadySetUp,
    /// Unknown, used or expired: one answer for all three.
    InvalidToken,
    /// The password or `public_url` is not acceptable (why); the token is
    /// not used up.
    Invalid(String),
}

struct SetupToken {
    hash: String,
    expires_at: i64,
}

/// Where the setup link was written (`Operator::announce_setup`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupLink {
    /// `<base>/setup#<token>`: the token in the fragment (3b decision 16).
    pub url: String,
    /// The 0600 file holding `url`.
    pub file: PathBuf,
}

pub struct Operator {
    conn: Mutex<Connection>,
    /// Loaded at open and replaced by setup: read on every browser request.
    public_url: RwLock<Option<PublicUrl>>,
    setup: Mutex<Option<SetupToken>>,
    /// The `setup-url` file, removed once setup is done.
    setup_file: Mutex<Option<PathBuf>>,
    /// `check_password`'s slots, each held until its verify ends.
    hashing: Arc<tokio::sync::Semaphore>,
    verifications: AtomicU64,
    /// Verifies running now, and the most ever at once (`check_password`'s
    /// bound, pinned by the unit tests below).
    #[cfg(test)]
    in_flight: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    max_in_flight: std::sync::atomic::AtomicUsize,
}

impl Operator {
    /// Open the kernel's tables in `hennery.db` (shared with the host
    /// registry and the sessions store).
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(db::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(db::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        db::migrate_component(&mut conn, schema::COMPONENT, schema::MIGRATIONS)?;
        let public_url = load_public_url(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            public_url: RwLock::new(public_url),
            setup: Mutex::new(None),
            setup_file: Mutex::new(None),
            hashing: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_HASHES)),
            verifications: AtomicU64::new(0),
            #[cfg(test)]
            in_flight: Default::default(),
            #[cfg(test)]
            max_in_flight: Default::default(),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("operator lock")
    }

    /// The owner's id, once setup has created it.
    pub fn owner_id(&self) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT id FROM owners ORDER BY created_at LIMIT 1", [], |r| r.get(0))
            .optional()?)
    }

    pub fn is_set_up(&self) -> Result<bool> {
        Ok(self.owner_id()?.is_some())
    }

    pub fn public_url(&self) -> Option<PublicUrl> {
        self.public_url.read().expect("public_url lock").clone()
    }

    /// A fresh setup token (64 hex characters), valid for
    /// `SETUP_TOKEN_TTL_SECS`, replacing any earlier one. `None` once there
    /// is an owner.
    pub fn issue_setup_token(&self, now: i64) -> Result<Option<String>> {
        if self.is_set_up()? {
            return Ok(None);
        }
        let token = hex::encode(random_bytes::<32>());
        *self.setup.lock().expect("setup lock") = Some(SetupToken {
            hash: sha256_hex(token.as_bytes()),
            expires_at: now + SETUP_TOKEN_TTL_SECS,
        });
        Ok(Some(token))
    }

    /// Before setup: issue a fresh setup token and write its link,
    /// `<base_url>/setup#<token>`, to `dir/setup-url` (kernel spec §3.1).
    /// The file is created 0600 under a temporary name and renamed into
    /// place, so an existing `setup-url` (even a symlink) is replaced, never
    /// written through. Once set up: remove a stale `setup-url` and return
    /// `None`.
    pub fn announce_setup(&self, dir: &Path, base_url: &str, now: i64) -> Result<Option<SetupLink>> {
        let file = dir.join(SETUP_URL_FILE);
        let Some(token) = self.issue_setup_token(now)? else {
            remove_setup_file(&file);
            return Ok(None);
        };
        let url = format!("{}/setup#{token}", base_url.trim_end_matches('/'));
        let temp = dir.join(format!(".{SETUP_URL_FILE}.{}.tmp", hex::encode(random_bytes::<8>())));
        let written = (|| {
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)?;
            writeln!(out, "{url}")?;
            out.sync_all()?;
            std::fs::rename(&temp, &file)
        })();
        if let Err(err) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(err).with_context(|| format!("write {}", file.display()));
        }
        *self.setup_file.lock().expect("setup file lock") = Some(file.clone());
        Ok(Some(SetupLink { url, file }))
    }

    /// Create the owner with `password` and store `public_url` (kernel spec
    /// §3.1), if `token` is the live setup token. The token is used up only
    /// once the owner is committed. Hashes the password: call it on a
    /// blocking thread. It takes no `check_password` slot: only the token's
    /// holder gets as far as the hash, and the setup lock serialises it.
    pub fn set_up(&self, token: &str, password: &str, public_url: &str, now: i64) -> Result<SetupOutcome> {
        // Held throughout, so two setups cannot both pass the checks.
        let mut setup = self.setup.lock().expect("setup lock");
        if self.is_set_up()? {
            return Ok(SetupOutcome::AlreadySetUp);
        }
        let live = setup
            .as_ref()
            .is_some_and(|t| t.expires_at > now && t.hash == sha256_hex(token.as_bytes()));
        if !live {
            return Ok(SetupOutcome::InvalidToken);
        }
        if let Some(problem) = password_problem(password) {
            return Ok(SetupOutcome::Invalid(problem));
        }
        let public_url = match PublicUrl::parse(public_url) {
            Ok(url) => url,
            Err(problem) => return Ok(SetupOutcome::Invalid(problem)),
        };
        let phc = password_auth::generate_hash(password);
        let owner_id = format!("owner-{}", hex::encode(random_bytes::<8>()));
        {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO owners(id, created_at) VALUES (?1, ?2)",
                params![owner_id, now],
            )?;
            tx.execute(
                "INSERT INTO password_credentials(owner_id, phc, updated_at) VALUES (?1, ?2, ?3)",
                params![owner_id, phc, now],
            )?;
            tx.execute(
                "INSERT INTO settings(owner_id, key, value) VALUES (?1, ?2, ?3)",
                params![owner_id, PUBLIC_URL_KEY, public_url.origin()],
            )?;
            tx.commit()?;
        }
        *setup = None;
        if let Some(file) = self.setup_file.lock().expect("setup file lock").take() {
            remove_setup_file(&file);
        }
        *self.public_url.write().expect("public_url lock") = Some(public_url);
        Ok(SetupOutcome::Done { owner_id })
    }

    /// Whether `password` is the owner's. Before setup it is checked
    /// against a dummy hash, so the answer takes as long either way.
    /// Blocking: prefer `check_password`.
    pub fn verify_password(&self, password: &str) -> Result<bool> {
        // First, so it is counted before `verifications` and dropped last.
        #[cfg(test)]
        let _gauge = InFlight::enter(self);
        self.verifications.fetch_add(1, Ordering::Relaxed);
        let phc: Option<String> = self
            .conn()
            .query_row("SELECT phc FROM password_credentials LIMIT 1", [], |r| r.get(0))
            .optional()?;
        let Some(phc) = phc else {
            let _ = password_auth::verify_password(password, dummy_hash());
            return Ok(false);
        };
        // An oversized password is still verified (and fails), so its
        // answer takes as long as any other.
        let password = if password.len() > MAX_PASSWORD_BYTES {
            ""
        } else {
            password
        };
        Ok(password_auth::verify_password(password, &phc).is_ok())
    }

    /// `verify_password` on a blocking thread, at most
    /// `MAX_CONCURRENT_HASHES` at once; others wait their turn. The slot
    /// moves into the blocking closure: a caller that goes away (a client
    /// disconnect, a timeout) cannot free it while its verify still runs.
    pub async fn check_password(self: &Arc<Self>, password: String) -> Result<bool> {
        let permit = self
            .hashing
            .clone()
            .acquire_owned()
            .await
            .context("the hashing semaphore is closed")?;
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            this.verify_password(&password)
        })
        .await?
    }

    /// How many password checks have run: every login attempt that is not
    /// rate limited runs exactly one (kernel spec §3.2's constant-time
    /// failure path), which tests pin with this.
    pub fn verifications(&self) -> u64 {
        self.verifications.load(Ordering::Relaxed)
    }
}

fn remove_setup_file(file: &Path) {
    match std::fs::remove_file(file) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(file = %file.display(), error = %err, "could not remove the setup link"),
    }
}

fn load_public_url(conn: &Connection) -> Result<Option<PublicUrl>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1 LIMIT 1",
            [PUBLIC_URL_KEY],
            |r| r.get(0),
        )
        .optional()?;
    stored
        .map(|s| PublicUrl::parse(&s).map_err(|why| anyhow::anyhow!("the stored public_url is invalid: {why}")))
        .transpose()
}

/// Why `password` cannot be the owner's, if it cannot.
fn password_problem(password: &str) -> Option<String> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Some(format!("the password must be at least {MIN_PASSWORD_CHARS} characters"));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Some(format!("the password must be at most {MAX_PASSWORD_BYTES} bytes"));
    }
    None
}

/// A hash no password is checked against for real, so a check without an
/// owner costs one Argon2 verify like any other.
fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| password_auth::generate_hash(hex::encode(random_bytes::<16>())))
}

/// A signed-in session lives this long past its last use (kernel spec §3.2).
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// A session's expiry slides at most this often, so a busy client does
/// not write to the database on every request.
pub const SESSION_SLIDE_SECS: i64 = 60;

/// A password check is fresh enough for step-up this long (kernel spec
/// §3.4).
pub const STEP_UP_SECS: i64 = 5 * 60;

/// The session cookie's name (kernel spec §3.2).
pub const SESSION_COOKIE: &str = "hennery_session";

/// The longest `User-Agent` kept with a session, in characters.
const MAX_USER_AGENT: usize = 256;

/// A request's session, once its cookie checks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authenticated {
    /// The session's id: the SHA-256 of its token, as stored.
    pub session_id: String,
    pub owner_id: String,
    pub last_step_up_at: Option<i64>,
    pub expires_at: i64,
    /// This request slid the expiry: the cookie is sent again with it.
    pub slid: bool,
}

impl Authenticated {
    /// Whether the last password check was within `STEP_UP_SECS`.
    pub fn stepped_up(&self, now: i64) -> bool {
        self.last_step_up_at.is_some_and(|at| now - at < STEP_UP_SECS)
    }
}

/// One signed-in session, as Settings lists them (kernel spec §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSession {
    pub id: String,
    pub user_agent: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub last_step_up_at: Option<i64>,
    pub expires_at: i64,
}

impl Operator {
    /// Open a session for the owner and return its token, the cookie's
    /// value. Only the token's hash is stored. The password was just
    /// checked, so the session starts stepped up. `None` before setup.
    pub fn open_session(&self, user_agent: &str, now: i64) -> Result<Option<String>> {
        let Some(owner_id) = self.owner_id()? else {
            return Ok(None);
        };
        let token = hex::encode(random_bytes::<32>());
        let user_agent: String = user_agent
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_USER_AGENT)
            .collect();
        let conn = self.conn();
        conn.execute("DELETE FROM auth_sessions WHERE expires_at <= ?1", [now])?;
        conn.execute(
            "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, last_step_up_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?5)",
            params![
                sha256_hex(token.as_bytes()),
                owner_id,
                user_agent,
                now,
                now + SESSION_TTL_SECS
            ],
        )?;
        Ok(Some(token))
    }

    /// The live session `token` names, if any. Its expiry slides to
    /// `SESSION_TTL_SECS` from now, at most every `SESSION_SLIDE_SECS`.
    pub fn authenticate(&self, token: &str, now: i64) -> Result<Option<Authenticated>> {
        if token.len() != 64 {
            return Ok(None);
        }
        let id = sha256_hex(token.as_bytes());
        let conn = self.conn();
        let row: Option<(String, i64, Option<i64>, i64)> = conn
            .query_row(
                "SELECT owner_id, last_seen_at, last_step_up_at, expires_at FROM auth_sessions
                 WHERE id_hash = ?1 AND expires_at > ?2",
                params![id, now],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((owner_id, last_seen_at, last_step_up_at, mut expires_at)) = row else {
            return Ok(None);
        };
        let slid = now - last_seen_at >= SESSION_SLIDE_SECS;
        if slid {
            expires_at = now + SESSION_TTL_SECS;
            conn.execute(
                "UPDATE auth_sessions SET last_seen_at = ?2, expires_at = ?3 WHERE id_hash = ?1",
                params![id, now, expires_at],
            )?;
        }
        Ok(Some(Authenticated {
            session_id: id,
            owner_id,
            last_step_up_at,
            expires_at,
            slid,
        }))
    }

    /// Record a fresh password check on a session (kernel spec §3.4).
    /// Whether the session still exists.
    pub fn step_up(&self, session_id: &str, now: i64) -> Result<bool> {
        let changed = self.conn().execute(
            "UPDATE auth_sessions SET last_step_up_at = ?2 WHERE id_hash = ?1",
            params![session_id, now],
        )?;
        Ok(changed > 0)
    }

    /// Every live session, most recently used first.
    pub fn sessions(&self, now: i64) -> Result<Vec<AuthSession>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id_hash, user_agent, created_at, last_seen_at, last_step_up_at, expires_at
             FROM auth_sessions WHERE expires_at > ?1 ORDER BY last_seen_at DESC, id_hash",
        )?;
        let rows = stmt.query_map([now], |r| {
            Ok(AuthSession {
                id: r.get(0)?,
                user_agent: r.get(1)?,
                created_at: r.get(2)?,
                last_seen_at: r.get(3)?,
                last_step_up_at: r.get(4)?,
                expires_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// End a session. Whether there was one.
    pub fn revoke_session(&self, session_id: &str) -> Result<bool> {
        let changed = self
            .conn()
            .execute("DELETE FROM auth_sessions WHERE id_hash = ?1", [session_id])?;
        Ok(changed > 0)
    }
}

/// `Set-Cookie` for a session (kernel spec §3.2): `HttpOnly`,
/// `SameSite=Strict`, `Path=/`, for `SESSION_TTL_SECS`, and `Secure`
/// unless `public_url` is loopback `http://`.
pub fn session_cookie(token: &str, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_TTL_SECS}{secure}")
}

/// `Set-Cookie` that removes the session cookie.
pub fn cleared_cookie(secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}")
}

/// The session token in a request's `Cookie` headers, if there is one.
pub fn session_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == SESSION_COOKIE).then_some(value)
        })
}

/// Counts one verify in `Operator::in_flight` for as long as it lives.
#[cfg(test)]
struct InFlight<'a>(&'a Operator);

#[cfg(test)]
impl<'a> InFlight<'a> {
    fn enter(op: &'a Operator) -> Self {
        let now = op.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        op.max_in_flight.fetch_max(now, Ordering::SeqCst);
        Self(op)
    }
}

#[cfg(test)]
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const NOW: i64 = 1_800_000_000;
    const PASSWORD: &str = "correct horse battery";

    fn set_up() -> Arc<Operator> {
        let op = Arc::new(Operator::open_in_memory().unwrap());
        let token = op.issue_setup_token(NOW).unwrap().unwrap();
        op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap();
        op
    }

    /// Wait (bounded) until `done` holds.
    async fn until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn at_most_two_checks_verify_at_once() {
        let op = set_up();
        let checks: Vec<_> = (0..8)
            .map(|_| {
                let op = op.clone();
                tokio::spawn(async move { op.check_password(PASSWORD.to_string()).await.unwrap() })
            })
            .collect();
        for check in checks {
            assert!(check.await.unwrap());
        }
        assert_eq!(op.verifications(), 8);
        assert!(op.max_in_flight.load(Ordering::SeqCst) <= MAX_CONCURRENT_HASHES);
    }

    /// A check whose caller goes away (a client disconnect, a timeout)
    /// keeps its slot until its verify ends: the verify runs on regardless.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_abandoned_check_keeps_its_slot_until_its_verify_ends() {
        let op = set_up();
        for round in 1..=8u64 {
            let checks: Vec<_> = (0..2)
                .map(|_| {
                    let op = op.clone();
                    tokio::spawn(async move { op.check_password(PASSWORD.to_string()).await })
                })
                .collect();
            until("both checks of the round verify", || op.verifications() == 2 * round).await;
            for check in &checks {
                check.abort();
            }
        }
        until("every verify ends", || op.in_flight.load(Ordering::SeqCst) == 0).await;
        let max = op.max_in_flight.load(Ordering::SeqCst);
        assert!(max <= MAX_CONCURRENT_HASHES, "{max} verifies ran at once");
    }
}
