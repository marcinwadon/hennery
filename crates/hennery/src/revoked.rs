//! A revoked host's record (distribution spec §5.2): `host run` writes it
//! when the collector revokes it, and `service status` and doctor read it.
//! Under launchd the revoked host exits 0, so that `KeepAlive` does not
//! start it again: its exit code no longer says it was revoked, and this
//! file does. It counts only while its host id is the pairing's: a re-pair
//! stores another id, and so retires it without anyone removing it.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// In the host's data directory.
pub const FILE: &str = "revoked";

/// What was revoked, by which collector, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revoked {
    pub host_id: String,
    pub collector_url: String,
    /// Unix seconds.
    pub at: i64,
}

impl Revoked {
    /// The collector's public URL, as `host join` takes it, from the
    /// WebSocket URL the host connected to (`collector_ws_url`'s inverse).
    pub fn public_url(&self) -> String {
        let url = self.collector_url.trim_end_matches("/api/hosts/ws");
        if let Some(rest) = url.strip_prefix("wss://") {
            format!("https://{rest}")
        } else if let Some(rest) = url.strip_prefix("ws://") {
            format!("http://{rest}")
        } else {
            url.to_string()
        }
    }

    /// What was revoked and what to do, `start` being how to start the
    /// service again once paired.
    pub fn said(&self, start: &str) -> (String, String) {
        (
            format!("revoked by the collector (at unix time {})", self.at),
            format!(
                "pair it again: `hennery host join {}` (it asks for a pairing code from the collector), then start it: `{start}`",
                self.public_url()
            ),
        )
    }
}

/// The exit code of a revoked `host run`: 78 (`EX_CONFIG`), which `up` and
/// systemd (`RestartPreventExitStatus=78`) keep down; but 0 when launchd
/// runs it as the host service, whose `KeepAlive` restarts any other code.
/// `up`'s child (with `--parent-fd`) keeps 78, also under a launchd `up`.
pub fn exit_code(service: Option<&str>, under_up: bool) -> u8 {
    match (service, under_up) {
        (Some("launchd"), false) => 0,
        _ => crate::supervisor::REVOKED_EXIT,
    }
}

/// Record `revoked` in `dir`, private, whole or not at all: written under a
/// temporary name made afresh (so its mode is this one), renamed into
/// place, and the temporary file removed whatever fails.
pub fn record(dir: &Path, revoked: &Revoked) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = dir.join(FILE);
    let tmp = dir.join(format!(".{FILE}.{}.tmp", std::process::id()));
    let text = toml::to_string(revoked)?;
    // One left by an earlier run of this pid: never reused as it stands.
    let _ = std::fs::remove_file(&tmp);
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(err) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("write {}", path.display()));
    }
    Ok(())
}

/// The record in `dir` that still counts: `None` when there is none, or it
/// names another host than the pairing there (a re-pair since), or there
/// is no pairing. Read only: the pairing is read, not rolled forward. An
/// error when the record cannot be read.
pub fn current(dir: &Path) -> Result<Option<Revoked>> {
    let path = dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    let revoked: Revoked = toml::from_str(&text).with_context(|| format!("read {}", path.display()))?;
    // Read only (no pairing rolled forward). A pairing being made, or one
    // half there, hides the record: doctor's check 7 reports those itself.
    let paired = hennery_host::identity::Paired::read(dir).ok().flatten();
    Ok(paired.filter(|p| p.host_id == revoked.host_id).map(|_| revoked))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_host_launchd_runs_itself_exits_0_when_revoked() {
        assert_eq!(exit_code(Some("launchd"), false), 0);
        assert_eq!(exit_code(Some("launchd"), true), 78);
        assert_eq!(exit_code(Some("systemd"), false), 78);
        assert_eq!(exit_code(Some("systemd"), true), 78);
        assert_eq!(exit_code(None, false), 78);
        assert_eq!(exit_code(None, true), 78);
        assert_eq!(exit_code(Some(""), false), 78);
    }

    fn paired(dir: &Path, host_id: &str) {
        hennery_host::identity::Paired {
            collector_url: "ws://127.0.0.1:1/api/hosts/ws".into(),
            host_id: host_id.into(),
            key: hennery_host::identity::HostKey::generate(),
            workspace_roots: Vec::new(),
        }
        .save(dir)
        .unwrap();
    }

    #[test]
    fn a_record_counts_only_for_the_pairing_it_names() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("host");
        assert_eq!(current(&host).unwrap(), None);
        paired(&host, "host-1");
        let revoked = Revoked {
            host_id: "host-1".into(),
            collector_url: "https://hennery.example".into(),
            at: 1_790_000_000,
        };
        record(&host, &revoked).unwrap();
        assert_eq!(current(&host).unwrap(), Some(revoked.clone()));
        let mode = std::fs::metadata(host.join(FILE)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let left: Vec<_> = std::fs::read_dir(&host)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        // Paired again: another host id, and the record is stale.
        paired(&host, "host-2");
        assert_eq!(current(&host).unwrap(), None);
        // No pairing at all: nothing counts.
        std::fs::remove_file(host.join(hennery_host::identity::CONFIG_FILE)).unwrap();
        std::fs::remove_file(host.join(hennery_host::identity::KEY_FILE)).unwrap();
        assert_eq!(current(&host).unwrap(), None);
    }

    #[test]
    fn the_public_url_is_the_one_host_join_takes() {
        let at = |url: &str| Revoked {
            host_id: "h".into(),
            collector_url: url.into(),
            at: 0,
        };
        assert_eq!(
            at("ws://127.0.0.1:7117/api/hosts/ws").public_url(),
            "http://127.0.0.1:7117"
        );
        assert_eq!(
            at("wss://hennery.example/api/hosts/ws").public_url(),
            "https://hennery.example"
        );
        // The inverse of the URL the host connects to, as pairing makes it.
        for public in ["http://127.0.0.1:7117", "http://[::1]:7117", "https://hennery.example"] {
            let ws = hennery_host::pairing::collector_ws_url(public).unwrap();
            assert_eq!(at(&ws).public_url(), public, "{ws}");
        }
        let (why, fix) = at("ws://127.0.0.1:7117/api/hosts/ws").said("systemctl --user restart hennery-host.service");
        assert!(why.contains("revoked by the collector"), "{why}");
        assert!(
            fix.contains("hennery host join http://127.0.0.1:7117") && fix.contains("systemctl --user restart"),
            "{fix}"
        );
    }

    #[test]
    fn a_record_that_cannot_be_read_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        paired(dir.path(), "host-1");
        std::fs::write(dir.path().join(FILE), "not = [toml").unwrap();
        assert!(current(dir.path()).is_err());
    }
}
