//! The operator (kernel spec §3): the one owner, their password, the
//! `public_url` setting, and the one-time setup link that sets them up.
//!
//! - **One owner.** It exists from the first start (the kernel's
//!   migrations create it, plan 3b-iii decision 1); setup sets it up, once,
//!   and a second setup is refused. Every query names that owner (kernel
//!   spec §1), bound when the operator opens (`db::kernel_owner`).
//! - **The setup token lives in memory only.** A restart before setup
//!   issues a new one, and the old one is dead by construction (kernel
//!   spec §3.1). Only its SHA-256 is kept.
//! - **Passwords are Argon2id PHC strings** (`password-auth`). Hashing and
//!   verifying are CPU- and memory-heavy (about 19 MiB each), so they run
//!   on a blocking thread and at most `MAX_CONCURRENT_HASHES` at once.
//! - **Recovery** (the admin socket, kernel spec §4.2): a password reset
//!   and a `public_url` reset each end every session, and so its streams.
//!
//! Every time is seconds since the Unix epoch, passed in by the caller.

use crate::db;
use crate::ratelimit::{Limiter, Policy};
use crate::secret::{random_bytes, sha256_hex};
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
    /// The host, when it is a domain rather than an IP address.
    rp_id: Option<String>,
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
        let rp_id = url.domain().map(str::to_string);
        Ok(Self { origin, https, rp_id })
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

    /// The passkeys' relying party id (kernel spec §3.2): the host, in
    /// lowercase. `None` when the host is an IP address, which WebAuthn
    /// cannot bind a credential to.
    pub fn rp_id(&self) -> Option<&str> {
        self.rp_id.as_deref()
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
        /// The PHC string just stored: the session setup opens is bound to
        /// it (`open_session`).
        phc: String,
    },
    /// The owner is set up already.
    AlreadySetUp,
    /// Unknown, used or expired: one answer for all three.
    InvalidToken,
    /// The password or `public_url` is not acceptable (why); the token is
    /// not used up.
    Invalid(String),
}

/// The outcome of `Operator::reset_password` and `reset_public_url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reset {
    /// Done; this many signed-in sessions were ended, and this many
    /// passkeys removed: every one, by a password reset (plan 3c decision
    /// 2), or those bound to a host name `public_url` no longer has
    /// (decision 9).
    Done {
        sessions_ended: usize,
        passkeys_removed: usize,
    },
    /// The owner is not set up yet: setup is the way in.
    NotSetUp,
    /// The new password or `public_url` is not acceptable (why); nothing
    /// changed.
    Invalid(String),
}

struct SetupToken {
    hash: String,
    expires_at: i64,
}

/// Where the setup link was written (`Operator::announce_setup`). `Debug`
/// leaves the token out: a failing assertion or a log line must not show it.
#[derive(Clone, PartialEq, Eq)]
pub struct SetupLink {
    /// `<base>/setup#<token>`: the token in the fragment (3b decision 16).
    pub url: String,
    /// The 0600 file holding `url`.
    pub file: PathBuf,
}

impl std::fmt::Debug for SetupLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let base = self.url.split('#').next().unwrap_or_default();
        f.debug_struct("SetupLink")
            .field("url", &format_args!("{base}#<redacted>"))
            .field("file", &self.file)
            .finish()
    }
}

