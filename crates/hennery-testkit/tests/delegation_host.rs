use hennery_host::{
    AgentCommand,
    delegation::{CatalogProbes, checked_cwd, security_classification},
    outbox::Outbox,
    profile::Profile,
    session::{self, Attach, Launch, SessionCmd, SessionOptions},
    uplink::Uplink,
};
use hennery_proto::{
    delegation_host::{ConfigCompleteness, PromptFence},
    frames::{ConfigValue, HostFrame, SessionBody, SessionConfig},
};
use hennery_testkit::{FakeScript, SCRIPT_ENV, sample_config_options};
use serde_json::json;
use std::{path::Path, time::Duration};

fn fake(script: FakeScript, root: &Path) -> AgentCommand {
    let program = env!("CARGO_BIN_EXE_hennery-fake-acp");
    hennery_testkit::first_exec(program);
    AgentCommand {
        program: program.into(),
        args: vec![],
        env: vec![
            (SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()),
            ("HOME".into(), root.display().to_string()),
            ("XDG_CONFIG_HOME".into(), root.display().to_string()),
            ("XDG_DATA_HOME".into(), root.display().to_string()),
            ("XDG_CACHE_HOME".into(), root.display().to_string()),
            ("SSL_CERT_FILE".into(), String::new()),
            ("SSL_CERT_DIR".into(), String::new()),
        ],
    }
}
fn script() -> FakeScript {
    FakeScript {
        config_options: sample_config_options(),
        ..Default::default()
    }
}
async fn until(up: &Uplink, pred: impl Fn(&[HostFrame]) -> bool) -> Vec<HostFrame> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let f = up.pending().unwrap();
            if pred(&f) {
                return f;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("host fact")
}
fn evidence(f: &[HostFrame]) -> hennery_proto::delegation_host::ConfigEvidence {
    f.iter()
        .rev()
        .find_map(|f| match f {
            HostFrame::Session {
                body:
                    SessionBody::SessionStarted { indexed, .. }
                    | SessionBody::ConfigApplied { indexed, .. }
                    | SessionBody::AcpUpdate { indexed, .. },
                ..
            } => indexed.config_evidence.as_deref().cloned(),
            _ => None,
        })
        .expect("generation-bound evidence")
}
fn launch(root: &Path, script: FakeScript, attach: Attach) -> Launch {
    Launch {
        mcp_source_policy: None,
        request_id: "start".into(),
        session_id: "s".into(),
        attach,
        config: SessionConfig::default(),
        agent: fake(script, root),
        cwd: root.canonicalize().unwrap(),
        profile: Profile::Generic,
        mcp_servers: vec![],
    }
}
fn prompt(expected: PromptFence, request: &str) -> SessionCmd {
    SessionCmd::FencedPrompt {
        request_id: request.into(),
        turn_id: request.into(),
        content: vec![json!({"type":"text","text":"brief"})],
        expected,
    }
}
async fn stop(handle: session::SessionHandle) {
    handle.send(SessionCmd::Close {
        request_id: "close".into(),
    });
    handle.finished().await;
}

#[tokio::test]
async fn unauthenticated_runtime_catalog_reports_auth_required() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let mut s = script();
    s.new_session_error = Some(-32000);
    let error = CatalogProbes::new(1, Duration::from_secs(10))
        .catalog("signed-out", fake(s, &cwd), Profile::Codex, cwd.clone(), vec![cwd])
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "auth_required");
}

#[tokio::test]
async fn unauthenticated_delegated_attach_preserves_session_login_failure_without_a_brief() {
    for attach in [
        Attach::New,
        Attach::Load {
            agent_session_id: "existing".into(),
        },
    ] {
        let root = tempfile::tempdir().unwrap();
        let (up, _) = Uplink::new(Outbox::open_in_memory().unwrap());
        let mut s = script();
        s.new_session_error = Some(-32000);
        s.load_error = Some(-32000);
        let handle =
            session::launch_delegated(up.clone(), launch(root.path(), s, attach), SessionOptions::default(), 1);
        let facts = until(&up, |facts| !facts.is_empty()).await;
        handle.finished().await;
        assert!(
            matches!(facts.as_slice(), [HostFrame::Session { body: SessionBody::StartFailed { code, message, .. }, .. }] if code == "agent_not_logged_in" && message == &agent_client_protocol::Error::auth_required().to_string()),
            "{facts:?}"
        );
    }
}

