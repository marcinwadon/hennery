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
    assert!(
        stdout.contains("not run here: 3, 4, 13 (no adapter set installed and no --agent); 5 (no service installed)"),
        "{stdout}"
    );
    // Warnings (no service, no adapter set) exit 0; no check fails here.
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(
        !dir.path().join("ran").exists(),
        "{:?}",
        std::fs::read_to_string(dir.path().join("ran"))
    );
    assert_eq!(tree(&host), before);
}

/// A Ctrl-C to doctor while an adapter it started has not answered yet
/// kills that adapter's whole group too: in a group of its own, it would
/// not see the terminal's signal. The service's `--agent` command stands in
/// for an adapter that never answers.
#[test]
fn an_interrupted_doctor_leaves_no_adapter_running() {
    let dir = tempfile::tempdir().unwrap();
    let host = dir.path().join("host");
    hennery_host::identity::Paired {
        collector_url: "ws://127.0.0.1:1/api/hosts/ws".into(),
        host_id: "host-doctor".into(),
        key: hennery_host::identity::HostKey::generate(),
        workspace_roots: Vec::new(),
    }
    .save(&host)
    .unwrap();
    let pid_file = dir.path().join("adapter.pid");
    let adapter = dir.path().join("silent-adapter");
    std::fs::write(
        &adapter,
        format!(
            "#!/bin/sh\n[ \"$1\" = --warm-up ] && exit 0\necho $$ > \"{}.tmp\"\nmv \"{}.tmp\" \"{}\"\nexec sleep 60\n",
            pid_file.display(),
            pid_file.display(),
            pid_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Its first run, which can wait on the machine's scan of a new
    // executable, is not the one doctor makes; on Linux a fork elsewhere
    // can still hold it open for writing (ETXTBSY) for a moment.
    let warm = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while let Err(err) = Command::new(&adapter).arg("--warm-up").status() {
        assert!(
            err.raw_os_error() == Some(libc::ETXTBSY) && std::time::Instant::now() < warm,
            "{err}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let home = dir.path().join("home");
    let config = dir.path().join("config");
    let argv = [
        "/usr/bin/true".to_string(),
        "host".to_string(),
        "run".to_string(),
        "--data-dir".to_string(),
        host.display().to_string(),
        "--agent".to_string(),
        format!("silent={}", adapter.display()),
    ];
    if cfg!(target_os = "macos") {
        let agents = home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        let args: String = argv.iter().map(|a| format!("\t\t<string>{a}</string>\n")).collect();
        std::fs::write(
            agents.join("dev.hennery.host.plist"),
            format!("<plist><dict>\n\t<key>ProgramArguments</key>\n\t<array>\n{args}\t</array>\n</dict></plist>\n"),
        )
        .unwrap();
    } else {
        let units = config.join("systemd/user");
        std::fs::create_dir_all(&units).unwrap();
        let exec: Vec<String> = argv.iter().map(|a| format!("\"{a}\"")).collect();
        std::fs::write(
            units.join("hennery-host.service"),
            format!("[Service]\nExecStart={}\n", exec.join(" ")),
        )
        .unwrap();
    }
    let stubs = dir.path().join("stubs");
    std::fs::create_dir_all(&stubs).unwrap();
    for name in ["launchctl", "systemctl", "loginctl"] {
        std::fs::write(stubs.join(name), "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(stubs.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // Ctrl-C, and a dropped SSH session's hangup.
    for signal in [libc::SIGINT, libc::SIGHUP] {
        let _ = std::fs::remove_file(&pid_file);
        let mut doctor = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args(["doctor", "--data-dir"])
            .arg(&host)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_DATA_HOME", dir.path().join("data"))
            .env("PATH", format!("{}:/usr/bin:/bin", stubs.display()))
            .env("HENNERY_NPM_REGISTRY", OFFLINE)
            .env("HENNERY_NODE_MIRROR", OFFLINE)
            .env_remove("HENNERY_DATA_DIR")
            .env_remove("HENNERY_HOST_DATA_DIR")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // Generous: doctor's own first run can wait on the same scan.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let adapter_pid: i32 = loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file) {
                break text.trim().parse().unwrap();
            }
            assert!(std::time::Instant::now() < deadline, "the adapter never started");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        // SAFETY: kill(2) of the doctor process this test started.
        unsafe { libc::kill(doctor.id() as i32, signal) };
        let status = loop {
            if let Some(status) = doctor.try_wait().unwrap() {
                break status;
            }
            assert!(std::time::Instant::now() < deadline, "doctor did not stop");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(status.code(), Some(130));
        // SAFETY: kill(2) with signal 0 only checks.
        while unsafe { libc::kill(adapter_pid, 0) } == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the adapter {adapter_pid} is still running"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
