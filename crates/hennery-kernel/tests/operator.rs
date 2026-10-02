//! The operator and their setup (kernel spec §3.1, §3.2, §11): the
//! one-time setup token, the owner's password and the `public_url`, and
//! the admin socket's recoveries (kernel spec §4.2).

use hennery_kernel::operator::{
    MAX_PASSWORD_BYTES, Operator, PublicUrl, PublicUrlChange, Reset, SETUP_TOKEN_TTL_SECS, SETUP_URL_FILE,
    STEP_UP_SECS, SetupOutcome,
};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

const NOW: i64 = 1_800_000_000;
const PASSWORD: &str = "correct horse battery";

#[test]
fn a_setup_token_is_single_use_and_sets_the_owner_up() {
    let op = Operator::open_in_memory().unwrap();
    let owner = op.owner_id().to_string();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    assert_eq!(
        op.set_up("0".repeat(64).as_str(), PASSWORD, "https://hennery.example", NOW)
            .unwrap(),
        SetupOutcome::InvalidToken
    );
    let SetupOutcome::Done { owner_id, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap()
    else {
        panic!("setup failed");
    };
    assert_eq!(owner_id, owner);
    assert_eq!(op.owner_id(), owner);
    assert!(op.is_set_up().unwrap());
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
    assert!(!op.is_set_up().unwrap());
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
    assert_eq!(op.verify_password(PASSWORD).unwrap(), None);
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    assert_eq!(op.verify_password(PASSWORD).unwrap(), Some(phc));
    assert_eq!(op.verify_password("correct horse battery!").unwrap(), None);
    assert_eq!(op.verify_password(&"x".repeat(MAX_PASSWORD_BYTES + 1)).unwrap(), None);
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
        results.push(check.await.unwrap().is_some());
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

/// A hard link planted at `setup-url` is replaced too, never written
/// through: its other name keeps its contents, and the two names no longer
/// share an inode (plan 3b-i, "Found in execution").
#[test]
fn a_hard_link_at_the_setup_link_is_replaced_and_its_other_name_left_alone() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere");
    std::fs::write(&target, "untouched").unwrap();
    std::fs::hard_link(&target, dir.path().join(SETUP_URL_FILE)).unwrap();
    let op = Operator::open_in_memory().unwrap();
    let link = op
        .announce_setup(dir.path(), "http://localhost:1", NOW)
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
    assert_eq!(std::fs::read_to_string(&link.file).unwrap(), format!("{}\n", link.url));
    let (target, file) = (
        std::fs::metadata(&target).unwrap(),
        std::fs::metadata(&link.file).unwrap(),
    );
    assert_ne!(file.ino(), target.ino(), "setup-url still shares the target's inode");
    assert_eq!((file.nlink(), target.nlink()), (1, 1));
    assert_eq!(mode(&link.file), 0o600);
}

/// A failing assertion or a log line that shows a `SetupLink` must not
/// show the live setup token.
#[test]
fn a_setup_link_does_not_show_its_token_in_debug() {
    let dir = tempfile::tempdir().unwrap();
    let op = Operator::open_in_memory().unwrap();
    let link = op
        .announce_setup(dir.path(), "http://localhost:1", NOW)
        .unwrap()
        .unwrap();
    let token = link.url.rsplit('#').next().unwrap();
    let shown = format!("{link:?}");
    assert!(!shown.contains(token) && shown.contains(SETUP_URL_FILE), "{shown}");
}

/// The admin socket's password reset (kernel spec §4.2): the new password
/// verifies and the old does not, every session ends (and the ending is
/// announced, so their streams end), and the login and step-up lockouts
/// are lifted. A password setup would refuse changes nothing, and there is
/// nothing to reset before setup.
#[tokio::test]
async fn a_password_reset_replaces_the_password_and_ends_every_session() {
    let op = Arc::new(Operator::open_in_memory().unwrap());
    assert_eq!(
        op.reset_password("a new long password".into(), NOW).await.unwrap(),
        Reset::NotSetUp
    );
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    let sessions = [
        op.open_session("a", &phc, NOW).unwrap().unwrap(),
        op.open_session("b", &phc, NOW).unwrap().unwrap(),
    ];
    let peer: std::net::IpAddr = "192.0.2.1".parse().unwrap();
    for _ in 0..5 {
        let _ = op.login_limiter.attempt(peer, std::time::Instant::now());
        let _ = op.step_up_limiter.attempt(peer, std::time::Instant::now());
    }
    assert!(op.login_limiter.attempt(peer, std::time::Instant::now()).is_err());
    for _ in 0..hennery_kernel::ratelimit::Policy::PASSKEY_LOGIN.free_failures {
        let _ = op.passkey_limiter.attempt(peer, std::time::Instant::now());
    }
    assert!(op.passkey_limiter.attempt(peer, std::time::Instant::now()).is_err());
    let ends = op.session_ends();

    assert_eq!(
        op.reset_password("short".into(), NOW).await.unwrap(),
        Reset::Invalid("the password must be at least 12 characters".into())
    );
    assert!(op.verify_password(PASSWORD).unwrap().is_some());
    assert!(op.authenticate(&sessions[0], NOW).unwrap().is_some());

    assert_eq!(
        op.reset_password("a new long password".into(), NOW + 1).await.unwrap(),
        Reset::Done {
            sessions_ended: 2,
            passkeys_removed: 0
        }
    );
    assert!(op.verify_password("a new long password").unwrap().is_some());
    assert!(op.verify_password(PASSWORD).unwrap().is_none());
    for session in &sessions {
        assert!(op.authenticate(session, NOW + 1).unwrap().is_none());
    }
    assert!(ends.has_changed().unwrap(), "the ending was not announced");
    assert!(op.login_limiter.attempt(peer, std::time::Instant::now()).is_ok());
    assert!(op.step_up_limiter.attempt(peer, std::time::Instant::now()).is_ok());
    assert!(op.passkey_limiter.attempt(peer, std::time::Instant::now()).is_ok());
}

/// The admin socket's `public_url` reset (3b decision 4's recovery): the
/// origin every browser request is checked against changes at once, not
/// only the stored row (a new `Operator` on the file reads it back), and
/// every session ends. An invalid URL changes nothing.
#[test]
fn a_public_url_reset_replaces_the_cached_origin_and_ends_every_session() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let op = Operator::open(&db).unwrap();
    assert_eq!(op.reset_public_url("https://moved.example").unwrap(), Reset::NotSetUp);
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "http://localhost:7117", NOW).unwrap() else {
        panic!("setup failed");
    };
    let session = op.open_session("browser", &phc, NOW).unwrap().unwrap();
    let ends = op.session_ends();

    let refused = op.reset_public_url("http://moved.example").unwrap();
    assert!(matches!(refused, Reset::Invalid(_)), "{refused:?}");
    assert_eq!(op.public_url().unwrap().origin(), "http://localhost:7117");
    assert!(op.authenticate(&session, NOW).unwrap().is_some());

    assert_eq!(
        op.reset_public_url("https://Moved.Example/").unwrap(),
        Reset::Done {
            sessions_ended: 1,
            passkeys_removed: 0
        }
    );
    assert_eq!(op.public_url().unwrap().origin(), "https://moved.example");
    assert!(op.public_url().unwrap().is_https());
    assert!(op.authenticate(&session, NOW).unwrap().is_none());
    assert!(ends.has_changed().unwrap(), "the ending was not announced");
    drop(op);
    let reopened = Operator::open(&db).unwrap();
    assert_eq!(reopened.public_url().unwrap().origin(), "https://moved.example");
}

