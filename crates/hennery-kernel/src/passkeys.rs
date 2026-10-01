//! Passkeys (kernel spec §3.2, plan 3c): WebAuthn with `webauthn-rs`.
//!
//! - **The relying party is `public_url`** (decision 1): its host is the RP
//!   id and its origin the one origin accepted, with no subdomain and no
//!   other port. A `public_url` whose host is an IP address has no RP id,
//!   so passkeys are unavailable there.
//! - **The user handle is derived from the owner's id** (decision 3), not
//!   stored: an authenticator keeps it, and it names no one.
//! - **Ceremonies live in memory** (decision 5, kernel spec §3.2), keyed by
//!   a random id: each is taken once, whatever its finish then finds, and
//!   lives at most `CEREMONY_TTL_SECS`. A restart drops them all; the
//!   browser starts again.
//! - **Passkeys add to the password** (decision 2): registering one needs
//!   a stepped-up session (decision 7), and removing one always leaves the
//!   password. Every query names the owner (kernel spec §1).

use crate::operator::{Operator, PublicUrl, STEP_UP_SECS};
use crate::secret::random_bytes;
use anyhow::Result;
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, RegisterPublicKeyCredential, Url, Uuid, Webauthn,
    WebauthnBuilder,
};

/// How long a ceremony may take from its start to its finish, and the
/// timeout the browser is given (decision 5).
pub const CEREMONY_TTL_SECS: i64 = 5 * 60;

/// At most this many login ceremonies are live at once, and as many
/// registrations and step-ups: two pools, so a flood of unauthenticated
/// login starts never pushes out a signed-in session's ceremony (decision
/// 5; 3c review, O1).
pub const MAX_LOGIN_CEREMONIES: usize = 1024;
pub const MAX_SESSION_CEREMONIES: usize = 1024;

/// The relying party's name, which an authenticator may show.
const RP_NAME: &str = "hennery";

/// The relying party for `public_url`: `None` when its host is an IP
/// address (decision 1).
pub fn relying_party(public_url: &PublicUrl) -> Option<Webauthn> {
    let rp_id = public_url.rp_id()?;
    let origin = Url::parse(public_url.origin()).ok()?;
    WebauthnBuilder::new(rp_id, &origin)
        .ok()?
        .rp_name(RP_NAME)
        .allow_subdomains(false)
        .allow_any_port(false)
        .timeout(std::time::Duration::from_secs(CEREMONY_TTL_SECS as u64))
        .build()
        .ok()
}

