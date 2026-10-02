//! The service commands against a fake service manager, in temporary
//! directories: no test here runs `launchctl`, `systemctl` or `loginctl`,
//! or touches a real home directory.

use super::*;
use std::cell::RefCell;

/// How `Fake` answers a command line it was asked `n` times before.
type Answer = Box<dyn Fn(&str, usize) -> Ran>;

/// Records every command and answers it with `answer`.
struct Fake {
    calls: RefCell<Vec<String>>,
    answer: Answer,
}

impl Fake {
    /// `answer(command line, how many times it was asked before)`.
    fn new(answer: impl Fn(&str, usize) -> Ran + 'static) -> Self {
        Self {
            calls: RefCell::default(),
            answer: Box::new(answer),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl Manager for Fake {
    fn run(&self, program: &str, args: &[&str]) -> Result<Ran> {
        let line = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        let before = self.calls.borrow().iter().filter(|c| **c == line).count();
        self.calls.borrow_mut().push(line.clone());
        Ok((self.answer)(&line, before))
    }
}

fn ok(stdout: &str) -> Ran {
    Ran {
        ok: true,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

fn failed(stderr: &str) -> Ran {
    Ran {
        ok: false,
        stdout: String::new(),
        stderr: stderr.into(),
    }
}

/// A machine in `dir`: its home, its `/` (for `/run` and `/proc`), this
/// test binary as the one to install, and `/bin/sh` as the login shell.
fn machine<'a>(dir: &Path, platform: Platform, manager: &'a Fake) -> Context<'a> {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    Context {
        platform,
        env: BTreeMap::from([("HOME".to_string(), home.display().to_string())]),
        home,
        uid: 501,
        user: "hennery-test".into(),
        shell: PathBuf::from("/bin/sh"),
        exe: std::env::current_exe().unwrap(),
        root,
        manager,
    }
}

/// A Linux machine with systemd running.
fn linux<'a>(dir: &Path, manager: &'a Fake) -> Context<'a> {
    let cx = machine(dir, Platform::Linux, manager);
    std::fs::create_dir_all(cx.root.join("run/systemd/system")).unwrap();
    cx
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// A host data directory with a pairing in it.
fn paired(dir: &Path) -> PathBuf {
    let data = dir.join("host-data");
    hennery_host::identity::Paired {
        collector_url: "http://127.0.0.1:7117".into(),
        host_id: "host-test".into(),
        key: hennery_host::identity::HostKey::generate(),
        workspace_roots: Vec::new(),
    }
    .save(&data)
    .unwrap();
    data
}

/// macOS (§6.2): the plist goes to `~/Library/LaunchAgents`, runs `up` on
/// the default data directory, has the captured PATH, and is loaded with
/// `launchctl bootstrap gui/$UID`; the log directory is private.
#[test]
fn installing_on_macos_writes_and_bootstraps_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|line, _| {
        if line.contains(" print ") {
            failed("not found")
        } else {
            ok("")
        }
    });
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    let mut out = Vec::new();
    install(&cx, Role::Up, None, None, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    let plist = cx.home.join("Library/LaunchAgents/dev.hennery.up.plist");
    assert_eq!(
        fake.calls(),
        [
            "launchctl print gui/501/dev.hennery.up".to_string(),
            format!("launchctl bootstrap gui/501 {}", plist.display()),
        ]
    );
    let text = std::fs::read_to_string(&plist).unwrap();
    let data = cx.home.join("Library/Application Support/hennery");
    let argv = unit::command_line(Role::Up, &cx.exe, &data).unwrap();
    let args: String = argv.iter().map(|a| format!("\t\t<string>{a}</string>\n")).collect();
    assert!(text.contains(&format!("<array>\n{args}\t</array>")), "{text}");
    let (_, path) = text.split_once("<key>PATH</key>\n\t\t<string>").unwrap();
    let (path, _) = path.split_once("</string>").unwrap();
    for dir in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        assert!(path.split(':').any(|e| e == dir), "{dir} not in {path}");
    }
    assert!(text.contains("<string>Aqua</string>"), "{text}");
    assert_eq!(unit::plist_shell(&text).as_deref(), Some("/bin/sh"), "{text}");
    assert!(out.contains("from the login shell: /bin/sh"), "{out}");
    let logs = cx.home.join("Library/Logs/hennery");
    assert_eq!(mode(&logs), 0o700);
    assert_eq!(mode(&logs.join("up.log")), 0o600);
    assert_eq!(mode(&plist), 0o644);
    assert!(out.contains(&plist.display().to_string()), "{out}");
    assert!(out.contains("PATH (in the plist): "), "{out}");
    assert!(
        out.contains(&format!("hennery admin --data-dir {} setup-url", data.display())),
        "{out}"
    );
    assert!(!out.contains("restarted"), "{out}");
}

