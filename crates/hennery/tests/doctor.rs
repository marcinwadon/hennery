//! `hennery doctor` as a process, in a scratch home: HOME and the XDG
//! directories in a temporary directory, and PATH starting with stand-ins
//! for `launchctl`, `systemctl` and `loginctl` that only record that they
//! ran. Doctor's checks themselves are tested in `src/doctor/tests.rs`.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

/// Where the managed runtime would fetch from: a loopback port nothing
/// listens on (as in `cli.rs`). Doctor fetches nothing; this keeps it so.
const OFFLINE: &str = "http://127.0.0.1:1/";

/// Every file under `dir`, with its size and mode.
fn tree(dir: &std::path::Path) -> Vec<(std::path::PathBuf, u64, u32)> {
    let mut seen = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(path) = todo.pop() {
        let meta = std::fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            todo.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        }
        seen.push((path, meta.len(), meta.permissions().mode()));
    }
    seen.sort();
    seen
}

/// On a paired host's directory with no service installed: a report that
/// names the directory and every check, no service manager asked, and the
/// directory left exactly as it was.
#[test]
fn doctor_reports_on_a_host_directory_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let stubs = dir.path().join("stubs");
    std::fs::create_dir_all(&stubs).unwrap();
    for name in ["launchctl", "systemctl", "loginctl"] {
        let stub = stubs.join(name);
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\necho {name} \"$@\" >> \"{}\"\nexit 1\n",
                dir.path().join("ran").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let host = dir.path().join("host");
    hennery_host::identity::Paired {
        collector_url: "ws://127.0.0.1:1/api/hosts/ws".into(),
        host_id: "host-doctor".into(),
        key: hennery_host::identity::HostKey::generate(),
        workspace_roots: Vec::new(),
    }
    .save(&host)
    .unwrap();
    let before = tree(&host);
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["doctor", "--data-dir"])
        .arg(&host)
        .env("HOME", dir.path().join("home"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("PATH", format!("{}:/usr/bin:/bin", stubs.display()))
        .env("HENNERY_NPM_REGISTRY", OFFLINE)
        .env("HENNERY_NODE_MIRROR", OFFLINE)
        .env_remove("HENNERY_DATA_DIR")
        .env_remove("HENNERY_HOST_DATA_DIR")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.starts_with(&format!("hennery doctor: {} (given with --data-dir", host.display())),
        "{stdout}{stderr}"
    );
    assert!(stdout.contains("\n  a host's\n"), "{stdout}");
    for check in [
        " 1 binary and adapter set: ",
        " 9 disk: ",
        "10 service: no service is installed",
        "14 host data directory: no host runs on it",
    ] {
        assert!(stdout.contains(check), "{check}: {stdout}");
    }
    assert!(stdout.contains("not run here: 5 (no service installed)"), "{stdout}");
    // Warnings (no service, no adapter set) exit 0; no check fails here.
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(
        !dir.path().join("ran").exists(),
        "{:?}",
        std::fs::read_to_string(dir.path().join("ran"))
    );
    assert_eq!(tree(&host), before);
}
