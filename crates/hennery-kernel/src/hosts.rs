//! Host identity and pairing (kernel spec §4): the `hosts` registry, one-time
//! pairing codes, and the check of a host's `hello` proof (ACP core §3.5).
//!
//! Every time is seconds since the Unix epoch and is passed in by the
//! caller (`secret::unix_now()` in production), so expiry is testable
//! without sleeping.

use crate::db;
use crate::secret::{random_bytes, sha256_hex};
use anyhow::Result;
use ed25519_dalek::{Signature, VerifyingKey};
use hennery_proto::frames::Capabilities;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

/// The kernel's component name in `schema_versions`.
const COMPONENT: &str = "kernel";

const MIGRATIONS: &[&str] = &["
    CREATE TABLE hosts (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        public_key TEXT NOT NULL UNIQUE,
        platform TEXT NOT NULL,
        host_version TEXT NOT NULL,
        capabilities TEXT NOT NULL DEFAULT '[]',
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked_at INTEGER);
    CREATE TABLE pairing_codes (
        code_hash TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        used_at INTEGER);
"];

/// A pairing code is valid this long (kernel spec §4.1).
pub const PAIRING_CODE_TTL_SECS: i64 = 10 * 60;

/// Crockford's base32 alphabet: no `I`, `L`, `O` or `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Longest accepted host name, version and platform string.
const MAX_FIELD: usize = 64;

/// A freshly minted pairing code, shown once.
#[derive(Debug, Clone, PartialEq)]
pub struct PairingCode {
    /// `XXXX-XXXX`.
    pub code: String,
    pub expires_at: i64,
}

/// What a host sends to enroll (kernel spec §4.1), besides the code.
#[derive(Debug, Clone, PartialEq)]
pub struct Enrollment {
    /// The host's Ed25519 public key, 64 lowercase hex characters.
    pub public_key: String,
    pub name: String,
    pub host_version: String,
    pub platform: String,
}

impl Enrollment {
    /// Why this enrollment cannot be stored, if it cannot.
    pub fn problem(&self) -> Option<String> {
        if parse_public_key(&self.public_key).is_none() {
            return Some("public_key must be an Ed25519 public key in hex".into());
        }
        for (field, value) in [
            ("name", &self.name),
            ("host_version", &self.host_version),
            ("platform", &self.platform),
        ] {
            let value = value.trim();
            if value.is_empty()
                || value.chars().count() > MAX_FIELD
                || value.chars().any(|c| c.is_control() || is_format_char(c))
            {
                return Some(format!("{field} must be 1 to {MAX_FIELD} printable characters"));
            }
        }
        None
    }
}

/// The outcome of `Hosts::enroll`.
#[derive(Debug, Clone, PartialEq)]
pub enum EnrollOutcome {
    Enrolled {
        host_id: String,
    },
    /// Unknown, expired or already used: one answer for all three.
    InvalidCode,
    /// A host with this public key is paired already; the code is not used.
    AlreadyPaired {
        host_id: String,
    },
    /// The enrollment itself is malformed (why); the code is not used.
    Invalid(String),
}

/// The outcome of `Hosts::register`.
#[derive(Debug, Clone, PartialEq)]
pub enum Registered {
    Created,
    AlreadyPaired { host_id: String },
}

/// The verdict on a `hello` (ACP core §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelloCheck {
    Accepted,
    /// The proof is valid, and the host is revoked.
    Revoked,
    /// Unknown host, or a proof that does not verify.
    BadProof,
}

/// The outcome of `Hosts::revoke`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revoke {
    Revoked,
    AlreadyRevoked,
    NotFound,
}

