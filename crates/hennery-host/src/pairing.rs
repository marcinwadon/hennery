//! `hennery host join <url> <code>` (kernel spec §4.1): generate the host's
//! key, enroll it with a pairing code, and store the pairing.

use crate::connection::{Standing, probe};
use crate::identity::{CONFIG_FILE, HostKey, KEY_FILE, Paired, create_private_dir, fsync_parent, write_config};
use crate::outbox::FILE as OUTBOX_FILE;
use anyhow::{Context, Result, bail};
use hennery_proto::rest::{ApiError, EnrollRequest, EnrollResponse};
use reqwest::Url;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long one enrollment request may take.
const ENROLL_TIMEOUT: Duration = Duration::from_secs(30);

/// What `join` did.
#[derive(Debug, Clone, PartialEq)]
pub enum Joined {
    /// Enrolled now, under this id.
    Paired { host_id: String },
    /// The collector still accepts the stored pairing; the code was not
    /// used.
    AlreadyPaired { host_id: String },
}

/// The collector's base URL, checked: `https://`, or `http://` to a
/// loopback address only (umbrella spec §7.5), with no path.
pub fn parse_public_url(public_url: &str) -> Result<Url> {
    let url = Url::parse(public_url.trim()).with_context(|| format!("{public_url} is not a URL"))?;
    let Some(host) = url.host_str() else {
        bail!("{public_url} has no host");
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => bail!("{public_url}: plain http is only allowed to a loopback address; use https"),
        other => bail!("{public_url}: unsupported scheme {other}"),
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        bail!("{public_url}: give the collector's base URL, without a path");
    }
    Ok(url)
}

/// The host WebSocket of the collector at `public_url`.
pub fn collector_ws_url(public_url: &str) -> Result<String> {
    let mut url = parse_public_url(public_url)?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).expect("ws and wss are valid schemes here");
    url.set_path("/api/hosts/ws");
    Ok(url.to_string())
}

/// `<os>-<arch>`, as enrollment reports it.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// This machine's host name, the default name of a new host.
pub fn default_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname(2) writes at most `buf.len()` bytes into `buf`.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    match std::str::from_utf8(&buf[..len]) {
        Ok(name) if rc == 0 && !name.is_empty() => name.chars().take(64).collect(),
        _ => "host".into(),
    }
}

/// Move an outbox that belongs to another identity out of the way, so the
/// new one starts empty and the old frames are kept for inspection:
/// `outbox.db` (and its `-wal`, `-shm`) becomes `outbox.db.orphaned-<label>`
/// (with a numeric suffix if that name is taken). `None` if there was none.
pub fn orphan_outbox(data_dir: &Path, label: &str) -> Result<Option<PathBuf>> {
    let db = data_dir.join(OUTBOX_FILE);
    if !db.exists() {
        return Ok(None);
    }
    let target = |n: u32| {
        let name = match n {
            0 => format!("{OUTBOX_FILE}.orphaned-{label}"),
            n => format!("{OUTBOX_FILE}.orphaned-{label}.{n}"),
        };
        data_dir.join(name)
    };
    let mut n = 0;
    while target(n).exists() {
        n += 1;
    }
    let base = target(n);
    for suffix in ["", "-wal", "-shm"] {
        let from = data_dir.join(format!("{OUTBOX_FILE}{suffix}"));
        if from.exists() {
            let mut to = base.clone().into_os_string();
            to.push(suffix);
            std::fs::rename(&from, &to).with_context(|| format!("move {} aside", from.display()))?;
        }
    }
    tracing::warn!(kept = %base.display(), "moved an outbox of an earlier identity aside");
    Ok(Some(base))
}

/// Whether two host WebSocket URLs name the same collector: equal, or both
/// on loopback (the all-in-one host's collector may listen on another port,
/// or be reached as `localhost` instead of `127.0.0.1`).
pub fn same_collector(a: &str, b: &str) -> bool {
    let loopback = |u: &str| {
        Url::parse(u)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .is_some_and(|h| {
                h.eq_ignore_ascii_case("localhost")
                    || h.trim_start_matches('[')
                        .trim_end_matches(']')
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            })
    };
    a == b || (loopback(a) && loopback(b))
}