#[tokio::test]
async fn operator_parent_reattached_for_a_notice_preserves_its_login_failure() {
    let root = tempfile::tempdir().unwrap();
    let (up, _) = Uplink::new(Outbox::open_in_memory().unwrap());
    let mut s = script();
    s.load_error = Some(-32000);
    // Notice delivery resumes the operator's parent through the same
    // generation-bound attach as children. A generation is not a child marker.
    let handle = session::launch_delegated(
        up.clone(),
        launch(
            root.path(),
            s,
            Attach::Load {
                agent_session_id: "operator-parent".into(),
            },
        ),
        SessionOptions::default(),
        3,
    );
    let facts = until(&up, |facts| !facts.is_empty()).await;
    handle.finished().await;
    let [
        HostFrame::Session {
            body: SessionBody::StartFailed { code, message, .. },
            ..
        },
    ] = facts.as_slice()
    else {
        panic!("{facts:?}");
    };
    assert_eq!(code, "agent_not_logged_in");
    assert_eq!(message, &agent_client_protocol::Error::auth_required().to_string());
}

#[tokio::test]
async fn ordinary_parent_attach_produces_evidence_and_accepts_a_fenced_report() {
    let root = tempfile::tempdir().unwrap();
    let (up, _) = Uplink::new(Outbox::open_in_memory().unwrap());
    let h = session::launch(
        up.clone(),
        launch(root.path(), script(), Attach::New),
        SessionOptions::default(),
    );
    let f = until(&up, |f| !f.is_empty()).await;
    let ev = evidence(&f);
    assert_eq!(ev.attach_generation, 0);
    assert_eq!(ev.completeness, ConfigCompleteness::Complete);
    h.send(prompt(
        PromptFence {
            attach_generation: 0,
            config_revision: ev.config_revision,
        },
        "report",
    ));
    until(&up, |f| {
        f.iter()
            .any(|f| matches!(f,HostFrame::Session {body:SessionBody::TurnEnded {turn_id,..},..} if turn_id=="report"))
    })
    .await;
    stop(h).await;
}

#[tokio::test]
async fn authoritative_delegated_configuration_failure_ends_before_attach_or_brief() {
    for refused_switch in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (up, _) = Uplink::new(Outbox::open_in_memory().unwrap());
        let mut s = script();
        let mut wanted = SessionConfig::default();
        if refused_switch {
            s.config_error = Some("requested model unavailable".into());
            wanted.model = Some("large".into());
        } else {
            s.config_options.retain(|option| option["id"] != "fast");
            wanted.axes.insert("fast".into(), ConfigValue::Bool(false));
        }
        let mut start = launch(root.path(), s, Attach::New);
        start.config = wanted;
        let h = session::launch_delegated(up.clone(), start, SessionOptions::default(), 1);
        let facts = until(&up, |facts| !facts.is_empty()).await;
        stop(h).await;
        assert!(facts.iter().any(|fact|matches!(fact,HostFrame::Session { body:SessionBody::StartFailed {code,message,..},.. } if code=="configuration_unavailable" && message.contains(if refused_switch {"requested model unavailable"} else {"fast"}))),"unachievable delegated configuration must fail promptly: {facts:?}");
        assert!(
            !facts.iter().any(|fact| matches!(
                fact,
                HostFrame::Session {
                    body: SessionBody::SessionStarted { .. } | SessionBody::TurnStarted { .. },
                    ..
                }
            )),
            "never announce an attached child or dispatch its brief after configuration refusal"
        );
    }
}

#[test]
fn workspace_fence_rejects_outside_and_symlink_swap() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    assert_eq!(
        checked_cwd(cwd.to_str().unwrap(), std::slice::from_ref(&cwd)).unwrap(),
        cwd
    );
    assert!(
        checked_cwd(
            outside.path().canonicalize().unwrap().to_str().unwrap(),
            std::slice::from_ref(&cwd)
        )
        .is_err()
    );
    assert!(checked_cwd(cwd.to_str().unwrap(), &[]).is_err());
    std::os::unix::fs::symlink(outside.path(), cwd.join("swapped")).unwrap();
    assert!(checked_cwd(cwd.join("swapped").to_str().unwrap(), &[cwd]).is_err());
    let cwd = root.path().canonicalize().unwrap();
    std::fs::create_dir(cwd.join("inside")).unwrap();
    std::os::unix::fs::symlink(cwd.join("inside"), cwd.join("alias")).unwrap();
    assert!(checked_cwd(cwd.join("alias").to_str().unwrap(), std::slice::from_ref(&cwd)).is_err());
}
#[test]
fn security_baselines_are_profile_bound_metadata() {
    let safe = security_classification(Profile::Claude);
    assert!(safe.known);
    assert!(safe.axes["mode"].contains(&ConfigValue::Id("default".into())));
    assert!(!safe.axes["mode"].contains(&ConfigValue::Id("bypassPermissions".into())));
    assert!(!security_classification(Profile::Generic).known);
    assert!(!security_classification(Profile::ClaudeOwnCli).known);
    let codex = security_classification(Profile::Codex);
    assert!(codex.known);
    assert_eq!(codex.axes["mode"], vec![ConfigValue::Id("read-only".into())]);
}
#[tokio::test]
async fn cold_catalog_uses_selected_adapter_without_prompt_or_mcp() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let log = cwd.join("sessions");
    let mut s = script();
    s.session_log = Some(log.display().to_string());
    s.prompt_error = Some("no prompt allowed".into());
    let probes = CatalogProbes::new(1, Duration::from_secs(10));
    let result = probes
        .catalog("chosen", fake(s, &cwd), Profile::Claude, cwd.clone(), vec![cwd.clone()])
        .await
        .unwrap();
    assert_eq!(result.agent, "chosen");
    assert_eq!(result.defaults.model.as_deref(), Some("small"));
    assert!(!result.adapter_revision.is_empty());
    assert!(result.security.known);
    assert!(serde_json::to_value(&result).unwrap().get("profile").is_none());
    assert_eq!(
        result.freshness,
        hennery_proto::delegation_host::CatalogFreshness::Fresh
    );
    let requests = std::fs::read_to_string(log).unwrap();
    let first: serde_json::Value = serde_json::from_str(requests.lines().next().unwrap()).unwrap();
    assert_eq!(first["params"]["mcpServers"], json!([]));
    assert_eq!(requests.lines().count(), 1);
}

