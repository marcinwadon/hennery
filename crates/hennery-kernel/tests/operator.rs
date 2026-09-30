//! The operator and their setup (kernel spec §3.1, §3.2, §11): the
//! one-time setup token, the owner's password and the `public_url`.

use hennery_kernel::operator::{
    MAX_PASSWORD_BYTES, Operator, PublicUrl, SETUP_TOKEN_TTL_SECS, SETUP_URL_FILE, SetupOutcome,
};
use std::os::unix::fs::PermissionsExt;
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

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Kernel spec §3.1: the link goes to `setup-url`, 0600, and is gone once
/// setup is done.
#[test]
fn the_setup_link_is_written_privately_and_removed_by_setup() {
    let dir = tempfile::tempdir().unwrap();
    let op = Operator::open(&dir.path().join("hennery.db")).unwrap();
    let link = op
        .announce_setup(dir.path(), "http://localhost:7117", NOW)
        .unwrap()
        .unwrap();
    assert_eq!(link.file, dir.path().join(SETUP_URL_FILE));
    assert_eq!(mode(&link.file), 0o600);
    assert_eq!(std::fs::read_to_string(&link.file).unwrap(), format!("{}\n", link.url));
    let token = link.url.strip_prefix("http://localhost:7117/setup#").unwrap();
    assert!(
        token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()),
        "{token}"
    );
    // No temporary file is left behind.
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        4,
        "hennery.db, -wal, -shm and setup-url"
    );

    assert!(matches!(
        op.set_up(token, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::Done { .. }
    ));
    assert!(!link.file.exists());
    // Set up: nothing is written, and a stale link is removed.
    std::fs::write(&link.file, "stale").unwrap();
    assert_eq!(
        op.announce_setup(dir.path(), "http://localhost:7117", NOW).unwrap(),
        None
    );
    assert!(!link.file.exists());
}

/// A second announcement replaces the first link, and only its token works.
#[test]
fn a_new_setup_link_replaces_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let op = Operator::open_in_memory().unwrap();
    let first = op
        .announce_setup(dir.path(), "http://localhost:1", NOW)
        .unwrap()
        .unwrap();
    let second = op
        .announce_setup(dir.path(), "http://localhost:1/", NOW)
        .unwrap()
        .unwrap();
    assert_ne!(first.url, second.url);
    assert_eq!(
        std::fs::read_to_string(&second.file).unwrap(),
        format!("{}\n", second.url)
    );
    let old = first.url.rsplit('#').next().unwrap();
    assert_eq!(
        op.set_up(old, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::InvalidToken
    );
}

/// A symlink planted at `setup-url` is replaced, never written through: the
/// token must not land in a file someone else chose.
#[test]
fn a_symlink_at_the_setup_link_is_replaced_and_its_target_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere");
    std::fs::write(&target, "untouched").unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join(SETUP_URL_FILE)).unwrap();
    let op = Operator::open_in_memory().unwrap();
    let link = op
        .announce_setup(dir.path(), "http://localhost:1", NOW)
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
    assert!(!std::fs::symlink_metadata(&link.file).unwrap().file_type().is_symlink());
    assert_eq!(mode(&link.file), 0o600);
}
