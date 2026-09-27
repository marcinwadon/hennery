use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn help_lists_the_skeleton_commands() {
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    for cmd in ["collector", "host", "up"] {
        assert!(text.contains(cmd), "missing {cmd} in help:\n{text}");
    }
}

#[test]
fn a_malformed_agent_flag_is_rejected() {
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args([
            "host",
            "run",
            "--collector",
            "ws://x",
            "--data-dir",
            "/tmp/x",
            "--dev-token",
            "t",
            "--agent",
            "noequals",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("name=command"));
}

/// A terminal's Ctrl-C delivers SIGINT to every process in the foreground
/// process group at once. `run_up` pulls its two children out of that group
/// (`process_group(0)` on both spawns) so only the supervisor is signalled
/// and can still forward an ordered shutdown (host, then collector) instead
/// of racing each child's own signal handler.
///
/// This spawns `up` as the leader of a brand-new process group (standing in
/// for a shell job) and sends SIGINT to that whole group, the same delivery
/// a terminal would do. It asserts `up` still exits cleanly and leaves no
/// child process behind. It does not assert the shutdown *order* directly
/// (host before collector): both children shut themselves down promptly on
/// SIGINT regardless of which fix is in place, so "no leftover processes" is
/// the observable signal here, not a strict ordering.
#[test]
fn sigint_to_ups_process_group_still_shuts_down_cleanly() {
    let free_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };
    let listen = format!("127.0.0.1:{free_port}");

    let dir = std::env::temp_dir().join(format!("hennery-cli-pgtest-{}-{free_port}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
    command
        .args(["up", "--listen", &listen])
        .arg("--data-dir")
        .arg(&dir)
        .args(["--dev-token", "t"]);
    // SAFETY: setpgid(0, 0) in the child, right after fork and before exec,
    // just makes it (and so `up`) the leader of a brand-new process group —
    // async-signal-safe and exactly what a shell does for a foreground job.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut up = command.spawn().unwrap();
    let up_pgid = up.id() as i32;

    // Wait for the collector to be listening.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect(&listen).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "collector never started listening");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Give the host child a moment to spawn too (it's launched right after
    // the collector, independent of the collector's readiness).
    std::thread::sleep(Duration::from_millis(300));
    let children = children_of(up.id() as i32);
    assert!(
        children.len() >= 2,
        "expected up to have spawned a collector and a host child, found {children:?}"
    );

    // Simulate a terminal delivering SIGINT to the whole foreground group.
    let rc = unsafe { libc::kill(-up_pgid, libc::SIGINT) };
    assert_eq!(rc, 0, "kill(-pgid, SIGINT) failed: {}", std::io::Error::last_os_error());

    let status = wait_with_timeout(&mut up, Duration::from_secs(15)).expect("up did not exit after SIGINT");
    assert!(status.success(), "up exited with {status:?}");

    // Give any lingering child a moment to finish reaping before we check.
    std::thread::sleep(Duration::from_millis(300));
    for pid in children {
        assert!(!pid_alive(pid), "child pid {pid} is still alive after up exited");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

fn children_of(ppid: i32) -> Vec<i32> {
    let out = Command::new("pgrep").args(["-P", &ppid.to_string()]).output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.trim().parse::<i32>().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn pid_alive(pid: i32) -> bool {
    // kill(pid, 0) checks for existence/permission without sending a signal.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
