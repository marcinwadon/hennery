//! Versioned external OMP CLI handshake, without creating a session.
//! An old CLI's successful `--help` is insufficient: it may ignore unknown
//! flags. This probe requires the versioned missing-file response. It does
//! not establish that an ownership lease is held or authorize any deletion.

use super::OmpConfig;
use crate::adapter::{Adapter, AgentCommand};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;

pub const MAX_OUTPUT: usize = 4096;
pub const TIMEOUT: Duration = Duration::from_secs(5);
pub const REPAIR: &str = "configure an external OMP build providing frozen --agent-dir and versioned session lease hold; managed adapter updates do not install OMP, and configuration alone does not enable runtime support";
pub const STRIPPED_ENV: &[&str] = &[
    "OMP_PROFILE",
    "PI_PROFILE",
    "PI_CODING_AGENT_SESSION_DIR",
    "PI_CODING_AGENT_DIR",
];

/// Use a fresh, absent child of the already validated root. The upstream
/// contract must report absence without creating this path or its parent.
pub struct Request {
    pub command: AgentCommand,
    missing_parent: PathBuf,
}

impl Request {
    /// Refuse a CLI that created even a symlink at the supposedly absent
    /// path. This detects mutation; it is not a filesystem ownership proof.
    pub fn untouched(&self) -> bool {
        std::fs::symlink_metadata(&self.missing_parent).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    }
}

pub fn request(config: &OmpConfig) -> Result<Request> {
    let mut command = crate::runtime::agents::omp_command(config)?;
    let missing_parent = config
        .agent_data_dir
        .join("sessions")
        .join(format!(".hennery-omp-probe-{}", uuid::Uuid::now_v7()));
    let missing = missing_parent.join("absent.jsonl");
    command.args = vec![
        "--agent-dir".into(),
        config.agent_data_dir.to_string_lossy().into_owned(),
        "session".into(),
        "lease".into(),
        "hold".into(),
        missing.to_string_lossy().into_owned(),
        "--json".into(),
    ];
    let request = Request {
        command,
        missing_parent,
    };
    anyhow::ensure!(request.untouched(), "OMP probe path is not absent");
    Ok(request)
}

/// Only the specified missing-file response proves the command contract was
/// understood. No stdout/stderr from the external program becomes a diagnostic.
pub fn recognized(exit_code: Option<i32>, output: &[u8]) -> bool {
    if exit_code != Some(1) || output.len() > MAX_OUTPUT {
        return false;
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        #[serde(rename = "schemaVersion")]
        schema_version: u32,
        state: String,
        code: String,
    }
    serde_json::from_slice::<Reply>(output)
        .is_ok_and(|reply| reply.schema_version == 1 && reply.state == "error" && reply.code == "session_missing")
}

/// Host-service environment and existing process-group/death-pipe supervision.
/// One deadline covers output and exit; a hung or overlong response is refused.
pub async fn missing_contract_answered(
    config: &OmpConfig,
    cwd: &Path,
    env: &[(String, String)],
    timeout: Duration,
) -> bool {
    let Ok(mut request) = request(config) else {
        return false;
    };
    request.command.env = env.to_vec();
    let Ok((mut process, io)) = Adapter::spawn_stripped(&request.command, cwd, STRIPPED_ENV) else {
        return false;
    };
    drop(io.stdin);
    let result = tokio::time::timeout(timeout, async {
        let mut output = Vec::new();
        io.stdout
            .take((MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut output)
            .await
            .ok()?;
        if output.len() > MAX_OUTPUT {
            return Some(false);
        }
        let status = process.exited().await;
        Some(recognized(status.code, &output))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(false);
    process.terminate(Duration::from_millis(100)).await;
    result && request.untouched()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_versioned_missing_file_reply_is_recognized() {
        let valid = br#"{"schemaVersion":1,"state":"error","code":"session_missing"}"#;
        assert!(recognized(Some(1), valid));
        for code in [None, Some(0), Some(2)] {
            assert!(!recognized(code, valid));
        }
        for value in [
            r#"{"schemaVersion":2,"state":"error","code":"session_missing"}"#,
            r#"{"schemaVersion":1,"state":"held","code":"session_missing"}"#,
            r#"{"schemaVersion":1,"state":"error","code":"busy"}"#,
            r#"{"schemaVersion":1,"state":"error","code":"session_missing","extra":true}"#,
            "Usage: omp acp",
            "{}",
            "[]",
        ] {
            assert!(!recognized(Some(1), value.as_bytes()), "{value}");
        }
        assert!(!recognized(Some(1), &[b' '; MAX_OUTPUT + 1]));
    }

    #[tokio::test]
    async fn probes_are_bounded_and_do_not_create_the_missing_family() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let program = root.join("omp");
        let config = OmpConfig {
            executable: program.clone(),
            agent_data_dir: root.clone(),
        };
        for (body, expected) in [
            (
                "printf '%s\\n' '{\"schemaVersion\":1,\"state\":\"error\",\"code\":\"session_missing\"}'; exit 1"
                    .to_string(),
                true,
            ),
            ("printf '%s\\n' 'Usage: omp acp'; exit 0".to_string(), false),
            (format!("printf '%s' '{}'; exit 1", "x".repeat(MAX_OUTPUT + 1)), false),
            ("sleep 30".to_string(), false),
        ] {
            std::fs::write(
                &program,
                format!("#!/bin/sh\n[ \"$1\" = --version ] && exit 0\n{body}\n"),
            )
            .unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
            hennery_testkit::first_exec(program.to_str().unwrap());
            let before = std::fs::read_dir(&root).unwrap().count();
            let env = [
                ("HOME".into(), root.to_string_lossy().into_owned()),
                ("XDG_DATA_HOME".into(), root.to_string_lossy().into_owned()),
                ("XDG_CONFIG_HOME".into(), root.to_string_lossy().into_owned()),
                ("XDG_STATE_HOME".into(), root.to_string_lossy().into_owned()),
            ];
            assert_eq!(
                missing_contract_answered(&config, &root, &env, Duration::from_secs(2)).await,
                expected
            );
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), before);
        }
    }

    #[tokio::test]
    async fn a_program_that_creates_the_absent_project_is_refused_without_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let program = root.join("omp");
        std::fs::write(
            &program,
            r#"#!/bin/sh
[ "$1" = --version ] && exit 0
mkdir -p "${6%/*}"
printf retained > "${6%/*}/evidence"
printf '%s\n' '{"schemaVersion":1,"state":"error","code":"session_missing"}'
exit 1
"#,
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        hennery_testkit::first_exec(program.to_str().unwrap());
        let config = OmpConfig {
            executable: program,
            agent_data_dir: root.clone(),
        };
        let env = [
            ("HOME".into(), root.to_string_lossy().into_owned()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ];
        assert!(!missing_contract_answered(&config, &root, &env, Duration::from_secs(2)).await);
        let created = std::fs::read_dir(root.join("sessions"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read_to_string(created.join("evidence")).unwrap(), "retained");
    }
}
