//! Opt-in live probes of hook-set delivery (skills spec §8, §10). Normal
//! tests never use credentials or infer. Re-run both at every adapter pin
//! bump:
//!
//! ```text
//! HENNERY_D4_PINNED_ADAPTER_ROOT=... HENNERY_D4_CLAUDE_AUTH=... \
//! HENNERY_D4_CODEX_AUTH=... \
//!   cargo test -p hennery-testkit --test hook_sets_live -- --ignored --nocapture
//! ```
//!
//! The environment is the one `delegation_live.rs` uses: a pinned adapter
//! root and explicit credential files, copied with mode 0600 into a scratch
//! HOME / `CLAUDE_CONFIG_DIR` / `CODEX_HOME`. Each probe drives the host's
//! own session actor with a delivered `SessionStart` command hook that
//! appends to a marker file, then runs one trivial turn.

use anyhow::{Context, Result, bail, ensure};
use hennery_host::{
    AgentCommand,
    outbox::Outbox,
    profile::Profile,
    session::{self, Attach, Launch, SessionCmd, SessionOptions},
    uplink::Uplink,
};
use hennery_proto::{
    frames::{HookDelivery, HookSetDelivery, HostFrame, SessionBody, TurnOutcome},
    hooks::HookSetConfig,
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

const BOUND: Duration = Duration::from_secs(240);

fn stage_auth(source_variable: &str, destination: &Path) -> Result<()> {
    let source = PathBuf::from(std::env::var(source_variable).with_context(|| format!("supply {source_variable}"))?);
    ensure!(source.is_file(), "{source_variable} must name a credential file");
    let bytes = std::fs::read(&source).context("read explicitly supplied credential file")?;
    let value: Value = serde_json::from_slice(&bytes).context("credential file must contain JSON")?;
    ensure!(value.is_object(), "credential file must contain a JSON object");
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)?;
    std::io::Write::write_all(&mut output, &bytes)?;
    Ok(())
}

/// The pinned adapter for `runtime` in a scratch home under `root`.
fn command(root: &Path, runtime: &str) -> Result<(AgentCommand, Profile, PathBuf)> {
    let home = root.join(format!("{runtime}-home"));
    for leaf in [
        "",
        "config",
        "data",
        "cache",
        "state",
        "claude",
        "codex",
        "codex-sqlite",
    ] {
        std::fs::create_dir_all(home.join(leaf))?;
    }
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
    let adapters =
        PathBuf::from(std::env::var("HENNERY_D4_PINNED_ADAPTER_ROOT").context("supply pinned node_modules")?);
    let managed_modules = adapters.join(runtime).join("node_modules");
    let adapters = if managed_modules.is_dir() {
        managed_modules
    } else {
        adapters
    };
    let manifest = hennery_host::runtime::manifest::Manifest::embedded();
    let adapter_pin = manifest.adapters.get(runtime).context("runtime is not pinned")?;
    let node = std::env::split_paths(&std::env::var_os("PATH").context("PATH missing")?)
        .map(|directory| directory.join("node"))
        .find(|file| file.is_file())
        .context("node missing from dev shell")?;
    hennery_testkit::first_exec(node.to_str().context("node path encoding")?);
    let mut env = vec![
        ("HOME".into(), home.display().to_string()),
        ("XDG_CONFIG_HOME".into(), home.join("config").display().to_string()),
        ("XDG_DATA_HOME".into(), home.join("data").display().to_string()),
        ("XDG_CACHE_HOME".into(), home.join("cache").display().to_string()),
        ("XDG_STATE_HOME".into(), home.join("state").display().to_string()),
        ("CLAUDE_CONFIG_DIR".into(), home.join("claude").display().to_string()),
        ("CODEX_HOME".into(), home.join("codex").display().to_string()),
        (
            "CODEX_SQLITE_HOME".into(),
            home.join("codex-sqlite").display().to_string(),
        ),
    ];
    // No accidental provider or config inheritance: only the staged auth file
    // supplies identity.
    for variable in [
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "HENNERY_DATA_DIR",
        "HENNERY_HOST_DATA_DIR",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "ANTHROPIC_BASE_URL",
        "OPENAI_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CODEX_CONFIG",
    ] {
        env.push((variable.into(), String::new()));
    }
    let profile = if runtime == "claude" {
        stage_auth("HENNERY_D4_CLAUDE_AUTH", &home.join("claude/.credentials.json"))?;
        std::fs::write(home.join("claude.json"), br#"{"hasCompletedOnboarding":true}"#)?;
        Profile::Claude
    } else {
        stage_auth("HENNERY_D4_CODEX_AUTH", &home.join("codex/auth.json"))?;
        let (platform, arch) = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            ("darwin-arm64", "aarch64-apple-darwin")
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            ("linux-x64", "x86_64-unknown-linux-musl")
        } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            ("linux-arm64", "aarch64-unknown-linux-musl")
        } else {
            bail!("supply the supported pinned native Codex platform")
        };
        let binary = adapters.join(format!("@openai/codex-{platform}/vendor/{arch}/bin/codex"));
        ensure!(binary.is_file(), "pinned native Codex is missing");
        hennery_testkit::first_exec(binary.to_str().context("Codex path encoding")?);
        env.push(("CODEX_PATH".into(), binary.display().to_string()));
        Profile::Codex
    };
    let entry = adapters.join(&adapter_pin.package);
    let metadata: Value = serde_json::from_slice(&std::fs::read(entry.join("package.json"))?)?;
    ensure!(
        metadata["version"] == adapter_pin.version,
        "adapter version differs from accepted pin"
    );
    Ok((
        AgentCommand {
            program: node.display().to_string(),
            args: vec![entry.join("dist/index.js").display().to_string()],
            env,
        },
        profile,
        home,
    ))
}

