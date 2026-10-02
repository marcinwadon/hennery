//! Doctor's checks on made-up machines: temporary homes, data directories
//! and roots (for `/proc` and `/etc`), a fake service manager that answers
//! only questions, and a fake runner. No test runs `launchctl`, `systemctl`
//! or `loginctl`, reaches the network, or touches a real data directory;
//! every one checks that doctor changed nothing it looked at, and that each
//! warning and failure says what to do.

use super::dirs::{Found, HOST_DIR_VAR};
use super::*;
use crate::service::unit::{self, Role};
use crate::service::{Manager, Platform};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// The service manager's verbs doctor may use: it asks, it never changes.
const READ_ONLY_VERBS: &[&str] = &["print", "is-active", "is-enabled", "show", "show-user"];

/// Answers each command line with `answer`, and fails the test on any verb
/// that is not a question.
struct Fake {
    calls: RefCell<Vec<String>>,
    answer: Box<dyn Fn(&str) -> Ran>,
    /// Every command fails to run, as without a user bus.
    unreachable: bool,
}

impl Fake {
    fn new(answer: impl Fn(&str) -> Ran + 'static) -> Self {
        Self {
            calls: RefCell::default(),
            answer: Box::new(answer),
            unreachable: false,
        }
    }

    /// Knows no service: `launchctl print` fails, `systemctl` says inactive.
    fn none() -> Self {
        Self::new(|line| {
            if line.starts_with("launchctl") {
                failed()
            } else {
                said("inactive")
            }
        })
    }
}

impl Manager for Fake {
    fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<Ran> {
        let verb = args.iter().copied().find(|a| !a.starts_with('-')).unwrap_or_default();
        assert!(
            READ_ONLY_VERBS.contains(&verb),
            "doctor ran `{program} {}`, which is not a question",
            args.join(" ")
        );
        let line = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        self.calls.borrow_mut().push(line.clone());
        if self.unreachable {
            anyhow::bail!("run {program}: no user bus");
        }
        Ok((self.answer)(&line))
    }
}

fn said(stdout: &str) -> Ran {
    Ran {
        ok: true,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

fn failed() -> Ran {
    Ran {
        ok: false,
        stdout: String::new(),
        stderr: "not found".into(),
    }
}

/// This test process's user, so the files it makes count as the user's own.
fn uid() -> u32 {
    // SAFETY: getuid(2) cannot fail.
    unsafe { libc::getuid() }
}

/// A machine in `dir`: its home, its `/` (for `/proc` and `/etc`), this test
/// binary as the one doctor runs as, `/bin/sh` as the login shell, and a
/// PATH of the base system only.
fn machine<'a>(dir: &Path, platform: Platform, manager: &'a Fake) -> Context<'a> {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    Context {
        platform,
        env: BTreeMap::from([
            ("HOME".to_string(), home.display().to_string()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ]),
        home,
        uid: uid(),
        user: "hennery-test".into(),
        shell: PathBuf::from("/bin/sh"),
        exe: std::env::current_exe().unwrap(),
        root,
        manager,
    }
}

/// A runner that runs nothing.
fn nothing(_: &Path, _: &[&str]) -> Option<Ran> {
    None
}

/// Every file and directory under `dir`: its kind, size, mode and mtime.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (bool, u64, u32, i64, i64)> {
    let mut seen = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(path) = todo.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            todo.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        }
        seen.insert(
            path,
            (meta.is_dir(), meta.len(), meta.mode(), meta.mtime(), meta.mtime_nsec()),
        );
    }
    seen
}

/// Run every check over `dirs` on `cx`, and check what holds for every
/// run: nothing under `watched` changed, and every warning and failure has
/// a fix.
fn checked(cx: &Context, dirs: Dirs, run: Runner, watched: &Path) -> Vec<Finding> {
    let before = snapshot(watched);
    let doctor = Doctor { cx, dirs, run };
    let findings = checks(&doctor);
    assert_eq!(snapshot(watched), before, "doctor changed what it looked at");
    for finding in &findings {
        if let Finding::Checked(check) = finding {
            assert!(!check.summary.is_empty(), "{check:?}");
            if check.status != Status::Ok {
                assert!(!check.fix.is_empty(), "a {:?} without a fix: {check:?}", check.status);
            }
        }
    }
    findings
}

/// Check `number`'s line.
fn line(findings: &[Finding], number: u8) -> &Check {
    findings
        .iter()
        .find_map(|f| match f {
            Finding::Checked(c) if c.number == number => Some(c),
            _ => None,
        })
        .unwrap_or_else(|| panic!("check {number} did not run: {findings:?}"))
}

/// Whether check `number` was not run.
fn not_run(findings: &[Finding], number: u8) -> bool {
    findings
        .iter()
        .any(|f| matches!(f, Finding::NotRun { number: n, .. } if *n == number))
}

/// The report as text.
fn report(dirs: &Dirs, findings: &[Finding]) -> String {
    let mut out = Vec::new();
    render(dirs, findings, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// A host data directory with a pairing in it.
fn paired(dir: &Path) -> PathBuf {
    hennery_host::identity::Paired {
        collector_url: "ws://127.0.0.1:1/api/hosts/ws".into(),
        host_id: "host-test".into(),
        key: hennery_host::identity::HostKey::generate(),
        workspace_roots: Vec::new(),
    }
    .save(dir)
    .unwrap();
    dir.to_path_buf()
}

/// `role`'s service installed on `cx`, running `exe` on `data`, with
/// `path` as its PATH.
fn install(cx: &Context, role: Role, exe: &Path, data: &Path, path: &str) {
    let argv = unit::command_line(role, exe, data).unwrap();
    let file = cx.service_file(role);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    match cx.platform {
        Platform::MacOs => std::fs::write(&file, unit::plist(role, &argv, path, "/tmp/hennery-test.log")).unwrap(),
        Platform::Linux => {
            std::fs::create_dir_all(cx.env_file().parent().unwrap()).unwrap();
            std::fs::write(cx.env_file(), unit::env_file(path)).unwrap();
            std::fs::write(&file, unit::systemd_unit(role, &argv, "/tmp/service.env").unwrap()).unwrap();
        }
    }
}

/// The directory: given, else `HENNERY_HOST_DATA_DIR`'s host, else the one
/// service's, else the default. A disagreeing host variable is said.
#[test]
fn the_directory_is_the_one_given_then_the_host_variables_then_the_services_then_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let mut cx = machine(dir.path(), Platform::Linux, &fake);
    let given = dir.path().join("given");
    assert_eq!(Dirs::discover(&cx, Some(&given)).unwrap().from, Found::Given);

    let host = paired(&dir.path().join("host-var"));
    cx.env.insert(HOST_DIR_VAR.into(), host.display().to_string());
    let dirs = Dirs::discover(&cx, None).unwrap();
    assert_eq!((dirs.from, dirs.host.as_ref()), (Found::HostVariable, Some(&host)));
    let dirs = Dirs::discover(&cx, Some(&given)).unwrap();
    assert_eq!(dirs.from, Found::Given);
    assert!(
        dirs.notes[0].contains("HENNERY_HOST_DATA_DIR names another host directory"),
        "{dirs:?}"
    );
    cx.env.remove(HOST_DIR_VAR);

    let data = dir.path().join("up-data");
    std::fs::create_dir_all(data.join("host")).unwrap();
    install(&cx, Role::Up, &cx.exe, &data, "/usr/bin:/bin");
    let dirs = Dirs::discover(&cx, None).unwrap();
    assert_eq!(dirs.from, Found::Service(Role::Up));
    assert_eq!(dirs.host, Some(data.join("host")));
    assert_eq!(dirs.collector, None);

    std::fs::remove_file(cx.service_file(Role::Up)).unwrap();
    let dirs = Dirs::discover(&cx, None).unwrap();
    assert_eq!(dirs.from, Found::Default);
    assert_eq!(dirs.root, dir.path().join("home/.local/share/hennery"));
}

/// What a directory holds is told by the names in it; an interrupted
/// pairing is left exactly as it is.
#[test]
fn what_a_directory_holds_is_told_by_its_names() {
    let dir = tempfile::tempdir().unwrap();
    let host = paired(&dir.path().join("host"));
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    assert_eq!(
        (dirs.host.as_ref(), dirs.describe().as_str()),
        (Some(&host), "a host's")
    );

    let up = dir.path().join("up");
    paired(&up.join("host"));
    std::fs::create_dir_all(up.join("collector")).unwrap();
    std::fs::write(up.join("collector/hennery.db"), "").unwrap();
    let dirs = Dirs::by_contents(up.clone(), Found::Given);
    assert_eq!(dirs.host, Some(up.join("host")));
    assert_eq!(dirs.collector, Some(up.join("collector")));

    let collector = dir.path().join("collector");
    std::fs::create_dir_all(&collector).unwrap();
    std::fs::write(collector.join("hennery.db"), "").unwrap();
    let dirs = Dirs::by_contents(collector.clone(), Found::Given);
    assert_eq!((dirs.host, dirs.collector.as_ref()), (None, Some(&collector)));

    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        Dirs::by_contents(empty, Found::Given).describe(),
        "nothing of hennery's is in it"
    );
    assert_eq!(
        Dirs::by_contents(dir.path().join("missing"), Found::Given).describe(),
        "it does not exist"
    );

    // Staged by a join that did not finish: a host's, and left staged.
    let staged = dir.path().join("staged");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("host.toml.pending"), "collector = \"x\"\n").unwrap();
    std::fs::write(staged.join("host.key.pending"), "00\n").unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let dirs = Dirs::by_contents(staged.clone(), Found::Given);
    assert_eq!(dirs.host, Some(staged.clone()));
    checked(&cx, dirs, &nothing, &staged);
    assert!(staged.join("host.toml.pending").exists());
    assert!(!staged.join("host.toml").exists());
}

