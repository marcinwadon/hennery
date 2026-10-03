//! The host registry (kernel spec §4, §11): pairing codes, enrollment, the
//! `hello` proof and revocation.

use ed25519_dalek::{Signer, SigningKey};
use hennery_kernel::hosts::{
    EnrollOutcome, Enrollment, HelloCheck, Hosts, MAX_LIVE_PAIRING_CODES, PAIRING_CODE_TTL_SECS, Registered, Revoke,
    TooManyPairingCodes, normalize_code, verify_proof,
};
use hennery_proto::frames::{Capabilities, Capability};
use hennery_proto::hello_proof_message;

const NOW: i64 = 1_800_000_000;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn enrollment(key: &SigningKey) -> Enrollment {
    Enrollment {
        public_key: hex::encode(key.verifying_key().as_bytes()),
        name: "laptop".into(),
        host_version: "0.0.0".into(),
        platform: "macos-aarch64".into(),
    }
}

fn proof(key: &SigningKey, nonce: &[u8], host_id: &str) -> String {
    hex::encode(key.sign(&hello_proof_message(nonce, host_id, "1.0")).to_bytes())
}

fn enrolled(outcome: EnrollOutcome) -> String {
    match outcome {
        EnrollOutcome::Enrolled { host_id } => host_id,
        other => panic!("expected an enrollment, got {other:?}"),
    }
}

#[test]
fn a_code_is_read_however_it_is_typed() {
    assert_eq!(normalize_code("ABCD-EFGH").as_deref(), Some("ABCDEFGH"));
    assert_eq!(normalize_code(" abcd efgh ").as_deref(), Some("ABCDEFGH"));
    // O is read as 0, I and L as 1: the alphabet has neither.
    assert_eq!(normalize_code("OOIL-2345").as_deref(), Some("00112345"));
    assert_eq!(normalize_code("ABCD-EFG"), None);
    assert_eq!(normalize_code("ABCD-EFGHJ"), None);
    // U is not in the alphabet, and nothing non-ASCII is.
    assert_eq!(normalize_code("ABCD-EFGU"), None);
    assert_eq!(normalize_code("ABCD-EFGÉ"), None);
}

#[test]
fn a_minted_code_enrolls_one_host_once() {
    let hosts = Hosts::open_in_memory().unwrap();
    let code = hosts.mint_pairing_code(NOW).unwrap();
    assert_eq!(code.expires_at, NOW + PAIRING_CODE_TTL_SECS);
    assert_eq!(code.code.len(), 9, "{}", code.code);
    assert_eq!(&code.code[4..5], "-");
    assert!(normalize_code(&code.code).is_some(), "{}", code.code);

    // Typed in lowercase, without the dash: still the same code.
    let typed = code.code.replace('-', "").to_lowercase();
    let host_id = enrolled(hosts.enroll(&typed, &enrollment(&key(1)), NOW + 1).unwrap());
    let listed = hosts.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].id.as_str(), listed[0].name.as_str(), listed[0].created_at),
        (host_id.as_str(), "laptop", NOW + 1)
    );

    // Single use, even for another key.
    assert_eq!(
        hosts.enroll(&code.code, &enrollment(&key(2)), NOW + 2).unwrap(),
        EnrollOutcome::InvalidCode
    );
}

#[test]
fn a_code_expires_after_ten_minutes() {
    let hosts = Hosts::open_in_memory().unwrap();
    let late = hosts.mint_pairing_code(NOW).unwrap();
    assert_eq!(
        hosts
            .enroll(&late.code, &enrollment(&key(1)), NOW + PAIRING_CODE_TTL_SECS)
            .unwrap(),
        EnrollOutcome::InvalidCode
    );
    let in_time = hosts.mint_pairing_code(NOW).unwrap();
    enrolled(
        hosts
            .enroll(&in_time.code, &enrollment(&key(1)), NOW + PAIRING_CODE_TTL_SECS - 1)
            .unwrap(),
    );
}

#[test]
fn wrong_codes_leave_other_outstanding_codes_valid() {
    let hosts = Hosts::open_in_memory().unwrap();
    let first = hosts.mint_pairing_code(NOW).unwrap();
    let second = hosts.mint_pairing_code(NOW).unwrap();
    assert_ne!(first.code, second.code);
    for wrong in ["0000-0000", "ZZZZ-ZZZZ", "not a code"] {
        assert_eq!(
            hosts.enroll(wrong, &enrollment(&key(1)), NOW).unwrap(),
            EnrollOutcome::InvalidCode
        );
    }
    enrolled(hosts.enroll(&first.code, &enrollment(&key(1)), NOW).unwrap());
    enrolled(hosts.enroll(&second.code, &enrollment(&key(2)), NOW).unwrap());
    assert_eq!(hosts.list().unwrap().len(), 2);
}