/// Pair the host whose data directory is `data_dir` with the collector at
/// `public_url` (kernel spec §4.1). Idempotent: if the collector still
/// accepts the stored key, nothing changes and the code is not spent. A
/// pairing it revoked is replaced by a new key and a new host id, and the
/// outbox of the old identity is moved aside (`orphan_outbox`). A pairing
/// it does not know is refused: that needs the operator's decision.
pub async fn join(public_url: &str, code: &str, data_dir: &Path, name: &str) -> Result<Joined> {
    let base = parse_public_url(public_url)?;
    if base.scheme() == "https" {
        // Enrolling would spend the code on a host that cannot connect.
        bail!(
            "{public_url}: hosts cannot connect over wss:// in this version yet; pair over a loopback http:// address, or wait for the wss:// support"
        );
    }
    let collector_url = collector_ws_url(public_url)?;
    if let Some(paired) = Paired::load(data_dir)? {
        if !same_collector(&paired.collector_url, &collector_url) {
            bail!(
                "{} is paired with {} already; remove {} and {} to pair it with {public_url} instead",
                data_dir.display(),
                paired.collector_url,
                data_dir.join(KEY_FILE).display(),
                data_dir.join(CONFIG_FILE).display()
            );
        }
        match probe(&collector_url, &paired.host_id, &paired.key).await? {
            Standing::Accepted => {
                return Ok(Joined::AlreadyPaired {
                    host_id: paired.host_id,
                });
            }
            // Only a revoke is certain: the collector checked the key and
            // refused it for good.
            Standing::Revoked => {
                orphan_outbox(data_dir, &paired.host_id)?;
            }
            // A collector that no longer knows the key may be one whose
            // database was reset or restored; re-pairing on a guess could
            // strand sessions that are only waiting for it to come back.
            Standing::Unknown => bail!(
                "the collector at {public_url} does not know host {} (its database may have been reset or restored). \
                 If this host should be paired anew, remove {} and {} and run `hennery host join` again",
                paired.host_id,
                data_dir.join(KEY_FILE).display(),
                data_dir.join(CONFIG_FILE).display()
            ),
        }
    }
    create_private_dir(data_dir)?;
    let key = HostKey::generate();
    // Written before enrolling, so a directory that cannot hold the key
    // never costs a code. Once enrolled, the same file is renamed into
    // `host.key` (see `finish_pairing`) rather than the seed being written
    // out a second time: exactly one copy of the private key ever touches
    // disk.
    let pending = data_dir.join(format!("{KEY_FILE}.pending"));
    key.save(&pending)?;
    let host_id = match enroll(&base, code, &key, name).await {
        Ok(host_id) => host_id,
        Err(err) => {
            let _ = std::fs::remove_file(&pending);
            return Err(err);
        }
    };
    // The code is spent from here on: `join` is not retried automatically,
    // so every failure path below must still remove the pending file —
    // otherwise a repeat run would trip over a leftover `host.key.pending`
    // next to a `host.key` (or no `host.key` at all) it does not expect.
    if let Err(err) = finish_pairing(data_dir, &pending, &collector_url, &host_id) {
        let _ = std::fs::remove_file(&pending);
        return Err(err);
    }
    Ok(Joined::Paired { host_id })
}

/// Once enrollment has spent the code: move the pending key into `host.key`,
/// set aside any outbox left by an earlier identity, and write `host.toml`.
fn finish_pairing(data_dir: &Path, pending: &Path, collector_url: &str, host_id: &str) -> Result<()> {
    let key_path = data_dir.join(KEY_FILE);
    std::fs::rename(pending, &key_path)
        .with_context(|| format!("rename {} to {}", pending.display(), key_path.display()))?;
    fsync_parent(&key_path)?;
    // Frames an earlier, unpaired host left behind are not this identity's.
    orphan_outbox(data_dir, "unpaired")?;
    write_config(data_dir, collector_url, host_id)
}