pub struct Operator {
    conn: Mutex<Connection>,
    /// The database's owner (`db::kernel_owner`), whom every query names.
    owner: String,
    /// Loaded at open and replaced by setup and by `reset_public_url`: read
    /// on every browser request.
    public_url: RwLock<Option<PublicUrl>>,
    setup: Mutex<Option<SetupToken>>,
    /// The link last written to `setup-url` (and so the file), removed
    /// once setup is done.
    announced: Mutex<Option<SetupLink>>,
    /// `check_password`'s slots, each held until its verify ends.
    hashing: Arc<tokio::sync::Semaphore>,
    verifications: AtomicU64,
    /// Wrong passwords per client address at login (kernel spec §3.2).
    pub login_limiter: Limiter,
    /// Wrong passwords per client address at step-up, a budget of its own
    /// (3b decision 10): a login flood from a shared address does not stop
    /// a signed-in owner stepping up, nor step-up guesses lock out login.
    pub step_up_limiter: Limiter,
    /// Passkey logins begun per client address (plan 3c decision 8): a
    /// budget of its own, untouched by wrong passwords.
    pub passkey_limiter: Limiter,
    /// Bumped whenever a session ends (revoked or signed out), and by
    /// `reset_password` and `reset_public_url`, each of which ends every
    /// session: streams held open by a session re-check it on every bump
    /// (3b decision 7).
    ended: tokio::sync::watch::Sender<u64>,
    /// The passkey ceremonies begun and not yet finished (plan 3c
    /// decision 5).
    pub ceremonies: crate::passkeys::Ceremonies,
    /// Verifies running now, and the most ever at once (`check_password`'s
    /// bound, pinned by the unit tests below).
    #[cfg(test)]
    in_flight: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    max_in_flight: std::sync::atomic::AtomicUsize,
    /// Password resets that have started hashing (`reset_password`'s
    /// permit, pinned by the unit tests below).
    #[cfg(test)]
    reset_hashes: AtomicU64,
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
        let owner = db::kernel_owner(&mut conn)?;
        let public_url = load_public_url(&conn, &owner)?;
        Ok(Self {
            conn: Mutex::new(conn),
            owner,
            public_url: RwLock::new(public_url),
            setup: Mutex::new(None),
            announced: Mutex::new(None),
            hashing: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_HASHES)),
            verifications: AtomicU64::new(0),
            login_limiter: Limiter::new(Policy::LOGIN),
            step_up_limiter: Limiter::new(Policy::LOGIN),
            passkey_limiter: Limiter::new(Policy::PASSKEY_LOGIN),
            ended: tokio::sync::watch::Sender::new(0),
            ceremonies: Default::default(),
            #[cfg(test)]
            in_flight: Default::default(),
            #[cfg(test)]
            max_in_flight: Default::default(),
            #[cfg(test)]
            reset_hashes: Default::default(),
        })
    }

    pub(crate) fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("operator lock")
    }

    /// The owner's id. The owner exists from the first start, set up or
    /// not (`is_set_up`).
    pub fn owner_id(&self) -> &str {
        &self.owner
    }

    /// Whether setup has run: the owner has a password and a `public_url`.
    pub fn is_set_up(&self) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT set_up_at IS NOT NULL FROM owners WHERE id = ?1",
            [&self.owner],
            |r| r.get(0),
        )?)
    }

    /// Whether the database answers a query (`/readyz`).
    pub fn ping(&self) -> Result<()> {
        self.conn().query_row("SELECT 1", [], |_| Ok(()))?;
        Ok(())
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
        let link = SetupLink { url, file };
        *self.announced.lock().expect("announced lock") = Some(link.clone());
        Ok(Some(link))
    }

    /// The setup link for the admin socket's `setup-url` (kernel spec
    /// §4.2): the one last announced while its token is live, else a fresh
    /// one, announced as `announce_setup` does (the old token dies). `None`
    /// once set up.
    pub fn setup_link(&self, dir: &Path, base_url: &str, now: i64) -> Result<Option<SetupLink>> {
        if self.is_set_up()? {
            return Ok(None);
        }
        let live = self
            .setup
            .lock()
            .expect("setup lock")
            .as_ref()
            .is_some_and(|t| t.expires_at > now);
        if live && let Some(link) = self.announced.lock().expect("announced lock").clone() {
            return Ok(Some(link));
        }
        self.announce_setup(dir, base_url, now)
    }

    /// Set the owner up with `password` and store `public_url` (kernel spec
    /// §3.1), if `token` is the live setup token, keeping the default hat's
    /// name. See `set_up_naming_hat`.
    pub fn set_up(&self, token: &str, password: &str, public_url: &str, now: i64) -> Result<SetupOutcome> {
        self.set_up_naming_hat(token, password, public_url, None, now)
    }

    /// Set the owner up with `password` and store `public_url` (kernel spec
    /// §3.1), if `token` is the live setup token, and name the default hat
    /// `hat_name` if one is given (plan 5a decision 1: the hat exists from
    /// the first start). The token is used up only once the owner is
    /// committed. Hashes the password: call it on a blocking thread. It
    /// takes no `check_password` slot: only the token's holder gets as far
    /// as the hash, and the setup lock serialises it.
    pub fn set_up_naming_hat(
        &self,
        token: &str,
        password: &str,
        public_url: &str,
        hat_name: Option<&str>,
        now: i64,
    ) -> Result<SetupOutcome> {
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
        if let Some(problem) = hat_name.and_then(crate::hats::hat_name_problem) {
            return Ok(SetupOutcome::Invalid(problem));
        }
        let phc = password_auth::generate_hash(password);
        let owner_id = self.owner.clone();
        {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            // Marked in the write, not only checked above: of two setups,
            // one marks the owner and the other finds it marked.
            let marked = tx.execute(
                "UPDATE owners SET set_up_at = ?2 WHERE id = ?1 AND set_up_at IS NULL",
                params![owner_id, now],
            )?;
            if marked == 0 {
                return Ok(SetupOutcome::AlreadySetUp);
            }
            tx.execute(
                "INSERT INTO password_credentials(owner_id, phc, updated_at) VALUES (?1, ?2, ?3)",
                params![owner_id, phc, now],
            )?;
            tx.execute(
                "INSERT INTO settings(owner_id, key, value) VALUES (?1, ?2, ?3)",
                params![owner_id, PUBLIC_URL_KEY, public_url.origin()],
            )?;
            if let Some(name) = hat_name {
                // The one hat there is before setup: the default for new
                // hosts, which `up`'s host may already have.
                let renamed = tx.execute(
                    "UPDATE hats SET name = ?2 WHERE owner_id = ?1
                         AND id = (SELECT value FROM settings WHERE owner_id = ?1 AND key = ?3)",
                    params![owner_id, name.trim(), crate::hats::DEFAULT_HAT_KEY],
                )?;
                anyhow::ensure!(renamed == 1, "the owner has no default hat to name");
            }
            tx.commit()?;
            // Still under the connection's lock: no reset lands between
            // the row and the cache (as `reset_public_url` does it too).
            *self.public_url.write().expect("public_url lock") = Some(public_url);
        }
        *setup = None;
        if let Some(link) = self.announced.lock().expect("announced lock").take() {
            remove_setup_file(&link.file);
        }
        Ok(SetupOutcome::Done { owner_id, phc })
    }

    /// Whether `password` is the owner's: the PHC string it verified
    /// against, or `None`. Before setup it is checked against a dummy hash,
    /// so the answer takes as long either way. A session opened on the
    /// strength of it passes the PHC to `open_session`, so a reset landing
    /// in between wins (3b-ii decision 11). Blocking: prefer
    /// `check_password`.
    pub fn verify_password(&self, password: &str) -> Result<Option<String>> {
        // First, so it is counted before `verifications` and dropped last.
        #[cfg(test)]
        let _gauge = InFlight::enter(self);
        self.verifications.fetch_add(1, Ordering::Relaxed);
        let phc: Option<String> = self
            .conn()
            .query_row(
                "SELECT phc FROM password_credentials WHERE owner_id = ?1",
                [&self.owner],
                |r| r.get(0),
            )
            .optional()?;
        let Some(phc) = phc else {
            let _ = password_auth::verify_password(password, dummy_hash());
            return Ok(None);
        };
        // An oversized password is still verified (and fails), so its
        // answer takes as long as any other.
        let password = if password.len() > MAX_PASSWORD_BYTES {
            ""
        } else {
            password
        };
        Ok(password_auth::verify_password(password, &phc).is_ok().then_some(phc))
    }

    /// `verify_password` on a blocking thread, at most
    /// `MAX_CONCURRENT_HASHES` at once; others wait their turn. The slot
    /// moves into the blocking closure: a caller that goes away (a client
    /// disconnect, a timeout) cannot free it while its verify still runs.
    pub async fn check_password(self: &Arc<Self>, password: String) -> Result<Option<String>> {
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

    /// Replace the owner's password (the admin socket's reset, kernel spec
    /// §4.2) and end every signed-in session, so their streams end too. The
    /// hash runs on a blocking thread and takes a `check_password` slot like
    /// any other, so a reset cannot add a third Argon2 run to a login
    /// flood. The limiters are cleared: the operator proved local access.
    /// It fails, changing nothing, unless it replaced exactly one password
    /// (3b-ii's O9): an owner with none (a passkey-only owner, in 3c) has
    /// nothing to reset. Every passkey is removed and every passkey
    /// ceremony begun before it ends, a finish re-checking under the lock
    /// in any case (plan 3c decision 2, amended by its review's A1): a
    /// passkey added with a stolen session must not survive the recovery.
    /// With no change-password route, every password change costs the
    /// passkeys.
    pub async fn reset_password(self: &Arc<Self>, password: String, now: i64) -> Result<Reset> {
        if let Some(problem) = password_problem(&password) {
            return Ok(Reset::Invalid(problem));
        }
        if !self.is_set_up()? {
            return Ok(Reset::NotSetUp);
        }
        let permit = self
            .hashing
            .clone()
            .acquire_owned()
            .await
            .context("the hashing semaphore is closed")?;
        #[cfg(test)]
        let this = self.clone();
        let phc = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            #[cfg(test)]
            this.reset_hashes.fetch_add(1, Ordering::SeqCst);
            password_auth::generate_hash(password)
        })
        .await?;
        let (ended, passkeys_removed) = {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            let replaced = tx.execute(
                "UPDATE password_credentials SET phc = ?2, updated_at = ?3 WHERE owner_id = ?1",
                params![self.owner, phc, now],
            )?;
            // Otherwise the transaction rolls back: no session ends.
            anyhow::ensure!(replaced == 1, "the owner has no password to reset");
            let ended = tx.execute("DELETE FROM auth_sessions WHERE owner_id = ?1", [&self.owner])?;
            // Each went with the session that subscribed it (plan 10a
            // decision 4): a device subscribed with a stolen session must
            // not survive the recovery either.
            tx.execute("DELETE FROM push_subscriptions WHERE owner_id = ?1", [&self.owner])?;
            let passkeys_removed = tx.execute("DELETE FROM passkeys WHERE owner_id = ?1", [&self.owner])?;
            tx.commit()?;
            // Still under the connection's lock, which every finish holds
            // from its check to its write: none lands after this.
            self.ceremonies.clear();
            (ended, passkeys_removed)
        };
        self.sessions_ended();
        self.login_limiter.clear();
        self.step_up_limiter.clear();
        self.passkey_limiter.clear();
        Ok(Reset::Done {
            sessions_ended: ended,
            passkeys_removed,
        })
    }

    /// Replace `public_url` (the admin socket's recovery when the collector
    /// moved, 3b decision 4): the stored row and the origin every browser
    /// request is checked against, which is cached here. Every signed-in
    /// session ends: they were opened at the old origin, and the new one
    /// signs in afresh. Passkeys are bound to the host name, the RP id
    /// (kernel spec §3.2): when it changes they stop working, and are
    /// removed; a move to another port or scheme keeps them (plan 3c
    /// decision 9). Every passkey ceremony begun before it ends too, a
    /// finish re-checking under the lock in any case.
    pub fn reset_public_url(&self, input: &str) -> Result<Reset> {
        let public_url = match PublicUrl::parse(input) {
            Ok(url) => url,
            Err(problem) => return Ok(Reset::Invalid(problem)),
        };
        if !self.is_set_up()? {
            return Ok(Reset::NotSetUp);
        }
        let (ended, passkeys_removed) = {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO settings(owner_id, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(owner_id, key) DO UPDATE SET value = excluded.value",
                params![self.owner, PUBLIC_URL_KEY, public_url.origin()],
            )?;
            let ended = tx.execute("DELETE FROM auth_sessions WHERE owner_id = ?1", [&self.owner])?;
            // Every subscription goes with its session (plan 10a decision
            // 4), and each was made by a service worker of the old origin.
            tx.execute("DELETE FROM push_subscriptions WHERE owner_id = ?1", [&self.owner])?;
            let same_host = self.public_url().as_ref().and_then(PublicUrl::rp_id) == public_url.rp_id();
            let passkeys_removed = if same_host {
                0
            } else {
                tx.execute("DELETE FROM passkeys WHERE owner_id = ?1", [&self.owner])?
            };
            tx.commit()?;
            // Still under the connection's lock: no other reset lands
            // between the row and the cache.
            *self.public_url.write().expect("public_url lock") = Some(public_url);
            self.ceremonies.clear();
            (ended, passkeys_removed)
        };
        self.sessions_ended();
        Ok(Reset::Done {
            sessions_ended: ended,
            passkeys_removed,
        })
    }

    /// The owner's push contact (kernel spec §6): an e-mail address, or
    /// `None` when they gave none and the VAPID token names `public_url`.
    pub fn contact(&self) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT contact FROM owners WHERE id = ?1", [&self.owner], |r| r.get(0))?)
    }

    /// Set the owner's push contact, or clear it with `None` (plan 10a
    /// decision 7). `Err` with the reason, and nothing stored, for one that
    /// is not a plain e-mail address.
    pub fn set_contact(&self, contact: Option<&str>) -> Result<Result<(), String>> {
        if let Some(problem) = contact.and_then(crate::push::contact_problem) {
            return Ok(Err(problem));
        }
        self.conn().execute(
            "UPDATE owners SET contact = ?2 WHERE id = ?1",
            params![self.owner, contact],
        )?;
        Ok(Ok(()))
    }

    /// Wake every stream held open by a session: some have ended.
    fn sessions_ended(&self) {
        self.ended.send_modify(|generation| *generation += 1);
    }
}

