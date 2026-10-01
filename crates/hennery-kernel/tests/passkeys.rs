//! Passkeys (kernel spec §3.2, §11; plan 3c), with a software passkey in
//! place of a browser and its authenticator (decision 10).

use hennery_kernel::operator::{Operator, PublicUrl, STEP_UP_SECS, SetupOutcome};
use hennery_kernel::passkeys::{
    CEREMONY_TTL_SECS, MAX_LABEL_CHARS, PasskeyRecord, Refused, Start, relying_party, user_handle,
};
use std::path::Path;
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RegisterPublicKeyCredential, Url, Uuid};

const NOW: i64 = 1_800_000_000;
const PASSWORD: &str = "correct horse battery";
const PUBLIC_URL: &str = "https://hennery.example";

/// Another owner, written straight into the database (none exists in v1),
/// created later than any real clock reaches (plan 3b-iii decision 3).
const OTHER: &str = "owner-00000000000000b2";
const OTHER_CREATED_AT: i64 = i64::MAX;

/// A software passkey that claims user verification, as a platform
/// authenticator does after a fingerprint or a PIN.
fn authenticator() -> WebauthnAuthenticator<SoftPasskey> {
    WebauthnAuthenticator::new(SoftPasskey::new(true))
}

/// `op` set up at `public_url`, and the id of a session it signed in.
fn set_up(op: &Operator, public_url: &str) -> String {
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, public_url, NOW).unwrap() else {
        panic!("setup failed");
    };
    let token = op.open_session("test", &phc, NOW).unwrap().unwrap();
    op.authenticate(&token, NOW).unwrap().unwrap().session_id
}

/// A registration begun by `session_id`: its ceremony id and the
/// authenticator's answer, made at `origin`.
fn begin_registration(
    op: &Operator,
    session_id: &str,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    origin: &str,
    label: &str,
) -> (String, serde_json::Value) {
    let Start::Begun { ceremony_id, options } = op.start_passkey_registration(session_id, label, NOW).unwrap() else {
        panic!("the registration did not begin");
    };
    let options: CreationChallengeResponse = serde_json::from_value(options).unwrap();
    let credential = authenticator
        .do_registration(Url::parse(origin).unwrap(), options)
        .unwrap();
    (ceremony_id, serde_json::to_value(credential).unwrap())
}

/// Register a passkey labelled `label` at `PUBLIC_URL`.
fn register(
    op: &Operator,
    session_id: &str,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    label: &str,
) -> PasskeyRecord {
    let (ceremony_id, credential) = begin_registration(op, session_id, authenticator, PUBLIC_URL, label);
    op.finish_passkey_registration(session_id, &ceremony_id, &credential, NOW)
        .unwrap()
        .expect("registered")
}

/// An operator on a file, so a test can also write the database itself.
fn on_file(path: &Path) -> (Operator, String) {
    let op = Operator::open(path).unwrap();
    let session_id = set_up(&op, PUBLIC_URL);
    (op, session_id)
}

/// Write `OTHER` into the database at `path`.
fn another_owner(path: &Path) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, ?2, ?2)",
            rusqlite::params![OTHER, OTHER_CREATED_AT],
        )
        .unwrap();
}

/// Decision 1: the RP id is `public_url`'s host, in lowercase; an IP
/// address has none.
#[test]
fn the_relying_party_is_the_public_urls_host() {
    for (public_url, rp_id) in [
        ("https://hennery.example", Some("hennery.example")),
        ("https://Hennery.Example:8443/", Some("hennery.example")),
        ("http://localhost:7117", Some("localhost")),
        ("http://127.0.0.1:7117", None),
        ("http://[::1]:7117", None),
        ("https://192.0.2.1", None),
    ] {
        let url = PublicUrl::parse(public_url).unwrap();
        assert_eq!(url.rp_id(), rp_id, "{public_url}");
        assert_eq!(relying_party(&url).is_some(), rp_id.is_some(), "{public_url}");
    }
}

