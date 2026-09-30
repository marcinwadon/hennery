//! The host registry (kernel spec §4, §11): pairing codes, enrollment, the
//! `hello` proof and revocation.

use ed25519_dalek::{Signer, SigningKey};
use hennery_kernel::hosts::{
    EnrollOutcome, Enrollment, HelloCheck, Hosts, PAIRING_CODE_TTL_SECS, Registered, Revoke, normalize_code,
    verify_proof,
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