/// Check 6: the nesting variables by name, never by value; and root.
#[test]
fn nesting_variables_and_root_warn_by_name_only() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let mut cx = machine(dir.path(), Platform::Linux, &fake);
    let dirs = Dirs::by_contents(dir.path().join("none"), Found::Given);
    let findings = checked(&cx, dirs.clone(), &nothing, dir.path());
    assert_eq!(line(&findings, 6).status, Status::Ok);

    cx.env.insert("CLAUDECODE".into(), "canary-7d-nesting".into());
    let findings = checked(&cx, dirs.clone(), &nothing, dir.path());
    let check = line(&findings, 6);
    assert_eq!(check.status, Status::Warn);
    assert!(check.summary.contains("CLAUDECODE"), "{check:?}");
    assert!(!report(&dirs, &findings).contains("canary-7d-nesting"));

    cx.env.remove("CLAUDECODE");
    cx.uid = 0;
    let findings = checked(&cx, dirs, &nothing, dir.path());
    assert!(line(&findings, 6).summary.contains("run as root"), "{findings:?}");
}

/// Check 11: the `hennery` PATH finds first must be this binary; one only
/// later on PATH is named; none at all warns; a relative entry is skipped.
#[test]
fn the_hennery_path_finds_first_must_be_this_one() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let mut cx = machine(dir.path(), Platform::Linux, &fake);
    let dirs = Dirs::by_contents(dir.path().join("none"), Found::Given);
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("hennery"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(other.join("hennery"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let this = dir.path().join("this");
    std::fs::create_dir_all(&this).unwrap();
    std::os::unix::fs::symlink(&cx.exe, this.join("hennery")).unwrap();

    let path = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap().into_string().unwrap();
    cx.env.insert("PATH".into(), path(&[&other, &this]));
    let findings = checked(&cx, dirs.clone(), &nothing, dir.path());
    let check = line(&findings, 11);
    assert_eq!(check.status, Status::Warn);
    assert!(check.summary.contains("another install"), "{check:?}");

    cx.env.insert("PATH".into(), path(&[&this, &other]));
    let findings = checked(&cx, dirs.clone(), &nothing, dir.path());
    let check = line(&findings, 11);
    assert_eq!(check.status, Status::Ok);
    assert!(check.summary.contains("later on PATH"), "{check:?}");

    cx.env.insert("PATH".into(), "/usr/bin:/bin".into());
    let findings = checked(&cx, dirs.clone(), &nothing, dir.path());
    assert!(line(&findings, 11).summary.contains("no hennery is on PATH"));

    // `other` as a relative entry is not searched.
    cx.env.insert("PATH".into(), format!("other:{}", this.display()));
    let findings = checked(&cx, dirs, &nothing, dir.path());
    assert_eq!(line(&findings, 11).status, Status::Ok);
}

/// The report names its directory, prints one line per check and a fix
/// under each warning and failure, groups the checks it did not run, exits
/// 1 only on a failure, and escapes control characters.
#[test]
fn the_report_fails_only_on_a_failure_and_escapes_control_characters() {
    let dirs = Dirs::by_contents(PathBuf::from("/nowhere/a\nb"), Found::Default);
    let mut warned = Verdict::default();
    warned.ok("fine");
    warned.warn("odd\u{1b}[31m", "do this");
    warned.ok("a\u{202e}b");
    let findings = vec![
        Finding::Checked(warned.check(6, "environment")),
        Finding::NotRun {
            number: 9,
            why: dirs::NO_HOST,
        },
        Finding::NotRun {
            number: 12,
            why: dirs::NO_HOST,
        },
    ];
    let text = report(&dirs, &findings);
    assert!(
        text.starts_with("hennery doctor: /nowhere/a\\nb (the default)\n"),
        "{text}"
    );
    assert!(
        text.contains("warn  6 environment: fine; odd\\u{1b}[31m; a\\u{202e}b\n        fix: do this\n"),
        "{text}"
    );
    assert!(text.contains("not run here: 9, 12 (no host data directory)"), "{text}");
    assert_eq!(exit_code(&findings), ExitCode::SUCCESS);

    let mut failed = Verdict::default();
    failed.warn("a", "fix a");
    failed.fail("b", "fix b");
    let check = failed.check(2, "platform");
    assert_eq!((check.status, check.fix.as_str()), (Status::Fail, "fix b"));
    assert_eq!(exit_code(&[Finding::Checked(check)]), ExitCode::FAILURE);
}

/// The set this binary pins, laid out in `host` as an install leaves it,
/// without downloading anything: each file's directory, each entry, the
/// record, Node (a script) and `current`. `None` where there is no managed
/// runtime.
fn pinned_set(host: &Path) -> Option<(String, PathBuf)> {
    use hennery_host::runtime::install::{LAYOUT, RecordAdapter, Selection, SetRecord};
    let selection = Selection::pinned(&Default::default()).ok()?;
    let id = selection.set_id();
    let set = host.join("adapters/sets").join(&id);
    let mut adapters = BTreeMap::new();
    for adapter in &selection.adapters {
        for file in &adapter.files {
            std::fs::create_dir_all(set.join(&adapter.name).join(&file.path)).unwrap();
        }
        let entry = set.join(&adapter.name).join(&adapter.entry);
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "// entry\n").unwrap();
        adapters.insert(
            adapter.name.clone(),
            RecordAdapter {
                version: adapter.version.clone(),
                entry: adapter.entry.clone(),
                cli_skipped: adapter.cli_skipped,
            },
        );
    }
    let record = SetRecord {
        layout: LAYOUT,
        id: id.clone(),
        manifest_hash: selection.manifest_hash.clone(),
        platform: selection.platform.key().to_string(),
        runtime: selection.runtime_name(),
        adapters,
    };
    std::fs::write(set.join("hennery-set.json"), serde_json::to_string(&record).unwrap()).unwrap();
    let bin = host.join("runtimes").join(selection.runtime_name()).join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("node"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(bin.join("node"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(Path::new("sets").join(&id), host.join("adapters/current")).unwrap();
    Some((id, set))
}

/// Check 1: this binary and its manifest; the pinned set with all its
/// packages is ok; a package or Node gone, or a record that cannot be
/// read, fails; a set others can write to warns. No host: 12 and 17 do not
/// run, and 1 still names the binary.
#[test]
fn the_pinned_set_present_is_ok_and_a_missing_part_fails() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let none = Dirs::by_contents(dir.path().join("none"), Found::Given);
    let findings = checked(&cx, none, &nothing, dir.path());
    assert!(line(&findings, 1).summary.starts_with("hennery "), "{findings:?}");
    assert!(not_run(&findings, 12) && not_run(&findings, 17), "{findings:?}");

    let host = paired(&dir.path().join("host"));
    let Some((id, set)) = pinned_set(&host) else {
        return;
    };
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    let check1 = |what: &str| {
        let findings = checked(&cx, dirs.clone(), &nothing, &host);
        let check = line(&findings, 1).clone();
        assert!(check.summary.contains(what), "{what}: {check:?}");
        check
    };
    let check = check1("every pinned package are present");
    assert_eq!(check.status, Status::Ok, "{check:?}");
    assert!(check.summary.contains(&id), "{check:?}");

    std::fs::set_permissions(&set, std::fs::Permissions::from_mode(0o777)).unwrap();
    let check = check1("can be written by other users");
    assert_eq!(check.status, Status::Warn, "{check:?}");
    std::fs::set_permissions(&set, std::fs::Permissions::from_mode(0o755)).unwrap();
    let bin = std::fs::read_dir(host.join("runtimes"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("bin");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o777)).unwrap();
    let check = check1("can be written by other users");
    assert_eq!(check.status, Status::Warn, "{check:?}");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let package = std::fs::read_dir(set.join("claude/node_modules"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::rename(&package, dir.path().join("aside")).unwrap();
    let check = check1("is missing");
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(check.fix.contains("remove"), "{check:?}");
    std::fs::rename(dir.path().join("aside"), &package).unwrap();

    let record = set.join("hennery-set.json");
    let node = std::fs::read_dir(host.join("runtimes"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::rename(node.join("bin/node"), dir.path().join("node")).unwrap();
    let check = check1("its Node");
    assert_eq!(check.status, Status::Fail, "{check:?}");
    std::fs::rename(dir.path().join("node"), node.join("bin/node")).unwrap();

    std::fs::write(&record, "{not json").unwrap();
    let check = check1("cannot be read");
    assert_eq!(check.status, Status::Fail, "{check:?}");
}

/// The command line a service of `role` would have with `--agent` added,
/// written as its unit or plist.
fn install_with_agents(cx: &Context, role: Role, data: &Path) {
    let mut argv = unit::command_line(role, &cx.exe, data).unwrap();
    argv.extend([
        "--agent".to_string(),
        "claude=/nix/store/x-claude/bin/claude-acp".to_string(),
    ]);
    let file = cx.service_file(role);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::create_dir_all(cx.env_file().parent().unwrap()).unwrap();
    std::fs::write(cx.env_file(), unit::env_file("/usr/bin:/bin")).unwrap();
    std::fs::write(&file, unit::systemd_unit(role, &argv, "/tmp/service.env").unwrap()).unwrap();
}

/// Check 12: no set, another set than the pin, or a hold warns; a service
/// that gives its agents with `--agent` needs no set.
#[test]
fn a_set_other_than_the_pin_or_none_or_a_hold_warns() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let host = paired(&dir.path().join("host"));
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    let check12 = || {
        let findings = checked(&cx, dirs.clone(), &nothing, &host);
        if hennery_host::runtime::manifest::Platform::current().is_none() {
            assert!(not_run(&findings, 12));
            return None;
        }
        Some(line(&findings, 12).clone())
    };
    let Some(check) = check12() else {
        return;
    };
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("no adapter set is installed"), "{check:?}");
    assert!(check.fix.contains("hennery host adapters update"), "{check:?}");

    install_with_agents(&cx, Role::Host, &host);
    let check = check12().unwrap();
    assert_eq!(check.status, Status::Ok, "{check:?}");
    assert!(check.summary.contains("--agent"), "{check:?}");
    // A service of another directory says nothing of this one.
    let elsewhere = paired(&dir.path().join("elsewhere"));
    install_with_agents(&cx, Role::Host, &elsewhere);
    assert_eq!(check12().unwrap().status, Status::Warn);
    std::fs::remove_file(cx.service_file(Role::Host)).unwrap();

    assert!(disk::next_set(&host).is_some());
    let (id, _) = pinned_set(&host).unwrap();
    assert_eq!(check12().unwrap().status, Status::Ok);
    // Current already: the next update installs nothing, so needs no room.
    assert_eq!(disk::next_set(&host), None);
    let current = host.join("adapters/current");
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(Path::new("sets").join("0".repeat(32)), &current).unwrap();
    let check = check12().unwrap();
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains(&format!("this binary pins {id}")), "{check:?}");

    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(Path::new("sets").join(&id), &current).unwrap();
    std::fs::write(host.join("adapters/hold"), format!("{id}\n")).unwrap();
    let check = check12().unwrap();
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("a rollback holds this host"), "{check:?}");
}

/// Check 17: no override is ok; one that runs is named; one that cannot
/// run fails; one for an agent that takes none warns; one others can write
/// warns.
#[test]
fn cli_overrides_are_named_and_one_that_cannot_run_fails() {
    use hennery_host::runtime::agents::{UseCli, set_cli_override};
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let host = paired(&dir.path().join("host"));
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    let check17 = || line(&checked(&cx, dirs.clone(), &nothing, &host), 17).clone();
    assert_eq!(check17().summary, "every agent runs its bundled CLI");

    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cli = bin.join("claude");
    std::fs::write(&cli, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cli = cli.canonicalize().unwrap();
    let choice = |agent: &str| UseCli {
        agent: agent.into(),
        path: Some(cli.clone()),
    };
    set_cli_override(&host, &choice("claude")).unwrap();
    let check = check17();
    assert_eq!(check.status, Status::Ok, "{check:?}");
    assert!(check.summary.contains("your own CLI"), "{check:?}");

    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(check17().status, Status::Warn);
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    set_cli_override(&host, &choice("gemini")).unwrap();
    let check = check17();
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("gemini, which takes none"), "{check:?}");

    std::fs::remove_file(&cli).unwrap();
    let check = check17();
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(check.fix.contains("--use-cli claude=bundled"), "{check:?}");
}

/// What glibc's loader path answers, told from its banner alone.
#[test]
fn glibc_is_told_from_its_loaders_banner() {
    use platform::{Libc, major_minor, parse_getconf, parse_loader};
    let ubuntu = "ld.so (Ubuntu GLIBC 2.35-0ubuntu3.8) stable release version 2.35.\nCopyright (C) 2022 Free Software Foundation, Inc.\n";
    assert_eq!(parse_loader(ubuntu), Libc::Glibc(2, 35));
    let fedora = "ld.so (GNU libc) stable release version 2.39.\n";
    assert_eq!(parse_loader(fedora), Libc::Glibc(2, 39));
    let el7 = "ld.so (GNU libc) stable release version 2.17, by Roland McGrath et al.\n";
    assert_eq!(parse_loader(el7), Libc::Glibc(2, 17));
    let musl = "\nmusl libc (x86_64)\nVersion 1.2.4\nDynamic Program Loader\n";
    assert_eq!(parse_loader(musl), Libc::Musl);
    let stub = "NixOS cannot run dynamically linked executables intended for generic\nlinux environments out of the box. For more information, see:\nhttps://nix.dev/permalink/stub-ld\n";
    assert_eq!(parse_loader(stub), Libc::NixStub);
    // A version without glibc's name is not taken for glibc's.
    assert_eq!(parse_loader("some loader, version 2.40\n"), Libc::Unknown);
    assert_eq!(parse_loader(""), Libc::Unknown);
    assert_eq!(parse_getconf("glibc 2.28\n"), Some((2, 28)));
    assert_eq!(parse_getconf("2.28"), None);
    assert_eq!(major_minor("5.15.0-91-generic"), Some((5, 15)));
    assert_eq!(major_minor("4.18.0-553.el8_10.x86_64"), Some((4, 18)));
    assert_eq!(major_minor("garbage"), None);
}

/// The loader is asked first; `/usr/bin/getconf` only when it says
/// nothing, and never on NixOS; both by absolute path under the root.
#[test]
fn the_loader_is_asked_first_and_getconf_never_on_nixos() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let dirs = Dirs::by_contents(dir.path().join("none"), Found::Given);
    let asked = RefCell::new(Vec::new());
    let answers = |banner: Option<&'static str>| {
        let asked = &asked;
        move |program: &Path, _: &[&str]| -> Option<Ran> {
            asked.borrow_mut().push(program.to_path_buf());
            if program.ends_with("getconf") {
                Some(said("glibc 2.31\n"))
            } else {
                banner.map(said)
            }
        }
    };
    let loader = "/lib64/ld-linux-x86-64.so.2";
    let behind = |run: Runner, nixos| {
        let doctor = Doctor {
            cx: &cx,
            dirs: dirs.clone(),
            run,
        };
        platform::libc_behind(&doctor, loader, nixos)
    };
    let banner = answers(Some("ld.so (GNU libc) stable release version 2.39.\n"));
    assert_eq!(behind(&banner, false), platform::Libc::Glibc(2, 39));
    assert_eq!(*asked.borrow(), [cx.root.join("lib64/ld-linux-x86-64.so.2")]);

    asked.borrow_mut().clear();
    let silent = answers(None);
    assert_eq!(behind(&silent, false), platform::Libc::Glibc(2, 31));
    assert_eq!(asked.borrow()[1], cx.root.join("usr/bin/getconf"));

    asked.borrow_mut().clear();
    assert_eq!(behind(&silent, true), platform::Libc::Unknown);
    assert_eq!(asked.borrow().len(), 1, "getconf asked on NixOS");
}

/// Check 2's verdicts: each reason a host cannot run fails where a host
/// needs the managed runtime, and is a warning elsewhere.
#[test]
fn a_machine_that_cannot_run_the_runtime_fails_only_where_a_host_needs_it() {
    use hennery_host::runtime::manifest::Platform as Runtime;
    use platform::{Facts, Libc, judge};
    let linux = |loader: bool, nixos: bool, libc: Libc, kernel: &str| Facts {
        name: "linux-x86_64".into(),
        platform: Some(Runtime::LinuxX64),
        loader: Some(("/lib64/ld-linux-x86-64.so.2", loader)),
        nixos,
        libc,
        kernel: Some(kernel.into()),
    };
    let fine = judge(&linux(true, false, Libc::Glibc(2, 35), "6.8.0-45-generic"), true);
    assert_eq!(fine.status, Status::Ok, "{fine:?}");
    assert_eq!(fine.summary, "linux-x64 with glibc 2.35; Linux 6.8.0-45-generic");
    let cases = [
        (
            linux(false, true, Libc::Unknown, "6.6.0"),
            "NixOS without nix-ld",
            "programs.nix-ld",
        ),
        (
            linux(false, false, Libc::Unknown, "6.6.0"),
            "no glibc loader",
            "only the collector",
        ),
        (
            linux(true, true, Libc::NixStub, "6.6.0"),
            "NixOS's stub",
            "programs.nix-ld",
        ),
        (linux(true, false, Libc::Musl, "6.6.0"), "musl's", "only the collector"),
        (
            linux(true, false, Libc::Glibc(2, 27), "6.6.0"),
            "glibc 2.27 is older than 2.28",
            "glibc 2.28",
        ),
        (
            linux(true, false, Libc::Glibc(2, 35), "4.14.0"),
            "Linux 4.14.0 is older than 4.18",
            "kernel",
        ),
    ];
    for (facts, why, fix) in cases {
        let strict = judge(&facts, true);
        assert_eq!(strict.status, Status::Fail, "{strict:?}");
        assert!(strict.summary.contains(why) && strict.fix.contains(fix), "{strict:?}");
        let lenient = judge(&facts, false);
        assert_eq!(lenient.status, Status::Warn, "{lenient:?}");
        assert!(
            lenient.summary.contains("no managed runtime can run here"),
            "{lenient:?}"
        );
    }
    let unknown = judge(&linux(true, false, Libc::Unknown, "weird"), true);
    assert_eq!(unknown.status, Status::Warn, "{unknown:?}");
    let intel_mac = Facts {
        name: "macos-x86_64".into(),
        platform: None,
        loader: None,
        nixos: false,
        libc: Libc::Unknown,
        kernel: None,
    };
    assert_eq!(judge(&intel_mac, true).status, Status::Fail);
    let mac = Facts {
        platform: Some(Runtime::DarwinArm64),
        ..intel_mac
    };
    assert_eq!(judge(&mac, true).summary, "macOS on Apple silicon");
}

/// Check 9: no room for the outbox fails; no room for the next set warns;
/// a large outbox and orphaned outboxes warn, by name.
#[test]
fn the_disk_fails_without_room_for_the_outbox_and_warns_without_room_for_a_set() {
    use hennery_host::runtime::install::SPACE_MARGIN;
    let host = Path::new("/srv/hennery/host");
    let space = |free: anyhow::Result<u64>, needed: Option<u64>| {
        let mut verdict = Verdict::default();
        disk::space(&mut verdict, host, free, needed);
        verdict.check(9, "disk")
    };
    let check = space(Ok(SPACE_MARGIN - 1), Some(1 << 30));
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(check.fix.contains("/srv/hennery/host"), "{check:?}");
    let check = space(Ok(SPACE_MARGIN + 1), Some(SPACE_MARGIN + 2));
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("the next adapter set needs"), "{check:?}");
    assert_eq!(space(Ok(1 << 40), Some(1 << 30)).status, Status::Ok);
    assert_eq!(space(Ok(SPACE_MARGIN + 1), None).status, Status::Ok);
    assert_eq!(space(Err(anyhow::anyhow!("statvfs")), None).status, Status::Warn);

    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let host = paired(&dir.path().join("host"));
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    let check9 = || line(&checked(&cx, dirs.clone(), &nothing, &host), 9).clone();
    assert!(check9().summary.contains("outbox 0 KB"), "{:?}", check9());

    // Sparse: no disk is spent on it.
    let outbox = std::fs::File::create(host.join("outbox.db")).unwrap();
    outbox.set_len(disk::LARGE_OUTBOX + 1).unwrap();
    let check = check9();
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("the outbox holds 1024 MB"), "{check:?}");
    // Its write-ahead log counts too.
    outbox.set_len(disk::LARGE_OUTBOX / 2).unwrap();
    let wal = std::fs::File::create(host.join("outbox.db-wal")).unwrap();
    wal.set_len(disk::LARGE_OUTBOX / 2 + 1).unwrap();
    assert_eq!(check9().status, Status::Warn);
    std::fs::remove_file(host.join("outbox.db")).unwrap();
    std::fs::remove_file(host.join("outbox.db-wal")).unwrap();

    for name in [
        "outbox.db.orphaned-host-1",
        "outbox.db.orphaned-host-1-wal",
        "outbox.db.orphaned-host-1-shm",
        "outbox.db.orphaned-unpaired.1",
    ] {
        std::fs::write(host.join(name), "").unwrap();
    }
    let check = check9();
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(
        check
            .summary
            .contains("never read again: outbox.db.orphaned-host-1, outbox.db.orphaned-unpaired.1"),
        "{check:?}"
    );
}

