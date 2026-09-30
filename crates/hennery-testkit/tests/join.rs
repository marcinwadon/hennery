//! `hennery host join` against a real collector (kernel spec §4.1): the host
//! generates its key, enrolls it with a code and stores the pairing; a host
//! the collector still accepts is left alone, and one it revoked pairs anew.

use hennery_host::HostConfig;
use hennery_host::identity::{KEY_FILE, Paired};
use hennery_host::pairing::{Joined, join};
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HelloCheck, Hosts};
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::rest::PairingCodeResponse;
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;

const TOKEN: &str = "dev-token-for-tests";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn public_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn mint(&self) -> String {
        let resp = reqwest::Client::new()
            .post(format!("{}/api/hosts/pairing-codes", self.public_url()))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
        resp.json::<PairingCodeResponse>().await.unwrap().code
    }
}

#[tokio::test]
async fn joining_pairs_the_host_with_a_key_the_collector_accepts() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let code = collector.mint().await;
    let joined = join(&collector.public_url(), &code, dir.path(), "laptop")
        .await
        .unwrap();
    let Joined::Paired { host_id } = joined else {
        panic!("expected a new pairing, got {joined:?}");
    };
    let paired = Paired::load(dir.path()).unwrap().expect("pairing stored");
    assert_eq!(paired.host_id, host_id);
    assert_eq!(paired.collector_url, format!("ws://{}/api/hosts/ws", collector.addr));
    assert!(!dir.path().join(format!("{KEY_FILE}.pending")).exists());
    let record = collector.state.hosts.host(&host_id).unwrap().unwrap();
    assert_eq!(record.name, "laptop");
    let nonce = [9u8; 32];
    let proof = paired.key.sign_hello(&nonce, &host_id, PROTOCOL_VERSION);
    assert_eq!(
        collector
            .state
            .hosts
            .check_hello(&host_id, &nonce, PROTOCOL_VERSION, &proof)
            .unwrap(),
        HelloCheck::Accepted
    );
}

#[tokio::test]
async fn joining_again_leaves_the_pairing_as_it_is_and_spends_no_code() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let first = collector.mint().await;
    let Joined::Paired { host_id } = join(&collector.public_url(), &first, dir.path(), "laptop")
        .await
        .unwrap()
    else {
        panic!("expected a new pairing");
    };
    let key_before = std::fs::read(dir.path().join(KEY_FILE)).unwrap();

    let second = collector.mint().await;
    assert_eq!(
        join(&collector.public_url(), &second, dir.path(), "laptop")
            .await
            .unwrap(),
        Joined::AlreadyPaired {
            host_id: host_id.clone()
        }
    );
    assert_eq!(std::fs::read(dir.path().join(KEY_FILE)).unwrap(), key_before);
    assert_eq!(collector.state.hosts.list().unwrap().len(), 1);
    // The second code was never sent: it still pairs another host.
    let other = Enrollment {
        public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
        name: "desk".into(),
        host_version: "0".into(),
        platform: "linux-x86_64".into(),
    };
    assert!(matches!(
        collector
            .state
            .hosts
            .enroll(&second, &other, hennery_kernel::secret::unix_now())
            .unwrap(),
        EnrollOutcome::Enrolled { .. }
    ));
}

#[tokio::test]
async fn a_wrong_code_fails_and_leaves_nothing_behind() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let err = join(&collector.public_url(), "0000-0000", dir.path(), "laptop")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown, used or expired"), "{err}");
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
    assert!(collector.state.hosts.list().unwrap().is_empty());
}

#[tokio::test]
async fn joining_over_https_is_refused_before_a_code_is_spent() {
    let dir = tempfile::tempdir().unwrap();
    let err = join("https://c.example", "0000-0000", dir.path(), "laptop")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("wss://"), "{err}");
    assert!(!dir.path().join(KEY_FILE).exists());
}