#[test]
fn a_key_that_is_paired_already_or_a_malformed_enrollment_does_not_use_the_code() {
    let hosts = Hosts::open_in_memory().unwrap();
    let first = hosts.mint_pairing_code(NOW).unwrap();
    let host_id = enrolled(hosts.enroll(&first.code, &enrollment(&key(1)), NOW).unwrap());

    let code = hosts.mint_pairing_code(NOW).unwrap();
    assert_eq!(
        hosts.enroll(&code.code, &enrollment(&key(1)), NOW).unwrap(),
        EnrollOutcome::AlreadyPaired {
            host_id: host_id.clone()
        }
    );
    // The same key in upper case is the same key.
    let mut shouted = enrollment(&key(1));
    shouted.public_key = shouted.public_key.to_uppercase();
    assert_eq!(
        hosts.enroll(&code.code, &shouted, NOW).unwrap(),
        EnrollOutcome::AlreadyPaired { host_id }
    );
    let mut bad_key = enrollment(&key(2));
    bad_key.public_key = "00".repeat(31);
    assert!(matches!(
        hosts.enroll(&code.code, &bad_key, NOW).unwrap(),
        EnrollOutcome::Invalid(why) if why.contains("public_key")
    ));
    let mut no_name = enrollment(&key(2));
    no_name.name = "  ".into();
    assert!(matches!(
        hosts.enroll(&code.code, &no_name, NOW).unwrap(),
        EnrollOutcome::Invalid(why) if why.contains("name")
    ));
    // A name that would display reversed, or hide characters, is refused.
    for disguised in ["lap\u{202E}pot", "lap\u{200B}top", "\u{2066}laptop"] {
        let mut named = enrollment(&key(2));
        named.name = disguised.into();
        assert!(
            matches!(
                hosts.enroll(&code.code, &named, NOW).unwrap(),
                EnrollOutcome::Invalid(_)
            ),
            "{disguised:?}"
        );
    }
    // The code is still good.
    enrolled(hosts.enroll(&code.code, &enrollment(&key(2)), NOW).unwrap());
}

#[test]
fn a_hello_is_accepted_only_with_a_proof_over_its_own_nonce() {
    let hosts = Hosts::open_in_memory().unwrap();
    assert_eq!(
        hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap(),
        Registered::Created
    );
    let nonce = [7u8; 32];
    let good = proof(&key(1), &nonce, "host-1");
    assert_eq!(
        hosts.check_hello("host-1", &nonce, "1.0", &good).unwrap(),
        HelloCheck::Accepted
    );
    // Replayed on another connection (another nonce).
    assert_eq!(
        hosts.check_hello("host-1", &[8u8; 32], "1.0", &good).unwrap(),
        HelloCheck::BadProof
    );
    // Signed by another key, for another version, or not hex at all.
    let other = proof(&key(2), &nonce, "host-1");
    assert_eq!(
        hosts.check_hello("host-1", &nonce, "1.0", &other).unwrap(),
        HelloCheck::BadProof
    );
    assert_eq!(
        hosts.check_hello("host-1", &nonce, "1.1", &good).unwrap(),
        HelloCheck::BadProof
    );
    assert_eq!(
        hosts.check_hello("host-1", &nonce, "1.0", "zz").unwrap(),
        HelloCheck::BadProof
    );
    // An unknown host looks exactly like a bad proof.
    let stranger = proof(&key(1), &nonce, "host-2");
    assert_eq!(
        hosts.check_hello("host-2", &nonce, "1.0", &stranger).unwrap(),
        HelloCheck::BadProof
    );
}

#[test]
fn a_revoked_host_is_told_so_only_with_a_valid_proof() {
    let hosts = Hosts::open_in_memory().unwrap();
    hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
    assert_eq!(hosts.revoke("host-2", NOW).unwrap(), Revoke::NotFound);
    assert!(!hosts.is_revoked("host-1").unwrap());
    assert_eq!(hosts.revoke("host-1", NOW + 5).unwrap(), Revoke::Revoked);
    assert_eq!(hosts.revoke("host-1", NOW + 6).unwrap(), Revoke::AlreadyRevoked);
    assert!(hosts.is_revoked("host-1").unwrap());
    assert_eq!(hosts.host("host-1").unwrap().unwrap().revoked_at, Some(NOW + 5));

    let nonce = [7u8; 32];
    assert_eq!(
        hosts
            .check_hello("host-1", &nonce, "1.0", &proof(&key(1), &nonce, "host-1"))
            .unwrap(),
        HelloCheck::Revoked
    );
    assert_eq!(
        hosts
            .check_hello("host-1", &nonce, "1.0", &proof(&key(2), &nonce, "host-1"))
            .unwrap(),
        HelloCheck::BadProof
    );
}