async fn wait_process_gone(pid: i32, group: bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            // SAFETY: signal zero only checks the process/group created by this test.
            let alive = unsafe { libc::kill(if group { -pid } else { pid }, 0) == 0 };
            if !alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("probe process subtree must be gone");
}

struct ProbeCleanup(std::path::PathBuf);
impl Drop for ProbeCleanup {
    fn drop(&mut self) {
        // A failed removal probe must not strand its intentionally surviving child.
        for (path, group) in [(&self.0, false), (&self.0.with_extension("group"), true)] {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|raw| raw.trim().parse::<i32>().ok())
            {
                // SAFETY: these files name only processes created by this test.
                unsafe {
                    libc::kill(if group { -pid } else { pid }, libc::SIGKILL);
                }
            }
        }
    }
}

#[tokio::test]
async fn successful_catalog_probe_terminates_the_whole_process_group() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let pid_file = cwd.join("grandchild.pid");
    let mut cleanup = ProbeCleanup(pid_file.clone());
    let mut s = script();
    s.grandchild_pid_file = Some(pid_file.display().to_string());
    let probes = CatalogProbes::new(1, Duration::from_secs(10));
    probes
        .catalog(
            "chosen",
            fake(s, &cwd),
            Profile::Generic,
            cwd.clone(),
            vec![cwd.clone()],
        )
        .await
        .unwrap();
    let child = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    let group = std::fs::read_to_string(pid_file.with_extension("group"))
        .unwrap()
        .parse()
        .unwrap();
    wait_process_gone(child, false).await;
    wait_process_gone(group, true).await;
    cleanup.0.clear();
    probes
        .catalog("again", fake(script(), &cwd), Profile::Generic, cwd.clone(), vec![cwd])
        .await
        .unwrap();
}

#[tokio::test]
async fn setsid_descendant_escapes_the_probe_group_and_requires_explicit_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let pid_file = cwd.join("escaped.pid");
    let cleanup = ProbeCleanup(pid_file.clone());
    let mut s = script();
    s.grandchild_pid_file = Some(pid_file.display().to_string());
    s.grandchild_detached = true;
    CatalogProbes::new(1, Duration::from_secs(10))
        .catalog("chosen", fake(s, &cwd), Profile::Generic, cwd.clone(), vec![cwd])
        .await
        .unwrap();
    let child: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    assert!(
        hennery_testkit::pid_alive(child),
        "setsid is outside process-group containment"
    );
    drop(cleanup);
    wait_process_gone(child, false).await;
}