/// Kernel spec §3.2: "subdomains and arbitrary ports not allowed". A
/// passkey made at `public_url`'s own origin registers; one made at a
/// subdomain, at another port or over another scheme does not.
#[test]
fn only_the_public_urls_own_origin_registers_a_passkey() {
    for (public_url, origin, registers) in [
        ("https://hennery.example", "https://hennery.example", true),
        ("https://hennery.example", "https://sub.hennery.example", false),
        ("https://hennery.example", "https://hennery.example:8443", false),
        ("http://localhost:7117", "http://localhost:7117", true),
        ("http://localhost:7117", "http://localhost:7118", false),
        ("https://localhost", "http://localhost", false),
    ] {
        let rp = relying_party(&PublicUrl::parse(public_url).unwrap()).unwrap();
        let (options, state) = rp
            .start_passkey_registration(Uuid::nil(), "owner", "owner", None)
            .unwrap();
        let credential = authenticator()
            .do_registration(Url::parse(origin).unwrap(), options)
            .unwrap();
        assert_eq!(
            rp.finish_passkey_registration(&credential, &state).is_ok(),
            registers,
            "{origin} for {public_url}"
        );
    }
}

/// Decision 3: the user handle is the owner's, the same every time, and
/// another owner's differs.
#[test]
fn the_user_handle_is_derived_from_the_owner() {
    assert_eq!(user_handle("owner-a"), user_handle("owner-a"));
    assert_ne!(user_handle("owner-a"), user_handle("owner-b"));
    assert_ne!(user_handle("owner-a"), Uuid::nil());
}

