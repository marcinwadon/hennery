//! The operator and their setup (kernel spec §3.1, §3.2, §11): the
//! one-time setup token, the owner's password and the `public_url`.

use hennery_kernel::operator::{MAX_PASSWORD_BYTES, Operator, PublicUrl, SETUP_TOKEN_TTL_SECS, SetupOutcome};
use std::sync::Arc;

const NOW: i64 = 1_800_000_000;
const PASSWORD: &str = "correct horse battery";

#[test]
fn a_setup_token_is_single_use_and_creates_one_owner() {
    let op = Operator::open_in_memory().unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    assert_eq!(
        op.set_up("0".repeat(64).as_str(), PASSWORD, "https://hennery.example", NOW)
            .unwrap(),
        SetupOutcome::InvalidToken
    );
    let SetupOutcome::Done { owner_id } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    assert_eq!(op.owner_id().unwrap(), Some(owner_id));
    assert_eq!(
        op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::AlreadySetUp
    );
    assert_eq!(op.issue_setup_token(NOW).unwrap(), None);
    assert_eq!(op.public_url().unwrap().origin(), "https://hennery.example");
}

#[test]
fn a_setup_token_expires_after_an_hour() {
    let op = Operator::open_in_memory().unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let later = NOW + SETUP_TOKEN_TTL_SECS;
    assert_eq!(
        op.set_up(&token, PASSWORD, "https://hennery.example", later).unwrap(),
        SetupOutcome::InvalidToken
    );
    assert_eq!(op.owner_id().unwrap(), None);
}

/// Kernel spec §3.1: a restart before setup issues a new token and
/// invalidates the old.
#[test]
fn a_restart_before_setup_invalidates_the_old_token() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let old = Operator::open(&db).unwrap().issue_setup_token(NOW).unwrap().unwrap();
    let op = Operator::open(&db).unwrap();
    assert_eq!(
        op.set_up(&old, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::InvalidToken
    );
    let new = op.issue_setup_token(NOW).unwrap().unwrap();
    assert_ne!(old, new);
    assert!(matches!(
        op.set_up(&new, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::Done { .. }
    ));
    // The owner and the public URL survive a restart.
    let reopened = Operator::open(&db).unwrap();
    assert!(reopened.is_set_up().unwrap());
    assert_eq!(reopened.public_url().unwrap().origin(), "https://hennery.example");
}

#[test]
fn a_bad_password_or_public_url_is_refused_and_keeps_the_token() {
    let op = Operator::open_in_memory().unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    for (password, url) in [
        ("short", "https://hennery.example"),
        (&*"x".repeat(MAX_PASSWORD_BYTES + 1), "https://hennery.example"),
        (PASSWORD, "http://hennery.example"),
        (PASSWORD, "https://hennery.example/app"),
    ] {
        assert!(
            matches!(op.set_up(&token, password, url, NOW).unwrap(), SetupOutcome::Invalid(_)),
            "{password:?} {url:?}"
        );
    }
    assert!(matches!(
        op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::Done { .. }
    ));
}

#[test]
fn only_the_owners_password_verifies_and_every_check_is_counted() {
    let op = Operator::open_in_memory().unwrap();
    // Before setup: a dummy hash, and never a match.
    assert!(!op.verify_password(PASSWORD).unwrap());
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap();
    assert!(op.verify_password(PASSWORD).unwrap());
    assert!(!op.verify_password("correct horse battery!").unwrap());
    assert!(!op.verify_password(&"x".repeat(MAX_PASSWORD_BYTES + 1)).unwrap());
    assert_eq!(op.verifications(), 4);
}

/// The public URL is kept as a browser serialises `Origin`, so a
/// `public_url` typed another way still matches (review focus 3).
#[test]
fn public_urls_are_normalised_to_their_browser_origin() {
    for (input, origin, https) in [
        ("https://Hennery.Example", "https://hennery.example", true),
        ("https://hennery.example:443/", "https://hennery.example", true),
        ("https://hennery.example:8443", "https://hennery.example:8443", true),
        ("http://localhost:7117", "http://localhost:7117", false),
        ("http://127.0.0.1:80", "http://127.0.0.1", false),
        ("http://[::1]:7117/", "http://[::1]:7117", false),
        (" HTTPS://hennery.example ", "https://hennery.example", true),
    ] {
        let url = PublicUrl::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!((url.origin(), url.is_https()), (origin, https), "{input}");
    }
    for bad in [
        "http://hennery.example",
        "http://192.168.1.2:7117",
        "ftp://hennery.example",
        "https://user:pw@hennery.example",
        "https://hennery.example/path",
        "https://hennery.example/?q=1",
        "https://hennery.example/#f",
        "hennery.example",
        "http://localhost.evil.example",
    ] {
        assert!(PublicUrl::parse(bad).is_err(), "{bad}");
    }
}

#[tokio::test]
async fn concurrent_checks_each_verify_once_and_answer_their_own_password() {
    let op = Arc::new(Operator::open_in_memory().unwrap());
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap();
    let checks: Vec<_> = (0..4)
        .map(|i| {
            let op = op.clone();
            let password = if i % 2 == 0 {
                PASSWORD.to_string()
            } else {
                "wrong".to_string()
            };
            tokio::spawn(async move { op.check_password(password).await.unwrap() })
        })
        .collect();
    let mut results = Vec::new();
    for check in checks {
        results.push(check.await.unwrap());
    }
    assert_eq!(results, [true, false, true, false]);
    assert_eq!(op.verifications(), 4);
}