#[tokio::test]
async fn operator_catalog_probe_does_not_consume_the_following_sessions_mcp_delivery() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let log = cwd.join("sessions");
    let mut script = script();
    script.session_log = Some(log.display().to_string());
    let probes = CatalogProbes::default();
    probes
        .operator_catalog("chosen", fake(script.clone(), &cwd), Profile::Generic, cwd.clone())
        .await
        .unwrap();
    let mut servers = vec![
        json!({"type":"http","name":"hennery-docs","url":"https://example.com/mcp/docs","headers":[]}),
        json!({"type":"stdio","name":"hennery-local","command":"tool","args":[],"env":[]}),
    ];
    let mut start = launch(&cwd, script, Attach::New);
    start.mcp_servers = serde_json::from_value(json!(servers)).unwrap();
    let (up, _) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::launch(up.clone(), start, SessionOptions::default());
    until(&up, |frames| {
        frames.iter().any(|frame| {
            matches!(
                frame,
                HostFrame::Session {
                    body: SessionBody::SessionStarted { .. },
                    ..
                }
            )
        })
    })
    .await;
    stop(handle).await;
    let receipts: Vec<serde_json::Value> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0]["method"], "session/new");
    assert_eq!(receipts[0]["params"]["mcpServers"], json!([]));
    assert_eq!(receipts[1]["method"], "session/new");
    // ACP stdio entries are untagged; the host frame is explicitly tagged.
    servers[1].as_object_mut().unwrap().remove("type");
    assert_eq!(receipts[1]["params"]["mcpServers"], json!(servers));
}
#[tokio::test]
async fn catalog_timeout_releases_permit_and_leaves_no_adapter() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let probes = CatalogProbes::new(1, Duration::from_secs(2));
    let pid_file = cwd.join("probe.pid");
    let hanging = AgentCommand {
        program: "sh".into(),
        args: vec!["-c".into(), "echo $$ > \"$PROBE_PID_FILE\"; exec sleep 100".into()],
        env: vec![("PROBE_PID_FILE".into(), pid_file.display().to_string())],
    };
    assert_eq!(
        probes
            .catalog("hang", hanging, Profile::Generic, cwd.clone(), vec![cwd.clone()])
            .await
            .unwrap_err()
            .to_string(),
        "timeout"
    );
    let pid: i32 = std::fs::read_to_string(pid_file).unwrap().trim().parse().unwrap();
    assert!(!hennery_testkit::pid_alive(pid), "probe process survived timeout");
    assert!(
        probes
            .catalog("fine", fake(script(), &cwd), Profile::Generic, cwd.clone(), vec![cwd])
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn fresh_attach_evidence_admits_matching_prompt_and_rejects_stale_generation() {
    let root = tempfile::tempdir().unwrap();
    let (up, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let h = session::launch_delegated(
        up.clone(),
        launch(root.path(), script(), Attach::New),
        SessionOptions::default(),
        9,
    );
    let f = until(&up, |f| !f.is_empty()).await;
    let ev = evidence(&f);
    assert_eq!(ev.attach_generation, 9);
    assert_eq!(ev.completeness, ConfigCompleteness::Complete);
    h.send(prompt(
        PromptFence {
            attach_generation: 8,
            config_revision: ev.config_revision,
        },
        "stale",
    ));
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(5),replies.recv()).await.unwrap().unwrap(),HostFrame::Error{code,..} if code=="config_fence")
    );
    h.send(prompt(
        PromptFence {
            attach_generation: 9,
            config_revision: ev.config_revision,
        },
        "fresh",
    ));
    until(&up, |f| {
        f.iter().any(|f| {
            matches!(
                f,
                HostFrame::Session {
                    body: SessionBody::TurnEnded { .. },
                    ..
                }
            )
        })
    })
    .await;
    stop(h).await;
}
#[tokio::test]
async fn legacy_mode_without_catalogue_axis_preserves_evidence_and_followups() {
    let root = tempfile::tempdir().unwrap();
    let (up, _) = Uplink::new(Outbox::open_in_memory().unwrap());
    let mut s = script();
    s.config_options.retain(|option| option["id"] != "mode");
    s.prompt_updates = vec![json!({"sessionUpdate":"current_mode_update","currentModeId":"legacy"})];
    let h = session::launch_delegated(
        up.clone(),
        launch(root.path(), s, Attach::New),
        SessionOptions::default(),
        9,
    );
    let initial = evidence(&until(&up, |f| !f.is_empty()).await);
    assert_eq!(initial.completeness, ConfigCompleteness::Complete);
    assert_eq!(initial.actual.mode, None);
    for request in ["first", "followup"] {
        h.send(prompt(
            PromptFence {
                attach_generation: initial.attach_generation,
                config_revision: initial.config_revision,
            },
            request,
        ));
        let frames = until(&up, |frames| {
            frames.iter().any(|f| {
                matches!(f,
                    HostFrame::Session {body: SessionBody::TurnEnded {turn_id,..},..} if turn_id == request
                )
            })
        })
        .await;
        assert_eq!(
            evidence(&frames),
            initial,
            "unsupported mode delta changed the configuration proof"
        );
    }
    stop(h).await;
}