/// launchd's answer for a job that runs as `pid`, with what `launchctl
/// print` also lists: the job's environment.
fn launchd_running(pid: u32) -> Fake {
    Fake::new(move |line| {
        if line.starts_with("launchctl print") {
            said(&format!(
                "gui/501/dev.hennery.up = {{\n\tstate = running\n\tpid = {pid}\n\tenvironment = {{\n\t\tTOKEN => canary-7d-launchctl\n\t}}\n}}\n"
            ))
        } else {
            said("")
        }
    })
}

/// systemd's answers for a unit that is `active` as `pid`, with linger
/// `linger`.
fn systemd(active: &'static str, pid: u32, linger: &'static str) -> Fake {
    Fake::new(move |line| {
        if line.contains(" is-active ") {
            said(&format!("{active}\n"))
        } else if line.contains(" is-enabled ") {
            said("enabled\n")
        } else if line.contains("MainPID") {
            said(&format!("{pid}\n"))
        } else if line.starts_with("loginctl") {
            said(&format!("Linger={linger}\n"))
        } else {
            failed()
        }
    })
}

/// Check 10's line on `cx`, with `dirs` and `watched` as for `checked`.
/// The service files and the made-up `/proc` are watched too: doctor must
/// not rewrite a unit, a plist or an environment file.
fn check10(cx: &Context, data: &Path) -> Check {
    let watched = [cx.home.join("Library"), cx.home.join(".config"), cx.root.clone()];
    let before: Vec<_> = watched.iter().map(|w| snapshot(w)).collect();
    let dirs = Dirs::by_contents(data.to_path_buf(), Found::Given);
    let check = line(&checked(cx, dirs, &nothing, data), 10).clone();
    let after: Vec<_> = watched.iter().map(|w| snapshot(w)).collect();
    assert_eq!(after, before, "doctor changed a service file or /proc");
    check
}