/// Decision 3: the `passkeys` table carries `owner_id` from its first
/// migration, a foreign key to `owners`, and a credential id unique across
/// owners.
#[test]
fn the_passkeys_table_carries_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    Operator::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let owner_id_not_null: bool = conn
        .query_row(
            "SELECT \"notnull\" FROM pragma_table_info('passkeys') WHERE name = 'owner_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(owner_id_not_null);
    let parent: String = conn
        .query_row(
            "SELECT \"table\" FROM pragma_foreign_key_list('passkeys') WHERE \"from\" = 'owner_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parent, "owners");
    let unique: bool = conn
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_index_list('passkeys') l
                 JOIN pragma_index_info(l.name) i WHERE l.\"unique\" = 1 AND i.name = 'credential_id')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(unique);
}

/// Kernel spec §3.2: several passkeys per owner, each with a label and a
/// last-used time. A second registration excludes the first credential.
#[test]
fn passkeys_register_and_are_listed() {
    let op = Operator::open_in_memory().unwrap();
    let session_id = set_up(&op, PUBLIC_URL);
    let mut laptop = authenticator();
    let first = register(&op, &session_id, &mut laptop, "  laptop ");
    assert_eq!(first.label, "laptop");
    assert_eq!(first.created_at, NOW);
    assert_eq!(first.last_used_at, None);
    assert!(first.id.starts_with("passkey-"));

    let Start::Begun { ceremony_id, options } = op.start_passkey_registration(&session_id, "phone", NOW).unwrap()
    else {
        panic!("the registration did not begin");
    };
    let excluded = options["publicKey"]["excludeCredentials"].as_array().unwrap();
    assert_eq!(excluded.len(), 1);
    // User verification is required (3c review, O2): pinned here, so a
    // version or a feature that relaxed it would fail.
    assert_eq!(
        options["publicKey"]["authenticatorSelection"]["userVerification"],
        "required"
    );
    let options: CreationChallengeResponse = serde_json::from_value(options).unwrap();
    let credential = authenticator()
        .do_registration(Url::parse(PUBLIC_URL).unwrap(), options)
        .unwrap();
    let second = op
        .finish_passkey_registration(
            &session_id,
            &ceremony_id,
            &serde_json::to_value(credential).unwrap(),
            NOW,
        )
        .unwrap()
        .unwrap();
    assert_eq!(op.passkeys().unwrap(), vec![first, second]);
}

/// Decision 5: a registration ceremony is taken once, by the session that
/// began it, within its time.
#[test]
fn a_registration_ceremony_is_single_use_and_bound_to_its_session() {
    let op = Operator::open_in_memory().unwrap();
    let session_id = set_up(&op, PUBLIC_URL);
    let mut passkey = authenticator();

    // Another session's finish uses the ceremony up, though that session
    // is live and stepped up too.
    let phc = op.verify_password(PASSWORD).unwrap().unwrap();
    let token = op.open_session("other", &phc, NOW).unwrap().unwrap();
    let other = op.authenticate(&token, NOW).unwrap().unwrap().session_id;
    let (ceremony_id, credential) = begin_registration(&op, &session_id, &mut passkey, PUBLIC_URL, "laptop");
    assert_eq!(
        op.finish_passkey_registration(&other, &ceremony_id, &credential, NOW)
            .unwrap(),
        Err(Refused::Ceremony)
    );
    assert_eq!(
        op.finish_passkey_registration(&session_id, &ceremony_id, &credential, NOW)
            .unwrap(),
        Err(Refused::Ceremony)
    );

    // A finished ceremony cannot be finished again.
    let (ceremony_id, credential) = begin_registration(&op, &session_id, &mut passkey, PUBLIC_URL, "laptop");
    assert!(
        op.finish_passkey_registration(&session_id, &ceremony_id, &credential, NOW)
            .unwrap()
            .is_ok()
    );
    assert_eq!(
        op.finish_passkey_registration(&session_id, &ceremony_id, &credential, NOW)
            .unwrap(),
        Err(Refused::Ceremony)
    );

    // An expired one is refused.
    let (ceremony_id, credential) = begin_registration(&op, &session_id, &mut passkey, PUBLIC_URL, "late");
    assert_eq!(
        op.finish_passkey_registration(&session_id, &ceremony_id, &credential, NOW + CEREMONY_TTL_SECS)
            .unwrap(),
        Err(Refused::Ceremony)
    );
    assert_eq!(op.passkeys().unwrap().len(), 1);
}

/// Decision 7 (3c review, A2): the registration's write itself requires
/// its session to be live and stepped up, so a finish that lands after the
/// session ended, or after its step-up lapsed, stores nothing. The
/// operator is called directly, past the routes' step-up layer: the check
/// in the write is what is tested.
#[test]
fn a_registration_finished_after_its_session_ended_stores_nothing() {
    let op = Operator::open_in_memory().unwrap();
    let session_id = set_up(&op, PUBLIC_URL);
    let (ceremony_id, credential) = begin_registration(&op, &session_id, &mut authenticator(), PUBLIC_URL, "laptop");
    assert!(op.revoke_session(&session_id, NOW).unwrap());
    assert_eq!(
        op.finish_passkey_registration(&session_id, &ceremony_id, &credential, NOW)
            .unwrap(),
        Err(Refused::Ceremony)
    );
    assert!(op.passkeys().unwrap().is_empty());

    // A session whose last check is `STEP_UP_SECS` old.
    let phc = op.verify_password(PASSWORD).unwrap().unwrap();
    let token = op.open_session("old", &phc, NOW - STEP_UP_SECS).unwrap().unwrap();
    let stale = op.authenticate(&token, NOW).unwrap().unwrap();
    assert!(!stale.stepped_up(NOW));
    let (ceremony_id, credential) =
        begin_registration(&op, &stale.session_id, &mut authenticator(), PUBLIC_URL, "laptop");
    assert_eq!(
        op.finish_passkey_registration(&stale.session_id, &ceremony_id, &credential, NOW)
            .unwrap(),
        Err(Refused::Ceremony)
    );
    assert!(op.passkeys().unwrap().is_empty());
}

/// A passkey made at another origin, or an answer that is not a
/// credential, is refused and stores nothing.
#[test]
fn a_registration_that_does_not_verify_stores_nothing() {
    let op = Operator::open_in_memory().unwrap();
    let session_id = set_up(&op, PUBLIC_URL);
    let mut passkey = authenticator();
    let (ceremony_id, credential) =
        begin_registration(&op, &session_id, &mut passkey, "https://sub.hennery.example", "laptop");
    assert!(matches!(
        op.finish_passkey_registration(&session_id, &ceremony_id, &credential, NOW)
            .unwrap(),
        Err(Refused::Credential(_))
    ));
    let (ceremony_id, _) = begin_registration(&op, &session_id, &mut passkey, PUBLIC_URL, "laptop");
    assert!(matches!(
        op.finish_passkey_registration(&session_id, &ceremony_id, &serde_json::json!({ "id": 1 }), NOW)
            .unwrap(),
        Err(Refused::Credential(_))
    ));
    assert!(op.passkeys().unwrap().is_empty());
}

/// Decision 3: a credential id is unique across owners, so a credential
/// already stored, even another owner's, is refused and nothing changes.
#[test]
fn a_credential_registered_already_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    let (op, session_id) = on_file(&path);
    another_owner(&path);
    let (ceremony_id, credential) = begin_registration(&op, &session_id, &mut authenticator(), PUBLIC_URL, "laptop");
    let raw: RegisterPublicKeyCredential = serde_json::from_value(credential.clone()).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO passkeys(id, owner_id, credential_id, credential, sign_count, label, created_at)
             VALUES ('passkey-theirs', ?1, ?2, '{}', 0, 'theirs', ?3)",
            rusqlite::params![OTHER, hex::encode(raw.raw_id.as_ref() as &[u8]), NOW],
        )
        .unwrap();
    assert_eq!(
        op.finish_passkey_registration(&session_id, &ceremony_id, &credential, NOW)
            .unwrap(),
        Err(Refused::AlreadyRegistered)
    );
    assert!(op.passkeys().unwrap().is_empty());
}