/// A hook set whose `SessionStart` command appends `tag` to `marker`.
fn marker_set(id: &str, tag: &str, marker: &Path) -> HookSetDelivery {
    let command = format!("sh -c 'echo {tag} >> \"{}\"; cat >/dev/null'", marker.display());
    HookSetDelivery {
        id: id.into(),
        name: id.into(),
        content_hash: format!("sha256:{tag}"),
        hooks: serde_json::from_value::<HookSetConfig>(json!({
            "SessionStart": [{"hooks": [{"type": "command", "command": command, "timeout": 10}]}]
        }))
        .expect("marker hook set"),
    }
}

/// Start one session with `hooks`, run one trivial turn, and close it.
/// Returns the ids of the hook sets the host acknowledged on `session_started`.
async fn run_one_turn(agent: AgentCommand, profile: Profile, cwd: &Path, hooks: HookDelivery) -> Result<Vec<String>> {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory()?);
    let launch = Launch {
        mcp_source_policy: None,
        request_id: "live-hooks".into(),
        session_id: "live-hooks".into(),
        attach: Attach::New,
        config: Default::default(),
        agent,
        cwd: cwd.to_path_buf(),
        profile,
        mcp_servers: vec![],
        hooks: Some(hooks),
    };
    let handle = session::launch(uplink.clone(), launch, SessionOptions::default());
    let result = tokio::time::timeout(BOUND, async {
        let mut applied: Option<Vec<String>> = None;
        let mut prompted = false;
        loop {
            for frame in uplink.pending()? {
                let HostFrame::Session { body, .. } = frame else {
                    continue;
                };
                match body {
                    SessionBody::SessionStarted { indexed, .. } if applied.is_none() => {
                        applied = Some(indexed.hook_sets_applied.iter().map(|s| s.id.clone()).collect());
                    }
                    SessionBody::StartFailed { code, message, .. } => bail!("start failed: {code}: {message}"),
                    SessionBody::TurnEnded { outcome, error, .. } => {
                        ensure!(outcome == TurnOutcome::Completed, "turn {outcome:?}: {error:?}");
                        return applied.context("turn ended before session_started");
                    }
                    _ => {}
                }
            }
            if applied.is_some() && !prompted {
                prompted = true;
                ensure!(
                    handle.send(SessionCmd::Prompt {
                        request_id: "live-hooks-prompt".into(),
                        turn_id: "live-hooks-turn".into(),
                        content: vec![
                            json!({"type": "text", "text": "Reply with exactly HOOKS_OK. Do not call any tools."})
                        ],
                        plugin_context: None,
                    }),
                    "the session actor refused the prompt"
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let _ = handle.send(SessionCmd::Close {
        request_id: "live-hooks-close".into(),
    });
    handle.finished().await;
    result.context("live hook probe timed out")?
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
#[ignore = "requires pinned adapters and credentials"]
async fn claude_runs_delivered_hooks_beside_file_hooks() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let (agent, profile, _home) = command(&root, "claude")?;
    let project = root.join("project");
    std::fs::create_dir_all(project.join(".claude"))?;
    let file_marker = root.join("file-hook.log");
    let delivered_marker = root.join("delivered-hook.log");
    // A hook from the project's own settings file: delivered hooks must run
    // beside it, not replace it.
    let file_command = format!(
        "sh -c 'echo FILE_HOOK >> \"{}\"; cat >/dev/null'",
        file_marker.display()
    );
    std::fs::write(
        project.join(".claude/settings.json"),
        serde_json::to_vec(&json!({"hooks": {"SessionStart": [
            {"hooks": [{"type": "command", "command": file_command, "timeout": 10}]}
        ]}}))?,
    )?;
    let hooks = HookDelivery {
        sets: vec![marker_set("hs-live", "DELIVERED_HOOK", &delivered_marker)],
    };
    let applied = run_one_turn(agent, profile, &project, hooks).await?;
    assert_eq!(applied, ["hs-live"], "session_started acknowledges the delivered set");
    assert_eq!(
        lines(&delivered_marker),
        ["DELIVERED_HOOK"],
        "the delivered hook ran once"
    );
    assert_eq!(lines(&file_marker), ["FILE_HOOK"], "the file hook still ran");
    println!(
        "{}",
        json!({"case": "claude_hook_sets", "delivered": true, "file": true})
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires pinned adapters and credentials"]
async fn codex_trusts_and_runs_delivered_hooks() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let (agent, profile, home) = command(&root, "codex")?;
    let project = root.join("project");
    std::fs::create_dir_all(&project)?;
    let marker = root.join("delivered-hook.log");
    let hooks = HookDelivery {
        sets: vec![marker_set("hs-live", "DELIVERED_HOOK", &marker)],
    };
    let applied = run_one_turn(agent, profile, &project, hooks).await?;
    assert_eq!(applied, ["hs-live"], "session_started acknowledges the delivered set");
    assert_eq!(lines(&marker), ["DELIVERED_HOOK"], "Codex ran the trusted hook once");
    let config = std::fs::read_to_string(home.join("codex/config.toml")).context("Codex wrote no config.toml")?;
    let parsed: toml::Table = toml::from_str(&config).context("Codex wrote a config.toml that does not parse")?;
    let state = parsed
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(toml::Value::as_table);
    assert!(
        state.is_some_and(|state| !state.is_empty()),
        "trusting the hook persisted a hooks.state entry: {config}"
    );
    println!(
        "{}",
        json!({"case": "codex_hook_sets", "delivered": true, "trusted": true})
    );
    Ok(())
}