/// Check 10: none installed warns; two roles fail.
#[test]
fn no_service_warns_and_two_roles_fail() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.fix.contains("hennery service install"), "{check:?}");

    install(&cx, Role::Up, &cx.exe, &data, "/usr/bin:/bin");
    install(&cx, Role::Collector, &cx.exe, &data, "/usr/bin:/bin");
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(
        check.summary.contains("2 roles are installed: up, collector"),
        "{check:?}"
    );
}

/// Check 10: running is ok, as launchd or systemd says; not running fails;
/// linger off warns; a user manager that cannot be asked is a warning, not
/// "not running".
#[test]
fn a_running_service_is_ok_and_one_not_running_fails() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let me = std::process::id();

    let fake = launchd_running(me);
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    install(&cx, Role::Up, &cx.exe, &data, "/usr/bin:/bin");
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Ok, "{check:?}");
    assert!(
        check.summary.contains(&format!("launchd: running, pid {me}")),
        "{check:?}"
    );
    std::fs::remove_file(cx.service_file(Role::Up)).unwrap();

    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    install(&cx, Role::Up, &cx.exe, &data, "/usr/bin:/bin");
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(check.summary.contains("not running (launchd: not loaded"), "{check:?}");
    std::fs::remove_file(cx.service_file(Role::Up)).unwrap();

    for (active, linger, status) in [
        ("active", "yes", Status::Ok),
        ("active", "no", Status::Warn),
        ("failed", "yes", Status::Fail),
    ] {
        let fake = systemd(active, me, linger);
        let cx = machine(dir.path(), Platform::Linux, &fake);
        install(&cx, Role::Host, &cx.exe, &data, "/usr/bin:/bin");
        let check = check10(&cx, &data);
        assert_eq!(check.status, status, "{active} {linger}: {check:?}");
        if linger == "no" {
            assert!(check.fix.contains("loginctl enable-linger hennery-test"), "{check:?}");
        }
    }

    let mut fake = Fake::none();
    fake.unreachable = true;
    let cx = machine(dir.path(), Platform::Linux, &fake);
    install(&cx, Role::Host, &cx.exe, &data, "/usr/bin:/bin");
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("`systemctl --user` cannot be run"), "{check:?}");
}

/// Check 10: a service whose binary is gone fails; one that runs another
/// binary than this one warns.
#[test]
fn a_missing_or_other_binary_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let fake = systemd("active", std::process::id(), "yes");
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let gone = dir.path().join("gone/hennery");
    install(&cx, Role::Up, &gone, &data, "/usr/bin:/bin");
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(check.summary.contains("which is missing"), "{check:?}");

    let other = dir.path().join("other-hennery");
    std::fs::write(&other, "#!/bin/sh\n").unwrap();
    install(&cx, Role::Up, &other, &data, "/usr/bin:/bin");
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("not this binary"), "{check:?}");
}

/// Check 10: `up`'s children judged as `service status` judges them: given
/// up on or revoked fails, each with its own fix; an unreadable report
/// fails; a stale one is judged by nothing.
#[test]
fn ups_children_given_up_on_or_revoked_fail() {
    use crate::supervisor::{self, ChildReport, ChildState, State};
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let me = std::process::id();
    let fake = launchd_running(me);
    let cx = machine(dir.path(), Platform::MacOs, &fake);
    install(&cx, Role::Up, &cx.exe, &data, "/usr/bin:/bin");
    let child = |state| ChildReport {
        state,
        crashes_in_window: 10,
        restarts: 9,
        last_exit: Some("exit status: 1".into()),
    };
    let write = |pid, host| {
        supervisor::write_state(
            &data,
            &State {
                pid,
                updated_at: 0,
                collector: child(ChildState::Running),
                host: child(host),
            },
        )
        .unwrap()
    };
    write(me, ChildState::Running);
    assert_eq!(check10(&cx, &data).status, Status::Ok);

    write(me, ChildState::GaveUp);
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(
        check.summary.contains("up's host: given up on after 10 crashes"),
        "{check:?}"
    );
    let restart = format!("launchctl kickstart -k gui/{}/dev.hennery.up", uid());
    assert!(check.fix.contains(&restart), "{check:?}");

    write(me, ChildState::Revoked);
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(
        check.fix.contains(&data.join("host/host.key").display().to_string()),
        "{check:?}"
    );

    let mut gone = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    write(gone.id(), ChildState::GaveUp);
    assert_eq!(check10(&cx, &data).status, Status::Ok);

    std::fs::write(data.join(supervisor::STATE_FILE), "{not json").unwrap();
    let check = check10(&cx, &data);
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(check.summary.contains("cannot be read"), "{check:?}");
}

/// Check 10: a binary replaced since the service started asks for a
/// restart. On Linux `/proc/<pid>/exe` decides (here a made-up `/proc`);
/// on macOS the process's path and the binary's ctime.
#[test]
fn a_replaced_binary_asks_for_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let fake = systemd("active", 4242, "yes");
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let exe = dir.path().join("bin/hennery");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "new\n").unwrap();
    let old = dir.path().join("old-hennery");
    std::fs::write(&old, "old\n").unwrap();
    install(&cx, Role::Host, &exe, &data, "/usr/bin:/bin");
    let proc = cx.root.join("proc/4242");
    std::fs::create_dir_all(&proc).unwrap();
    let restart = "systemctl --user restart hennery-host.service";

    std::os::unix::fs::symlink(&old, proc.join("exe")).unwrap();
    let check = check10(&cx, &data);
    assert!(
        check.summary.contains("pid 4242 still runs the old binary"),
        "{check:?}"
    );
    assert!(check.fix.contains(restart), "{check:?}");

    std::fs::remove_file(proc.join("exe")).unwrap();
    std::os::unix::fs::symlink(&exe, proc.join("exe")).unwrap();
    let check = check10(&cx, &data);
    assert!(!check.summary.contains("old binary"), "{check:?}");
    assert_eq!(process::runs(&cx, 4242, &exe), Some(true));

    std::fs::remove_file(proc.join("exe")).unwrap();
    std::os::unix::fs::symlink(format!("{} (deleted)", exe.display()), proc.join("exe")).unwrap();
    assert_eq!(process::runs(&cx, 4242, &exe), Some(false));

    if cfg!(target_os = "macos") {
        let fake = Fake::none();
        let mac = machine(dir.path(), Platform::MacOs, &fake);
        let me = std::process::id();
        let this = std::env::current_exe().unwrap();
        assert_eq!(process::runs(&mac, me, &this), Some(true));
        // Another file than the one this process runs.
        assert_eq!(process::runs(&mac, me, &exe), Some(false));
    }
}

/// The installed PATH is read back from either file, and nothing else of
/// it: another key of the plist's environment, or another line of the
/// environment file, is never returned.
#[test]
fn the_service_path_is_read_back_alone() {
    let awkward = "/a dir/with \"quotes\" & <tags> $HOME `tick` \\back:/usr/bin";
    let argv = vec!["/bin/hennery".to_string(), "up".to_string()];
    let plist = unit::plist(Role::Up, &argv, awkward, "/tmp/l");
    assert_eq!(unit::plist_path(&plist).as_deref(), Some(awkward));
    let env = unit::env_file(awkward);
    assert_eq!(unit::env_file_path(&env).as_deref(), Some(awkward));

    let extra = plist.replace(
        "<key>EnvironmentVariables</key>\n\t<dict>\n",
        "<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>TOKEN</key>\n\t\t<string>canary-7d-plist</string>\n",
    );
    assert_eq!(unit::plist_path(&extra).as_deref(), Some(awkward));
    let extra = format!("TOKEN=\"canary-7d-env\"\n{env}");
    assert_eq!(unit::env_file_path(&extra).as_deref(), Some(awkward));
    // systemd takes the last assignment.
    let later = format!("{env}PATH=\"/later/bin\"\n");
    assert_eq!(unit::env_file_path(&later).as_deref(), Some("/later/bin"));
    assert_eq!(unit::plist_path("<plist></plist>"), None);
    assert_eq!(unit::env_file_path("PATH=unquoted\n"), None);
}