/// Plan 4d-B4 decision 1: `PATCH /api/settings` changes `public_url` and
/// the push contact through `change_public_url`, in one transaction. Both
/// are checked before anything is written: a refused contact keeps the
/// old URL and its sessions, a refused URL the old contact. `None` leaves
/// the contact as it is, `Some(None)` clears it. `Done` names the origin
/// left and the one taken (the review's A2).
#[test]
fn a_public_url_change_sets_the_contact_in_its_transaction_or_changes_nothing() {
    let op = Operator::open_in_memory().unwrap();
    assert_eq!(
        op.change_public_url("https://moved.example", Some(Some("me@example.com")), None)
            .unwrap(),
        PublicUrlChange::NotSetUp
    );
    assert_eq!(op.contact().unwrap(), None);
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    op.set_contact(Some("old@example.com")).unwrap().unwrap();
    let session = op.open_session("browser", &phc, NOW).unwrap().unwrap();
    let unchanged = |op: &Operator| {
        assert_eq!(op.public_url().unwrap().origin(), "https://hennery.example");
        assert_eq!(op.contact().unwrap().as_deref(), Some("old@example.com"));
        assert!(op.authenticate(&session, NOW).unwrap().is_some());
    };

    let refused = op
        .change_public_url("https://moved.example", Some(Some("me@example.com?cc=x")), None)
        .unwrap();
    assert!(matches!(refused, PublicUrlChange::Invalid(_)), "{refused:?}");
    unchanged(&op);
    let refused = op
        .change_public_url("http://moved.example", Some(Some("me@example.com")), None)
        .unwrap();
    assert!(matches!(refused, PublicUrlChange::Invalid(_)), "{refused:?}");
    unchanged(&op);

    assert_eq!(
        op.change_public_url("https://moved.example", Some(Some("me@example.com")), None)
            .unwrap(),
        PublicUrlChange::Done {
            from: Some(PublicUrl::parse("https://hennery.example").unwrap()),
            to: PublicUrl::parse("https://moved.example").unwrap(),
            sessions_ended: 1,
            passkeys_removed: 0
        }
    );
    assert_eq!(op.public_url().unwrap().origin(), "https://moved.example");
    assert_eq!(op.contact().unwrap().as_deref(), Some("me@example.com"));
    assert!(op.authenticate(&session, NOW).unwrap().is_none());

    op.change_public_url("https://hennery.example", None, None).unwrap();
    assert_eq!(op.contact().unwrap().as_deref(), Some("me@example.com"));
    op.change_public_url("https://moved.example", Some(None), None).unwrap();
    assert_eq!(op.contact().unwrap(), None);
}