#[tokio::test]
async fn no_resume_readback_is_not_old_configuration_evidence() {
    let root = tempfile::tempdir().unwrap();
    let (up, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let mut s = script();
    s.config_in_update_only = true;
    let h = session::launch_delegated(
        up.clone(),
        launch(
            root.path(),
            s,
            Attach::Load {
                agent_session_id: "old".into(),
            },
        ),
        SessionOptions::default(),
        12,
    );
    let ev = evidence(&until(&up, |f| !f.is_empty()).await);
    assert_eq!(ev.completeness, ConfigCompleteness::Partial);
    h.send(prompt(
        PromptFence {
            attach_generation: 12,
            config_revision: ev.config_revision,
        },
        "held",
    ));
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(5),replies.recv()).await.unwrap().unwrap(),HostFrame::Error{code,..} if code=="config_fence")
    );
    stop(h).await;
}
#[tokio::test]
async fn switch_in_flight_fences_prompt_then_new_readback_repairs() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("answer");
    let log = root.path().join("switches");
    let mut s = script();
    s.model_switch_answer_on_file = Some(marker.display().to_string());
    s.config_log = Some(log.display().to_string());
    let (up, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let h = session::launch_delegated(
        up.clone(),
        launch(root.path(), s, Attach::New),
        SessionOptions::default(),
        2,
    );
    let ev = evidence(&until(&up, |f| !f.is_empty()).await);
    h.send(SessionCmd::SetConfig {
        request_id: "switch".into(),
        config_id: "model".into(),
        value: ConfigValue::Id("large".into()),
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !log.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .unwrap();
    h.send(prompt(
        PromptFence {
            attach_generation: 2,
            config_revision: ev.config_revision,
        },
        "old",
    ));
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(5),replies.recv()).await.unwrap().unwrap(),HostFrame::Error{code,..} if code=="config_fence")
    );
    std::fs::write(marker, "").unwrap();
    let f = until(&up, |f| {
        f.iter().any(|f| {
            matches!(
                f,
                HostFrame::Session {
                    body: SessionBody::ConfigApplied { .. },
                    ..
                }
            )
        })
    })
    .await;
    let new = evidence(&f);
    assert!(new.config_revision > ev.config_revision);
    assert_eq!(new.actual.model.as_deref(), Some("large"));
    h.send(prompt(
        PromptFence {
            attach_generation: 2,
            config_revision: new.config_revision,
        },
        "repaired",
    ));
    until(&up, |f| {
        f.iter().any(|f| {
            matches!(
                f,
                HostFrame::Session {
                    body: SessionBody::TurnEnded { .. },
                    ..
                }
            )
        })
    })
    .await;
    stop(h).await;
}
#[tokio::test]
async fn failed_or_empty_reapply_never_produces_complete_evidence() {
    for empty in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (up, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
        let mut s = script();
        s.empty_config_read_back = empty;
        if !empty {
            s.config_in_update_only = true;
            s.config_error = Some("cannot reapply".into());
        }
        let mut launch = launch(
            root.path(),
            s,
            Attach::Load {
                agent_session_id: "old".into(),
            },
        );
        launch.config.model = Some("large".into());
        let h = session::launch_delegated(up.clone(), launch, SessionOptions::default(), 4);
        let ev = evidence(&until(&up, |f| !f.is_empty()).await);
        assert_eq!(ev.completeness, ConfigCompleteness::Partial);
        assert!(ev.actual.is_empty());
        h.send(prompt(
            PromptFence {
                attach_generation: 4,
                config_revision: ev.config_revision,
            },
            "held",
        ));
        assert!(
            matches!(tokio::time::timeout(Duration::from_secs(5),replies.recv()).await.unwrap().unwrap(),HostFrame::Error{code,..} if code=="config_fence")
        );
        stop(h).await;
    }
}
#[tokio::test]
async fn independent_config_notification_advances_fence() {
    let root = tempfile::tempdir().unwrap();
    let (up, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let mut s = script();
    s.prompt_sets_mode = Some("bypass".into());
    let h = session::launch_delegated(
        up.clone(),
        launch(root.path(), s, Attach::New),
        SessionOptions::default(),
        7,
    );
    let ev = evidence(&until(&up, |f| !f.is_empty()).await);
    let old = PromptFence {
        attach_generation: 7,
        config_revision: ev.config_revision,
    };
    h.send(prompt(old.clone(), "first"));
    let f = until(&up, |f| {
        f.iter().any(|f| {
            matches!(
                f,
                HostFrame::Session {
                    body: SessionBody::TurnEnded { .. },
                    ..
                }
            )
        })
    })
    .await;
    let current = evidence(&f);
    assert!(current.config_revision > ev.config_revision);
    assert_eq!(current.actual.mode.as_deref(), Some("bypass"));
    h.send(prompt(old, "late"));
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(5),replies.recv()).await.unwrap().unwrap(),HostFrame::Error{code,..} if code=="config_fence")
    );
    stop(h).await;
}

