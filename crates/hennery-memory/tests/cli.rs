use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};
fn command(home: &Path) -> Command {
    let binary = env!("CARGO_BIN_EXE_hennery-memory");
    hennery_testkit::first_exec(binary);
    let mut command = Command::new(binary);
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("HENNERY_DATA_DIR")
        .env_remove("HENNERY_HOST_DATA_DIR")
        .env_remove("HENNERY_MASTER_KEY")
        .env_remove("CREDENTIALS_DIRECTORY")
        .env_remove("CODEX_HOME")
        .env_remove("CLAUDE_CONFIG_DIR");
    command
}
#[test]
fn bootstrap_is_explicit_private_and_only_hashes_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("private");
    let output = command(dir.path())
        .arg("--data-dir")
        .arg(&data)
        .args(["create", "--owner", "owner", "--hat", "hat"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let created: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(created["tool_credential"].as_str().unwrap().starts_with("hnry_plugin_"));
    assert!(
        created["lifecycle_credential"]
            .as_str()
            .unwrap()
            .starts_with("hnry_lifecycle_")
    );
    assert_eq!(data.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(
        data.join("memory.db").metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    let bytes = std::fs::read(data.join("memory.db")).unwrap();
    for key in ["tool_credential", "lifecycle_credential"] {
        let token = created[key].as_str().unwrap().as_bytes();
        assert!(!bytes.windows(token.len()).any(|s| s == token));
    }
}
#[test]
fn missing_data_dir_and_unsafe_directory_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        !command(dir.path())
            .args(["create", "--owner", "owner", "--hat", "hat"])
            .status()
            .unwrap()
            .success()
    );
    let public = dir.path().join("public");
    std::fs::create_dir(&public).unwrap();
    std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        !command(dir.path())
            .arg("--data-dir")
            .arg(&public)
            .args(["create", "--owner", "owner", "--hat", "hat"])
            .status()
            .unwrap()
            .success()
    );
    assert!(!public.join("memory.db").exists());
    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(
        !command(dir.path())
            .arg("--data-dir")
            .arg(&link)
            .args(["create", "--owner", "owner", "--hat", "hat"])
            .status()
            .unwrap()
            .success()
    );
    assert!(!target.join("memory.db").exists());
}

#[test]
fn ui_bootstrap_uses_separate_private_auth_and_never_prints_password() {
    use std::io::Write;
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("private");
    let mut child = command(d.path())
        .arg("--data-dir")
        .arg(&data)
        .args([
            "ui-init",
            "--owner",
            "fixture-owner",
            "--public-url",
            "https://memory.example.test",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"synthetic bootstrap password\n")
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("synthetic bootstrap password"));
    assert_eq!(
        data.join("memory-auth.db").metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!data.join("hennery.db").exists());
    let operator = hennery_kernel::operator::Operator::open(&data.join("memory-auth.db")).unwrap();
    assert!(
        operator
            .verify_password("synthetic bootstrap password")
            .unwrap()
            .is_some()
    );
    let store = hennery_memory::store::Store::open(&data.join("memory.db")).unwrap();
    assert!(store.manager(operator.owner_id()).is_ok());
}