/// The review's A1: the change re-checks its caller's session in its
/// transaction, live and stepped up, as a passkey registration's write
/// does. A session revoked, or a step-up lapsed, since the request's own
/// checks changes nothing at all, and announces no ending.
#[test]
fn a_change_whose_caller_ended_or_stepped_down_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let op = Operator::open(&db).unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    let id = |token: &str| op.authenticate(token, NOW).unwrap().unwrap().session_id;
    let revoked = id(&op.open_session("browser", &phc, NOW).unwrap().unwrap());
    let kept = op.open_session("browser", &phc, NOW).unwrap().unwrap();
    let kept_id = id(&kept);
    assert!(op.revoke_session(&revoked, NOW).unwrap());
    let dump = || {
        let conn = rusqlite::Connection::open(&db).unwrap();
        ["owners", "settings", "auth_sessions", "push_subscriptions", "passkeys"].map(|table| {
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
            let columns = stmt.column_count();
            stmt.query_map([], |r| {
                Ok((0..columns)
                    .map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap()))
                    .collect::<Vec<_>>())
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        })
    };
    let before = dump();
    let mut ends = op.session_ends();
    ends.borrow_and_update();

    for (caller, now, outcome) in [
        ((revoked.as_str(), NOW), NOW, PublicUrlChange::SignedOut),
        // Exactly five minutes after the last check: no longer fresh.
        (
            (kept_id.as_str(), NOW + STEP_UP_SECS),
            NOW + STEP_UP_SECS,
            PublicUrlChange::StepUpRequired,
        ),
    ] {
        assert_eq!(
            op.change_public_url("https://moved.example", Some(Some("me@example.com")), Some(caller))
                .unwrap(),
            outcome
        );
        assert_eq!(dump(), before, "{outcome:?}");
        assert_eq!(op.public_url().unwrap().origin(), "https://hennery.example");
        assert!(op.authenticate(&kept, now).unwrap().is_some());
    }
    assert!(!ends.has_changed().unwrap(), "an ending was announced");

    // A second inside the window, the change is made.
    assert!(matches!(
        op.change_public_url("https://moved.example", None, Some((&kept_id, NOW + STEP_UP_SECS - 1)))
            .unwrap(),
        PublicUrlChange::Done { sessions_ended: 1, .. }
    ));
}