#[tokio::test]
async fn delegated_actor_rechecks_roots_immediately_before_spawn() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let (up, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let h = session::launch_delegated_in(
        up.clone(),
        launch(outside.path(), script(), Attach::New),
        SessionOptions::default(),
        1,
        vec![root.path().canonicalize().unwrap()],
    );
    let f = until(&up, |f| !f.is_empty()).await;
    assert!(matches!(&f[0],HostFrame::Session{body:SessionBody::StartFailed{code,..},..} if code=="cwd_unavailable"));
    h.finished().await;
}

#[tokio::test]
async fn host_wire_routes_catalog_attach_and_fenced_prompt() {
    use futures::{SinkExt, StreamExt};
    use hennery_host::{HostConfig, identity::HostKey};
    use hennery_proto::{
        HELLO_NONCE_HEADER, PROTOCOL_VERSION,
        frames::{Capability, CollectorFrame},
    };
    use tokio_tungstenite::tungstenite::Message;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host-test",
        HostKey::from_seed([8; 32]),
        cwd.join("host"),
    );
    cfg.agents.insert("chosen".into(), fake(script(), &cwd));
    let cancelled_pid = cwd.join("cancelled.pid");
    let disconnected_pid = cwd.join("disconnected.pid");
    for (agent, pid_file) in [("hanging", &cancelled_pid), ("hanging-disconnect", &disconnected_pid)] {
        cfg.agents.insert(
            agent.into(),
            AgentCommand {
                program: "sh".into(),
                args: vec!["-c".into(), "echo $$ > \"$PROBE_PID_FILE\"; sleep 600 & wait".into()],
                env: vec![("PROBE_PID_FILE".into(), pid_file.display().to_string())],
            },
        );
    }
    let mut signed_out = script();
    signed_out.new_session_error = Some(-32000);
    cfg.agents.insert("signed-out".into(), fake(signed_out, &cwd));
    cfg.workspace_roots = vec![cwd.clone()];
    cfg.git = Some(hennery_host::git::find_git().expect("git in dev shell"));
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(hennery_host::run_until(cfg, async {
        let _ = shutdown_rx.await;
    }));
    let (tcp, _) = listener.accept().await.unwrap();
    struct Nonce;
    impl tokio_tungstenite::tungstenite::handshake::server::Callback for Nonce {
        fn on_request(
            self,
            _: &tokio_tungstenite::tungstenite::handshake::server::Request,
            mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
        ) -> Result<
            tokio_tungstenite::tungstenite::handshake::server::Response,
            tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
        > {
            response
                .headers_mut()
                .insert(HELLO_NONCE_HEADER, hex::encode([5u8; 32]).parse().unwrap());
            Ok(response)
        }
    }
    let mut ws = tokio_tungstenite::accept_hdr_async(tcp, Nonce).await.unwrap();
    async fn read(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> HostFrame {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(Ok(Message::Text(s))) = ws.next().await {
                    return serde_json::from_str(&s).unwrap();
                }
            }
        })
        .await
        .unwrap()
    }
    async fn send(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, frame: CollectorFrame) {
        ws.send(Message::text(serde_json::to_string(&frame).unwrap()))
            .await
            .unwrap();
    }
    match read(&mut ws).await {
        HostFrame::Hello {
            capabilities,
            mcp_isolation,
            runtime_profiles,
            ..
        } => {
            assert!(capabilities.has(Capability::CatalogProbe));
            assert!(
                serde_json::to_value(&capabilities)
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("catalog_probe_cancel")),
                "new host must advertise early cancellation"
            );
            assert!(capabilities.has(Capability::DelegationConfig));
            assert!(capabilities.has(Capability::OperatorResume));
            assert!(mcp_isolation.0.contains_key("chosen"));
            assert_eq!(
                runtime_profiles["chosen"],
                hennery_proto::delegation_host::RuntimeProfile::Generic
            );
        }
        other => panic!("{other:?}"),
    }
    send(
        &mut ws,
        CollectorFrame::HelloAck {
            protocol_version: PROTOCOL_VERSION.into(),
            collector_version: "test".into(),
            committed: Default::default(),
        },
    )
    .await;
    assert!(matches!(read(&mut ws).await, HostFrame::ResendComplete));
    for operator_initiated in [false, true] {
        send(
            &mut ws,
            CollectorFrame::ProbeCatalog {
                request_id: "signed-out-catalog".into(),
                agent: "signed-out".into(),
                cwd: cwd.display().to_string(),
                operator_initiated,
            },
        )
        .await;
        assert!(
            matches!(read(&mut ws).await, HostFrame::Error { code, message, .. } if code == "auth_required" && message.contains("not signed in"))
        );
    }
    send(
        &mut ws,
        CollectorFrame::DelegatedAttach {
            operator_initiated: false,
            request_id: "signed-out-attach".into(),
            session_id: "signed-out-child".into(),
            committed_seq: 0,
            agent: "signed-out".into(),
            cwd: cwd.display().to_string(),
            config: SessionConfig::default(),
            hat_id: "hat-test".into(),
            mcp: Default::default(),
            attach_generation: 1,
            agent_session_id: None,
        },
    )
    .await;
    assert!(
        matches!(read(&mut ws).await, HostFrame::Session { body: SessionBody::StartFailed { code, message, .. }, .. } if code == "agent_not_logged_in" && message.contains("Authentication required"))
    );
    let root = read(&mut ws).await;
    assert!(
        matches!(&root, HostFrame::Session { session_id, seq: 2, body: SessionBody::ProjectRoot { project_root }, .. }
            if session_id == "signed-out-child" && project_root == cwd.to_str().unwrap()),
        "{root:?}"
    );
    send(
        &mut ws,
        CollectorFrame::ProbeCatalog {
            request_id: "catalog".into(),
            agent: "chosen".into(),
            cwd: cwd.display().to_string(),
            operator_initiated: false,
        },
    )
    .await;
    assert!(
        matches!(read(&mut ws).await,HostFrame::Catalog{catalog,..} if catalog.defaults.model.as_deref()==Some("small"))
    );
    send(
        &mut ws,
        CollectorFrame::ProbeCatalog {
            request_id: "cancel-me".into(),
            agent: "hanging".into(),
            cwd: cwd.display().to_string(),
            operator_initiated: false,
        },
    )
    .await;
    let pid: i32 = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(pid) = std::fs::read_to_string(&cancelled_pid)
                .ok()
                .and_then(|raw| raw.trim().parse().ok())
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("probe exec signal");
    // SAFETY: this test owns the running probe; getpgid only reads its group.
    let group = unsafe { libc::getpgid(pid) };
    assert!(group > 0);
    send(
        &mut ws,
        CollectorFrame::CancelCatalogProbe {
            request_id: "cancel-me".into(),
        },
    )
    .await;
    wait_process_gone(pid, false).await;
    wait_process_gone(group, true).await;
    // Cancellation is early (the normal probe bound is 30 s) and frees capacity.
    send(
        &mut ws,
        CollectorFrame::ProbeCatalog {
            request_id: "after-cancel".into(),
            agent: "chosen".into(),
            cwd: cwd.display().to_string(),
            operator_initiated: false,
        },
    )
    .await;
    assert!(matches!(read(&mut ws).await, HostFrame::Catalog { request_id, .. } if request_id == "after-cancel"));
    let attach = |generation| CollectorFrame::DelegatedAttach {
        operator_initiated: false,
        request_id: format!("attach-{generation}"),
        session_id: "wire-child".into(),
        committed_seq: 0,
        agent: "chosen".into(),
        cwd: cwd.display().to_string(),
        config: SessionConfig::default(),
        hat_id: "hat-test".into(),
        mcp: Default::default(),
        attach_generation: generation,
        agent_session_id: None,
    };
    send(&mut ws, attach(3)).await;
    let evidence = match read(&mut ws).await {
        HostFrame::Session {
            body: SessionBody::SessionStarted { indexed, .. },
            ..
        } => indexed.config_evidence.unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(evidence.attach_generation, 3);
    let root = read(&mut ws).await;
    assert!(
        matches!(&root, HostFrame::Session { session_id, seq: 2, body: SessionBody::ProjectRoot { project_root }, .. }
            if session_id == "wire-child" && project_root == cwd.to_str().unwrap()),
        "{root:?}"
    );
    send(&mut ws, attach(4)).await;
    assert!(matches!(read(&mut ws).await,HostFrame::Error{code,..} if code=="attach_generation_conflict"));
    let prompt = |generation, id: &str| CollectorFrame::DelegatedPrompt {
        request_id: id.into(),
        session_id: "wire-child".into(),
        turn_id: id.into(),
        content: vec![json!({"type":"text","text":"brief"})],
        expected: PromptFence {
            attach_generation: generation,
            config_revision: evidence.config_revision,
        },
    };
    send(&mut ws, prompt(2, "stale")).await;
    assert!(matches!(read(&mut ws).await,HostFrame::Error{code,..} if code=="config_fence"));
    send(&mut ws, prompt(3, "fresh")).await;
    loop {
        if matches!(
            read(&mut ws).await,
            HostFrame::Session {
                body: SessionBody::TurnEnded { .. },
                ..
            }
        ) {
            break;
        }
    }
    send(
        &mut ws,
        CollectorFrame::CloseSession {
            request_id: "close".into(),
            session_id: "wire-child".into(),
        },
    )
    .await;
    assert!(matches!(
        read(&mut ws).await,
        HostFrame::Session {
            body: SessionBody::SessionClosed,
            ..
        }
    ));
    send(
        &mut ws,
        CollectorFrame::ProbeCatalog {
            request_id: "disconnect-me".into(),
            agent: "hanging-disconnect".into(),
            cwd: cwd.display().to_string(),
            operator_initiated: false,
        },
    )
    .await;
    let pid: i32 = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(pid) = std::fs::read_to_string(&disconnected_pid)
                .ok()
                .and_then(|raw| raw.trim().parse().ok())
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("probe exec before disconnect");
    // SAFETY: reading the group of the probe this test owns.
    let group = unsafe { libc::getpgid(pid) };
    assert!(group > 0);
    ws.close(None).await.unwrap();
    wait_process_gone(pid, false).await;
    wait_process_gone(group, true).await;
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn zero_probe_capacity_is_busy_without_starting_adapter() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let err = CatalogProbes::new(0, Duration::from_secs(1))
        .catalog("none", fake(script(), &cwd), Profile::Generic, cwd.clone(), vec![cwd])
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "busy");
}

