//! Plan 4d-B1-i: the host's agents, static and live, against plain shell
//! adapters and CLIs: every outcome of `initialize` (check 3) and of a
//! CLI's login status (check 4), what a probe makes of them, that it keeps
//! to its budget, and that nothing it starts outlives it.

use hennery_host::adapter::{AgentCommand, exit_status};
use hennery_host::availability::{
    self, AgentChecks, LoginCheck, NoChecks, OneProbe, Started, initialize, logged_in, static_agents,
};
use hennery_proto::agents::{AgentAuth, AgentCli, AgentInfo, MAX_AGENTS};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

fn sh(script: &str) -> AgentCommand {
    AgentCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        env: Vec::new(),
    }
}

/// An adapter that reads `initialize` and answers it with `line`, then
/// stays up, as a real one does.
fn answering(line: &str) -> AgentCommand {
    sh(&format!("read line; printf '%s\\n' '{line}'; exec sleep 60"))
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes for existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Poll until `pid` is gone.
async fn wait_dead(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "pid {pid} is still alive");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Poll until `path` holds a pid.
async fn read_pid(path: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|s| s.trim().parse().ok()) {
            return pid;
        }
        assert!(Instant::now() < deadline, "no pid in {}", path.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn agent(name: &str, cli: AgentCli) -> AgentInfo {
    AgentInfo {
        agent: name.into(),
        available: true,
        auth: AgentAuth::Unknown,
        cli,
        adapter_version: None,
        images: None,
        note: None,
    }
}

// Check 3: `initialize`.

#[tokio::test]
async fn an_adapter_that_answers_gives_its_version_and_whether_it_takes_images() {
    let dir = tempfile::tempdir().unwrap();
    let started = initialize(
        &answering(
            r#"{"jsonrpc":"2.0","id":0,"result":{"agentInfo":{"name":"a","version":"1.2.3"},"agentCapabilities":{"promptCapabilities":{"image":true}}}}"#,
        ),
        dir.path(),
        soon(),
    )
    .await;
    assert_eq!(
        started,
        Started::Answered {
            version: Some("1.2.3".into()),
            images: true
        }
    );
}

/// An answer that names neither: no version, no images. Lines that are not
/// its answer (noise, another id) are skipped.
#[tokio::test]
async fn an_answer_without_agent_info_or_prompt_capabilities_offers_no_images() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = sh(
        r#"read line; echo 'starting up'; echo '{"jsonrpc":"2.0","id":7,"result":{}}'; echo '{"jsonrpc":"2.0","id":0,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}'; exec sleep 60"#,
    );
    assert_eq!(
        initialize(&adapter, dir.path(), soon()).await,
        Started::Answered {
            version: None,
            images: false
        }
    );
    let bare = answering(r#"{"jsonrpc":"2.0","id":0,"result":{}}"#);
    assert_eq!(
        initialize(&bare, dir.path(), soon()).await,
        Started::Answered {
            version: None,
            images: false
        }
    );
}

/// What the adapter printed never reaches the reason.
#[tokio::test]
async fn an_error_answer_fails_without_quoting_it() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = answering(r#"{"jsonrpc":"2.0","id":0,"error":{"code":-1,"message":"canary-4d"}}"#);
    let Started::Failed(why) = initialize(&adapter, dir.path(), soon()).await else {
        panic!("an error answer is a failure")
    };
    assert!(why.contains("answered `initialize` with an error"), "{why}");
    assert!(!why.contains("canary"), "{why}");
}

#[tokio::test]
async fn an_adapter_that_exits_without_answering_fails() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = sh("read line; echo 'canary-4d'; exit 3");
    let Started::Failed(why) = initialize(&adapter, dir.path(), soon()).await else {
        panic!("an exit is a failure")
    };
    assert!(why.contains("exited without answering"), "{why}");
    assert!(!why.contains("canary"), "{why}");
}

/// The review's A6: an adapter's output is read up to `MAX_OUTPUT` bytes
/// (1 MiB): an answer after 2 MiB with no newline is never read.
#[tokio::test]
async fn an_answer_after_too_many_bytes_is_never_read() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = sh(
        r#"read line; head -c 2097152 /dev/zero | tr '\0' a; echo; echo '{"jsonrpc":"2.0","id":0,"result":{}}'; exec sleep 60"#,
    );
    let Started::Failed(why) = initialize(&adapter, dir.path(), soon()).await else {
        panic!("an answer past the byte cap is a failure")
    };
    assert!(why.contains("wrote too much"), "{why}");
}

