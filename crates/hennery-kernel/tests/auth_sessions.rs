//! Signed-in sessions (kernel spec §3.2, §3.4): a random token in a cookie,
//! stored hashed, with a sliding 30-day expiry and a step-up stamp.

use axum::http::{HeaderMap, HeaderValue, header};
use hennery_kernel::operator::{
    Operator, SESSION_SLIDE_SECS, SESSION_TTL_SECS, STEP_UP_SECS, cleared_cookie, session_cookie, session_token,
};

const NOW: i64 = 1_800_000_000;

fn set_up(op: &Operator) {
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, "correct horse battery", "https://hennery.example", NOW)
        .unwrap();
}

#[test]
fn there_is_no_session_before_setup() {
    let op = Operator::open_in_memory().unwrap();
    assert_eq!(op.open_session("browser", NOW).unwrap(), None);
}

#[test]
fn a_session_authenticates_until_it_expires_and_use_slides_its_expiry() {
    let op = Operator::open_in_memory().unwrap();
    set_up(&op);
    let token = op.open_session("browser", NOW).unwrap().unwrap();
    assert_eq!(token.len(), 64);

    // Used again within the slide interval: nothing is written.
    let first = op.authenticate(&token, NOW + 1).unwrap().unwrap();
    assert_eq!((first.expires_at, first.slid), (NOW + SESSION_TTL_SECS, false));
    // Used later: the expiry moves to 30 days from then.
    let later = NOW + SESSION_SLIDE_SECS;
    let slid = op.authenticate(&token, later).unwrap().unwrap();
    assert_eq!((slid.expires_at, slid.slid), (later + SESSION_TTL_SECS, true));
    assert_eq!(slid.session_id, first.session_id);
    // Unused for 30 days after that, it is gone.
    assert!(op.authenticate(&token, later + SESSION_TTL_SECS - 1).unwrap().is_some());
    let idle = later + SESSION_TTL_SECS - 1 + SESSION_TTL_SECS;
    assert_eq!(op.authenticate(&token, idle).unwrap(), None);
    // A token that is not a session, or not even the right shape, is refused.
    assert_eq!(op.authenticate(&"0".repeat(64), NOW).unwrap(), None);
    assert_eq!(op.authenticate("", NOW).unwrap(), None);
}

#[test]
fn only_the_hash_of_a_session_token_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let op = Operator::open(&db).unwrap();
    set_up(&op);
    let token = op.open_session("browser", NOW).unwrap().unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    let stored: String = conn
        .query_row("SELECT id_hash FROM auth_sessions", [], |r| r.get(0))
        .unwrap();
    assert_ne!(stored, token);
    assert_eq!(stored, op.authenticate(&token, NOW).unwrap().unwrap().session_id);
}

#[test]
fn a_new_session_is_stepped_up_for_five_minutes_and_a_step_up_renews_it() {
    let op = Operator::open_in_memory().unwrap();
    set_up(&op);
    let token = op.open_session("browser", NOW).unwrap().unwrap();
    let session = op.authenticate(&token, NOW).unwrap().unwrap();
    assert!(session.stepped_up(NOW + STEP_UP_SECS - 1));
    assert!(!session.stepped_up(NOW + STEP_UP_SECS));

    let later = NOW + 3600;
    assert!(op.step_up(&session.session_id, later).unwrap());
    let renewed = op.authenticate(&token, later).unwrap().unwrap();
    assert!(renewed.stepped_up(later + STEP_UP_SECS - 1));
    assert!(!op.step_up(&"0".repeat(64), later).unwrap());
}

#[test]
fn sessions_are_listed_most_recent_first_and_a_revoked_one_is_gone() {
    let op = Operator::open_in_memory().unwrap();
    set_up(&op);
    let phone = op.open_session("phone\u{7}", NOW).unwrap().unwrap();
    let laptop = op.open_session(&"L".repeat(300), NOW + 10).unwrap().unwrap();
    let listed = op.sessions(NOW + 10).unwrap();
    assert_eq!(listed.len(), 2);
    // Control characters are dropped and the user agent is capped.
    assert_eq!(listed[0].user_agent, "L".repeat(256));
    assert_eq!(listed[1].user_agent, "phone");

    let phone_id = op.authenticate(&phone, NOW + 10).unwrap().unwrap().session_id;
    assert!(op.revoke_session(&phone_id, NOW + 10).unwrap());
    assert!(!op.revoke_session(&phone_id, NOW + 10).unwrap());
    assert_eq!(op.authenticate(&phone, NOW + 10).unwrap(), None);
    assert!(op.authenticate(&laptop, NOW + 10).unwrap().is_some());
    assert_eq!(op.sessions(NOW + 10).unwrap().len(), 1);
    // An expired session is not listed, nor there to revoke.
    assert!(op.sessions(NOW + 10 + SESSION_TTL_SECS).unwrap().is_empty());
    let laptop_id = op.authenticate(&laptop, NOW + 10).unwrap().unwrap().session_id;
    assert!(!op.revoke_session(&laptop_id, NOW + 10 + SESSION_TTL_SECS).unwrap());
}

#[test]
fn the_cookie_is_http_only_strict_and_secure_except_on_loopback() {
    assert_eq!(
        session_cookie("abc", true),
        "hennery_session=abc; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000; Secure"
    );
    assert_eq!(
        session_cookie("abc", false),
        "hennery_session=abc; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000"
    );
    assert_eq!(
        cleared_cookie(true),
        "hennery_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0; Secure"
    );
}

#[test]
fn the_session_token_is_found_among_other_cookies() {
    let mut headers = HeaderMap::new();
    assert_eq!(session_token(&headers), None);
    headers.append(header::COOKIE, HeaderValue::from_static("theme=dark; other_session=x"));
    assert_eq!(session_token(&headers), None);
    headers.append(header::COOKIE, HeaderValue::from_static("a=1;hennery_session=tok; b=2"));
    assert_eq!(session_token(&headers), Some("tok"));
}