/// Reinstalling a loaded agent boots the old one out and waits until
/// launchd has let it go before bootstrapping the new one.
#[test]
fn reinstalling_on_macos_waits_for_the_bootout() {
    let dir = tempfile::tempdir().unwrap();
    // Loaded at first, and for one more look after the bootout.
    let fake = Fake::new(|line, before| {
        if line.contains(" print ") && before < 2 {
            ok("state = running")
        } else if line.contains(" print ") {
            failed("not found")
        } else {
            ok("")
        }
    });
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    let mut out = Vec::new();
    install(&cx, Role::Up, None, None, &mut out).unwrap();
    let calls = fake.calls();
    let bootout = calls.iter().position(|c| c.contains(" bootout ")).unwrap();
    let bootstrap = calls.iter().position(|c| c.contains(" bootstrap ")).unwrap();
    assert_eq!(calls[bootout], "launchctl bootout gui/501/dev.hennery.up");
    assert_eq!(calls[bootout + 1..bootstrap].len(), 2, "{calls:?}");
    assert!(String::from_utf8(out).unwrap().contains("restarted"));
}

/// No GUI session (over SSH only): the error says what a launchd user agent
/// needs.
#[test]
fn a_failed_bootstrap_explains_the_gui_session() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|line, _| {
        if line.contains(" bootstrap ") {
            failed("Bootstrap failed: 125: Domain does not support specified action")
        } else {
            failed("not found")
        }
    });
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    let err = install(&cx, Role::Up, None, None, &mut Vec::new()).unwrap_err();
    let err = format!("{err:#}");
    assert!(err.contains("GUI login session") && err.contains("125"), "{err}");
}

/// Linux (§6.3): the unit and its environment file are written, the unit
/// enabled and (re)started, and linger is checked but only advised.
#[test]
fn installing_on_linux_writes_enables_and_starts_the_unit() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|line, _| {
        if line.starts_with("loginctl ") {
            ok("Linger=no\n")
        } else if line.contains("is-active") {
            failed("")
        } else {
            ok("")
        }
    });
    let mut cx = linux(dir.path(), &fake);
    let config = dir.path().join("xdg-config");
    cx.env.insert("XDG_CONFIG_HOME".into(), config.display().to_string());
    let data = paired(dir.path());
    let mut out = Vec::new();
    install(&cx, Role::Host, Some(&data), None, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(
        fake.calls(),
        [
            "systemctl --user is-active --quiet hennery-host.service",
            "systemctl --user daemon-reload",
            "systemctl --user enable hennery-host.service",
            "systemctl --user reset-failed hennery-host.service",
            "systemctl --user restart hennery-host.service",
            "loginctl show-user 501 -p Linger",
        ]
    );
    let unit_file = config.join("systemd/user/hennery-host.service");
    let text = std::fs::read_to_string(&unit_file).unwrap();
    let exec = format!(
        "ExecStart=\"{}\" \"host\" \"run\" \"--data-dir\" \"{}\"\n",
        cx.exe.display(),
        data.display()
    );
    assert!(text.contains(&exec), "{text}");
    let env_file = config.join("hennery/service.env");
    assert!(
        text.contains(&format!("EnvironmentFile={}\n", env_file.display())),
        "{text}"
    );
    assert!(text.contains("KillMode=mixed"), "{text}");
    assert_eq!(mode(&env_file), 0o600);
    let env = std::fs::read_to_string(&env_file).unwrap();
    let line = env.lines().last().unwrap();
    assert!(line.starts_with("PATH=\"/") && line.ends_with('"'), "{env}");
    assert!(line.split([':', '"']).any(|e| e == "/usr/bin"), "{env}");
    assert!(!env.contains("CLAUDE"), "{env}");
    assert_eq!(unit::env_file_shell(&env).as_deref(), Some("/bin/sh"), "{env}");
    assert!(out.contains("loginctl enable-linger hennery-test"), "{out}");
    assert!(out.contains(&format!("PATH (in {})", env_file.display())), "{out}");
}