#[test]
fn local_snapshot_import_requires_signed_preview_and_exact_digest() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("private");
    let created = command(d.path())
        .arg("--data-dir")
        .arg(&data)
        .args(["create", "--owner", "fixture-owner", "--hat", "garden"])
        .output()
        .unwrap();
    assert!(created.status.success());
    let c: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let snapshot = d.path().join("synthetic-snapshot.tar");
    let mapping = d.path().join("mapping.json");
    let proposal = d.path().join("reviewed-preview.json");
    let mut builder = tar::Builder::new(Vec::new());
    let body =
        b"---\nname: Imported fixture\nmetadata:\n  scope: garden\n---\nRetained synthetic text hnry_plugin_DEADBEEF";
    let mut header = tar::Header::new_ustar();
    header.set_path("memories/garden/fixture.md").unwrap();
    header.set_mode(0o600);
    header.set_size(body.len() as u64);
    header.set_cksum();
    builder.append(&header, &body[..]).unwrap();
    std::fs::write(&snapshot, builder.into_inner().unwrap()).unwrap();
    std::fs::write(&mapping,serde_json::to_vec(&serde_json::json!({"scopes":{"garden":{"action":"namespace","namespace":c["namespace"]}},"files":{},"mixed":[]})).unwrap()).unwrap();
    let preview = command(d.path())
        .arg("--data-dir")
        .arg(&data)
        .args([
            "import-preview",
            "--owner",
            "fixture-owner",
            "--vault",
            "invented-vault",
            "--snapshot",
        ])
        .arg(&snapshot)
        .arg("--mapping")
        .arg(&mapping)
        .output()
        .unwrap();
    assert!(preview.status.success(), "{}", String::from_utf8_lossy(&preview.stderr));
    assert!(!String::from_utf8_lossy(&preview.stdout).contains("DEADBEEF"));
    let preview_json: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    std::fs::write(&proposal, &preview.stdout).unwrap();
    let wrong = command(d.path())
        .arg("--data-dir")
        .arg(&data)
        .args(["import-apply", "--owner", "fixture-owner", "--preview"])
        .arg(&proposal)
        .args(["--digest", "wrong"])
        .output()
        .unwrap();
    assert!(!wrong.status.success());
    for expected in [1, 0] {
        let result = command(d.path())
            .arg("--data-dir")
            .arg(&data)
            .args(["import-apply", "--owner", "fixture-owner", "--preview"])
            .arg(&proposal)
            .arg("--digest")
            .arg(preview_json["digest"].as_str().unwrap())
            .output()
            .unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(result[0]["status"], "committed");
        assert_eq!(result[0]["imported"], expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serving_standalone_memory_recovers_pending_cleanup_without_a_restart() {
    use hennery_memory::{management::NewMemory, store::Store};
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Stdio},
        sync::mpsc,
        time::Duration,
    };
    struct Running(Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("private");
    std::fs::create_dir(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = data.join("memory.db");
    let store = Store::open(&path).unwrap();
    assert!(!store.has_feed_credentials().unwrap());
    let c = store.create("owner", "standalone").unwrap();
    let manager = store.filesystem_manager("owner").unwrap();
    let entry = store
        .managed_create(
            &manager,
            &c.namespace,
            NewMemory {
                title: "reference".into(),
                body: "standalone server cleanup marker".into(),
            },
        )
        .unwrap();
    // Raw test reader pins WAL without retaining the kernel's process-owner
    // lock when Store is dropped before the actual service process starts.
    let reader = rusqlite::Connection::open(&path).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT count(*) FROM entries WHERE namespace=?1", [&c.namespace], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(
        store
            .managed_delete(&manager, &c.namespace, &entry.id, entry.revision)
            .is_err()
    );
    drop(store);
    let listen = std::net::SocketAddr::from(([127, 0, 0, 1], 0)).to_string();
    let mut server = Running(
        command(dir.path())
            .arg("--data-dir")
            .arg(&data)
            .args(["serve", "--listen", &listen])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = server.0.stdout.take().unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let readiness = std::thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        let _ = ready_tx.send(line);
    });
    let line = ready_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    readiness.join().unwrap();
    let address = line.trim().strip_prefix("memory listening on ").unwrap();
    let url = format!("http://{address}/mcp");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_recent","arguments":{}}});
    assert_eq!(
        client
            .post(&url)
            .bearer_auth(&c.tool)
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    reader.execute_batch("ROLLBACK").unwrap();
    let response = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = client
                .post(&url)
                .bearer_auth(&c.tool)
                .json(&request)
                .send()
                .await
                .unwrap();
            if response.status().is_success() {
                break response;
            }
            assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let rpc: serde_json::Value = response.json().await.unwrap();
    let page: serde_json::Value = serde_json::from_str(rpc["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(page["entries"], serde_json::json!([]));
    for name in ["memory.db", "memory.db-wal"] {
        if let Ok(bytes) = std::fs::read(data.join(name)) {
            assert!(
                !bytes
                    .windows(b"standalone server cleanup marker".len())
                    .any(|w| w == b"standalone server cleanup marker")
            );
        }
    }
}
#[test]
fn serve_refuses_unsafe_proxy_peers_before_listening() {
    let dir = tempfile::tempdir().unwrap();
    let nine = (1..=9).map(|n| format!("192.0.2.{n}")).collect::<Vec<_>>();
    for peers in [
        vec!["0.0.0.0".to_string()],
        vec!["::".into()],
        vec!["224.0.0.1".into()],
        vec!["127.0.0.1".into(), "::ffff:127.0.0.1".into()],
        vec!["127.0.0.0/8".into()],
        vec!["localhost".into()],
        vec!["127.0.0.1:7332".into()],
        nine,
    ] {
        let mut command = command(dir.path());
        command
            .arg("--data-dir")
            .arg(dir.path().join("private"))
            .args(["serve", "--listen", "127.0.0.1:0"]);
        for peer in &peers {
            command.args(["--proxy-client-ip-peer", peer]);
        }
        let mut child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // A refusal exits; the bound is only a failure detector for a serve.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() > deadline {
                child.kill().unwrap();
                panic!("{peers:?} was served");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let mut stdout = String::new();
        std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut stdout).unwrap();
        assert!(!status.success(), "{peers:?}");
        assert!(!stdout.contains("listening"), "{peers:?}: {stdout}");
    }
}