/// The admin socket's `setup-url` (kernel spec §4.2): the link announced
/// at start while its token is live, a fresh one (and a fresh file) once it
/// has expired, and none once set up.
#[test]
fn the_setup_link_is_the_live_one_or_a_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let op = Operator::open_in_memory().unwrap();
    let base = "http://localhost:7117";
    let first = op.announce_setup(dir.path(), base, NOW).unwrap().unwrap();
    let again = op.setup_link(dir.path(), base, NOW + 1).unwrap().unwrap();
    assert_eq!(again, first);
    let later = NOW + SETUP_TOKEN_TTL_SECS;
    let fresh = op.setup_link(dir.path(), base, later).unwrap().unwrap();
    assert_ne!(fresh.url, first.url);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(SETUP_URL_FILE))
            .unwrap()
            .trim_end(),
        fresh.url
    );
    let token = fresh.url.rsplit_once('#').unwrap().1;
    assert!(matches!(
        op.set_up(token, PASSWORD, "https://hennery.example", later).unwrap(),
        SetupOutcome::Done { .. }
    ));
    assert_eq!(op.setup_link(dir.path(), base, later).unwrap(), None);
    assert!(!dir.path().join(SETUP_URL_FILE).exists());
}

/// 3b-ii decision 11, amended (A1): a login whose password check ran just
/// before a reset gets no session afterwards. The session is opened on the
/// PHC string the check verified, and the reset replaced it; a check
/// against the new password opens one.
#[tokio::test]
async fn a_login_checked_before_a_password_reset_opens_no_session_after_it() {
    let op = Arc::new(Operator::open_in_memory().unwrap());
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap();
    let stale = op
        .check_password(PASSWORD.into())
        .await
        .unwrap()
        .expect("the old password");
    assert_eq!(
        op.reset_password("a new long password".into(), NOW).await.unwrap(),
        Reset::Done {
            sessions_ended: 0,
            passkeys_removed: 0
        }
    );
    assert_eq!(op.open_session("browser", &stale, NOW).unwrap(), None);
    assert!(op.sessions(NOW).unwrap().is_empty());
    let fresh = op
        .check_password("a new long password".into())
        .await
        .unwrap()
        .expect("the new password");
    assert!(op.open_session("browser", &fresh, NOW).unwrap().is_some());
}

/// 3b-ii's O9: a reset replaces the password only when there is exactly
/// one to replace. With none (deleted here; a passkey-only owner, in 3c)
/// it fails and changes nothing: the sessions stay.
#[tokio::test]
async fn a_password_reset_with_no_password_to_replace_fails_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let op = Arc::new(Operator::open(&db).unwrap());
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    let session = op.open_session("browser", &phc, NOW).unwrap().unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute("DELETE FROM password_credentials", []).unwrap();
    let err = op
        .reset_password("a new long password".into(), NOW)
        .await
        .expect_err("a reset with no password to replace succeeded");
    assert!(format!("{err:#}").contains("no password to reset"), "{err:#}");
    assert!(op.authenticate(&session, NOW).unwrap().is_some());
    let left: i64 = conn
        .query_row("SELECT count(*) FROM password_credentials", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
}