/// The owner's WebAuthn user handle: the first 16 bytes of the SHA-256 of
/// their id. The same owner always gets the same handle.
pub fn user_handle(owner_id: &str) -> Uuid {
    let digest = Sha256::digest(owner_id.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

/// A ceremony begun and not yet finished: what it is for, and the state
/// `webauthn-rs` checks its finish against.
pub enum Ceremony {
    /// Registering a passkey, labelled `label`, from a signed-in session.
    Register {
        session_id: String,
        label: String,
        state: PasskeyRegistration,
    },
    /// Signing in.
    Login { state: PasskeyAuthentication },
    /// Stepping up a signed-in session.
    StepUp {
        session_id: String,
        state: PasskeyAuthentication,
    },
}

impl Ceremony {
    /// The session a registration or a step-up belongs to, and its kind:
    /// a session has at most one live ceremony of each.
    fn slot(&self) -> Option<(&str, u8)> {
        match self {
            Self::Register { session_id, .. } => Some((session_id, 0)),
            Self::StepUp { session_id, .. } => Some((session_id, 1)),
            Self::Login { .. } => None,
        }
    }
}

struct Live {
    ceremony: Ceremony,
    expires_at: i64,
    /// The order ceremonies began in: past capacity, the oldest goes.
    seq: u64,
}

#[derive(Default)]
struct Table {
    live: HashMap<String, Live>,
    seq: u64,
}

/// The live ceremonies (decision 5).
pub struct Ceremonies {
    /// Login ceremonies, and the session-bound ones, each pool apart.
    logins: usize,
    sessions: usize,
    table: Mutex<Table>,
}

impl Default for Ceremonies {
    fn default() -> Self {
        Self::with_capacity(MAX_LOGIN_CEREMONIES, MAX_SESSION_CEREMONIES)
    }
}

impl Ceremonies {
    pub fn with_capacity(logins: usize, sessions: usize) -> Self {
        Self {
            logins,
            sessions,
            table: Mutex::new(Table::default()),
        }
    }

    fn table(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table.lock().expect("ceremonies lock")
    }

    /// Begin `ceremony` and return its id (64 hex characters). Expired
    /// ceremonies are dropped first, and the session's earlier ceremony of
    /// the same kind; past its pool's capacity, the oldest live one of that
    /// pool goes too.
    pub fn begin(&self, ceremony: Ceremony, now: i64) -> String {
        let id = hex::encode(random_bytes::<32>());
        let mut table = self.table();
        table.live.retain(|_, live| live.expires_at > now);
        if let Some(slot) = ceremony.slot() {
            table.live.retain(|_, live| live.ceremony.slot() != Some(slot));
        }
        let login = ceremony.slot().is_none();
        let capacity = if login { self.logins } else { self.sessions };
        loop {
            let pool = table
                .live
                .iter()
                .filter(|(_, live)| live.ceremony.slot().is_none() == login);
            if pool.clone().count() < capacity.max(1) {
                break;
            }
            let Some(oldest) = pool.min_by_key(|(_, live)| live.seq).map(|(id, _)| id.clone()) else {
                break;
            };
            table.live.remove(&oldest);
        }
        table.seq += 1;
        let seq = table.seq;
        table.live.insert(
            id.clone(),
            Live {
                ceremony,
                expires_at: now + CEREMONY_TTL_SECS,
                seq,
            },
        );
        id
    }

    /// Take the ceremony `id` names: it is gone whatever its finish then
    /// finds. `None` when there is none, or it has expired.
    pub fn take(&self, id: &str, now: i64) -> Option<Ceremony> {
        let live = self.table().live.remove(id)?;
        (live.expires_at > now).then_some(live.ceremony)
    }

    /// Drop every ceremony (`public_url` moved: decision 9).
    pub fn clear(&self) {
        self.table().live.clear();
    }

    /// How many are live, expired ones included until the next `begin`.
    pub fn len(&self) -> usize {
        self.table().live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The longest label a passkey takes, in characters.
pub const MAX_LABEL_CHARS: usize = 64;

/// The account name and display name an authenticator stores with the
/// passkey and may show beside the RP id. The label is hennery's own.
const USER_NAME: &str = "owner";
const USER_DISPLAY_NAME: &str = "hennery owner";

/// One of the owner's passkeys, as Settings lists them (kernel spec §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasskeyRecord {
    pub id: String,
    pub label: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// The outcome of starting a ceremony.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    /// Begun: `options` go to the browser's `navigator.credentials`, and
    /// the finish names `ceremony_id`.
    Begun {
        ceremony_id: String,
        options: serde_json::Value,
    },
    /// There is no `public_url` yet, or its host is an IP address
    /// (decision 1).
    Unavailable,
    /// The owner has no passkey to sign in or step up with.
    NoPasskeys,
    /// The label is not acceptable (why).
    Invalid(String),
}

/// Why a finish was refused. Nothing changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The ceremony is unknown, used, expired, or of another kind or
    /// session: one answer for all.
    Ceremony,
    /// The authenticator's answer is malformed or did not verify (why, for
    /// the log).
    Credential(String),
    /// The credential is registered already, to this owner or another
    /// (decision 3).
    AlreadyRegistered,
    /// The passkey is gone, or its counter did not move on (decision 6).
    Passkey,
    /// `public_url` no longer names a host (decision 1).
    Unavailable,
}

/// Why `label` cannot name a passkey, if it cannot.
fn label_problem(label: &str) -> Option<String> {
    let chars = label.chars().count();
    if chars == 0 || chars > MAX_LABEL_CHARS {
        return Some(format!("a passkey's label is 1 to {MAX_LABEL_CHARS} characters"));
    }
    if label.chars().any(char::is_control) {
        return Some("a passkey's label has no control characters".into());
    }
    None
}

/// How a credential id is stored: lowercase hex of its bytes.
fn stored_id(id: &[u8]) -> String {
    hex::encode(id)
}

impl Operator {
    /// The relying party for the `public_url` in effect (decision 1).
    fn relying_party(&self) -> Option<Webauthn> {
        self.public_url().as_ref().and_then(relying_party)
    }

    /// The owner's passkeys, oldest first, as `webauthn-rs` reads them.
    fn owner_passkeys(&self) -> Result<Vec<Passkey>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT credential FROM passkeys WHERE owner_id = ?1 ORDER BY created_at, rowid")?;
        let rows = stmt.query_map([self.owner_id()], |r| r.get::<_, String>(0))?;
        let mut passkeys = Vec::new();
        for json in rows {
            passkeys.push(serde_json::from_str(&json?)?);
        }
        Ok(passkeys)
    }

    /// Begin registering a passkey labelled `label` for the signed-in
    /// session `session_id` (decision 7: the route requires step-up). The
    /// owner's passkeys are excluded, so an authenticator that holds one
    /// says so rather than making a second.
    pub fn start_passkey_registration(&self, session_id: &str, label: &str, now: i64) -> Result<Start> {
        let label = label.trim();
        if let Some(problem) = label_problem(label) {
            return Ok(Start::Invalid(problem));
        }
        let Some(rp) = self.relying_party() else {
            return Ok(Start::Unavailable);
        };
        let exclude = self.owner_passkeys()?.iter().map(|p| p.cred_id().clone()).collect();
        let (options, state) = rp.start_passkey_registration(
            user_handle(self.owner_id()),
            USER_NAME,
            USER_DISPLAY_NAME,
            Some(exclude),
        )?;
        let ceremony = Ceremony::Register {
            session_id: session_id.into(),
            label: label.into(),
            state,
        };
        Ok(Start::Begun {
            ceremony_id: self.ceremonies.begin(ceremony, now),
            options: serde_json::to_value(options)?,
        })
    }

    /// Finish the registration `ceremony_id` names, begun by `session_id`,
    /// with the browser's `credential` (`RegisterPublicKeyCredential` as
    /// JSON), and store the passkey.
    pub fn finish_passkey_registration(
        &self,
        session_id: &str,
        ceremony_id: &str,
        credential: &serde_json::Value,
        now: i64,
    ) -> Result<std::result::Result<PasskeyRecord, Refused>> {
        let Some(Ceremony::Register {
            session_id: begun_by,
            label,
            state,
        }) = self.ceremonies.take(ceremony_id, now)
        else {
            return Ok(Err(Refused::Ceremony));
        };
        if begun_by != session_id {
            return Ok(Err(Refused::Ceremony));
        }
        // Held from the relying party's read to the write (3c review, A4):
        // a `public_url` reset, which takes this lock too, lands before or
        // after the whole finish, never between its check and its write.
        let conn = self.conn();
        let Some(rp) = self.relying_party() else {
            return Ok(Err(Refused::Unavailable));
        };
        let credential: RegisterPublicKeyCredential = match serde_json::from_value(credential.clone()) {
            Ok(credential) => credential,
            Err(err) => return Ok(Err(Refused::Credential(format!("malformed: {err}")))),
        };
        let passkey = match rp.finish_passkey_registration(&credential, &state) {
            Ok(passkey) => passkey,
            Err(err) => return Ok(Err(Refused::Credential(err.to_string()))),
        };
        let record = PasskeyRecord {
            id: format!("passkey-{}", hex::encode(random_bytes::<8>())),
            label,
            created_at: now,
            last_used_at: None,
        };
        // The counter starts at 0, below any the authenticator sends next
        // (decision 6). The write itself requires the session to be live
        // and stepped up (decision 7; 3c review, A2), as `open_session`
        // requires its password: a session ended, or a step-up lapsed,
        // since the route's check stores nothing.
        let inserted = conn.execute(
            "INSERT INTO passkeys(id, owner_id, credential_id, credential, sign_count, label, created_at)
             SELECT ?1, ?2, ?3, ?4, 0, ?5, ?6
             WHERE EXISTS (SELECT 1 FROM auth_sessions
                           WHERE id_hash = ?7 AND owner_id = ?2 AND expires_at > ?6 AND last_step_up_at > ?6 - ?8)
             ON CONFLICT(credential_id) DO NOTHING",
            params![
                record.id,
                self.owner_id(),
                stored_id(passkey.cred_id().as_ref()),
                serde_json::to_string(&passkey)?,
                record.label,
                now,
                session_id,
                STEP_UP_SECS
            ],
        )?;
        if inserted == 0 {
            // Nothing written: the session, or else a clash (decision 3).
            let live: bool = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM auth_sessions
                     WHERE id_hash = ?1 AND owner_id = ?2 AND expires_at > ?3 AND last_step_up_at > ?3 - ?4)",
                params![session_id, self.owner_id(), now, STEP_UP_SECS],
                |r| r.get(0),
            )?;
            return Ok(Err(if live {
                Refused::AlreadyRegistered
            } else {
                Refused::Ceremony
            }));
        }
        Ok(Ok(record))
    }

    /// The owner's passkeys, oldest first.
    pub fn passkeys(&self) -> Result<Vec<PasskeyRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, label, created_at, last_used_at FROM passkeys WHERE owner_id = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map([self.owner_id()], |r| {
            Ok(PasskeyRecord {
                id: r.get(0)?,
                label: r.get(1)?,
                created_at: r.get(2)?,
                last_used_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Remove the passkey `id` (the route requires step-up, kernel spec
    /// §3.4). Whether there was one. The password stays, so a login method
    /// always remains (decision 2).
    pub fn remove_passkey(&self, id: &str) -> Result<bool> {
        let removed = self.conn().execute(
            "DELETE FROM passkeys WHERE id = ?1 AND owner_id = ?2",
            params![id, self.owner_id()],
        )?;
        Ok(removed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn rp() -> Webauthn {
        relying_party(&PublicUrl::parse("https://hennery.example").unwrap()).unwrap()
    }

    fn login() -> Ceremony {
        let (_, state) = rp().start_passkey_authentication(&[]).unwrap();
        Ceremony::Login { state }
    }

    fn step_up(session_id: &str) -> Ceremony {
        let (_, state) = rp().start_passkey_authentication(&[]).unwrap();
        Ceremony::StepUp {
            session_id: session_id.into(),
            state,
        }
    }

    fn register(session_id: &str) -> Ceremony {
        let (_, state) = rp()
            .start_passkey_registration(Uuid::nil(), "owner", "owner", None)
            .unwrap();
        Ceremony::Register {
            session_id: session_id.into(),
            label: "laptop".into(),
            state,
        }
    }

    #[test]
    fn a_ceremony_is_taken_once() {
        let ceremonies = Ceremonies::default();
        let id = ceremonies.begin(login(), NOW);
        assert_eq!(id.len(), 64);
        assert!(matches!(ceremonies.take(&id, NOW), Some(Ceremony::Login { .. })));
        assert!(ceremonies.take(&id, NOW).is_none());
        assert!(ceremonies.take("unknown", NOW).is_none());
    }

    #[test]
    fn a_ceremony_expires_after_its_ttl() {
        let ceremonies = Ceremonies::default();
        let live = ceremonies.begin(login(), NOW);
        let expired = ceremonies.begin(login(), NOW);
        assert!(ceremonies.take(&live, NOW + CEREMONY_TTL_SECS - 1).is_some());
        assert!(ceremonies.take(&expired, NOW + CEREMONY_TTL_SECS).is_none());
        // An expired ceremony is gone once taken, even at an earlier time.
        assert!(ceremonies.take(&expired, NOW).is_none());
    }

    #[test]
    fn a_sessions_new_ceremony_replaces_its_last_of_that_kind() {
        let ceremonies = Ceremonies::default();
        let first = ceremonies.begin(register("s1"), NOW);
        let step_up_s1 = ceremonies.begin(step_up("s1"), NOW);
        let other_session = ceremonies.begin(register("s2"), NOW);
        let login_1 = ceremonies.begin(login(), NOW);
        let login_2 = ceremonies.begin(login(), NOW);
        let second = ceremonies.begin(register("s1"), NOW);
        assert!(ceremonies.take(&first, NOW).is_none());
        for id in [&step_up_s1, &other_session, &login_1, &login_2, &second] {
            assert!(ceremonies.take(id, NOW).is_some());
        }
    }

    #[test]
    fn past_capacity_the_oldest_ceremony_goes() {
        let ceremonies = Ceremonies::with_capacity(3, 3);
        let ids: Vec<String> = (0..4).map(|_| ceremonies.begin(login(), NOW)).collect();
        assert_eq!(ceremonies.len(), 3);
        assert!(ceremonies.take(&ids[0], NOW).is_none());
        for id in &ids[1..] {
            assert!(ceremonies.take(id, NOW).is_some());
        }
    }

    /// Expired ceremonies make room before a live one is pushed out.
    #[test]
    fn expired_ceremonies_go_before_the_oldest_live_one() {
        let ceremonies = Ceremonies::with_capacity(2, 2);
        let expiring = ceremonies.begin(login(), NOW - CEREMONY_TTL_SECS);
        let live = ceremonies.begin(login(), NOW - 1);
        let new = ceremonies.begin(login(), NOW);
        assert_eq!(ceremonies.len(), 2);
        assert!(ceremonies.take(&expiring, NOW - CEREMONY_TTL_SECS).is_none());
        assert!(ceremonies.take(&live, NOW).is_some());
        assert!(ceremonies.take(&new, NOW).is_some());
    }

    /// Login ceremonies have a pool of their own (3c review, O1): a flood
    /// of unauthenticated starts pushes out only other logins, never a
    /// signed-in session's registration or step-up. Past their own
    /// capacity, those push out only each other.
    #[test]
    fn a_login_flood_never_pushes_out_a_sessions_ceremony() {
        let ceremonies = Ceremonies::with_capacity(2, 2);
        let registration = ceremonies.begin(register("s1"), NOW);
        let step_up_s1 = ceremonies.begin(step_up("s1"), NOW);
        let logins: Vec<String> = (0..5).map(|_| ceremonies.begin(login(), NOW)).collect();
        assert_eq!(ceremonies.len(), 4);
        assert!(ceremonies.take(&registration, NOW).is_some());
        assert!(ceremonies.take(&step_up_s1, NOW).is_some());
        assert!(ceremonies.take(&logins[2], NOW).is_none());
        assert!(ceremonies.take(&logins[4], NOW).is_some());
        let sessions: Vec<String> = (0..3)
            .map(|n| ceremonies.begin(register(&format!("s{n}")), NOW))
            .collect();
        assert!(ceremonies.take(&logins[3], NOW).is_some());
        assert!(ceremonies.take(&sessions[0], NOW).is_none());
        assert!(ceremonies.take(&sessions[2], NOW).is_some());
    }

    #[test]
    fn clear_drops_every_ceremony() {
        let ceremonies = Ceremonies::default();
        let ids = [
            ceremonies.begin(login(), NOW),
            ceremonies.begin(register("s1"), NOW),
            ceremonies.begin(step_up("s1"), NOW),
        ];
        ceremonies.clear();
        assert!(ceremonies.is_empty());
        for id in &ids {
            assert!(ceremonies.take(id, NOW).is_none());
        }
    }
}