#[tokio::test]
async fn joining_next_to_an_old_outbox_moves_it_aside() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("outbox.db"), b"frames of an unpaired host").unwrap();
    let code = collector.mint().await;
    join(&collector.public_url(), &code, dir.path(), "laptop")
        .await
        .unwrap();
    assert!(!dir.path().join("outbox.db").exists());
    assert_eq!(
        std::fs::read(dir.path().join("outbox.db.orphaned-unpaired")).unwrap(),
        b"frames of an unpaired host"
    );
}

async fn joined(collector: &Collector, dir: &std::path::Path) -> String {
    let code = collector.mint().await;
    match join(&collector.public_url(), &code, dir, "laptop").await.unwrap() {
        Joined::Paired { host_id } => host_id,
        other => panic!("expected a new pairing, got {other:?}"),
    }
}

#[tokio::test]
async fn joining_again_while_the_host_runs_leaves_it_running() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let host_id = joined(&collector, dir.path()).await;
    let paired = Paired::load(dir.path()).unwrap().unwrap();
    let host = tokio::spawn(hennery_host::run(HostConfig::new(
        paired.collector_url,
        paired.host_id,
        paired.key,
        dir.path().to_path_buf(),
    )));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while !collector.state.hub.is_ready(&host_id) {
        assert!(tokio::time::Instant::now() < deadline, "host never connected");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // The probe is refused `already_connected`, which the collector says
    // only of a valid proof: still paired.
    let code = collector.mint().await;
    assert_eq!(
        join(&collector.public_url(), &code, dir.path(), "laptop")
            .await
            .unwrap(),
        Joined::AlreadyPaired {
            host_id: host_id.clone()
        }
    );
    assert!(collector.state.hub.is_ready(&host_id));
    host.abort();
}

#[tokio::test]
async fn joining_again_after_a_revoke_pairs_anew_and_moves_the_old_outbox_aside() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let old_id = joined(&collector, dir.path()).await;
    let old_key = std::fs::read(dir.path().join(KEY_FILE)).unwrap();
    std::fs::write(dir.path().join("outbox.db"), b"frames of the old identity").unwrap();
    collector
        .state
        .hosts
        .revoke(&old_id, hennery_kernel::secret::unix_now())
        .unwrap();

    let new_id = joined(&collector, dir.path()).await;
    assert_ne!(new_id, old_id);
    assert_ne!(std::fs::read(dir.path().join(KEY_FILE)).unwrap(), old_key);
    assert!(!dir.path().join("outbox.db").exists());
    assert_eq!(
        std::fs::read(dir.path().join(format!("outbox.db.orphaned-{old_id}"))).unwrap(),
        b"frames of the old identity"
    );
    assert_eq!(Paired::load(dir.path()).unwrap().unwrap().host_id, new_id);
    let hosts = collector.state.hosts.list().unwrap();
    assert_eq!(hosts.len(), 2);
    assert!(hosts.iter().any(|h| h.id == old_id && h.revoked_at.is_some()));
}

/// Two collectors on loopback count as one (the all-in-one collector may
/// come back on another port), so the second is asked about the key: it
/// does not know it, and that is refused rather than guessed at.
#[tokio::test]
async fn joining_a_collector_that_does_not_know_the_key_is_refused() {
    let first = Collector::start().await;
    let second = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    joined(&first, dir.path()).await;
    std::fs::write(dir.path().join("outbox.db"), b"frames of the first pairing").unwrap();
    let code = second.mint().await;
    let err = join(&second.public_url(), &code, dir.path(), "laptop")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not know host"), "{err}");
    assert!(err.contains(&dir.path().join(KEY_FILE).display().to_string()), "{err}");
    assert!(second.state.hosts.list().unwrap().is_empty());
    // Nothing was touched.
    assert!(dir.path().join("outbox.db").exists());
    assert!(Paired::load(dir.path()).unwrap().is_some());
}
