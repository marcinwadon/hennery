//! Web Push (kernel spec §6; plan 10a): the VAPID key pair in
//! `<data>/vapid.key` and the tokens it signs, the owner's push
//! subscriptions, and each hat's push policy. Delivery and its triggers are
//! plan 10b's. The subscriptions and the policies share the host registry's
//! connection and owner, as hats do, and every query names the owner
//! (kernel spec §1).

use crate::hosts::{Hosts, is_valid_display_field};
use crate::secret::random_bytes;
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Signer;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use zeroize::Zeroizing;

/// The VAPID private key's file in the collector's data directory
/// (kernel spec §1).
pub const VAPID_KEY_FILE: &str = "vapid.key";

/// How long a VAPID token is valid. RFC 8292 §2 allows at most 24 hours.
pub const VAPID_TOKEN_SECS: i64 = 12 * 60 * 60;

/// The subscriptions an owner may have at once (decision 5): each is a
/// request per notification.
pub const MAX_SUBSCRIPTIONS: usize = 32;

/// The longest endpoint accepted (decision 3). The push services' own are
/// a few hundred bytes.
pub const MAX_ENDPOINT: usize = 2048;

/// The longest contact address accepted (RFC 5321's path limit).
pub const MAX_CONTACT: usize = 254;

/// Names that never reach a public push service (decision 3; the review's
/// A3): special-use and private-network suffixes. The address check at
/// every send is the guard; this refuses the obvious early.
const PRIVATE_SUFFIXES: &[&str] = &[
    "localhost",
    "local",
    "localdomain",
    "lan",
    "internal",
    "home.arpa",
    "onion",
    "test",
    "invalid",
    "example",
    "alt",
];

/// The collector's VAPID key pair (RFC 8292): a P-256 key generated on the
/// first start and kept in `vapid.key` (kernel spec §6). Every push
/// subscription is bound to its public key, so the file is never replaced:
/// one that cannot be read stops the start (decision 1). The private key
/// leaves this type only as signatures.
pub struct VapidKey {
    secret: p256::SecretKey,
    /// The public key as `applicationServerKey` takes it: the uncompressed
    /// point, base64url without padding.
    public: String,
}

impl std::fmt::Debug for VapidKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VapidKey")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

impl VapidKey {
    /// A fresh key held only in memory: for tests, and for a collector
    /// that has not loaded its file yet.
    pub fn generate() -> Self {
        Self::from_secret(fresh_secret())
    }

    /// The key in `dir/vapid.key`, created 0600 when there is none. A file
    /// there that is not a private, regular, 32-byte P-256 key is an error,
    /// never replaced (decision 1).
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let path = dir.join(VAPID_KEY_FILE);
        if let Some(key) = load(&path)? {
            return Ok(key);
        }
        create(dir, &path)?;
        load(&path)?.with_context(|| format!("{} vanished as it was created", path.display()))
    }

    /// The public key, base64url: `GET /api/push/vapid`.
    pub fn public_key(&self) -> &str {
        &self.public
    }

    /// The `Authorization` header of a push to `endpoint` at `now` (RFC 8292
    /// §3): `vapid t=<token>, k=<public key>`. The token names the
    /// endpoint's origin (`audience`), `subject` (`subject`) when there is
    /// one, and an expiry `VAPID_TOKEN_SECS` ahead.
    pub fn authorization(&self, endpoint: &url::Url, subject: Option<&str>, now: i64) -> String {
        let token = self.token(&audience(endpoint), subject, now + VAPID_TOKEN_SECS);
        format!("vapid t={token}, k={}", self.public)
    }

    /// A JWT (RFC 7519) signed with ES256 (RFC 7515, 7518 §3.4): header
    /// `{"typ":"JWT","alg":"ES256"}`, claims `aud`, `exp` and, when there is
    /// one, `sub`, every part base64url without padding (decision 8).
    pub fn token(&self, audience: &str, subject: Option<&str>, expires_at: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let mut claims = serde_json::json!({ "aud": audience, "exp": expires_at });
        if let Some(subject) = subject {
            claims["sub"] = subject.into();
        }
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let input = format!("{header}.{claims}");
        let signature = URL_SAFE_NO_PAD.encode(self.sign(input.as_bytes()));
        format!("{input}.{signature}")
    }

    /// ECDSA over P-256 with SHA-256, deterministic (RFC 6979), in the form
    /// JWS takes (RFC 7518 §3.4): `r ‖ s`, 32 bytes each, never DER.
    fn sign(&self, message: &[u8]) -> [u8; 64] {
        let key = p256::ecdsa::SigningKey::from(&self.secret);
        let signature: p256::ecdsa::Signature = key.sign(message);
        let mut out = [0u8; 64];
        out.copy_from_slice(&signature.to_bytes());
        out
    }

    fn from_secret(secret: p256::SecretKey) -> Self {
        let public = URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes());
        Self { secret, public }
    }
}

/// A VAPID token's `aud` (RFC 8292 §2): the endpoint's origin,
/// `scheme://host`, with `:port` only when it is not the scheme's default.
pub fn audience(endpoint: &url::Url) -> String {
    endpoint.origin().ascii_serialization()
}

/// A VAPID token's `sub` (kernel spec §6; maintainer decision 6b): `mailto:`
/// the owner's contact (`contact_problem` keeps it free of anything `mailto:`
/// would need escaped), else the `public_url` origin when it is `https`.
/// `None` for an `http` one (loopback): kernel §6 never sends a
/// non-routable `sub`, and Apple refuses anything but `mailto:` or `https:`
/// (the review's N1), so the token carries none.
pub fn subject(contact: Option<&str>, public_url: &crate::operator::PublicUrl) -> Option<String> {
    match contact {
        Some(contact) => Some(format!("mailto:{contact}")),
        None => public_url.is_https().then(|| public_url.origin().to_string()),
    }
}

/// A random scalar from the OS: a draw out of range (zero, or the group
/// order or more, about 2^-32 likely) is drawn again.
fn fresh_secret() -> p256::SecretKey {
    loop {
        let bytes = Zeroizing::new(random_bytes::<32>());
        if let Ok(secret) = p256::SecretKey::from_bytes(p256::FieldBytes::from_slice(&bytes[..])) {
            return secret;
        }
    }
}