#[test]
fn an_accepted_hello_updates_the_hosts_version_capabilities_and_last_seen() {
    let hosts = Hosts::open_in_memory().unwrap();
    hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
    hosts
        .record_hello("host-1", "0.1.0", &Capabilities(vec![Capability::Park]), NOW + 9)
        .unwrap();
    let record = hosts.host("host-1").unwrap().unwrap();
    assert_eq!(
        (record.host_version.as_str(), record.capabilities, record.last_seen_at),
        ("0.1.0", Capabilities(vec![Capability::Park]), Some(NOW + 9))
    );
}

#[test]
fn a_malformed_host_version_on_hello_is_ignored_but_last_seen_and_capabilities_still_update() {
    let hosts = Hosts::open_in_memory().unwrap();
    hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
    hosts
        .record_hello("host-1", "0.1.0", &Capabilities::default(), NOW + 1)
        .unwrap();

    // Too long, and hiding characters: neither is a version a host should
    // be able to make the registry display.
    let too_long = "0.".to_string() + &"9".repeat(64);
    for bad_version in [too_long.as_str(), "lap\u{202E}top"] {
        hosts
            .record_hello("host-1", bad_version, &Capabilities(vec![Capability::Park]), NOW + 2)
            .unwrap();
        let record = hosts.host("host-1").unwrap().unwrap();
        // The stored version is untouched by the bad report...
        assert_eq!(record.host_version, "0.1.0", "{bad_version:?}");
        // ...but this hello still counts: capabilities and last_seen_at move.
        assert_eq!(
            (record.capabilities, record.last_seen_at),
            (Capabilities(vec![Capability::Park]), Some(NOW + 2)),
            "{bad_version:?}"
        );
    }
}

/// Plan 6c decision 7: the roots stored are those that can be shown, at
/// most `MAX_ROOTS`, and a later report replaces them.
#[test]
fn workspace_roots_keep_only_what_can_be_shown() {
    let hosts = Hosts::open_in_memory().unwrap();
    hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
    assert!(hosts.host("host-1").unwrap().unwrap().workspace_roots.is_empty());
    let reported: Vec<String> = [
        "/srv/projects",
        "relative",
        "~/src",
        "/with\nnewline",
        "/lap\u{202E}top",
        &format!("/{}", "x".repeat(4096)),
        "/home/u",
    ]
    .iter()
    .map(|r| r.to_string())
    .collect();
    hosts.record_workspace_roots("host-1", &reported).unwrap();
    assert_eq!(
        hosts.host("host-1").unwrap().unwrap().workspace_roots,
        ["/srv/projects", "/home/u"]
    );
    let many: Vec<String> = (0..40).map(|n| format!("/r{n}")).collect();
    hosts.record_workspace_roots("host-1", &many).unwrap();
    assert_eq!(
        hosts.host("host-1").unwrap().unwrap().workspace_roots,
        many[..hennery_kernel::hosts::MAX_ROOTS]
    );
    hosts.record_workspace_roots("host-1", &[]).unwrap();
    assert!(hosts.list().unwrap()[0].workspace_roots.is_empty());
}

/// The vector `hennery-host`'s signer is checked against too: a fixed key,
/// nonce and host id give this exact signature (Ed25519 is deterministic).
const VECTOR_SIGNATURE: &str = "bd2b7388413c333e9ed69c330b4a8be8ffb6228609979b30607236fcdefab259\
cdf6b48fb39bfaa9b5a3cd01538280ec9e6d50c8831e9aae4d791f68112a6c04";

#[test]
fn the_fixed_proof_vector_verifies() {
    let key = key(1);
    let nonce = [2u8; 32];
    let signature = proof(&key, &nonce, "host-1");
    assert_eq!(signature, VECTOR_SIGNATURE);
    assert!(verify_proof(
        &hex::encode(key.verifying_key().as_bytes()),
        &nonce,
        "host-1",
        "1.0",
        VECTOR_SIGNATURE
    ));
}