/// `--shell` is the shell the PATH is captured from, and the one recorded;
/// one that is not an absolute path is refused before it is run.
#[test]
fn the_shell_given_is_recorded_and_must_be_absolute() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|line, _| {
        if line.contains(" print ") {
            failed("not found")
        } else {
            ok("")
        }
    });
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    let shell = crate::service::path::marker_shell(dir.path());
    let mut out = Vec::new();
    install(&cx, Role::Up, None, Some(&shell), &mut out).unwrap();
    let plist = std::fs::read_to_string(cx.service_file(Role::Up)).unwrap();
    assert_eq!(unit::plist_shell(&plist).as_deref(), shell.to_str(), "{plist}");
    let marker = dir.path().join("marker-bin");
    let path = unit::plist_path(&plist).unwrap();
    assert!(path.split(':').any(|e| Path::new(e) == marker), "{plist}");
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains(&format!("from the login shell: {}", shell.display())),
        "{out}"
    );

    // The same script, named relative to this process's directory: it would
    // run if it were tried.
    let ran = dir.path().join("marker-ran");
    std::fs::remove_file(&ran).unwrap();
    let cwd = std::env::current_dir().unwrap();
    let up = "../".repeat(cwd.components().count() - 1);
    let relative = Path::new(&up).join(shell.strip_prefix("/").unwrap());
    assert!(relative.is_relative() && relative.is_file(), "{}", relative.display());
    let before = fake.calls().len();
    let err = install(&cx, Role::Up, None, Some(&relative), &mut Vec::new()).unwrap_err();
    assert!(format!("{err:#}").contains("--shell"), "{err:#}");
    assert!(!ran.exists(), "the relative shell was run");
    assert_eq!(fake.calls().len(), before, "nothing runs for a relative shell");
}

/// Linger on: nothing to advise.
#[test]
fn linger_on_is_not_mentioned() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|line, _| {
        if line.starts_with("loginctl ") {
            ok("Linger=yes\n")
        } else {
            ok("")
        }
    });
    let cx = linux(dir.path(), &fake);
    let mut out = Vec::new();
    install(&cx, Role::Collector, None, None, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(!out.contains("linger"), "{out}");
    assert!(out.contains("restarted"), "{out}");
    let unit_file = cx.home.join(".config/systemd/user/hennery-collector.service");
    let text = std::fs::read_to_string(unit_file).unwrap();
    let data = cx.home.join(".local/share/hennery");
    let exec = format!("\"collector\" \"--data-dir\" \"{}\"\n", data.display());
    assert!(text.contains(&exec), "{text}");
}

/// A host data directory with no pairing would crash-loop: refused before
/// anything is written or run.
#[test]
fn a_host_without_a_pairing_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|_, _| ok(""));
    let cx = linux(dir.path(), &fake);
    let empty = dir.path().join("empty");
    let err = install(&cx, Role::Host, Some(&empty), None, &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("hennery host join"), "{err}");
    assert!(fake.calls().is_empty());
    assert!(!cx.home.join(".config").exists());

    // A pairing left half-done is refused too, and left as it is: putting
    // it in place is the host's, under its lock (decision 8).
    let data = paired(dir.path());
    let pending = data.join("host.toml.pending");
    std::fs::write(&pending, "staged").unwrap();
    let err = install(&cx, Role::Host, Some(&data), None, &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("was interrupted"), "{err}");
    assert_eq!(std::fs::read_to_string(&pending).unwrap(), "staged");
    assert!(fake.calls().is_empty());
}

/// Distribution spec §7 check 10: one role per machine.
#[test]
fn a_second_role_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|_, _| ok(""));
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    let up = cx.service_file(Role::Up);
    std::fs::create_dir_all(up.parent().unwrap()).unwrap();
    std::fs::write(&up, "").unwrap();
    let err = install(&cx, Role::Collector, None, None, &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("uninstall --role up"), "{err}");
    assert!(fake.calls().is_empty());
}

/// No systemd: on WSL the error says how to turn it on; elsewhere that
/// there is none.
#[test]
fn without_systemd_install_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|_, _| ok(""));
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let err = install(&cx, Role::Up, None, None, &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("systemd is not running"), "{err}");
    std::fs::create_dir_all(cx.root.join("proc/sys/kernel")).unwrap();
    std::fs::write(
        cx.root.join("proc/sys/kernel/osrelease"),
        "5.15.153.1-microsoft-standard-WSL2\n",
    )
    .unwrap();
    let err = install(&cx, Role::Up, None, None, &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("/etc/wsl.conf"), "{err}");
    assert!(fake.calls().is_empty());
}

/// A login shell that fails is an error naming `--shell`, not a silent
/// fallback to some other PATH (D-3).
#[test]
fn a_failing_login_shell_fails_the_install() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|_, _| ok(""));
    let cx = linux(dir.path(), &fake);
    let err = install(&cx, Role::Up, None, Some(Path::new("/usr/bin/false")), &mut Vec::new()).unwrap_err();
    assert!(format!("{err:#}").contains("--shell"), "{err:#}");
    assert!(fake.calls().is_empty());
    assert!(!cx.service_file(Role::Up).exists());
}