/// The review's A6: and up to `MAX_LINES` lines (1000): an answer after
/// 1001 other lines is never read.
#[tokio::test]
async fn an_answer_after_too_many_lines_is_never_read() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = sh(
        r#"read line; i=0; while [ $i -lt 1001 ]; do echo '{}'; i=$((i+1)); done; echo '{"jsonrpc":"2.0","id":0,"result":{}}'; exec sleep 60"#,
    );
    let Started::Failed(why) = initialize(&adapter, dir.path(), soon()).await else {
        panic!("an answer past the line cap is a failure")
    };
    assert!(why.contains("wrote too much"), "{why}");
}

#[tokio::test]
async fn an_adapter_that_cannot_be_started_fails() {
    let dir = tempfile::tempdir().unwrap();
    let missing = AgentCommand {
        program: "/nonexistent/adapter".into(),
        args: Vec::new(),
        env: Vec::new(),
    };
    let Started::Failed(why) = initialize(&missing, dir.path(), soon()).await else {
        panic!("a missing program is a failure")
    };
    assert!(why.contains("cannot be started"), "{why}");
}

/// A silent adapter fails at the deadline, and its whole group (a
/// grandchild included) is gone afterwards.
#[tokio::test]
async fn a_silent_adapter_fails_at_the_deadline_and_leaves_nothing_running() {
    let dir = tempfile::tempdir().unwrap();
    let grandchild = dir.path().join("grandchild");
    let adapter = sh(&format!(
        "sleep 600 & echo $! > {}; read line; exec sleep 600",
        grandchild.display()
    ));
    let began = Instant::now();
    let started = initialize(&adapter, dir.path(), Instant::now() + Duration::from_millis(500)).await;
    let Started::Failed(why) = started else {
        panic!("a silence is a failure")
    };
    assert!(why.contains("did not answer `initialize` in the probe's time"), "{why}");
    assert!(why.contains("macOS"), "{why}");
    assert!(began.elapsed() < Duration::from_secs(5), "{:?}", began.elapsed());
    wait_dead(read_pid(&grandchild).await).await;
}

/// An answered adapter is killed too: nothing a probe starts stays.
#[tokio::test]
async fn an_answered_adapter_is_killed_with_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let grandchild = dir.path().join("grandchild");
    let adapter = sh(&format!(
        r#"sleep 600 & echo $! > {}; read line; echo '{{"jsonrpc":"2.0","id":0,"result":{{}}}}'; exec sleep 600"#,
        grandchild.display()
    ));
    assert!(matches!(
        initialize(&adapter, dir.path(), soon()).await,
        Started::Answered { .. }
    ));
    wait_dead(read_pid(&grandchild).await).await;
}

/// Hazard (d): the adapter runs as a session's does, in the directory it
/// is given, with its own variables, and without what the host strips
/// even when the agent's own configuration names it.
#[tokio::test]
async fn an_adapter_is_started_in_its_directory_with_the_hosts_stripped_environment() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen");
    let mut adapter = sh(&format!(
        r#"{{ pwd; env; }} > {}; read line; echo '{{"jsonrpc":"2.0","id":0,"result":{{}}}}'; exec sleep 60"#,
        seen.display()
    ));
    adapter.env = vec![
        ("AGENT_OWN".into(), "kept".into()),
        ("HENNERY_DEV_TOKEN".into(), "secret".into()),
        ("CLAUDECODE".into(), "1".into()),
    ];
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    assert!(matches!(
        initialize(&adapter, &cwd, soon()).await,
        Started::Answered { .. }
    ));
    let text = std::fs::read_to_string(&seen).unwrap();
    assert_eq!(text.lines().next(), Some(cwd.to_str().unwrap()), "{text}");
    assert!(text.contains("AGENT_OWN=kept"), "{text}");
    assert!(!text.contains("HENNERY_DEV_TOKEN"), "{text}");
    assert!(!text.contains("CLAUDECODE"), "{text}");
}