/// The key at `path`, or `None` when there is no file. A symlink, a file
/// its group or others can read, or anything but 32 bytes of a valid key
/// is an error, which never quotes the file's bytes.
fn load(path: &Path) -> Result<Option<VapidKey>> {
    // Not followed if a symlink; and a FIFO put there does not block the
    // open (the review's A5): it is refused as not a regular file.
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("open {} (a symlink is refused)", path.display())),
    };
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "{} is not a regular file", path.display());
    ensure!(
        meta.permissions().mode() & 0o077 == 0,
        "{} must be readable by its owner only (chmod 600 it)",
        path.display()
    );
    let mut bytes = Zeroizing::new(Vec::with_capacity(33));
    file.take(33).read_to_end(&mut bytes)?;
    if bytes.len() != 32 {
        bail!(
            "{} is not a VAPID key (32 bytes). Every push subscription depends on it, so it is never \
             replaced: restore it from a backup, or remove it and subscribe every device again",
            path.display()
        );
    }
    let secret = p256::SecretKey::from_bytes(p256::FieldBytes::from_slice(&bytes[..]))
        .map_err(|_| anyhow::anyhow!("{} does not hold a valid P-256 key", path.display()))?;
    Ok(Some(VapidKey::from_secret(secret)))
}

/// Write a fresh key to `path`, 0600, under a temporary name first, then
/// hard-linked into place: a link never replaces a file, so of two starts
/// racing, the first key stays and the other start loads it. The temporary
/// file is removed whatever happens (the review's A6).
fn create(dir: &Path, path: &Path) -> Result<()> {
    let secret = fresh_secret();
    let bytes = Zeroizing::new(secret.to_bytes());
    let temp = dir.join(format!(".{VAPID_KEY_FILE}.{}.tmp", hex::encode(random_bytes::<8>())));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)
        .with_context(|| format!("create {}", temp.display()))?;
    let written = out.write_all(&bytes[..]).and_then(|()| out.sync_all());
    drop(out);
    let linked = written.and_then(|()| std::fs::hard_link(&temp, path));
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("create {}", path.display())),
    }
    std::fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .with_context(|| format!("sync {}", dir.display()))
}

/// `endpoint` as a push subscription's (decision 3), in the form it is
/// stored, looked up and sent to (the review's A2), or why it cannot be one.
/// The push services' endpoints are `https` URLs on their own domain names
/// at the default port; anything else is refused before it is stored, and
/// the address it resolves to is checked again on every send (kernel spec
/// §7.1).
pub fn parse_endpoint(endpoint: &str) -> Result<url::Url, String> {
    let too_long = || format!("an endpoint is at most {MAX_ENDPOINT} bytes");
    if endpoint.len() > MAX_ENDPOINT {
        return Err(too_long());
    }
    let Ok(url) = url::Url::parse(endpoint) else {
        return Err("an endpoint must be a URL".into());
    };
    if url.as_str().len() > MAX_ENDPOINT {
        return Err(too_long());
    }
    if url.scheme() != "https" {
        return Err("an endpoint must be https".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("an endpoint must not carry credentials".into());
    }
    if url.port().is_some() {
        return Err("an endpoint must use the default port".into());
    }
    if url.fragment().is_some() {
        return Err("an endpoint must not have a fragment".into());
    }
    let Some(url::Host::Domain(host)) = url.host() else {
        return Err("an endpoint must name its host by a domain name".into());
    };
    let host = host.trim_end_matches('.');
    let private = PRIVATE_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")));
    if !host.contains('.') || private {
        return Err("an endpoint must name a public host".into());
    }
    Ok(url)
}

/// The subscription's public key and secret (base64url, padded or not),
/// in the form they are stored: base64url without padding, so delivery
/// decodes them one way. `Err` unless they are a P-256 point, uncompressed,
/// and 16 bytes (RFC 8291 §3.2).
pub fn parse_keys(p256dh: &str, auth: &str) -> Result<(String, String), String> {
    let decode = |text: &str| URL_SAFE_NO_PAD.decode(text.trim_end_matches('='));
    let point = match decode(p256dh) {
        Ok(point) if point.len() == 65 && p256::PublicKey::from_sec1_bytes(&point).is_ok() => point,
        _ => return Err("keys.p256dh must be an uncompressed P-256 public key, base64url".into()),
    };
    match decode(auth) {
        Ok(secret) if secret.len() == 16 => Ok((URL_SAFE_NO_PAD.encode(point), URL_SAFE_NO_PAD.encode(secret))),
        _ => Err("keys.auth must be 16 bytes, base64url".into()),
    }
}

/// Why `contact` cannot be the owner's push contact, or `None`: a plain
/// e-mail address, which the VAPID token names as `mailto:` (kernel spec
/// §6; plan 10a decision 7). Letters, digits and `.+_-` before the `@`; a
/// domain name with a dot after it.
pub fn contact_problem(contact: &str) -> Option<String> {
    let valid = contact.len() <= MAX_CONTACT
        && contact.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && local
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '_' | '-'))
                && domain.contains('.')
                && domain.split('.').all(|label| {
                    !label.is_empty()
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
        });
    (!valid).then(|| format!("a contact must be an e-mail address of at most {MAX_CONTACT} characters"))
}

/// A subscription as the browser sent it (`PushSubscription.toJSON()`).
#[derive(Clone)]
pub struct NewSubscription<'a> {
    pub endpoint: &'a str,
    pub p256dh: &'a str,
    pub auth: &'a str,
    /// What Settings lists it as; the endpoint's host name when absent.
    pub device_label: Option<&'a str>,
    /// `expirationTime`, in seconds since the epoch.
    pub expires_at: Option<i64>,
}

/// The endpoint is a capability URL and `auth` a secret: neither is
/// printed (decision 3).
impl std::fmt::Debug for NewSubscription<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewSubscription")
            .field("endpoint_host", &endpoint_host(self.endpoint))
            .field("device_label", &self.device_label)
            .finish_non_exhaustive()
    }
}