#[tokio::test]
async fn absent_catalog_is_unknown_and_never_admits_a_prompt() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let catalog = CatalogProbes::new(1, Duration::from_secs(10))
        .catalog(
            "none",
            fake(FakeScript::default(), &cwd),
            Profile::Generic,
            cwd.clone(),
            vec![cwd],
        )
        .await
        .unwrap();
    assert_eq!(
        catalog.freshness,
        hennery_proto::delegation_host::CatalogFreshness::Unknown
    );
    assert!(catalog.defaults.is_empty());
    let (up, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let h = session::launch_delegated(
        up.clone(),
        launch(root.path(), FakeScript::default(), Attach::New),
        SessionOptions::default(),
        5,
    );
    let ev = evidence(&until(&up, |f| !f.is_empty()).await);
    assert_eq!(ev.completeness, ConfigCompleteness::Unknown);
    assert!(ev.actual.is_empty());
    h.send(prompt(
        PromptFence {
            attach_generation: 5,
            config_revision: ev.config_revision,
        },
        "held",
    ));
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(5),replies.recv()).await.unwrap().unwrap(),HostFrame::Error{code,..} if code=="config_fence")
    );
    stop(h).await;
}

#[tokio::test]
async fn out_of_turn_question_fences_delegated_prompt() {
    let root = tempfile::tempdir().unwrap();
    let mut s = script();
    s.ask_on_load = true;
    s.asks = vec![hennery_testkit::FakeAsk::Permission];
    let (up, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let h = session::launch_delegated(
        up.clone(),
        launch(
            root.path(),
            s,
            Attach::Load {
                agent_session_id: "old".into(),
            },
        ),
        SessionOptions::default(),
        6,
    );
    let f = until(&up, |f| {
        f.iter().any(|f| {
            matches!(
                f,
                HostFrame::Session {
                    body: SessionBody::PendingOpened { .. },
                    ..
                }
            )
        })
    })
    .await;
    let ev = evidence(&f);
    h.send(prompt(
        PromptFence {
            attach_generation: 6,
            config_revision: ev.config_revision,
        },
        "held",
    ));
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(5),replies.recv()).await.unwrap().unwrap(),HostFrame::Error{code,..} if code=="config_fence")
    );
    stop(h).await;
}

