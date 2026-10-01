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

use crate::operator::PublicUrl;
use crate::secret::random_bytes;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use webauthn_rs::prelude::{PasskeyAuthentication, PasskeyRegistration, Url, Uuid, Webauthn, WebauthnBuilder};

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