/// Uninstall stops and removes the unit and its environment file, and
/// keeps the data directory.
#[test]
fn uninstalling_on_linux_removes_the_unit_and_keeps_the_data() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|_, _| ok("Linger=yes"));
    let cx = linux(dir.path(), &fake);
    install(&cx, Role::Up, None, None, &mut Vec::new()).unwrap();
    std::fs::create_dir_all(cx.default_data_dir()).unwrap();
    fake.calls.borrow_mut().clear();
    let mut out = Vec::new();
    uninstall(&cx, None, &mut out).unwrap();
    assert_eq!(
        fake.calls(),
        [
            "systemctl --user disable --now hennery.service",
            "systemctl --user daemon-reload"
        ]
    );
    assert!(!cx.service_file(Role::Up).exists());
    assert!(!cx.env_file().exists());
    assert!(cx.default_data_dir().exists());
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("data directory is kept"), "{out}");
    let mut again = Vec::new();
    uninstall(&cx, None, &mut again).unwrap();
    assert!(
        String::from_utf8(again)
            .unwrap()
            .contains("no hennery service is installed")
    );
}

#[test]
fn uninstalling_on_macos_boots_out_and_removes_the_plist() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|line, before| {
        if line.contains(" print ") && before == 0 {
            ok("state = running")
        } else if line.contains(" print ") {
            failed("not found")
        } else {
            ok("")
        }
    });
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    let plist = cx.service_file(Role::Host);
    std::fs::create_dir_all(plist.parent().unwrap()).unwrap();
    std::fs::write(
        &plist,
        unit::plist(Role::Host, &["/x".into()], "/usr/bin", "/bin/sh", "/tmp/l"),
    )
    .unwrap();
    let mut out = Vec::new();
    uninstall(&cx, Some(Role::Host), &mut out).unwrap();
    assert!(
        fake.calls()
            .contains(&"launchctl bootout gui/501/dev.hennery.host".to_string())
    );
    assert!(!plist.exists());
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("its log is kept"), "{out}");
}

/// `service.env` is every role's: with two roles installed (which `status`
/// warns about), removing one keeps it for the other.
#[test]
fn uninstalling_one_of_two_roles_keeps_the_env_file() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|_, _| ok("Linger=yes"));
    let cx = linux(dir.path(), &fake);
    install(&cx, Role::Collector, None, None, &mut Vec::new()).unwrap();
    let up = cx.service_file(Role::Up);
    std::fs::write(&up, "").unwrap();
    uninstall(&cx, Some(Role::Up), &mut Vec::new()).unwrap();
    assert!(cx.env_file().exists());
    uninstall(&cx, Some(Role::Collector), &mut Vec::new()).unwrap();
    assert!(!cx.env_file().exists());
}

/// Status names the installed role, what it runs, what launchd says, and
/// `up`'s children; a child given up on makes it exit 1.
#[test]
fn status_reports_the_service_and_ups_children() {
    let dir = tempfile::tempdir().unwrap();
    // This test's own process stands in for `up`, so the report is live.
    let pid = std::process::id();
    let fake = Fake::new(move |line, _| {
        if line.contains(" print ") {
            ok(&format!(
                "gui/501/dev.hennery.up = {{\n\tstate = running\n\tpid = {pid}\n}}"
            ))
        } else {
            ok("")
        }
    });
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let argv = unit::command_line(Role::Up, &cx.exe, &data).unwrap();
    let plist = cx.service_file(Role::Up);
    std::fs::create_dir_all(plist.parent().unwrap()).unwrap();
    std::fs::write(&plist, unit::plist(Role::Up, &argv, "/usr/bin", "/bin/sh", "/tmp/l")).unwrap();
    let report = |state| supervisor::ChildReport {
        state,
        crashes_in_window: 10,
        restarts: 9,
        last_exit: Some("exit status: 1".into()),
    };
    let mut state = supervisor::State {
        pid,
        updated_at: 0,
        collector: report(supervisor::ChildState::Running),
        host: report(supervisor::ChildState::Running),
    };
    supervisor::write_state(&data, &state).unwrap();
    let mut out = Vec::new();
    assert_eq!(status(&cx, None, &mut out).unwrap(), ExitCode::SUCCESS);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains(&format!("launchd: running, pid {pid}")), "{text}");
    assert!(text.contains(&format!("runs: {}", argv.join(" "))), "{text}");

    state.host = report(supervisor::ChildState::GaveUp);
    supervisor::write_state(&data, &state).unwrap();
    let mut out = Vec::new();
    assert_eq!(status(&cx, None, &mut out).unwrap(), ExitCode::FAILURE);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("host: given up on after 10 crashes"), "{text}");

    // Written by a process that is gone: stale, and judged by nothing.
    let mut gone = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    state.pid = gone.id();
    supervisor::write_state(&data, &state).unwrap();
    let mut out = Vec::new();
    assert_eq!(status(&cx, None, &mut out).unwrap(), ExitCode::SUCCESS);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("the last report is stale"), "{text}");
    assert!(!text.contains("given up on"), "{text}");
}