fn remove_setup_file(file: &Path) {
    match std::fs::remove_file(file) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(file = %file.display(), error = %err, "could not remove the setup link"),
    }
}

fn load_public_url(conn: &Connection, owner: &str) -> Result<Option<PublicUrl>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE owner_id = ?1 AND key = ?2",
            [owner, PUBLIC_URL_KEY],
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

/// `user_agent` as a session keeps it: no control characters, at most
/// `MAX_USER_AGENT` characters.
pub(crate) fn kept_user_agent(user_agent: &str) -> String {
    user_agent
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_USER_AGENT)
        .collect()
}

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

/// Store a session for `owner` that `token` opens, stepped up at `now`,
/// after dropping the owner's expired sessions: what every login does, by
/// password (`open_session`) or by passkey. With `verified`, the session is
/// stored only while that PHC string is still the owner's password, and
/// `false` says it was not. `None` skips that binding, so it is only for a
/// caller that proved the login some other way in the same transaction (a
/// passkey login's `record_use`).
pub(crate) fn insert_auth_session(
    conn: &Connection,
    owner: &str,
    token: &str,
    user_agent: &str,
    verified: Option<&str>,
    now: i64,
) -> Result<bool> {
    conn.execute(
        "DELETE FROM auth_sessions WHERE expires_at <= ?1 AND owner_id = ?2",
        params![now, owner],
    )?;
    let opened = conn.execute(
        "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, last_step_up_at, expires_at)
         SELECT ?1, ?2, ?3, ?4, ?4, ?4, ?5
         WHERE ?6 IS NULL OR EXISTS (SELECT 1 FROM password_credentials WHERE owner_id = ?2 AND phc = ?6)",
        params![
            sha256_hex(token.as_bytes()),
            owner,
            kept_user_agent(user_agent),
            now,
            now + SESSION_TTL_SECS,
            verified
        ],
    )?;
    Ok(opened > 0)
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
    /// checked, so the session starts stepped up. `verified` is the PHC
    /// string that check verified against (`verify_password`): the session
    /// is opened only while it is still the owner's, so a login whose check
    /// raced a password reset gets no session (`None`), like one before
    /// setup, when the owner has no password.
    pub fn open_session(&self, user_agent: &str, verified: &str, now: i64) -> Result<Option<String>> {
        let token = hex::encode(random_bytes::<32>());
        let opened = insert_auth_session(&self.conn(), &self.owner, &token, user_agent, Some(verified), now)?;
        Ok(opened.then_some(token))
    }

    /// The live session `token` names, if any. Its expiry slides to
    /// `SESSION_TTL_SECS` from now, at most every `SESSION_SLIDE_SECS`.
    pub fn authenticate(&self, token: &str, now: i64) -> Result<Option<Authenticated>> {
        if token.len() != 64 {
            return Ok(None);
        }
        let id = sha256_hex(token.as_bytes());
        let conn = self.conn();
        let row: Option<(i64, Option<i64>, i64)> = conn
            .query_row(
                "SELECT last_seen_at, last_step_up_at, expires_at FROM auth_sessions
                 WHERE id_hash = ?1 AND expires_at > ?2 AND owner_id = ?3",
                params![id, now, self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((last_seen_at, last_step_up_at, mut expires_at)) = row else {
            return Ok(None);
        };
        let slid = now - last_seen_at >= SESSION_SLIDE_SECS;
        if slid {
            expires_at = now + SESSION_TTL_SECS;
            conn.execute(
                "UPDATE auth_sessions SET last_seen_at = ?2, expires_at = ?3 WHERE id_hash = ?1 AND owner_id = ?4",
                params![id, now, expires_at, self.owner],
            )?;
        }
        Ok(Some(Authenticated {
            session_id: id,
            owner_id: self.owner.clone(),
            last_step_up_at,
            expires_at,
            slid,
        }))
    }

    /// Record a fresh password check on a session (kernel spec §3.4).
    /// Whether the session still exists.
    pub fn step_up(&self, session_id: &str, now: i64) -> Result<bool> {
        let changed = self.conn().execute(
            "UPDATE auth_sessions SET last_step_up_at = ?2 WHERE id_hash = ?1 AND owner_id = ?3",
            params![session_id, now, self.owner],
        )?;
        Ok(changed > 0)
    }

    /// Every live session, most recently used first.
    pub fn sessions(&self, now: i64) -> Result<Vec<AuthSession>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id_hash, user_agent, created_at, last_seen_at, last_step_up_at, expires_at
             FROM auth_sessions WHERE expires_at > ?1 AND owner_id = ?2 ORDER BY last_seen_at DESC, id_hash",
        )?;
        let rows = stmt.query_map(params![now, self.owner], |r| {
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

    /// End a live session, and the push subscriptions it made (plan 10a
    /// decision 4): a device signed out, or revoked as lost, gets no more
    /// notifications. Whether there was one: an expired session is not
    /// listed by `sessions`, so it is not there to revoke either.
    pub fn revoke_session(&self, session_id: &str, now: i64) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "DELETE FROM auth_sessions WHERE id_hash = ?1 AND expires_at > ?2 AND owner_id = ?3",
            params![session_id, now, self.owner],
        )?;
        if changed > 0 {
            tx.execute(
                "DELETE FROM push_subscriptions WHERE auth_session = ?1 AND owner_id = ?2",
                params![session_id, self.owner],
            )?;
        }
        tx.commit()?;
        drop(conn);
        if changed > 0 {
            self.sessions_ended();
        }
        Ok(changed > 0)
    }

    /// Changes whenever a session ends (`revoke_session`, and so logout,
    /// and the resets).
    pub fn session_ends(&self) -> tokio::sync::watch::Receiver<u64> {
        self.ended.subscribe()
    }

    /// When the live session `session_id` expires, without sliding it;
    /// `None` once it is gone or expired.
    pub fn session_expires_at(&self, session_id: &str, now: i64) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT expires_at FROM auth_sessions WHERE id_hash = ?1 AND expires_at > ?2 AND owner_id = ?3",
                params![session_id, now, self.owner],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The first of the request's session cookies (`session_tokens`) that
    /// authenticates, with its token; `None` when none does. Like
    /// `authenticate`, it slides the session it finds.
    pub fn authenticate_cookies(
        &self,
        headers: &axum::http::HeaderMap,
        now: i64,
    ) -> Result<Option<(String, Authenticated)>> {
        for token in session_tokens(headers) {
            if let Some(session) = self.authenticate(token, now)? {
                return Ok(Some((token.to_string(), session)));
            }
        }
        Ok(None)
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

/// At most this many `hennery_session` cookies of a request are tried, so
/// one request cannot cost many session lookups.
pub const MAX_SESSION_COOKIES: usize = 4;

/// Every session token in a request's `Cookie` headers, in order, at most
/// `MAX_SESSION_COOKIES`. There can be more than one: a sibling subdomain
/// can set a cookie of the same name for the parent domain, and the browser
/// may send it first. Taking only the first would let it sign the owner out.
pub fn session_tokens(headers: &axum::http::HeaderMap) -> Vec<&str> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == SESSION_COOKIE).then_some(value)
        })
        .take(MAX_SESSION_COOKIES)
        .collect()
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
            assert!(check.await.unwrap().is_some());
        }
        assert_eq!(op.verifications(), 8);
        assert!(op.max_in_flight.load(Ordering::SeqCst) <= MAX_CONCURRENT_HASHES);
    }

    /// A password reset hashes only once it holds a slot: with both taken
    /// (two verifies of a login flood, say) it waits, and runs once one is
    /// free.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_password_reset_hashes_only_with_a_hashing_slot() {
        let op = set_up();
        let held = op
            .hashing
            .clone()
            .acquire_many_owned(MAX_CONCURRENT_HASHES as u32)
            .await
            .unwrap();
        let reset = {
            let op = op.clone();
            tokio::spawn(async move { op.reset_password("a new long password".into(), NOW).await })
        };
        // Load can only make this pass falsely (the reset not yet started),
        // never fail falsely.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(op.reset_hashes.load(Ordering::SeqCst), 0, "hashed without a slot");
        drop(held);
        assert_eq!(
            reset.await.unwrap().unwrap(),
            Reset::Done {
                sessions_ended: 0,
                passkeys_removed: 0
            }
        );
        assert_eq!(op.reset_hashes.load(Ordering::SeqCst), 1);
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