/// `pairing_codes` stays bounded: every mint first deletes the spent and
/// expired codes, and refuses once `MAX_LIVE_PAIRING_CODES` are live, so a
/// flood of mints cannot grow the table.
#[test]
fn minting_prunes_spent_and_expired_codes_and_caps_the_live_ones() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let rows = || {
        rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT count(*) FROM pairing_codes", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let first = hosts.mint_pairing_code(NOW).unwrap();
    let second = hosts.mint_pairing_code(NOW).unwrap();
    for _ in 2..MAX_LIVE_PAIRING_CODES {
        hosts.mint_pairing_code(NOW).unwrap();
    }
    let refused = hosts.mint_pairing_code(NOW).unwrap_err();
    assert!(refused.downcast_ref::<TooManyPairingCodes>().is_some(), "{refused:#}");
    assert_eq!(rows(), MAX_LIVE_PAIRING_CODES as i64);

    // `up`'s own mint at start is not capped.
    hosts.mint_local_pairing_code(NOW).unwrap();
    assert_eq!(rows(), MAX_LIVE_PAIRING_CODES as i64 + 1);
    let refused = hosts.mint_pairing_code(NOW).unwrap_err();
    assert!(refused.downcast_ref::<TooManyPairingCodes>().is_some(), "{refused:#}");

    // A spent code is no longer live, and goes at the next mint.
    enrolled(hosts.enroll(&first.code, &enrollment(&key(1)), NOW + 1).unwrap());
    hosts.mint_pairing_code(NOW + 1).unwrap_err();
    assert_eq!(rows(), MAX_LIVE_PAIRING_CODES as i64, "pruned even though refused");
    hosts.mint_pairing_code(NOW + 1).unwrap_err();
    enrolled(hosts.enroll(&second.code, &enrollment(&key(3)), NOW + 1).unwrap());
    hosts.mint_pairing_code(NOW + 1).unwrap();
    assert_eq!(rows(), MAX_LIVE_PAIRING_CODES as i64);

    // Once they have expired, every older code goes.
    let later = hosts.mint_pairing_code(NOW + PAIRING_CODE_TTL_SECS + 1).unwrap();
    assert_eq!(rows(), 1);
    enrolled(
        hosts
            .enroll(&later.code, &enrollment(&key(2)), NOW + PAIRING_CODE_TTL_SECS + 2)
            .unwrap(),
    );
}

/// Plan 8e decision E7: a `hello`'s per-agent MCP isolation is kept for the
/// host list, as of the latest one; none before the first.
#[test]
fn a_hellos_mcp_isolation_is_kept_as_of_the_latest() {
    use hennery_proto::frames::{AgentIsolation, McpIsolation};
    let hosts = Hosts::open_in_memory().unwrap();
    hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
    assert_eq!(hosts.host("host-1").unwrap().unwrap().mcp_isolation, None);
    let first = AgentIsolation(
        [
            ("claude".to_string(), McpIsolation::ClaudeStrict),
            ("codex".to_string(), McpIsolation::None),
        ]
        .into(),
    );
    hosts.record_mcp_isolation("host-1", &first).unwrap();
    assert_eq!(hosts.host("host-1").unwrap().unwrap().mcp_isolation, Some(first));
    let second = AgentIsolation([("codex".to_string(), McpIsolation::None)].into());
    hosts.record_mcp_isolation("host-1", &second).unwrap();
    assert_eq!(hosts.host("host-1").unwrap().unwrap().mcp_isolation, Some(second));
}

/// The host's own words, bounded: at most 32 agents, each id displayable
/// and at most 64 bytes.
#[test]
fn a_hellos_mcp_isolation_is_bounded() {
    use hennery_kernel::hosts::MAX_AGENTS;
    use hennery_proto::frames::{AgentIsolation, McpIsolation};
    let hosts = Hosts::open_in_memory().unwrap();
    hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
    let mut many: std::collections::BTreeMap<String, McpIsolation> =
        (0..40).map(|i| (format!("agent-{i:02}"), McpIsolation::None)).collect();
    many.insert("a".repeat(65), McpIsolation::ClaudeStrict);
    many.insert("bidi\u{202e}".into(), McpIsolation::ClaudeStrict);
    many.insert(String::new(), McpIsolation::ClaudeStrict);
    hosts.record_mcp_isolation("host-1", &AgentIsolation(many)).unwrap();
    let kept = hosts.host("host-1").unwrap().unwrap().mcp_isolation.unwrap();
    assert_eq!(kept.0.len(), MAX_AGENTS);
    assert!(kept.0.keys().all(|agent| agent.starts_with("agent-")), "{kept:?}");
}

/// A bidi-override agent id is dropped on its own, not merely outsorted by
/// `take(MAX_AGENTS)`: recorded alongside a single valid id, far below the
/// cap, only the valid one survives.
#[test]
fn a_bidi_override_agent_id_is_dropped_from_mcp_isolation() {
    use hennery_proto::frames::{AgentIsolation, McpIsolation};
    let hosts = Hosts::open_in_memory().unwrap();
    hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
    let isolation = AgentIsolation(
        [
            ("claude".to_string(), McpIsolation::ClaudeStrict),
            ("a\u{202e}".to_string(), McpIsolation::ClaudeStrict),
        ]
        .into(),
    );
    hosts.record_mcp_isolation("host-1", &isolation).unwrap();
    let kept = hosts.host("host-1").unwrap().unwrap().mcp_isolation.unwrap();
    assert_eq!(kept.0.keys().collect::<Vec<_>>(), vec!["claude"], "{kept:?}");
}