// Check 4: a CLI's login status, by its exit status alone.

#[tokio::test]
async fn a_status_exit_of_zero_is_logged_in_and_any_other_exit_code_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let ask = |script: &str| logged_in(LoginCheck::Ask(sh(script)), dir.path(), soon());
    assert_eq!(ask("echo 'you@example.com'; exit 0").await, (AgentAuth::Ok, None));
    assert_eq!(ask("echo 'canary-4d'; exit 1").await, (AgentAuth::Missing, None));
    assert_eq!(ask("exit 2").await, (AgentAuth::Missing, None));
}

/// The review's O1: a CLI ended by a signal said nothing (Gatekeeper or a
/// keychain prompt can end a logged-in CLI on macOS): `unknown`, with why.
#[tokio::test]
async fn a_cli_ended_by_a_signal_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let (auth, note) = logged_in(LoginCheck::Ask(sh("kill -9 $$")), dir.path(), soon()).await;
    assert_eq!(auth, AgentAuth::Unknown);
    assert!(note.unwrap().contains("ended by a signal"));
}

/// Hazard (d), the review's A4: a CLI runs as an adapter does, in the
/// directory it is given, with its own variables, and without what the
/// host strips even when its own command names it.
#[tokio::test]
async fn a_cli_runs_in_its_directory_with_the_hosts_stripped_environment() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen");
    let mut cli = sh(&format!("{{ pwd; env; }} > {}; exit 0", seen.display()));
    cli.env = vec![
        ("AGENT_OWN".into(), "kept".into()),
        ("HENNERY_DEV_TOKEN".into(), "secret".into()),
        ("CLAUDECODE".into(), "1".into()),
    ];
    let cwd = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(
        logged_in(LoginCheck::Ask(cli), &cwd, soon()).await,
        (AgentAuth::Ok, None)
    );
    let text = std::fs::read_to_string(&seen).unwrap();
    assert_eq!(text.lines().next(), Some(cwd.to_str().unwrap()), "{text}");
    assert!(text.contains("AGENT_OWN=kept"), "{text}");
    assert!(!text.contains("HENNERY_DEV_TOKEN"), "{text}");
    assert!(!text.contains("CLAUDECODE"), "{text}");
}

#[tokio::test]
async fn a_cli_not_asked_is_unknown_with_its_note() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        logged_in(LoginCheck::NotAsked(None), dir.path(), soon()).await,
        (AgentAuth::Unknown, None)
    );
    assert_eq!(
        logged_in(LoginCheck::NotAsked(Some("why".into())), dir.path(), soon()).await,
        (AgentAuth::Unknown, Some("why".into()))
    );
}

#[tokio::test]
async fn a_silent_cli_is_unknown_at_the_deadline_and_leaves_nothing_running() {
    let dir = tempfile::tempdir().unwrap();
    let grandchild = dir.path().join("grandchild");
    let cli = sh(&format!(
        "sleep 600 & echo $! > {}; exec sleep 600",
        grandchild.display()
    ));
    let began = Instant::now();
    let (auth, note) = logged_in(
        LoginCheck::Ask(cli),
        dir.path(),
        Instant::now() + Duration::from_millis(500),
    )
    .await;
    assert_eq!(auth, AgentAuth::Unknown);
    let note = note.unwrap();
    assert!(note.contains("did not say whether it is logged in"), "{note}");
    assert!(began.elapsed() < Duration::from_secs(5), "{:?}", began.elapsed());
    wait_dead(read_pid(&grandchild).await).await;
}

#[tokio::test]
async fn a_cli_that_cannot_be_started_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let missing = AgentCommand {
        program: "/nonexistent/claude".into(),
        args: vec!["auth".into(), "status".into()],
        env: Vec::new(),
    };
    let (auth, note) = logged_in(LoginCheck::Ask(missing), dir.path(), soon()).await;
    assert_eq!(auth, AgentAuth::Unknown);
    assert!(note.unwrap().contains("cannot be started"));
}