#[test]
fn a_label_is_one_to_64_characters_without_control_characters() {
    let op = Operator::open_in_memory().unwrap();
    let session_id = set_up(&op, PUBLIC_URL);
    let longest = "x".repeat(MAX_LABEL_CHARS);
    for label in ["", "   ", &format!("{longest}y"), "lap\ntop", "lap\u{7f}top"] {
        assert!(
            matches!(
                op.start_passkey_registration(&session_id, label, NOW).unwrap(),
                Start::Invalid(_)
            ),
            "{label:?}"
        );
    }
    for label in [longest.as_str(), "ノートパソコン"] {
        assert!(
            matches!(
                op.start_passkey_registration(&session_id, label, NOW).unwrap(),
                Start::Begun { .. }
            ),
            "{label:?}"
        );
    }
}

/// Decision 1: without a `public_url`, or with one whose host is an IP
/// address, there is no relying party.
#[test]
fn passkeys_are_unavailable_without_a_host_name() {
    let op = Operator::open_in_memory().unwrap();
    assert_eq!(
        op.start_passkey_registration("session", "laptop", NOW).unwrap(),
        Start::Unavailable
    );
    let session_id = set_up(&op, "http://127.0.0.1:7117");
    assert_eq!(
        op.start_passkey_registration(&session_id, "laptop", NOW).unwrap(),
        Start::Unavailable
    );
}

#[test]
fn a_passkey_is_removed_once() {
    let op = Operator::open_in_memory().unwrap();
    let session_id = set_up(&op, PUBLIC_URL);
    let kept = register(&op, &session_id, &mut authenticator(), "kept");
    let removed = register(&op, &session_id, &mut authenticator(), "removed");
    assert!(op.remove_passkey(&removed.id).unwrap());
    assert!(!op.remove_passkey(&removed.id).unwrap());
    assert!(!op.remove_passkey("passkey-unknown").unwrap());
    assert_eq!(op.passkeys().unwrap(), vec![kept]);
}

/// Another owner's passkeys are not listed, not removable, and not
/// excluded from a registration.
#[test]
fn another_owners_passkeys_are_invisible_to_registration_and_the_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hennery.db");
    let (op, session_id) = on_file(&path);
    another_owner(&path);
    let mine = register(&op, &session_id, &mut authenticator(), "mine");
    let theirs = register(&op, &session_id, &mut authenticator(), "theirs");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE passkeys SET owner_id = ?1 WHERE id = ?2",
            rusqlite::params![OTHER, theirs.id],
        )
        .unwrap();
    assert_eq!(op.passkeys().unwrap(), vec![mine]);
    assert!(!op.remove_passkey(&theirs.id).unwrap());
    let Start::Begun { options, .. } = op.start_passkey_registration(&session_id, "third", NOW).unwrap() else {
        panic!("the registration did not begin");
    };
    assert_eq!(options["publicKey"]["excludeCredentials"].as_array().unwrap().len(), 1);
    let still_there: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM passkeys WHERE owner_id = ?1", [OTHER], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(still_there, 1);
}