/// Executable stand-ins named `names` in a new directory `dir/name`.
fn tools(dir: &Path, name: &str, names: &[&str]) -> PathBuf {
    let bin = dir.join(name);
    std::fs::create_dir_all(&bin).unwrap();
    for tool in names {
        std::fs::write(bin.join(tool), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(bin.join(tool), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

/// The PATH `service install` would give `cx`'s login shell now.
fn captured_path(cx: &Context) -> String {
    let macos = cx.platform == Platform::MacOs;
    let env = crate::service::path::shell_environment(&cx.env, &cx.shell, macos);
    let captured =
        crate::service::path::login_environment(&cx.shell, &env, crate::service::path::CAPTURE_TIMEOUT).unwrap();
    crate::service::path::service_path(&captured, &cx.home, macos)
        .unwrap()
        .path
}

/// Check 5: `sh` or `git` missing fails, `rg` is optional; an entry that
/// is gone or short-lived warns; a PATH the login shell no longer gives
/// warns, naming the shell.
#[test]
fn a_service_path_without_sh_or_git_fails_and_drift_warns() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let fake = systemd("active", std::process::id(), "yes");
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let check5 = |path: &str| {
        install(&cx, Role::Up, &cx.exe, &data, path);
        let dirs = Dirs::by_contents(data.clone(), Found::Given);
        line(&checked(&cx, dirs, &nothing, &data), 5).clone()
    };
    let no_git = tools(dir.path(), "no-git", &["sh", "rg"]);
    let check = check5(&no_git.display().to_string());
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(check.summary.contains("git is not on the service's PATH"), "{check:?}");
    assert!(!check.summary.contains("sh, git and rg are on"), "{check:?}");

    let now = captured_path(&cx);
    let both = tools(dir.path(), "both", &["sh", "git"]);
    let check = check5(&format!("{}:{now}", both.display()));
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(
        check
            .summary
            .contains("your login shell (/bin/sh) now gives another PATH"),
        "{check:?}"
    );
    assert!(check.fix.contains("--shell"), "{check:?}");

    // What this machine's login shell gives may lack git, or name a
    // directory the scratch home lacks: only the drift is pinned here.
    let check = check5(&now);
    assert!(
        check.summary.contains("your login shell (/bin/sh) gives the same PATH"),
        "{check:?}"
    );
    assert!(!check.summary.contains("another PATH"), "{check:?}");

    // A collector runs no agents: it needs neither sh nor git.
    std::fs::remove_file(cx.service_file(Role::Up)).unwrap();
    install(&cx, Role::Collector, &cx.exe, &data, &no_git.display().to_string());
    let dirs = Dirs::by_contents(data.clone(), Found::Given);
    let check = line(&checked(&cx, dirs, &nothing, &data), 5).clone();
    assert!(!check.summary.contains("not on the service's PATH"), "{check:?}");
    std::fs::remove_file(cx.service_file(Role::Collector)).unwrap();

    let check = check5(&format!("{now}:/nonexistent-7d:/nix/store/abc-tools/bin"));
    assert!(
        check
            .summary
            .contains("/nonexistent-7d on the service's PATH does not exist"),
        "{check:?}"
    );
    assert!(check.summary.contains("Nix store"), "{check:?}");
}

/// Check 14: who holds `host.lock`. Nobody, or a dead pid's file, is no
/// host; this process holding it is a host started by hand, the host
/// service's own process, or not the service's; under `up`, its host is
/// the one whose parent is `up`. Linux's `/proc/locks` is trusted over the
/// file. A FIFO in its place is named, without blocking.
#[test]
fn host_lock_is_judged_by_who_holds_it() {
    let dir = tempfile::tempdir().unwrap();
    let me = std::process::id();
    let up = dir.path().join("up");
    let host = paired(&up.join("host"));
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let check14 = |cx: &Context| {
        let dirs = Dirs::by_contents(host.clone(), Found::Given);
        line(&checked(cx, dirs, &nothing, &host), 14).clone()
    };
    assert_eq!(check14(&cx).summary, "no host runs on it");

    let mut gone = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    std::fs::write(host.join("host.lock"), format!("{}\n", gone.id())).unwrap();
    assert_eq!(check14(&cx).summary, "no host runs on it");

    let lock = crate::lock::acquire(&host, crate::lock::HOST_LOCK, "hennery host run").unwrap();
    assert_eq!(
        check14(&cx).summary,
        format!("pid {me} serves it: a host started by hand")
    );
    // Held, before its holder wrote its pid: not "no host".
    std::fs::write(host.join("host.lock"), "").unwrap();
    let check = check14(&cx);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("has not written its pid"), "{check:?}");
    std::fs::write(host.join("host.lock"), format!("{me}\n")).unwrap();

    let fake = systemd("active", me, "yes");
    let cx = machine(dir.path(), Platform::Linux, &fake);
    install(&cx, Role::Host, &cx.exe, &host, "/usr/bin:/bin");
    assert_eq!(check14(&cx).summary, format!("the host service (pid {me}) serves it"));
    let fake = systemd("active", 1, "yes");
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let check = check14(&cx);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("not the host service's host"), "{check:?}");
    // Two roles: which one should serve it cannot be told.
    install(&cx, Role::Collector, &cx.exe, &host, "/usr/bin:/bin");
    let check = check14(&cx);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("which service should is unknown"), "{check:?}");
    std::fs::remove_file(cx.service_file(Role::Collector)).unwrap();
    // The manager names no process: unknown, and never `kill`.
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let check = check14(&cx);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(
        check
            .summary
            .contains("whether it is the host service's host is unknown"),
        "{check:?}"
    );
    assert!(!check.fix.contains("kill"), "{check:?}");
    std::fs::remove_file(cx.service_file(Role::Host)).unwrap();

    let fake = systemd("active", 77, "yes");
    let cx = machine(dir.path(), Platform::Linux, &fake);
    install(&cx, Role::Up, &cx.exe, &up, "/usr/bin:/bin");
    let proc = cx.root.join(format!("proc/{me}"));
    std::fs::create_dir_all(&proc).unwrap();
    std::fs::write(proc.join("stat"), format!("{me} (hennery) S 77 1 1 0\n")).unwrap();
    assert_eq!(check14(&cx).summary, format!("up's host (pid {me}) serves it"));
    std::fs::write(proc.join("stat"), format!("{me} (hennery) S 78 1 1 0\n")).unwrap();
    assert_eq!(check14(&cx).status, Status::Warn);
    // Its parent cannot be read (`hidepid`): unknown, and never `kill`.
    std::fs::remove_file(proc.join("stat")).unwrap();
    let check = check14(&cx);
    assert!(check.summary.contains("its parent cannot be read"), "{check:?}");
    assert!(!check.fix.contains("kill"), "{check:?}");
    std::fs::remove_file(cx.service_file(Role::Up)).unwrap();

    // `/proc/locks` says who holds it, whatever the file says.
    let meta = std::fs::metadata(host.join("host.lock")).unwrap();
    let id = format!("{}:{}", process::linux_device(meta.dev()), meta.ino());
    std::fs::write(
        cx.root.join("proc/locks"),
        format!("1: POSIX  ADVISORY  WRITE 1 00:00:1 0 EOF\n2: FLOCK  ADVISORY  WRITE 5555 {id} 0 EOF\n"),
    )
    .unwrap();
    assert_eq!(check14(&cx).summary, "pid 5555 serves it: a host started by hand");
    // No line names it (on btrfs or overlayfs the device differs): not an
    // answer; the lock, really held, is found by the probe.
    let other = format!("{}:{}", process::linux_device(meta.dev() + 1), meta.ino());
    std::fs::write(
        cx.root.join("proc/locks"),
        format!("2: FLOCK  ADVISORY  WRITE 5555 {other} 0 EOF\n"),
    )
    .unwrap();
    assert_eq!(
        check14(&cx).summary,
        format!("pid {me} serves it: a host started by hand")
    );
    std::fs::remove_file(cx.root.join("proc/locks")).unwrap();
    drop(lock);
    // This live process's pid is still in the file, but nothing holds the
    // lock: the probe finds it free, so no host runs. A dropped `flock` is
    // released at once, but a process forked meanwhile by another test
    // holds a copy of the descriptor until it execs: wait for that.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while check14(&cx).summary != "no host runs on it" {
        assert!(std::time::Instant::now() < deadline, "{:?}", check14(&cx));
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    std::fs::remove_file(host.join("host.lock")).unwrap();
    let fifo = std::ffi::CString::new(host.join("host.lock").into_os_string().into_encoded_bytes()).unwrap();
    // SAFETY: mkfifo(3) with a NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let check = check14(&cx);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("is not a regular file"), "{check:?}");
}

/// Check 14: a host directory or key other users can read warns, told by
/// their modes alone: the key is never read.
#[test]
fn a_readable_host_directory_or_key_warns_by_mode_only() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let host = paired(&dir.path().join("host"));
    std::fs::write(host.join("host.key"), "canary-7d-key\n").unwrap();
    std::fs::set_permissions(host.join("host.key"), std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o700)).unwrap();
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    let findings = checked(&cx, dirs.clone(), &nothing, &host);
    assert_eq!(line(&findings, 14).status, Status::Ok);

    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(host.join("host.key"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let findings = checked(&cx, dirs.clone(), &nothing, &host);
    let check = line(&findings, 14);
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(
        check.fix.contains("chmod 700") && check.fix.contains("chmod 600"),
        "{check:?}"
    );
    assert!(!report(&dirs, &findings).contains("canary-7d-key"));

    // A key that is a link is judged by what it links to.
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o700)).unwrap();
    let kept = dir.path().join("kept.key");
    std::fs::rename(host.join("host.key"), &kept).unwrap();
    std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&kept, host.join("host.key")).unwrap();
    let findings = checked(&cx, dirs, &nothing, &host);
    assert_eq!(line(&findings, 14).status, Status::Ok, "{findings:?}");
}