/// Hazard (c): a CLI's output goes nowhere. One that writes far more than a
/// pipe holds still ends, and ends as it chose: it was never blocked, and
/// never killed by a closed pipe.
#[tokio::test]
async fn a_cli_writes_to_nowhere_and_is_never_blocked_by_its_output() {
    let dir = tempfile::tempdir().unwrap();
    let loud = sh("head -c 1048576 /dev/zero; head -c 1048576 /dev/zero >&2; exit 0");
    let ended = exit_status(&loud, dir.path(), Duration::from_secs(10)).await.unwrap();
    assert_eq!(ended.map(|e| e.code), Some(Some(0)));
    // Its standard input is empty: a CLI that waits on it ends at once.
    let reads = sh("cat; exit 4");
    let ended = exit_status(&reads, dir.path(), Duration::from_secs(10)).await.unwrap();
    assert_eq!(ended.map(|e| e.code), Some(Some(4)));
}

/// Hazard (b): a CLI runs in a group of its own that it does not lead: the
/// host's guard leads it (`Adapter::spawn`'s group).
#[tokio::test]
async fn a_cli_runs_in_a_guarded_group_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let ids = dir.path().join("ids");
    let cli = sh(&format!("echo $$ $(ps -o pgid= -p $$) > {}; exit 0", ids.display()));
    exit_status(&cli, dir.path(), Duration::from_secs(10)).await.unwrap();
    let text = std::fs::read_to_string(&ids).unwrap();
    let mut parts = text.split_whitespace().map(|p| p.parse::<i32>().unwrap());
    let (pid, pgid) = (parts.next().unwrap(), parts.next().unwrap());
    assert_ne!(pid, pgid, "the CLI leads its own group: no guard");
    // SAFETY: getpgrp(2) has no failure mode.
    assert_ne!(pgid, unsafe { libc::getpgrp() }, "the CLI is in the test's group");
}

// The probe: every agent at once, within one budget.

#[derive(Debug)]
struct Logins(HashMap<String, LoginCheck>);

impl AgentChecks for Logins {
    fn login(&self, agent: &str) -> LoginCheck {
        self.0.get(agent).cloned().unwrap_or(LoginCheck::NotAsked(None))
    }
}