fn endpoint_host(endpoint: &str) -> String {
    url::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default()
}

/// One stored subscription.
#[derive(Clone, PartialEq, Eq)]
pub struct Subscription {
    pub id: String,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub device_label: String,
    /// The signed-in session that subscribed it (`Authenticated::session_id`).
    pub auth_session: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("endpoint_host", &self.endpoint_host())
            .field("device_label", &self.device_label)
            .finish_non_exhaustive()
    }
}

impl Subscription {
    /// The endpoint's host name: what Settings shows of it.
    pub fn endpoint_host(&self) -> String {
        endpoint_host(&self.endpoint)
    }
}

/// The outcome of `Hosts::subscribe` and `Hosts::rotate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subscribed {
    Created(Subscription),
    /// The endpoint was the owner's already: its keys, label, session and
    /// expiry are replaced (a browser re-subscribing after a key change).
    /// A rotation's answer.
    Replaced(Subscription),
    Invalid(String),
    /// `MAX_SUBSCRIPTIONS` already.
    TooMany,
    /// Another owner has this endpoint (the review's A7).
    Conflict,
    /// A rotation whose old endpoint is not the owner's.
    NotFound,
    /// The subscribing session ended before the subscription was stored (a
    /// sign-out racing it; Task 1's review): nothing is stored.
    SignedOut,
}

/// A hat's push policy (kernel spec §6). The default, for a hat with no
/// row: not muted, no details, the session title shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PushPolicy {
    /// No notification for the hat's sessions.
    pub muted: bool,
    /// Include the agent's question title.
    pub details: bool,
    /// "Session needs your answer", without the session title.
    pub generic_title: bool,
}

const SUBSCRIPTION_COLUMNS: &str = "id, endpoint, p256dh, auth, device_label, auth_session, created_at, \
     expires_at, last_success_at, last_error";

fn read_subscription(r: &rusqlite::Row<'_>) -> rusqlite::Result<Subscription> {
    Ok(Subscription {
        id: r.get(0)?,
        endpoint: r.get(1)?,
        p256dh: r.get(2)?,
        auth: r.get(3)?,
        device_label: r.get(4)?,
        auth_session: r.get(5)?,
        created_at: r.get(6)?,
        expires_at: r.get(7)?,
        last_success_at: r.get(8)?,
        last_error: r.get(9)?,
    })
}

/// A new subscription checked, in the form it is stored: its endpoint,
/// its keys, and its label when it gave one.
struct Checked {
    endpoint: url::Url,
    p256dh: String,
    auth: String,
    label: Option<String>,
}

/// `new` checked at `now`, or the reason it cannot be stored.
fn checked(new: &NewSubscription<'_>, now: i64) -> Result<Checked, String> {
    let endpoint = parse_endpoint(new.endpoint)?;
    let (p256dh, auth) = parse_keys(new.p256dh, new.auth)?;
    let label = match new.device_label {
        Some(label) if is_valid_display_field(label) => Some(label.trim().to_string()),
        Some(_) => return Err("a device label must be 1 to 64 printable characters".into()),
        None => None,
    };
    if new.expires_at.is_some_and(|at| at <= now) {
        return Err("the subscription has expired already".into());
    }
    Ok(Checked {
        endpoint,
        p256dh,
        auth,
        label,
    })
}