#[tokio::test]
async fn queued_close_fences_prompt_before_actor_reads_close() {
    let root = tempfile::tempdir().unwrap();
    let (up, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let hooks = hennery_host::session::test_hooks::TestHooks::hold_after_first_switch();
    let options = SessionOptions {
        test_hooks: Some(hooks.clone()),
        ..Default::default()
    };
    let h = session::launch_delegated(up.clone(), launch(root.path(), script(), Attach::New), options, 8);
    let ev = evidence(&until(&up, |f| !f.is_empty()).await);
    h.send(SessionCmd::SetConfig {
        request_id: "switch".into(),
        config_id: "model".into(),
        value: ConfigValue::Id("large".into()),
    });
    tokio::time::timeout(Duration::from_secs(20), hooks.holding())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), hooks.answers_queued(1))
        .await
        .unwrap();
    h.send(prompt(
        PromptFence {
            attach_generation: 8,
            config_revision: ev.config_revision + 1,
        },
        "fenced-by-close",
    ));
    h.send(SessionCmd::Close {
        request_id: "close".into(),
    });
    hooks.release();
    tokio::time::timeout(Duration::from_secs(20), h.finished())
        .await
        .unwrap();
    assert!(matches!(replies.try_recv().unwrap(),HostFrame::Error{code,..} if code=="config_fence"));
    assert!(!up.pending().unwrap().iter().any(|f| matches!(
        f,
        HostFrame::Session {
            body: SessionBody::TurnStarted { .. },
            ..
        }
    )));
}