#[tokio::test]
async fn a_probe_reports_each_agent_live() {
    let dir = tempfile::tempdir().unwrap();
    let mut agents = HashMap::new();
    agents.insert(
        "claude".to_string(),
        answering(
            r#"{"jsonrpc":"2.0","id":0,"result":{"agentInfo":{"version":"2.0.0"},"agentCapabilities":{"promptCapabilities":{"image":true}}}}"#,
        ),
    );
    agents.insert(
        "codex".to_string(),
        answering(r#"{"jsonrpc":"2.0","id":0,"result":{"agentInfo":{"version":"see https://x"}}}"#),
    );
    agents.insert("broken".to_string(), sh("read line; exit 1"));
    let report = vec![
        AgentInfo {
            adapter_version: Some("9.9.9".into()),
            note: Some("a static note".into()),
            ..agent("broken", AgentCli::Bundled)
        },
        AgentInfo {
            adapter_version: Some("1.0.0".into()),
            ..agent("claude", AgentCli::Bundled)
        },
        AgentInfo {
            adapter_version: Some("1.0.0".into()),
            ..agent("codex", AgentCli::Override)
        },
        // Not launchable as configured: nothing is started for it.
        AgentInfo {
            available: false,
            note: Some("left out".into()),
            ..agent("left-out", AgentCli::Bundled)
        },
    ];
    let logins = Logins(HashMap::from([
        ("claude".to_string(), LoginCheck::Ask(sh("exit 0"))),
        ("codex".to_string(), LoginCheck::Ask(sh("exit 1"))),
        ("broken".to_string(), LoginCheck::NotAsked(Some("not asked".into()))),
    ]));
    let live = availability::probe(&report, &agents, Arc::new(logins), dir.path(), Duration::from_secs(10)).await;
    let by_name: HashMap<&str, &AgentInfo> = live.iter().map(|a| (a.agent.as_str(), a)).collect();
    assert_eq!(live.len(), 4);
    let claude = by_name["claude"];
    assert!(claude.available);
    assert_eq!(claude.auth, AgentAuth::Ok);
    assert_eq!(claude.adapter_version.as_deref(), Some("2.0.0"));
    assert_eq!(claude.images, Some(true));
    assert_eq!(claude.note, None);
    // An unreadable version: the set's stands.
    let codex = by_name["codex"];
    assert!(codex.available);
    assert_eq!((codex.auth, codex.cli), (AgentAuth::Missing, AgentCli::Override));
    assert_eq!(codex.adapter_version.as_deref(), Some("1.0.0"));
    assert_eq!(codex.images, Some(false));
    // Both notes, the static one replaced.
    let broken = by_name["broken"];
    assert!(!broken.available);
    assert_eq!(broken.images, None);
    let note = broken.note.as_deref().unwrap();
    assert!(
        note.contains("exited without answering") && note.ends_with("; not asked"),
        "{note}"
    );
    assert_eq!(*by_name["left-out"], report[3]);
}

/// Every check runs at once: two silent agents, each with a silent CLI,
/// cost one budget, not four.
#[tokio::test]
async fn a_probe_keeps_to_one_budget_however_many_checks_hang() {
    let dir = tempfile::tempdir().unwrap();
    let silent = sh("read line; exec sleep 600");
    let agents = HashMap::from([("a".to_string(), silent.clone()), ("b".to_string(), silent)]);
    let report = vec![agent("a", AgentCli::Given), agent("b", AgentCli::Given)];
    let logins = Logins(HashMap::from([
        ("a".to_string(), LoginCheck::Ask(sh("exec sleep 600"))),
        ("b".to_string(), LoginCheck::Ask(sh("exec sleep 600"))),
    ]));
    let budget = Duration::from_secs(1);
    let began = Instant::now();
    let live = availability::probe(&report, &agents, Arc::new(logins), dir.path(), budget).await;
    let took = began.elapsed();
    assert!(took >= budget && took < 3 * budget, "{took:?}");
    for agent in &live {
        assert!(!agent.available);
        assert_eq!(agent.auth, AgentAuth::Unknown);
    }
}

/// A host given no checks asks no CLI.
#[tokio::test]
async fn without_checks_no_cli_is_asked() {
    let dir = tempfile::tempdir().unwrap();
    let agents = HashMap::from([("fake".to_string(), answering(r#"{"jsonrpc":"2.0","id":0,"result":{}}"#))]);
    let live = availability::probe(
        &[agent("fake", AgentCli::Given)],
        &agents,
        Arc::new(NoChecks),
        dir.path(),
        Duration::from_secs(10),
    )
    .await;
    assert_eq!((live[0].available, live[0].auth), (true, AgentAuth::Unknown));
}

// The static view.

#[test]
fn the_static_view_adds_every_given_agent_and_keeps_the_runtimes() {
    let agents = HashMap::from([("fake".to_string(), sh("true")), ("claude".to_string(), sh("true"))]);
    let infos = vec![
        AgentInfo {
            adapter_version: Some("1.0.0".into()),
            ..agent("claude", AgentCli::Bundled)
        },
        AgentInfo {
            available: false,
            note: Some("unavailable".into()),
            ..agent("codex", AgentCli::Bundled)
        },
    ];
    let view = static_agents(&agents, &infos);
    assert_eq!(
        view,
        [infos[0].clone(), infos[1].clone(), agent("fake", AgentCli::Given)]
    );
    // Bounded like any report.
    let many: HashMap<String, AgentCommand> = (0..MAX_AGENTS + 3).map(|i| (format!("a{i:02}"), sh("true"))).collect();
    assert_eq!(static_agents(&many, &[]).len(), MAX_AGENTS);
}

#[test]
fn one_probe_at_a_time_until_the_first_ends() {
    let one = OneProbe::default();
    let first = one.try_start().expect("free");
    assert!(one.try_start().is_none(), "a second probe while the first runs");
    // Shared by every clone: the host keeps one across its connections.
    assert!(one.clone().try_start().is_none());
    drop(first);
    assert!(one.try_start().is_some(), "free again once the first ended");
}