/// Either condition alone makes the report stale: a live pid that is not
/// the one launchd names, or a dead pid when launchd names none. And a
/// report that cannot be read is said, and fails `status`.
#[test]
fn a_report_is_stale_by_either_condition_and_an_unreadable_one_fails() {
    let dir = tempfile::tempdir().unwrap();
    let launchd_pid = std::rc::Rc::new(std::cell::Cell::new(None::<u32>));
    let named = launchd_pid.clone();
    let fake = Fake::new(move |line, _| {
        if line.contains(" print ") {
            match named.get() {
                Some(pid) => ok(&format!("state = running\npid = {pid}\n")),
                None => ok("state = running\n"),
            }
        } else {
            ok("")
        }
    });
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let argv = unit::command_line(Role::Up, &cx.exe, &data).unwrap();
    let plist = cx.service_file(Role::Up);
    std::fs::create_dir_all(plist.parent().unwrap()).unwrap();
    std::fs::write(&plist, unit::plist(Role::Up, &argv, "/usr/bin", "/bin/sh", "/tmp/l")).unwrap();
    let gave_up = supervisor::ChildReport {
        state: supervisor::ChildState::GaveUp,
        crashes_in_window: 10,
        restarts: 9,
        last_exit: None,
    };
    let report = |pid| supervisor::State {
        pid,
        updated_at: 0,
        collector: gave_up.clone(),
        host: gave_up.clone(),
    };
    let stale = |cx: &Context| {
        let mut out = Vec::new();
        let code = status(cx, None, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        (code, text.contains("the last report is stale"), text)
    };

    // Alive, but not the process launchd runs.
    supervisor::write_state(&data, &report(std::process::id())).unwrap();
    launchd_pid.set(Some(1));
    let (code, is_stale, text) = stale(&cx);
    assert!(is_stale, "{text}");
    assert_eq!(code, ExitCode::SUCCESS, "{text}");

    // launchd names no pid, and the report's is gone.
    let mut gone = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    supervisor::write_state(&data, &report(gone.id())).unwrap();
    launchd_pid.set(None);
    let (_, is_stale, text) = stale(&cx);
    assert!(is_stale, "{text}");

    // Neither: the report counts, and its give-up fails `status`.
    supervisor::write_state(&data, &report(std::process::id())).unwrap();
    let (code, is_stale, text) = stale(&cx);
    assert!(!is_stale, "{text}");
    assert_eq!(code, ExitCode::FAILURE, "{text}");

    std::fs::write(data.join(supervisor::STATE_FILE), "{not json").unwrap();
    let mut out = Vec::new();
    assert_eq!(status(&cx, None, &mut out).unwrap(), ExitCode::FAILURE);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("the report cannot be read"), "{text}");
}

/// Status: nothing installed, a missing binary, a unit that is not active.
#[test]
fn status_fails_when_the_service_cannot_run() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|line, _| {
        if line.contains("is-active") {
            ok("failed\n")
        } else if line.contains("is-enabled") {
            ok("enabled\n")
        } else {
            ok("Linger=yes")
        }
    });
    let cx = linux(dir.path(), &fake);
    let mut out = Vec::new();
    assert_eq!(status(&cx, None, &mut out).unwrap(), ExitCode::FAILURE);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("no hennery service is installed")
    );
    assert!(fake.calls().is_empty());

    let argv = unit::command_line(
        Role::Collector,
        Path::new("/nonexistent/hennery"),
        &cx.default_data_dir(),
    )
    .unwrap();
    let file = cx.service_file(Role::Collector);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, unit::systemd_unit(Role::Collector, &argv, "/tmp/env").unwrap()).unwrap();
    let mut out = Vec::new();
    assert_eq!(status(&cx, None, &mut out).unwrap(), ExitCode::FAILURE);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("its binary is missing"), "{text}");
    assert!(text.contains("systemd: failed, enabled"), "{text}");
}