/// One paired host, as `GET /api/hosts` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct HostRecord {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub host_version: String,
    /// From its latest accepted `hello`.
    pub capabilities: Capabilities,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Invisible Unicode format characters (bidi overrides and isolates,
/// zero-width characters, the byte-order mark): a host name holding one can
/// display as another host's name.
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// A pairing code as typed, reduced to its eight canonical characters:
/// case, dashes and spaces ignored, `O` read as `0`, `I` and `L` as `1`.
pub fn normalize_code(input: &str) -> Option<String> {
    let mut out = String::with_capacity(8);
    for c in input.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' | '\t' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == 8).then_some(out)
}

/// A new random code, `XXXX-XXXX` (40 bits).
fn new_code() -> String {
    let bits = u64::from_be_bytes({
        let mut b = [0u8; 8];
        b[3..].copy_from_slice(&random_bytes::<5>());
        b
    });
    let mut out = String::with_capacity(9);
    for i in 0..8 {
        if i == 4 {
            out.push('-');
        }
        let index = (bits >> (35 - 5 * i)) & 0x1f;
        out.push(ALPHABET[index as usize] as char);
    }
    out
}

fn code_hash(normalized: &str) -> String {
    sha256_hex(normalized.as_bytes())
}

/// A hex Ed25519 public key that is a valid point and not of small order.
pub fn parse_public_key(hex_key: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = hex::decode(hex_key).ok()?.try_into().ok()?;
    let key = VerifyingKey::from_bytes(&bytes).ok()?;
    (!key.is_weak()).then_some(key)
}

/// Whether `proof` (hex) is `public_key`'s signature over the hello message
/// for this nonce, host id and protocol version.
pub fn verify_proof(public_key: &str, nonce: &[u8], host_id: &str, protocol_version: &str, proof: &str) -> bool {
    let Some(key) = parse_public_key(public_key) else {
        return false;
    };
    let Some(signature) = hex::decode(proof)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .map(|b| Signature::from_bytes(&b))
    else {
        return false;
    };
    let message = hennery_proto::hello_proof_message(nonce, host_id, protocol_version);
    key.verify_strict(&message, &signature).is_ok()
}

pub struct Hosts {
    conn: Mutex<Connection>,
}

impl Hosts {
    /// Open the kernel's tables in `hennery.db` (shared with the sessions
    /// store; each migrates only its own tables).
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(db::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(db::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        db::migrate_component(&mut conn, COMPONENT, MIGRATIONS)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("hosts lock")
    }

    /// Mint a single-use pairing code valid for `PAIRING_CODE_TTL_SECS`.
    /// Only its hash is stored.
    pub fn mint_pairing_code(&self, now: i64) -> Result<PairingCode> {
        let code = new_code();
        let expires_at = now + PAIRING_CODE_TTL_SECS;
        let normalized = normalize_code(&code).expect("a minted code is canonical");
        self.conn().execute(
            "INSERT INTO pairing_codes(code_hash, created_at, expires_at) VALUES (?1, ?2, ?3)",
            params![code_hash(&normalized), now, expires_at],
        )?;
        Ok(PairingCode { code, expires_at })
    }

    /// Pair a host with a code (kernel spec §4.1): check the code, create the
    /// host with a fresh id, and use the code up, in one transaction. A
    /// wrong code changes nothing, so other outstanding codes stay valid.
    pub fn enroll(&self, code: &str, enrollment: &Enrollment, now: i64) -> Result<EnrollOutcome> {
        if let Some(problem) = enrollment.problem() {
            return Ok(EnrollOutcome::Invalid(problem));
        }
        let Some(normalized) = normalize_code(code) else {
            return Ok(EnrollOutcome::InvalidCode);
        };
        let hash = code_hash(&normalized);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let usable = tx
            .query_row(
                "SELECT 1 FROM pairing_codes WHERE code_hash = ?1 AND used_at IS NULL AND expires_at > ?2",
                params![hash, now],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !usable {
            return Ok(EnrollOutcome::InvalidCode);
        }
        let host_id = new_host_id();
        match insert_host(&tx, &host_id, enrollment, now)? {
            Registered::Created => {}
            Registered::AlreadyPaired { host_id } => return Ok(EnrollOutcome::AlreadyPaired { host_id }),
        }
        tx.execute(
            "UPDATE pairing_codes SET used_at = ?2 WHERE code_hash = ?1",
            params![hash, now],
        )?;
        tx.commit()?;
        Ok(EnrollOutcome::Enrolled { host_id })
    }

    /// Store a host under a known id, without a code: what `enroll` does once
    /// the code checks out. Also how tests pair a host under a fixed id.
    pub fn register(&self, host_id: &str, enrollment: &Enrollment, now: i64) -> Result<Registered> {
        anyhow::ensure!(
            enrollment.problem().is_none(),
            "invalid enrollment: {:?}",
            enrollment.problem()
        );
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let registered = insert_host(&tx, host_id, enrollment, now)?;
        tx.commit()?;
        Ok(registered)
    }

    /// Check a `hello` (ACP core §3.5). The proof is verified before the
    /// revocation is looked at, so `revoked` is only ever told to the
    /// holder of the key.
    pub fn check_hello(&self, host_id: &str, nonce: &[u8], protocol_version: &str, proof: &str) -> Result<HelloCheck> {
        let row: Option<(String, Option<i64>)> = self
            .conn()
            .query_row(
                "SELECT public_key, revoked_at FROM hosts WHERE id = ?1",
                [host_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((public_key, revoked_at)) = row else {
            return Ok(HelloCheck::BadProof);
        };
        if !verify_proof(&public_key, nonce, host_id, protocol_version, proof) {
            return Ok(HelloCheck::BadProof);
        }
        Ok(if revoked_at.is_some() {
            HelloCheck::Revoked
        } else {
            HelloCheck::Accepted
        })
    }

    /// Update what an accepted `hello` reports (kernel spec §4.3).
    pub fn record_hello(&self, host_id: &str, host_version: &str, capabilities: &Capabilities, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE hosts SET host_version = ?2, capabilities = ?3, last_seen_at = ?4 WHERE id = ?1",
            params![host_id, host_version, serde_json::to_string(capabilities)?, now],
        )?;
        Ok(())
    }

    pub fn is_revoked(&self, host_id: &str) -> Result<bool> {
        let revoked: Option<Option<i64>> = self
            .conn()
            .query_row("SELECT revoked_at FROM hosts WHERE id = ?1", [host_id], |r| r.get(0))
            .optional()?;
        Ok(matches!(revoked, Some(Some(_))))
    }

    /// Mark a host revoked (kernel spec §4.3). Its future `hello`s get
    /// `revoked`; closing its connection and parking its sessions is the
    /// caller's part.
    pub fn revoke(&self, host_id: &str, now: i64) -> Result<Revoke> {
        let conn = self.conn();
        let revoked: Option<Option<i64>> = conn
            .query_row("SELECT revoked_at FROM hosts WHERE id = ?1", [host_id], |r| r.get(0))
            .optional()?;
        Ok(match revoked {
            None => Revoke::NotFound,
            Some(Some(_)) => Revoke::AlreadyRevoked,
            Some(None) => {
                conn.execute("UPDATE hosts SET revoked_at = ?2 WHERE id = ?1", params![host_id, now])?;
                Revoke::Revoked
            }
        })
    }

    pub fn host(&self, host_id: &str) -> Result<Option<HostRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {HOST_COLUMNS} FROM hosts WHERE id = ?1"))?;
        Ok(stmt.query_row([host_id], read_host).optional()?)
    }

    /// Every paired host, revoked ones included, oldest first.
    pub fn list(&self) -> Result<Vec<HostRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {HOST_COLUMNS} FROM hosts ORDER BY created_at, id"))?;
        let rows = stmt.query_map([], read_host)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

const HOST_COLUMNS: &str = "id, name, platform, host_version, capabilities, created_at, last_seen_at, revoked_at";

fn read_host(r: &rusqlite::Row<'_>) -> rusqlite::Result<HostRecord> {
    let capabilities: String = r.get(4)?;
    Ok(HostRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        platform: r.get(2)?,
        host_version: r.get(3)?,
        capabilities: serde_json::from_str(&capabilities).unwrap_or_default(),
        created_at: r.get(5)?,
        last_seen_at: r.get(6)?,
        revoked_at: r.get(7)?,
    })
}

fn new_host_id() -> String {
    format!("host-{}", hex::encode(random_bytes::<8>()))
}

fn insert_host(tx: &rusqlite::Transaction<'_>, host_id: &str, e: &Enrollment, now: i64) -> Result<Registered> {
    // Stored lowercase, so the same key in another case is the same key.
    let public_key = e.public_key.to_ascii_lowercase();
    let existing: Option<String> = tx
        .query_row("SELECT id FROM hosts WHERE public_key = ?1", [&public_key], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(host_id) = existing {
        return Ok(Registered::AlreadyPaired { host_id });
    }
    tx.execute(
        "INSERT INTO hosts(id, name, public_key, platform, host_version, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            host_id,
            e.name.trim(),
            public_key,
            e.platform.trim(),
            e.host_version.trim(),
            now
        ],
    )?;
    Ok(Registered::Created)
}