/// A process's parent: from `/proc/<pid>/stat` on Linux (here a made-up
/// one, whose name holds a space and parentheses), from the kernel on
/// macOS.
#[test]
fn the_parent_of_a_process_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let proc = cx.root.join("proc/4242");
    std::fs::create_dir_all(&proc).unwrap();
    std::fs::write(proc.join("stat"), "4242 (hennery (up)) S 77 4242 4242 0 -1 4194560\n").unwrap();
    assert_eq!(process::parent(&cx, 4242), Some(77));
    assert_eq!(process::parent(&cx, 4243), None);
    if cfg!(target_os = "macos") {
        let mac = machine(dir.path(), Platform::MacOs, &fake);
        // SAFETY: getppid(2) cannot fail.
        let parent = unsafe { libc::getppid() } as u32;
        assert_eq!(process::parent(&mac, std::process::id()), Some(parent));
    }
}

/// No secret reaches the report: not another key of the plist's
/// environment, another line of the environment file, the login shell's
/// exports, the host key, or what `launchctl print` lists.
#[test]
fn no_secret_reaches_the_report() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let host = paired(&data.join("host"));
    std::fs::write(host.join("host.key"), "canary-7d-key\n").unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    std::fs::write(
        dir.path().join("home/.profile"),
        "export HENNERY_CANARY=canary-7d-shell\n",
    )
    .unwrap();
    for platform in [Platform::MacOs, Platform::Linux] {
        let fake = match platform {
            Platform::MacOs => launchd_running(std::process::id()),
            Platform::Linux => systemd("active", std::process::id(), "yes"),
        };
        let cx = machine(dir.path(), platform, &fake);
        install(&cx, Role::Up, &cx.exe, &data, "/usr/bin:/bin");
        match platform {
            Platform::MacOs => {
                let file = cx.service_file(Role::Up);
                let text = std::fs::read_to_string(&file).unwrap().replace(
                    "<key>EnvironmentVariables</key>\n\t<dict>\n",
                    "<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>TOKEN</key>\n\t\t<string>canary-7d-plist</string>\n",
                );
                std::fs::write(&file, text).unwrap();
            }
            Platform::Linux => {
                let text = std::fs::read_to_string(cx.env_file()).unwrap();
                std::fs::write(cx.env_file(), format!("TOKEN=\"canary-7d-env\"\n{text}")).unwrap();
            }
        }
        let dirs = Dirs::discover(&cx, None).unwrap();
        let findings = checked(&cx, dirs.clone(), &nothing, &data);
        let text = report(&dirs, &findings);
        assert!(!text.contains("canary-7d"), "{platform:?}: {text}");
        std::fs::remove_file(cx.service_file(Role::Up)).unwrap();
    }
}

/// On Linux, `/proc/locks` names this process as the holder of a lock it
/// really holds, at the real root: the device it prints matches `stat`'s on
/// CI's filesystem.
#[cfg(target_os = "linux")]
#[test]
fn proc_locks_names_the_real_holder() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let mut cx = machine(dir.path(), Platform::Linux, &fake);
    cx.root = PathBuf::from("/");
    let lock = crate::lock::acquire(dir.path(), crate::lock::HOST_LOCK, "hennery host run").unwrap();
    assert_eq!(
        process::flock_holder(&cx, &dir.path().join(crate::lock::HOST_LOCK)),
        Some(std::process::id())
    );
    drop(lock);
}

/// What `script` passes a fixture to run it once: each fixture exits at
/// once on it, before it records anything.
const WARM_UP: &str = "--warm-up";