/// Whether `err` is a uniqueness constraint failing: of the subscriptions'
/// columns only `endpoint` is unique, and only another owner's row can hold
/// it by then. Any other constraint is an error, not a conflict.
fn is_taken(err: &rusqlite::Error) -> bool {
    matches!(err, rusqlite::Error::SqliteFailure(e, _) if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
}

fn stored(tx: &Transaction<'_>, owner: &str, id: &str) -> Result<Subscription> {
    Ok(tx.query_row(
        &format!("SELECT {SUBSCRIPTION_COLUMNS} FROM push_subscriptions WHERE id = ?1 AND owner_id = ?2"),
        params![id, owner],
        read_subscription,
    )?)
}

impl Hosts {
    /// Store the subscription `new`, made by the signed-in session
    /// `auth_session` at `now`. An endpoint the owner has already is
    /// replaced (`Subscribed::Replaced`), not counted twice, and keeps its
    /// label unless `new` names one. The session must still be live when
    /// the row is written (`Subscribed::SignedOut`; Task 1's review): ending
    /// it removes its subscriptions, so one stored after would outlive it.
    pub fn subscribe(&self, new: &NewSubscription<'_>, auth_session: &str, now: i64) -> Result<Subscribed> {
        let new_one = match checked(new, now) {
            Ok(checked) => checked,
            Err(why) => return Ok(Subscribed::Invalid(why)),
        };
        let endpoint = new_one.endpoint.as_str();
        let owner = self.owner_id();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let live: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE id_hash = ?1 AND owner_id = ?2 AND expires_at > ?3)",
            params![auth_session, owner, now],
            |r| r.get(0),
        )?;
        if !live {
            return Ok(Subscribed::SignedOut);
        }
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM push_subscriptions WHERE endpoint = ?1 AND owner_id = ?2",
                params![endpoint, owner],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            tx.execute(
                "UPDATE push_subscriptions SET p256dh = ?3, auth = ?4, device_label = coalesce(?5, device_label),
                     auth_session = ?6, expires_at = ?7, last_error = NULL
                 WHERE id = ?1 AND owner_id = ?2",
                params![
                    id,
                    owner,
                    new_one.p256dh,
                    new_one.auth,
                    new_one.label,
                    auth_session,
                    new.expires_at
                ],
            )?;
            let sub = stored(&tx, owner, &id)?;
            tx.commit()?;
            return Ok(Subscribed::Replaced(sub));
        }
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM push_subscriptions WHERE owner_id = ?1",
            [owner],
            |r| r.get(0),
        )?;
        if count as usize >= MAX_SUBSCRIPTIONS {
            return Ok(Subscribed::TooMany);
        }
        let id = format!("push-{}", hex::encode(random_bytes::<8>()));
        let label = new_one
            .label
            .or_else(|| new_one.endpoint.host_str().map(str::to_string))
            .unwrap_or_default();
        let inserted = tx.execute(
            "INSERT INTO push_subscriptions(id, owner_id, endpoint, p256dh, auth, device_label, auth_session,
                 created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                owner,
                endpoint,
                new_one.p256dh,
                new_one.auth,
                label,
                auth_session,
                now,
                new.expires_at
            ],
        );
        match inserted {
            Ok(_) => {}
            Err(err) if is_taken(&err) => return Ok(Subscribed::Conflict),
            Err(err) => return Err(err.into()),
        }
        let sub = stored(&tx, owner, &id)?;
        tx.commit()?;
        Ok(Subscribed::Created(sub))
    }

    /// Replace the subscription of `old_endpoint` with `new` (a push service
    /// rotating a subscription, which the browser reports to its service
    /// worker; the review's A1). It keeps its id, `created_at` and the
    /// session that made it, and its label unless `new` names one. Only an
    /// endpoint the owner has is rotated (`Subscribed::NotFound`): the API
    /// never shows endpoints, so knowing one is holding that browser.
    pub fn rotate(&self, old_endpoint: &str, new: &NewSubscription<'_>, now: i64) -> Result<Subscribed> {
        let Ok(old_endpoint) = parse_endpoint(old_endpoint) else {
            return Ok(Subscribed::NotFound);
        };
        let new_one = match checked(new, now) {
            Ok(checked) => checked,
            Err(why) => return Ok(Subscribed::Invalid(why)),
        };
        let endpoint = new_one.endpoint.as_str();
        let owner = self.owner_id();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(id) = tx
            .query_row(
                "SELECT id FROM push_subscriptions WHERE endpoint = ?1 AND owner_id = ?2",
                params![old_endpoint.as_str(), owner],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        else {
            return Ok(Subscribed::NotFound);
        };
        // A row the browser made for its new endpoint already gives way.
        tx.execute(
            "DELETE FROM push_subscriptions WHERE endpoint = ?1 AND id <> ?2 AND owner_id = ?3",
            params![endpoint, id, owner],
        )?;
        let updated = tx.execute(
            "UPDATE push_subscriptions SET endpoint = ?3, p256dh = ?4, auth = ?5,
                 device_label = coalesce(?6, device_label), expires_at = ?7, last_error = NULL
             WHERE id = ?1 AND owner_id = ?2",
            params![
                id,
                owner,
                endpoint,
                new_one.p256dh,
                new_one.auth,
                new_one.label,
                new.expires_at
            ],
        );
        match updated {
            Ok(_) => {}
            Err(err) if is_taken(&err) => return Ok(Subscribed::Conflict),
            Err(err) => return Err(err.into()),
        }
        let sub = stored(&tx, owner, &id)?;
        tx.commit()?;
        Ok(Subscribed::Replaced(sub))
    }

    /// Every subscription of the owner's, oldest first.
    pub fn subscriptions(&self) -> Result<Vec<Subscription>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM push_subscriptions WHERE owner_id = ?1 ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([self.owner_id()], read_subscription)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Remove the subscription `id`. Whether there was one.
    pub fn unsubscribe(&self, id: &str) -> Result<bool> {
        let removed = self.conn().execute(
            "DELETE FROM push_subscriptions WHERE id = ?1 AND owner_id = ?2",
            params![id, self.owner_id()],
        )?;
        Ok(removed > 0)
    }

    /// Remove the subscription of `endpoint`: a browser unsubscribing
    /// itself, which knows its endpoint and not its id. Whether there was
    /// one.
    pub fn unsubscribe_endpoint(&self, endpoint: &str) -> Result<bool> {
        let Ok(endpoint) = parse_endpoint(endpoint) else {
            return Ok(false);
        };
        let removed = self.conn().execute(
            "DELETE FROM push_subscriptions WHERE endpoint = ?1 AND owner_id = ?2",
            params![endpoint.as_str(), self.owner_id()],
        )?;
        Ok(removed > 0)
    }

    // Delivery's three outcomes name the endpoint the push went to, as
    // well as the subscription: a browser rotating it meanwhile keeps the
    // id, and the old endpoint's answer is not the new one's (plan 10b-ii).

    /// Record that a push to `endpoint` reached its push service at `now`,
    /// clearing the subscription `id`'s last error (plan 10b-ii).
    pub fn record_push_success(&self, id: &str, endpoint: &str, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE push_subscriptions SET last_success_at = ?2, last_error = NULL
             WHERE id = ?1 AND endpoint = ?4 AND owner_id = ?3",
            params![id, now, self.owner_id(), endpoint],
        )?;
        Ok(())
    }

    /// Record why a push to the subscription `id` at `endpoint` failed: a
    /// short reason of delivery's own, never the endpoint or the service's
    /// answer (plan 10a's review, O2).
    pub fn record_push_error(&self, id: &str, endpoint: &str, error: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE push_subscriptions SET last_error = ?2 WHERE id = ?1 AND endpoint = ?4 AND owner_id = ?3",
            params![id, error, self.owner_id(), endpoint],
        )?;
        Ok(())
    }

    /// Remove the subscription `id` its push service says is gone, if it
    /// still has the `endpoint` the push went to (plan 10b-ii).
    pub fn remove_gone_subscription(&self, id: &str, endpoint: &str) -> Result<bool> {
        let removed = self.conn().execute(
            "DELETE FROM push_subscriptions WHERE id = ?1 AND endpoint = ?2 AND owner_id = ?3",
            params![id, endpoint, self.owner_id()],
        )?;
        Ok(removed > 0)
    }

    /// Remove the subscriptions whose `expirationTime` has passed at `now`
    /// (plan 10b-ii): how many.
    pub fn prune_expired_subscriptions(&self, now: i64) -> Result<usize> {
        Ok(self.conn().execute(
            "DELETE FROM push_subscriptions WHERE expires_at IS NOT NULL AND expires_at <= ?1 AND owner_id = ?2",
            params![now, self.owner_id()],
        )?)
    }

    /// Every hat's push policy, the hats oldest first: the default for a
    /// hat that never had one set.
    pub fn push_policies(&self) -> Result<Vec<(String, PushPolicy)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT h.id, coalesce(p.muted, 0), coalesce(p.details, 0), coalesce(p.generic_title, 0)
             FROM hats h LEFT JOIN hat_push_policies p ON p.hat_id = h.id AND p.owner_id = h.owner_id
             WHERE h.owner_id = ?1 ORDER BY h.created_at, h.id",
        )?;
        let rows = stmt.query_map([self.owner_id()], |r| {
            Ok((
                r.get(0)?,
                PushPolicy {
                    muted: r.get(1)?,
                    details: r.get(2)?,
                    generic_title: r.get(3)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The push policy of `hat_id`: the default for a hat with none set,
    /// and for a hat that does not exist (a session from before hats has
    /// none).
    pub fn push_policy(&self, hat_id: &str) -> Result<PushPolicy> {
        Ok(self
            .conn()
            .query_row(
                "SELECT muted, details, generic_title FROM hat_push_policies WHERE hat_id = ?1 AND owner_id = ?2",
                params![hat_id, self.owner_id()],
                |r| {
                    Ok(PushPolicy {
                        muted: r.get(0)?,
                        details: r.get(1)?,
                        generic_title: r.get(2)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    /// Set the push policy of `hat_id`. `false`, and nothing stored, when
    /// the owner has no such hat.
    pub fn set_push_policy(&self, hat_id: &str, policy: PushPolicy) -> Result<bool> {
        let owner = self.owner_id();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM hats WHERE id = ?1 AND owner_id = ?2)",
            params![hat_id, owner],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO hat_push_policies(hat_id, owner_id, muted, details, generic_title) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(hat_id) DO UPDATE SET muted = excluded.muted, details = excluded.details,
                 generic_title = excluded.generic_title
                 WHERE hat_push_policies.owner_id = ?2",
            // The `WHERE` cannot fail after the owner's `EXISTS` above: it
            // keeps the statement owner-scoped on its own (kernel spec §1).
            params![hat_id, owner, policy.muted, policy.details, policy.generic_title],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

/// How soon a push service should deliver a notification (RFC 8030 §5.3):
/// `high` for "needs your answer", `normal` otherwise (kernel spec §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    High,
    Normal,
}

/// A notification for the owner's devices, before the hat's policy is
/// applied (kernel spec §6; plan 10b decision 1). The modules that own a
/// trigger fill it in (ACP core §10, gateway §7); delivery applies the
/// policy, so no caller re-implements it:
/// - a muted hat: nothing is sent;
/// - `generic_title`: `generic_title` replaces `title`, and `body` is
///   dropped;
/// - `details`: `detail`, when there is one, replaces `body`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The hat whose policy applies; `''` (a session from before hats) has
    /// the default.
    pub hat_id: String,
    pub urgency: Urgency,
    /// The session title, or the connection's label.
    pub title: String,
    /// The title with nothing of the session's own: "Session needs your
    /// answer".
    pub generic_title: String,
    pub body: String,
    /// More than the default shows (the agent's question title), only
    /// under a hat with `details`.
    pub detail: Option<String>,
    /// The same-origin path the notification opens: `/sessions/<id>`, or
    /// `/mcp` for the gateway's. Never a URL.
    pub url: String,
    /// One notification per tag on a device: a newer one replaces it.
    pub tag: String,
}

/// The tags waiting to be delivered, at most (plan 10b-i decision 5). A
/// tag holds one notice, its latest, so one session cannot fill the queue
/// for the others (10b-i's review, A2).
pub const NOTICE_QUEUE: usize = 256;

#[derive(Default)]
struct Queue {
    /// Tags in the order they were first queued.
    order: std::collections::VecDeque<String>,
    /// Each queued tag's latest notice.
    latest: std::collections::HashMap<String, Notice>,
    /// Notices dropped because `NOTICE_QUEUE` tags were waiting.
    dropped: u64,
    /// Whether a reader is there; without one, every notice is dropped.
    read: bool,
}

struct Shared {
    queue: std::sync::Mutex<Queue>,
    ready: tokio::sync::Notify,
}

/// Where notices go to be delivered (kernel spec §6): a queue that
/// delivery (plan 10b-ii) drains. Cheap to clone; `notify` never waits and
/// never fails, so a trigger can call it from any thread, async or not.
#[derive(Clone)]
pub struct Push {
    shared: std::sync::Arc<Shared>,
}

/// The queue's reading end, for delivery. Dropping it makes `notify` drop
/// everything.
pub struct Notices {
    shared: std::sync::Arc<Shared>,
}

impl Push {
    /// A queue of `NOTICE_QUEUE` tags and its reading end.
    pub fn new() -> (Push, Notices) {
        let shared = std::sync::Arc::new(Shared {
            queue: std::sync::Mutex::new(Queue {
                read: true,
                ..Queue::default()
            }),
            ready: tokio::sync::Notify::new(),
        });
        (Push { shared: shared.clone() }, Notices { shared })
    }

    /// A queue nobody reads: every notice is dropped. What a collector has
    /// until delivery runs, and tests that do not look.
    pub fn detached() -> Push {
        Push::new().0
    }

    /// Queue `notice` for delivery. Never waits. A notice for a tag already
    /// waiting replaces it there, keeping its place: a device shows one
    /// notification per tag anyway. A new tag when `NOTICE_QUEUE` are
    /// waiting is dropped and counted, with a warning at the first drop and
    /// every power of two after. The log names no notice's text.
    pub fn notify(&self, notice: Notice) {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !queue.read {
            return;
        }
        if let Some(waiting) = queue.latest.get_mut(&notice.tag) {
            *waiting = notice;
        } else if queue.order.len() >= NOTICE_QUEUE {
            queue.dropped += 1;
            if queue.dropped.is_power_of_two() {
                tracing::warn!(dropped = queue.dropped, "push queue full; notifications dropped");
            }
            return;
        } else {
            queue.order.push_back(notice.tag.clone());
            queue.latest.insert(notice.tag.clone(), notice);
        }
        drop(queue);
        self.shared.ready.notify_one();
    }
}

impl Notices {
    /// The next notice, oldest tag first; waits for one.
    pub async fn recv(&mut self) -> Notice {
        let shared = self.shared.clone();
        loop {
            let ready = shared.ready.notified();
            if let Some(notice) = self.try_recv() {
                return notice;
            }
            ready.await;
        }
    }

    /// The next notice, if one is waiting.
    pub fn try_recv(&mut self) -> Option<Notice> {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tag = queue.order.pop_front()?;
        queue.latest.remove(&tag)
    }

    /// Notices dropped so far because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .dropped
    }
}

impl Drop for Notices {
    fn drop(&mut self) {
        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.read = false;
        queue.order.clear();
        queue.latest.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;
    use serde_json::{Value, json};

    fn notice(tag: &str, body: &str) -> Notice {
        Notice {
            hat_id: "hat-1".into(),
            urgency: Urgency::Normal,
            title: "t".into(),
            generic_title: "g".into(),
            body: body.into(),
            detail: None,
            url: format!("/sessions/{tag}"),
            tag: tag.into(),
        }
    }

    /// 10b-i's review, A2: a tag holds its latest notice, in its first
    /// place, so one session's burst is one notice.
    #[test]
    fn a_tag_waiting_keeps_its_place_and_takes_the_latest_notice() {
        let (push, mut notices) = Push::new();
        push.notify(notice("s1", "needs your answer"));
        push.notify(notice("s2", "finished"));
        push.notify(notice("s1", "failed"));
        assert_eq!(
            notices.try_recv().map(|n| (n.tag, n.body)),
            Some(("s1".into(), "failed".into()))
        );
        assert_eq!(notices.try_recv().map(|n| n.tag), Some("s2".into()));
        assert_eq!(notices.try_recv(), None);
    }

    #[test]
    fn a_full_queue_drops_new_tags_and_counts_them() {
        let (push, mut notices) = Push::new();
        for n in 0..NOTICE_QUEUE + 3 {
            push.notify(notice(&format!("s{n}"), "finished"));
        }
        assert_eq!(notices.dropped(), 3);
        // A waiting tag is still replaced.
        push.notify(notice("s0", "failed"));
        assert_eq!(notices.try_recv().map(|n| n.body), Some("failed".into()));
        assert_eq!(std::iter::from_fn(|| notices.try_recv()).count(), NOTICE_QUEUE - 1);
    }

    #[test]
    fn without_a_reader_every_notice_is_dropped() {
        let push = Push::detached();
        push.notify(notice("s1", "finished"));
        let (push, notices) = Push::new();
        drop(notices);
        push.notify(notice("s1", "finished"));
        assert!(push.shared.queue.lock().unwrap().latest.is_empty());
    }

    #[tokio::test]
    async fn a_reader_waits_for_the_next_notice() {
        let (push, mut notices) = Push::new();
        let reader = tokio::spawn(async move { notices.recv().await });
        tokio::task::yield_now().await;
        push.notify(notice("s1", "finished"));
        assert_eq!(reader.await.unwrap().tag, "s1");
    }

    /// A real browser's keys (web-push-native's example subscription).
    const P256DH: &str = "BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY";
    const AUTH: &str = "_ordMnz7uTCmrpBTeUV4Bw";

    /// RFC 8292 §2.4's example `Authorization` header, as printed there.
    const RFC8292_EXAMPLE: &str = "vapid t=eyJ0eXAiOiJKV1QiLCJhbGciOiJFUzI1NiJ9.eyJhdWQiOiJodHRwczovL3\
        B1c2guZXhhbXBsZS5uZXQiLCJleHAiOjE0NTM1MjM3NjgsInN1YiI6Im1ha\
        Wx0bzpwdXNoQGV4YW1wbGUuY29tIn0.i3CYb7t4xfxCDquptFOepC9GAu_H\
        LGkMlMuCGSK2rpiUfnK9ojFwDXb1JrErtmysazNjjvW2L9OkSSHzvoD1oA, \
        k=BA1Hxzyi1RUM1b5wjxsn7nGxAszw2u61m164i3MrAIxHF6YK5h4SDYic-dR\
        uU_RCPCfA5aq9ojSwk5Y2EmClBPs";

    /// Why a header failed the verifier: which check.
    #[derive(Debug, PartialEq)]
    enum Bad {
        Format(&'static str),
        Signature,
    }

    /// Strict base64url without padding: anything else is not a JWS part.
    fn part(text: &str) -> Result<Vec<u8>, Bad> {
        if !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Bad::Format("not unpadded base64url"));
        }
        URL_SAFE_NO_PAD.decode(text).map_err(|_| Bad::Format("not base64url"))
    }

    /// A verifier written from RFC 8292 §3 and RFC 7515 alone, not from
    /// the signing code: parse `vapid t=…, k=…`, split the compact JWS,
    /// check the header, take the signature as raw `r ‖ s` (64 bytes; DER
    /// is longer, and refused), and verify it with the key in `k`. The
    /// claims, parsed, or which check failed.
    fn verify(authorization: &str) -> Result<Value, Bad> {
        let rest = authorization
            .strip_prefix("vapid ")
            .ok_or(Bad::Format("not the vapid scheme"))?;
        let (t, k) = rest.split_once(", ").ok_or(Bad::Format("not t and k"))?;
        let token = t.strip_prefix("t=").ok_or(Bad::Format("no t="))?;
        let key = part(k.strip_prefix("k=").ok_or(Bad::Format("no k="))?)?;
        let parts: Vec<&str> = token.split('.').collect();
        let [header, claims, signature] = parts[..] else {
            return Err(Bad::Format("not a compact JWS"));
        };
        let header: Value = serde_json::from_slice(&part(header)?).map_err(|_| Bad::Format("header JSON"))?;
        if header != json!({ "typ": "JWT", "alg": "ES256" }) {
            return Err(Bad::Format("not the ES256 JWT header"));
        }
        let raw = part(signature)?;
        if raw.len() != 64 {
            return Err(Bad::Format("ES256 is r ‖ s, 64 bytes (RFC 7518 §3.4)"));
        }
        let signature = p256::ecdsa::Signature::from_slice(&raw).map_err(|_| Bad::Format("r or s"))?;
        let verifying = p256::ecdsa::VerifyingKey::from_sec1_bytes(&key).map_err(|_| Bad::Format("k"))?;
        let parsed: Value = serde_json::from_slice(&part(claims)?).map_err(|_| Bad::Format("claims JSON"))?;
        verifying
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .map_err(|_| Bad::Signature)?;
        Ok(parsed)
    }

    fn verified(authorization: &str) -> Value {
        verify(authorization).unwrap()
    }

    #[test]
    fn the_verifier_accepts_rfc_8292s_example() {
        assert_eq!(
            verified(RFC8292_EXAMPLE),
            json!({ "aud": "https://push.example.net", "exp": 1453523768, "sub": "mailto:push@example.com" })
        );
    }

    #[test]
    fn the_verifier_refuses_a_tampered_token() {
        // Another `aud`, still valid JSON: only the signature can tell.
        let (t, k) = RFC8292_EXAMPLE.split_once(", ").unwrap();
        let [header, _, signature]: [&str; 3] = t.strip_prefix("vapid t=").unwrap().split('.').collect::<Vec<_>>()[..]
            .try_into()
            .unwrap();
        let claims = URL_SAFE_NO_PAD
            .encode(r#"{"aud":"https://push.example.org","exp":1453523768,"sub":"mailto:push@example.com"}"#);
        let tampered = format!("vapid t={header}.{claims}.{signature}, {k}");
        assert_eq!(verify(&tampered), Err(Bad::Signature));
    }

    /// RFC 6979 A.2.5: P-256, SHA-256, message "sample". Deterministic, so
    /// the signature is known: `r ‖ s`, raw, as JWS takes it.
    #[test]
    fn signatures_are_rfc_6979_and_raw_r_s() {
        let x = hex::decode("C9AFA9D845BA75166B5C215767B1D6934E50C3DB36E89B127B8A622B120F6721").unwrap();
        let key = VapidKey::from_secret(p256::SecretKey::from_bytes(p256::FieldBytes::from_slice(&x)).unwrap());
        let expected = hex::decode(
            "EFD48B2AACB6A8FD1140DD9CD45E81D69D2C877B56AAF991C34D0EA84EAF3716\
             F7CB1C942D657C41D436C7A1B6E29F65F3E900DBB9AFF4064DC4AB2F843ACDA8",
        )
        .unwrap();
        assert_eq!(key.sign(b"sample").to_vec(), expected);
    }

    #[test]
    fn a_push_is_authorised_for_the_endpoints_origin_for_twelve_hours() {
        let key = VapidKey::generate();
        let endpoint = url::Url::parse("https://fcm.googleapis.com/fcm/send/abc:def?x=1").unwrap();
        let now = 1_800_000_000;
        let header = key.authorization(&endpoint, Some("mailto:me@example.com"), now);
        assert!(header.ends_with(&format!(", k={}", key.public_key())), "{header}");
        let claims = verified(&header);
        assert_eq!(
            claims,
            json!({ "aud": "https://fcm.googleapis.com", "exp": now + VAPID_TOKEN_SECS, "sub": "mailto:me@example.com" })
        );
        let lifetime = claims["exp"].as_i64().unwrap() - now;
        assert!(lifetime > 0 && lifetime <= 24 * 60 * 60);
        // Another key's `k` does not verify this token.
        let other = header.replace(key.public_key(), VapidKey::generate().public_key());
        assert_eq!(verify(&other), Err(Bad::Signature));
    }

    #[test]
    fn the_audience_is_the_origin_and_nothing_else() {
        for (endpoint, aud) in [
            ("https://fcm.googleapis.com/fcm/send/abc", "https://fcm.googleapis.com"),
            ("https://Web.Push.Apple.com/QAbc?x#y", "https://web.push.apple.com"),
            ("https://push.example.net:443/p", "https://push.example.net"),
            ("https://push.example.net:8443/p", "https://push.example.net:8443"),
            ("http://127.0.0.1:4000/push/1", "http://127.0.0.1:4000"),
        ] {
            assert_eq!(audience(&url::Url::parse(endpoint).unwrap()), aud, "{endpoint}");
        }
    }

    #[test]
    fn the_subject_is_the_contact_else_an_https_public_url() {
        let https = crate::operator::PublicUrl::parse("https://hennery.example").unwrap();
        let loopback = crate::operator::PublicUrl::parse("http://localhost:7117").unwrap();
        assert_eq!(
            subject(Some("me@example.com"), &loopback).as_deref(),
            Some("mailto:me@example.com")
        );
        assert_eq!(subject(None, &https).as_deref(), Some("https://hennery.example"));
        assert_eq!(subject(None, &loopback), None);
    }

    /// The review's N1: with no subject, the token carries no `sub` at all.
    #[test]
    fn a_token_without_a_subject_has_no_sub_claim() {
        let key = VapidKey::generate();
        let endpoint = url::Url::parse("https://fcm.googleapis.com/fcm/send/abc").unwrap();
        let claims = verified(&key.authorization(&endpoint, None, 1_800_000_000));
        assert_eq!(
            claims,
            json!({ "aud": "https://fcm.googleapis.com", "exp": 1_800_000_000 + VAPID_TOKEN_SECS })
        );
    }

    /// The private key never leaves the type but as signatures: not in
    /// `Debug`, not in a token or header, in no encoding.
    #[test]
    fn the_private_key_is_never_shown() {
        let key = VapidKey::generate();
        let secret = key.secret.to_bytes();
        let shown = [
            format!("{key:?}"),
            key.authorization(
                &url::Url::parse("https://fcm.googleapis.com/x").unwrap(),
                Some("mailto:a@b.co"),
                0,
            ),
        ];
        for encoded in [
            hex::encode(secret),
            hex::encode_upper(secret),
            URL_SAFE_NO_PAD.encode(secret),
            base64::engine::general_purpose::STANDARD.encode(secret),
        ] {
            for text in &shown {
                assert!(!text.contains(&encoded), "{text}");
            }
        }
    }

    #[test]
    fn the_push_services_endpoints_are_accepted_as_they_are() {
        for endpoint in [
            "https://fcm.googleapis.com/fcm/send/abc:def",
            "https://updates.push.services.mozilla.com/wpush/v2/gAAAA",
            "https://web.push.apple.com/QAbc",
            "https://wns2-par02p.notify.windows.com/w/?token=BQYAAA",
        ] {
            assert_eq!(parse_endpoint(endpoint).map(String::from), Ok(endpoint.to_string()));
        }
    }

    /// The review's A2: what is stored is what was checked.
    #[test]
    fn an_endpoint_is_stored_as_parsed() {
        assert_eq!(
            parse_endpoint(" https://FCM.googleapis.com/fcm/send/a\n").map(String::from),
            Ok("https://fcm.googleapis.com/fcm/send/a".to_string())
        );
    }

    #[test]
    fn an_endpoint_that_could_reach_inside_is_refused() {
        for endpoint in [
            "http://fcm.googleapis.com/fcm/send/abc",
            "https://127.0.0.1/push",
            "https://[::1]/push",
            "https://10.0.0.1/push",
            "https://0x7f000001/push",
            "https://2130706433/push",
            "https://localhost/push",
            "https://localhost./push",
            "https://push.localhost/push",
            "https://printer.local/push",
            "https://nas.lan/push",
            "https://metadata.google.internal/push",
            "https://router.home.arpa/push",
            "https://push.example/push",
            "https://abc.onion/push",
            "https://intranet/push",
            "https://fcm.googleapis.com:8443/push",
            "https://user:pw@fcm.googleapis.com/push",
            "https://fcm.googleapis.com/push#x",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(parse_endpoint(endpoint).is_err(), "{endpoint}");
        }
        let long = format!("https://fcm.googleapis.com/{}", "a".repeat(MAX_ENDPOINT));
        assert!(parse_endpoint(&long).is_err());
    }

    #[test]
    fn keys_must_be_a_point_and_sixteen_bytes() {
        let unpadded = Ok((P256DH.to_string(), AUTH.to_string()));
        assert_eq!(parse_keys(P256DH, AUTH), unpadded);
        // Padded keys are accepted, and given back unpadded.
        assert_eq!(parse_keys(&format!("{P256DH}="), &format!("{AUTH}==")), unpadded);
        // 15 bytes: `Auth::clone_from_slice` would panic on it (plan 10a
        // decision 3).
        assert!(parse_keys(P256DH, &URL_SAFE_NO_PAD.encode([0u8; 15])).is_err());
        assert!(parse_keys(P256DH, &URL_SAFE_NO_PAD.encode([0u8; 17])).is_err());
        assert!(parse_keys(&URL_SAFE_NO_PAD.encode([4u8; 65]), AUTH).is_err());
        let compressed = URL_SAFE_NO_PAD.encode(
            p256::PublicKey::from_sec1_bytes(&URL_SAFE_NO_PAD.decode(P256DH).unwrap())
                .unwrap()
                .to_encoded_point(true)
                .as_bytes(),
        );
        assert!(parse_keys(&compressed, AUTH).is_err());
        assert!(parse_keys("not base64!", AUTH).is_err());
    }

    #[test]
    fn a_contact_is_a_plain_address() {
        for contact in ["me@example.com", "first.last+push@mail.example.org", "a_b-c@x-y.io"] {
            assert_eq!(contact_problem(contact), None, "{contact}");
        }
        for contact in [
            "",
            "me",
            "@example.com",
            "me@",
            "me@localhost",
            "me@example..com",
            "me@-example.com",
            "me@example.com?subject=x",
            "me@example.com,you@example.com",
            "me @example.com",
            "mailto:me@example.com",
            "m\u{e9}@example.com",
        ] {
            assert!(contact_problem(contact).is_some(), "{contact:?}");
        }
        let long = format!("{}@example.com", "a".repeat(MAX_CONTACT));
        assert!(contact_problem(&long).is_some());
    }

    #[test]
    fn a_generated_key_has_a_65_byte_public_key() {
        let key = VapidKey::generate();
        let public = URL_SAFE_NO_PAD.decode(key.public_key()).unwrap();
        assert_eq!(public.len(), 65);
        assert_eq!(public[0], 4);
        assert_ne!(VapidKey::generate().public_key(), key.public_key());
    }

    #[test]
    fn the_key_file_is_created_private_and_then_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let first = VapidKey::load_or_create(dir.path()).unwrap();
        let path = dir.path().join(VAPID_KEY_FILE);
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(meta.len(), 32);
        let again = VapidKey::load_or_create(dir.path()).unwrap();
        assert_eq!(again.public_key(), first.public_key());
        // No temporary file is left behind.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [VAPID_KEY_FILE]);
    }

    #[test]
    fn a_bad_key_file_stops_the_start_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(VAPID_KEY_FILE);
        for (contents, mode) in [
            (vec![7u8; 31], 0o600),
            (vec![7u8; 33], 0o600),
            (vec![0u8; 32], 0o600),
            (vec![0xffu8; 32], 0o600),
            (vec![7u8; 32], 0o640),
            (vec![7u8; 32], 0o604),
        ] {
            std::fs::write(&path, &contents).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let err = VapidKey::load_or_create(dir.path()).unwrap_err();
            // The error names the file, never its bytes.
            assert!(!format!("{err:#}").contains(&hex::encode(&contents)));
            assert_eq!(std::fs::read(&path).unwrap(), contents);
        }
    }

    #[test]
    fn a_symlinked_key_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        std::fs::write(&target, [7u8; 32]).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join(VAPID_KEY_FILE)).unwrap();
        assert!(VapidKey::load_or_create(dir.path()).is_err());
    }

    /// The review's A5: a FIFO is refused, not waited on.
    #[test]
    fn a_fifo_for_a_key_file_is_refused_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::ffi::CString::new(dir.path().join(VAPID_KEY_FILE).as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let err = VapidKey::load_or_create(dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("not a regular file"), "{err:#}");
    }
}