async fn enroll(base: &Url, code: &str, key: &HostKey, name: &str) -> Result<String> {
    let url = base.join("/api/hosts/enroll").expect("an absolute path joins");
    let response = reqwest::Client::builder()
        .timeout(ENROLL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        // The code and the public key are the only secrets this host has
        // left to spend; `parse_public_url` already limited plain http to
        // loopback, but reqwest still honours `HTTP_PROXY`/`ALL_PROXY` by
        // default, which would hand both of them to whatever the
        // environment names — defeating that check for anyone who can set
        // it. No proxy, ever, for this request. `wss://` support must
        // reintroduce proxy support deliberately, if at all.
        .no_proxy()
        .build()?
        .post(url.clone())
        .json(&EnrollRequest {
            code: code.into(),
            public_key: key.public_key_hex(),
            name: name.into(),
            host_version: env!("CARGO_PKG_VERSION").into(),
            platform: platform(),
        })
        .send()
        .await
        .with_context(|| format!("reach {url}"))?;
    let status = response.status();
    if status.as_u16() == 201 {
        let host_id = response.json::<EnrollResponse>().await?.host_id;
        if !is_valid_host_id(&host_id) {
            // Trusted from here on: stored, shown, and (from Task 7) used as
            // an outbox path fragment. A malformed id from the collector
            // must never reach any of that.
            bail!("the collector returned a malformed host id");
        }
        return Ok(host_id);
    }
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let message = match response.json::<ApiError>().await {
        // Escaped: this text reaches a terminal (CLI stdout/stderr) verbatim,
        // and it comes from whatever answered at `base` — not necessarily
        // the collector we expect.
        Ok(e) => e.message.escape_debug().to_string(),
        Err(_) => format!("the collector answered {status}"),
    };
    match (status.as_u16(), retry_after) {
        (429, Some(secs)) => bail!("{message} (retry in {secs} s)"),
        _ => bail!("pairing failed: {message}"),
    }
}

/// `host-` followed by 16 lowercase hex characters (kernel spec §4.1): the
/// only shape a host id ever takes.
fn is_valid_host_id(id: &str) -> bool {
    id.strip_prefix("host-")
        .is_some_and(|rest| rest.len() == 16 && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_websocket_is_derived_from_the_public_url() {
        assert_eq!(
            collector_ws_url("https://c.example").unwrap(),
            "wss://c.example/api/hosts/ws"
        );
        assert_eq!(
            collector_ws_url("https://c.example:8443/").unwrap(),
            "wss://c.example:8443/api/hosts/ws"
        );
        assert_eq!(
            collector_ws_url("http://127.0.0.1:7117").unwrap(),
            "ws://127.0.0.1:7117/api/hosts/ws"
        );
        assert_eq!(
            collector_ws_url("http://localhost:7117").unwrap(),
            "ws://localhost:7117/api/hosts/ws"
        );
        assert_eq!(
            collector_ws_url("http://[::1]:7117").unwrap(),
            "ws://[::1]:7117/api/hosts/ws"
        );
    }

    #[test]
    fn an_outbox_moved_aside_keeps_its_files_together_under_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(orphan_outbox(dir.path(), "host-1").unwrap(), None);
        for round in ["first", "second"] {
            for suffix in ["", "-wal", "-shm"] {
                std::fs::write(dir.path().join(format!("outbox.db{suffix}")), round).unwrap();
            }
            orphan_outbox(dir.path(), "host-1").unwrap().unwrap();
        }
        for name in [
            "outbox.db.orphaned-host-1",
            "outbox.db.orphaned-host-1-wal",
            "outbox.db.orphaned-host-1-shm",
            "outbox.db.orphaned-host-1.1",
            "outbox.db.orphaned-host-1.1-wal",
        ] {
            assert!(dir.path().join(name).exists(), "{name}");
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("outbox.db.orphaned-host-1")).unwrap(),
            "first"
        );
        assert!(!dir.path().join("outbox.db").exists());
    }

    #[test]
    fn a_host_id_is_valid_only_as_host_plus_16_lowercase_hex_chars() {
        for id in ["host-0123456789abcdef", "host-aaaaaaaaaaaaaaaa"] {
            assert!(is_valid_host_id(id), "{id}");
        }
        for id in [
            "host-0123456789ABCDEF",  // uppercase hex is another key entirely
            "host-0123456789abcde",   // 15 chars
            "host-0123456789abcdef0", // 17 chars
            "host_0123456789abcdef",  // wrong separator
            "host-01234567-9abcdef",  // not hex throughout
            "../../etc/passwd",
            "",
        ] {
            assert!(!is_valid_host_id(id), "{id}");
        }
    }

    #[test]
    fn any_two_loopback_urls_name_the_same_collector() {
        assert!(same_collector(
            "ws://127.0.0.1:7117/api/hosts/ws",
            "ws://localhost:7200/api/hosts/ws"
        ));
        assert!(same_collector(
            "ws://[::1]:7117/api/hosts/ws",
            "ws://127.0.0.2:7117/api/hosts/ws"
        ));
        assert!(same_collector(
            "wss://c.example/api/hosts/ws",
            "wss://c.example/api/hosts/ws"
        ));
        assert!(!same_collector(
            "wss://c.example/api/hosts/ws",
            "wss://d.example/api/hosts/ws"
        ));
        assert!(!same_collector(
            "ws://127.0.0.1:7117/api/hosts/ws",
            "wss://c.example/api/hosts/ws"
        ));
    }

    #[test]
    fn plain_http_off_loopback_paths_and_other_schemes_are_refused() {
        for (url, why) in [
            ("http://c.example", "only allowed to a loopback"),
            ("http://192.168.1.2:7117", "only allowed to a loopback"),
            ("ftp://c.example", "unsupported scheme"),
            ("https://c.example/hennery", "without a path"),
            ("c.example", "not a URL"),
        ] {
            let err = collector_ws_url(url).unwrap_err().to_string();
            assert!(err.contains(why), "{url}: {err}");
        }
    }
}