/// Write the shell script `text` (its `#!/bin/sh` line first) to `path`,
/// executable, with a first line that exits on `WARM_UP`, and run it once
/// that way. The first run of a new executable can wait on the machine's
/// scan of it (seconds on a Mac with endpoint security), which would eat
/// a check's budget; on Linux, a fork elsewhere in the test binary can
/// still hold the file open for writing (ETXTBSY), so that run is retried.
fn script(path: &Path, text: &str) {
    let body = text.strip_prefix("#!/bin/sh\n").expect("a sh script");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!("#!/bin/sh\nfor a; do [ \"$a\" = {WARM_UP} ] && exit 0; done\n{body}"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..100 {
        match std::process::Command::new(path).arg(WARM_UP).status() {
            Ok(status) => {
                assert!(status.success(), "{}: {status}", path.display());
                return;
            }
            Err(err) if err.raw_os_error() == Some(libc::ETXTBSY) => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            Err(err) => panic!("{}: {err}", path.display()),
        }
    }
    panic!("{} stayed busy", path.display());
}

/// A pinned set whose programs are stand-ins (`fake_agents`): `node` runs
/// an adapter that answers `initialize` with the version written in its
/// entry file (`silent` never answers), or, given Codex's script, the fake
/// CLI; Claude's bundled CLI is the fake CLI too. Each program records its
/// pid, its environment and its working directory in `dir/runs`, and
/// prints a canary on both outputs. The fake CLI says `--version` from
/// `dir/<agent>-version` and is logged in while `dir/<agent>-logged-in`
/// exists.
struct FakeAgents {
    host: PathBuf,
    set: PathBuf,
    runs: PathBuf,
    dir: PathBuf,
}

fn fake_agents(dir: &Path) -> Option<FakeAgents> {
    let host = paired(&dir.join("host"));
    let (_, set) = pinned_set(&host)?;
    let runs = dir.join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let cli = dir.join("fake-cli");
    script(
        &cli,
        &format!(
            "#!/bin/sh\nagent=$1; shift\nenv > \"{runs}/env-cli-$$\"; pwd > \"{runs}/cwd-cli-$$\"\n\
             echo canary-7d-cli-out; echo canary-7d-cli-err >&2\n\
             case \"$1\" in --version) cat \"{dir}/$agent-version\"; exit 0;; esac\n\
             test -e \"{dir}/$agent-logged-in\"\n",
            runs = runs.display(),
            dir = dir.display()
        ),
    );
    let node = std::fs::read_dir(host.join("runtimes"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("bin/node");
    script(
        &node,
        &format!(
            "#!/bin/sh\ncase \"$1\" in *codex.js) shift; exec \"{cli}\" codex \"$@\";; esac\n\
             echo $$ > \"{runs}/pid-$$\"; env > \"{runs}/env-$$\"; pwd > \"{runs}/cwd-$$\"\n\
             echo canary-7d-adapter-err >&2\nread line\nv=$(cat \"$1\")\n\
             test \"$v\" = silent && exec sleep 60\n\
             printf '{{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{}},\"agentInfo\":{{\"name\":\"canary-7d-name\",\"version\":\"%s\"}}}}}}\\n' \"$v\"\n\
             exec sleep 60\n",
            cli = cli.display(),
            runs = runs.display()
        ),
    );
    let record: hennery_host::runtime::install::SetRecord =
        serde_json::from_str(&std::fs::read_to_string(set.join("hennery-set.json")).unwrap()).unwrap();
    for (name, adapter) in &record.adapters {
        std::fs::write(set.join(name).join(&adapter.entry), &adapter.version).unwrap();
    }
    script(
        &set.join(format!(
            "claude/node_modules/@anthropic-ai/claude-agent-sdk-{}/claude",
            record.platform
        )),
        &format!("#!/bin/sh\nexec \"{}\" claude \"$@\"\n", cli.display()),
    );
    let codex = set.join("codex/node_modules/@openai/codex/bin/codex.js");
    std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
    std::fs::write(&codex, "// codex\n").unwrap();
    for (agent, version) in [("claude", "2.1.3 (Claude Code)\n"), ("codex", "codex-cli 0.155.1\n")] {
        std::fs::write(dir.join(format!("{agent}-version")), version).unwrap();
    }
    Some(FakeAgents {
        host,
        set,
        runs,
        dir: dir.to_path_buf(),
    })
}

impl FakeAgents {
    /// The entry file of `agent`, whose text is the version it answers.
    fn entry(&self, agent: &str) -> PathBuf {
        let record: hennery_host::runtime::install::SetRecord =
            serde_json::from_str(&std::fs::read_to_string(self.set.join("hennery-set.json")).unwrap()).unwrap();
        self.set.join(agent).join(&record.adapters[agent].entry)
    }

    /// Every file of `runs` whose name starts with `prefix`, read.
    fn runs(&self, prefix: &str) -> Vec<String> {
        std::fs::read_dir(&self.runs)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .collect()
    }
}

/// Every pid the fake adapters recorded is gone: their groups were killed.
/// Polled, as the kernel reaps the group's other members on its own time.
fn all_gone(fake: &FakeAgents) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    for pid in fake.runs("pid-") {
        let pid: i32 = pid.trim().parse().unwrap();
        // SAFETY: kill(2) with signal 0 only checks.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(std::time::Instant::now() < deadline, "pid {pid} is still running");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// Check 3: each adapter answers `initialize` with its pinned version; one
/// that answers another warns; one that never answers fails within its
/// budget. Each was started in the home directory, with an environment
/// built rather than inherited, and its whole group is gone afterwards.
#[test]
fn adapters_start_as_the_host_starts_them_and_are_all_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let mut cx = machine(dir.path(), Platform::Linux, &fake);
    cx.env.insert("CLAUDECODE".into(), "canary-7d-nesting".into());
    let Some(agents) = fake_agents(dir.path()) else {
        return;
    };
    let dirs = Dirs::by_contents(agents.host.clone(), Found::Given);
    let check3 = || line(&checked(&cx, dirs.clone(), &nothing, &agents.host), 3).clone();
    let check = check3();
    assert_eq!(check.status, Status::Ok, "{check:?}");
    assert!(check.summary.contains("claude 0.81.0 answers"), "{check:?}");
    all_gone(&agents);
    for env in agents.runs("env-") {
        assert!(!env.contains("CLAUDECODE"), "{env}");
        assert!(env.contains(&format!("HOME={}", cx.home.display())), "{env}");
        assert!(env.contains("PATH=/usr/bin:/bin"), "{env}");
    }
    for cwd in agents.runs("cwd-") {
        assert_eq!(
            Path::new(cwd.trim()).canonicalize().unwrap(),
            cx.home.canonicalize().unwrap()
        );
    }

    std::fs::write(agents.entry("claude"), "0.80.9").unwrap();
    let check = check3();
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(check.summary.contains("not the pinned 0.81.0"), "{check:?}");

    std::fs::write(agents.entry("claude"), "silent").unwrap();
    let started = std::time::Instant::now();
    let check = check3();
    assert_eq!(check.status, Status::Fail, "{check:?}");
    assert!(check.summary.contains("did not answer `initialize`"), "{check:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(40),
        "{:?}",
        started.elapsed()
    );
    all_gone(&agents);
    let text = report(&dirs, &[Finding::Checked(check)]);
    assert!(!text.contains("canary-7d"), "{text}");
}

/// The adapter's answer is read for its version alone, and one that
/// writes noise first, or answers with an error, is told apart.
#[test]
fn initialize_reads_its_answer_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = dir.path().join("adapter");
    let answer = |text: &str| {
        script(
            &adapter,
            &format!("#!/bin/sh\nread line\nprintf '%s\\n' '{text}'\nexec sleep 60\n"),
        );
        let agent = hennery_host::AgentCommand {
            program: adapter.display().to_string(),
            args: Vec::new(),
            env: Vec::new(),
        };
        let env = vec![("HOME".to_string(), dir.path().display().to_string())];
        spawn::initialize(&agent, &env, spawn::INITIALIZE_TIMEOUT)
    };
    assert_eq!(
        answer(r#"{"jsonrpc":"2.0","id":0,"result":{"agentInfo":{"name":"a","version":"1.2.3"}}}"#),
        spawn::Started::Answered(Some("1.2.3".into()))
    );
    assert_eq!(
        answer(r#"{"jsonrpc":"2.0","id":0,"result":{}}"#),
        spawn::Started::Answered(None)
    );
    assert!(matches!(
        answer(r#"{"jsonrpc":"2.0","id":0,"error":{"code":-1,"message":"canary-7d"}}"#),
        spawn::Started::Failed(why) if !why.contains("canary")
    ));
    assert!(matches!(
        answer(r#"{"jsonrpc":"2.0","id":7,"result":{}}"#),
        spawn::Started::Failed(why) if why.contains("did not answer")
    ));

    // An agent's own variables pass, but not what the host's spawn drops
    // even when the agent's configuration names it.
    let seen = dir.path().join("seen");
    script(
        &adapter,
        &format!(
            "#!/bin/sh\nenv > \"{}\"\nread line\nprintf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{{}}}}'\nexec sleep 60\n",
            seen.display()
        ),
    );
    let env: Vec<(String, String)> = [
        "CODEX_PATH",
        "CLAUDECODE",
        "HENNERY_DEV_TOKEN",
        "HENNERY_LOG_DIR",
        "HENNERY_SERVICE",
    ]
    .iter()
    .map(|name| (name.to_string(), "canary-7d-own".to_string()))
    .collect();
    let agent = hennery_host::AgentCommand {
        program: adapter.display().to_string(),
        args: Vec::new(),
        env,
    };
    let home = vec![("HOME".to_string(), dir.path().display().to_string())];
    assert_eq!(
        spawn::initialize(&agent, &home, spawn::INITIALIZE_TIMEOUT),
        spawn::Started::Answered(None)
    );
    let seen = std::fs::read_to_string(&seen).unwrap();
    assert!(seen.contains("CODEX_PATH=canary-7d-own"), "{seen}");
    for name in ["CLAUDECODE", "HENNERY_DEV_TOKEN", "HENNERY_LOG_DIR", "HENNERY_SERVICE"] {
        assert!(!seen.contains(name), "{name}: {seen}");
    }
    assert_eq!(spawn::version_in("2.1.3 (Claude Code)"), Some((2, 1, 3)));
    assert_eq!(spawn::version_in("codex-cli 0.155.1"), Some((0, 155, 1)));
    assert_eq!(spawn::version_in("no version"), None);
    assert!(spawn::far_apart((2, 1, 3), (2, 2, 0)) && spawn::far_apart((2, 1, 3), (3, 1, 3)));
    assert!(!spawn::far_apart((2, 1, 3), (2, 1, 9)));
}

/// Check 4: a CLI that says it is logged in is ok, one that does not warns
/// with how to log in; its output, which may name the account, is never
/// read.
#[test]
fn logged_in_is_the_clis_exit_code_alone() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let Some(agents) = fake_agents(dir.path()) else {
        return;
    };
    let dirs = Dirs::by_contents(agents.host.clone(), Found::Given);
    let check4 = || {
        let findings = checked(&cx, dirs.clone(), &nothing, &agents.host);
        assert!(!report(&dirs, &findings).contains("canary-7d"));
        line(&findings, 4).clone()
    };
    let check = check4();
    assert_eq!(check.status, Status::Warn, "{check:?}");
    assert!(
        check
            .summary
            .contains("claude is not logged in; codex is not logged in"),
        "{check:?}"
    );
    assert!(check.fix.contains("doctor never logs in"), "{check:?}");
    for agent in ["claude", "codex"] {
        std::fs::write(agents.dir.join(format!("{agent}-logged-in")), "").unwrap();
    }
    let check = check4();
    assert_eq!(check.status, Status::Ok, "{check:?}");
    assert_eq!(check.summary, "claude is logged in; codex is logged in");
    for cwd in agents.runs("cwd-cli-") {
        assert_eq!(
            Path::new(cwd.trim()).canonicalize().unwrap(),
            cx.home.canonicalize().unwrap()
        );
    }
}

/// Check 13: the bundled CLI against the terminal's: another minor warns,
/// the same is ok, none on PATH is ok. Check 17: an override against the
/// bundled CLI.
#[test]
fn bundled_and_terminal_clis_are_compared() {
    use hennery_host::runtime::agents::{UseCli, set_cli_override};
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let mut cx = machine(dir.path(), Platform::Linux, &fake);
    let Some(agents) = fake_agents(dir.path()) else {
        return;
    };
    let dirs = Dirs::by_contents(agents.host.clone(), Found::Given);
    let check = |n| line(&checked(&cx, dirs.clone(), &nothing, &agents.host), n).clone();
    let thirteen = check(13);
    assert_eq!(
        thirteen.summary,
        "claude: bundled 2.1.3, none on PATH; codex: bundled 0.155.1, none on PATH"
    );

    let terminal = dir.path().join("terminal");
    std::fs::create_dir_all(&terminal).unwrap();
    script(&terminal.join("claude"), "#!/bin/sh\necho '2.4.0 (Claude Code)'\n");
    cx.env
        .insert("PATH".into(), format!("{}:/usr/bin:/bin", terminal.display()));
    let thirteen = line(&checked(&cx, dirs.clone(), &nothing, &agents.host), 13).clone();
    assert_eq!(thirteen.status, Status::Warn, "{thirteen:?}");
    assert!(
        thirteen.summary.contains("bundled 2.1.3, terminal 2.4.0"),
        "{thirteen:?}"
    );
    script(&terminal.join("claude"), "#!/bin/sh\necho '2.1.9 (Claude Code)'\n");
    assert_eq!(
        line(&checked(&cx, dirs.clone(), &nothing, &agents.host), 13).status,
        Status::Ok
    );

    set_cli_override(
        &agents.host,
        &UseCli {
            agent: "claude".into(),
            path: Some(terminal.join("claude")),
        },
    )
    .unwrap();
    let seventeen = line(&checked(&cx, dirs.clone(), &nothing, &agents.host), 17).clone();
    assert!(
        seventeen.summary.contains("your CLI 2.1.9, the pinned 2.1.3"),
        "{seventeen:?}"
    );
    script(&terminal.join("claude"), "#!/bin/sh\necho '3.0.0 (Claude Code)'\n");
    let seventeen = line(&checked(&cx, dirs, &nothing, &agents.host), 17).clone();
    assert_eq!(seventeen.status, Status::Warn, "{seventeen:?}");
    assert!(
        seventeen.summary.contains("your CLI is 3.0.0, the pinned one 2.1.3"),
        "{seventeen:?}"
    );
}

/// The service's `--agent` commands are started in place of a set, with
/// the service's PATH, and without what a host never gives an agent.
#[test]
fn a_services_agents_and_path_are_the_ones_used() {
    let dir = tempfile::tempdir().unwrap();
    let fake = systemd("active", 1, "yes");
    let mut cx = machine(dir.path(), Platform::Linux, &fake);
    for name in ["HENNERY_SERVICE", "HENNERY_LOG_DIR", "HENNERY_DEV_TOKEN"] {
        cx.env.insert(name.into(), "canary-7d-host".into());
    }
    let host = paired(&dir.path().join("host"));
    let adapter = dir.path().join("given-adapter");
    let env_out = dir.path().join("given-env");
    script(
        &adapter,
        &format!(
            "#!/bin/sh\nenv > \"{}\"\nread line\nprintf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{{\"agentInfo\":{{\"name\":\"n\",\"version\":\"9.9.9\"}}}}}}'\nexec sleep 60\n",
            env_out.display()
        ),
    );
    let mut argv = unit::command_line(Role::Host, &cx.exe, &host).unwrap();
    argv.extend(["--agent".to_string(), format!("nix={}", adapter.display())]);
    let file = cx.service_file(Role::Host);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::create_dir_all(cx.env_file().parent().unwrap()).unwrap();
    std::fs::write(cx.env_file(), unit::env_file("/service/bin:/usr/bin:/bin")).unwrap();
    std::fs::write(
        &file,
        unit::systemd_unit(Role::Host, &argv, "/tmp/service.env").unwrap(),
    )
    .unwrap();
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    let findings = checked(&cx, dirs, &nothing, &host);
    let check = line(&findings, 3);
    assert_eq!(check.summary, "nix answers (9.9.9; given by --agent, no pin)");
    let env = std::fs::read_to_string(&env_out).unwrap();
    assert!(env.contains("PATH=/service/bin:/usr/bin:/bin"), "{env}");
    // The host's spawn drops what picks its own log, and its secrets.
    for name in ["HENNERY_SERVICE", "HENNERY_LOG_DIR", "HENNERY_DEV_TOKEN"] {
        assert!(!env.contains(name), "{name}: {env}");
    }
}

/// A collector served in this process on `dir`'s database: the runtime
/// serving it (keep it alive), its state and its address.
fn live_collector(
    dir: &Path,
) -> (
    tokio::runtime::Runtime,
    hennery_sessions::AppState,
    std::net::SocketAddr,
) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let (state, addr) = rt.block_on(async {
        let db = dir.join("hennery.db");
        let state = hennery_sessions::AppState::new(
            hennery_sessions::store::Store::open(&db).unwrap(),
            hennery_kernel::hosts::Hosts::open(&db).unwrap(),
            hennery_kernel::operator::Operator::open(&db).unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        (state, addr)
    });
    (rt, state, addr)
}

/// `host` paired with the collector at `addr` through a code `state` mints:
/// its host id.
fn pair(
    rt: &tokio::runtime::Runtime,
    state: &hennery_sessions::AppState,
    addr: std::net::SocketAddr,
    host: &Path,
) -> String {
    let code = state
        .hosts
        .mint_pairing_code(hennery_kernel::secret::unix_now())
        .unwrap()
        .code;
    match rt
        .block_on(hennery_host::pairing::join(
            &format!("http://{addr}"),
            &code,
            host,
            "doctor-test",
        ))
        .unwrap()
    {
        hennery_host::pairing::Joined::Paired { host_id } => host_id,
        other => panic!("{other:?}"),
    }
}

/// A server that answers every request with `answer`, on loopback.
fn answering(answer: String) -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(answer.as_bytes());
        }
    });
    addr
}

/// `host.toml`'s collector pointed at `addr`, keeping the pairing.
fn point_at(host: &Path, url: &str) {
    let path = host.join("host.toml");
    let mut table: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    table.insert("collector".into(), toml::Value::String(url.to_string()));
    std::fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
}

/// Check 7 against a real collector: an accepted hello is ok and gives
/// check 8 the collector's `Date`; a revoked host fails with re-pairing; an
/// unknown one fails with the files to remove; a host connected elsewhere
/// with this key warns; while a host holds `host.lock`, no hello is sent.
#[test]
fn the_collector_is_reached_step_by_step_and_its_hello_judged() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let (rt, state, addr) = live_collector(&dir.path().join("collector"));
    let host = dir.path().join("host");
    let host_id = pair(&rt, &state, addr, &host);
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    let run = || checked(&cx, dirs.clone(), &nothing, &host);
    let findings = run();
    let seven = line(&findings, 7);
    assert_eq!(seven.status, Status::Ok, "{seven:?}");
    assert!(seven.summary.contains("its hello is accepted"), "{seven:?}");
    let eight = line(&findings, 8);
    assert_eq!(eight.status, Status::Ok, "{eight:?}");

    // A host connected with the key elsewhere: `already_connected`.
    let paired = hennery_host::identity::Paired::read(&host).unwrap().unwrap();
    let mut cfg = hennery_host::HostConfig::new(
        paired.collector_url.clone(),
        paired.host_id.clone(),
        paired.key.clone(),
        dir.path().join("elsewhere"),
    );
    cfg.agents = Default::default();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let elsewhere = rt.spawn(hennery_host::run_until(cfg, async {
        let _ = stopped.await;
    }));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let seven = loop {
        let seven = line(&run(), 7).clone();
        if seven.status != Status::Ok || std::time::Instant::now() > deadline {
            break seven;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    assert_eq!(seven.status, Status::Warn, "{seven:?}");
    assert!(
        seven
            .summary
            .contains("another connection with this host's key is live"),
        "{seven:?}"
    );
    let _ = stop.send(());
    let _ = rt.block_on(elsewhere);

    // A host holds the lock here: no hello.
    let lock = crate::lock::acquire(&host, crate::lock::HOST_LOCK, "hennery host run").unwrap();
    assert!(
        line(&run(), 7).summary.contains("hello not sent"),
        "{:?}",
        line(&run(), 7)
    );
    drop(lock);

    state
        .hosts
        .revoke(&host_id, hennery_kernel::secret::unix_now())
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let seven = loop {
        let seven = line(&run(), 7).clone();
        if seven.status == Status::Fail || std::time::Instant::now() > deadline {
            break seven;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    assert!(seven.summary.contains("revoked"), "{seven:?}");
    assert!(seven.fix.contains("hennery host join"), "{seven:?}");

    let mut table: toml::Table = toml::from_str(&std::fs::read_to_string(host.join("host.toml")).unwrap()).unwrap();
    table.insert("host_id".into(), toml::Value::String("host-unknown".into()));
    std::fs::write(host.join("host.toml"), toml::to_string(&table).unwrap()).unwrap();
    let seven = line(&run(), 7).clone();
    assert_eq!(seven.status, Status::Fail, "{seven:?}");
    assert!(seven.fix.contains("remove host.key and host.toml"), "{seven:?}");
}

/// Check 7's earlier steps, each failing on its own: plain http to another
/// machine, nothing listening, a status other than 200; and check 8 on a
/// skewed `Date`. Nothing beyond loopback is reached.
#[test]
fn each_step_to_the_collector_fails_on_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let host = paired(&dir.path().join("host"));
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    let run = || checked(&cx, dirs.clone(), &nothing, &host);

    point_at(&host, "ws://192.0.2.7:7117/api/hosts/ws");
    let seven = line(&run(), 7).clone();
    assert_eq!(seven.status, Status::Fail, "{seven:?}");
    assert!(
        seven.summary.contains("only allowed to a loopback address"),
        "{seven:?}"
    );

    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    point_at(&host, &format!("ws://{closed}/api/hosts/ws"));
    let findings = run();
    assert!(
        line(&findings, 7).summary.contains("nothing answers on"),
        "{findings:?}"
    );
    assert!(not_run(&findings, 8));

    let unavailable = answering("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n".into());
    point_at(&host, &format!("ws://{unavailable}/api/hosts/ws"));
    assert!(
        line(&run(), 7).summary.contains("answered 503"),
        "{:?}",
        line(&run(), 7)
    );

    let old =
        "HTTP/1.1 200 OK\r\nDate: Thu, 01 Oct 2026 00:00:00 GMT\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let skewed = answering(old.into());
    point_at(&host, &format!("ws://{skewed}/api/hosts/ws"));
    let findings = run();
    let eight = line(&findings, 8);
    assert_eq!(eight.status, Status::Fail, "{eight:?}");
    assert!(eight.fix.contains("NTP"), "{eight:?}");

    assert_eq!(
        collector::http_date("Thu, 01 Oct 2026 00:00:00 GMT"),
        Some(1_790_812_800)
    );
    assert_eq!(collector::http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
    assert_eq!(collector::http_date("yesterday"), None);
    assert_eq!(collector::skew(Some(1000), 1020).status, Status::Ok);
    assert_eq!(collector::skew(Some(1000), 1031).status, Status::Warn);
    assert_eq!(collector::skew(Some(1000), 1301).status, Status::Fail);
    assert_eq!(collector::skew(None, 1000).status, Status::Warn);
}

/// Check 7 on a pairing that was interrupted: it warns, and the pairing is
/// left as it is (only its host finishes it, under `host.lock`): nothing
/// in the directory changes, and no hello is sent.
#[test]
fn an_interrupted_pairing_is_left_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let host = dir.path().join("host");
    let staged = dir.path().join("staged");
    paired(&staged);
    std::fs::create_dir_all(&host).unwrap();
    std::fs::copy(staged.join("host.key"), host.join("host.key")).unwrap();
    std::fs::copy(staged.join("host.toml"), host.join("host.toml.pending")).unwrap();
    let dirs = Dirs::by_contents(host.clone(), Found::Given);
    let findings = checked(&cx, dirs, &nothing, &host);
    let seven = line(&findings, 7);
    assert_eq!(seven.status, Status::Warn, "{seven:?}");
    assert!(seven.summary.contains("interrupted"), "{seven:?}");
    assert!(seven.fix.contains("`hennery host run` once"), "{seven:?}");
    assert!(not_run(&findings, 8));
    assert!(host.join("host.toml.pending").exists());
    assert!(!host.join("host.toml").exists());
}

/// Check 16: each listen address answers or is named; `public_url`
/// answering `/healthz` is ok, one that reaches nothing warns.
#[test]
fn every_listener_and_public_url_is_tried() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::none();
    let cx = machine(dir.path(), Platform::Linux, &fake);
    let collector = dir.path().join("collector");
    let (_rt, _state, addr) = live_collector(&collector);
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    std::fs::write(
        collector.join("config.toml"),
        format!("listen = [\"{addr}\", \"{closed}\"]\npublic_url = \"http://{addr}\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(collector.join("config.toml"), std::fs::Permissions::from_mode(0o600)).unwrap();
    let dirs = Dirs::by_contents(collector.clone(), Found::Given);
    let sixteen = line(&checked(&cx, dirs.clone(), &nothing, &collector), 16).clone();
    assert_eq!(sixteen.status, Status::Warn, "{sixteen:?}");
    assert!(sixteen.summary.contains(&format!("{addr} answers")), "{sixteen:?}");
    assert!(
        sixteen.summary.contains(&format!("{closed}: no collector answers")),
        "{sixteen:?}"
    );
    assert!(
        sixteen
            .summary
            .contains(&format!("public_url http://{addr} answers /healthz")),
        "{sixteen:?}"
    );

    std::fs::write(
        collector.join("config.toml"),
        format!("listen = [\"{addr}\"]\npublic_url = \"http://{closed}\"\n"),
    )
    .unwrap();
    let sixteen = line(&checked(&cx, dirs, &nothing, &collector), 16).clone();
    assert_eq!(sixteen.status, Status::Warn, "{sixteen:?}");
    assert!(sixteen.summary.contains("reaches no listener or proxy"), "{sixteen:?}");
}
