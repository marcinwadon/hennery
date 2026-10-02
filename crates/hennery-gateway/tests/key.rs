//! The master key (gateway spec §6, kernel spec §10): 32 random bytes at
//! `<data>/master.key`, created once, private, checked on every load, or
//! supplied by the environment or a systemd credential. A key that is
//! missing while credentials exist is an error, never a new key.

use hennery_gateway::crypto::{STATIC_TOKEN, open, seal};
use hennery_gateway::key::{CREDENTIAL_NAME, KEY_FILE, KeyOrigin, KeySource, MasterKey, load_or_create};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn bytes() -> [u8; 32] {
    std::array::from_fn(|i| i as u8)
}

fn source(dir: &Path) -> KeySource {
    KeySource {
        data_dir: dir.to_path_buf(),
        env: None,
        credentials_dir: None,
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Whether `a` opens what `b` sealed: the two hold the same key.
fn same_key(a: &MasterKey, b: &MasterKey) -> bool {
    let blob = seal(b, "conn-1", STATIC_TOKEN, b"probe");
    open(a, "conn-1", STATIC_TOKEN, b.version(), &blob).is_ok()
}

fn error(source: &KeySource, ciphertext_exists: bool) -> String {
    format!("{:#}", load_or_create(source, ciphertext_exists).unwrap_err())
}

fn write_private(path: &Path, content: &[u8]) {
    std::fs::write(path, content).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn a_new_key_is_created_once_private_and_loaded_again() {
    let dir = tempfile::tempdir().unwrap();
    let (created, origin) = load_or_create(&source(dir.path()), false).unwrap();
    assert_eq!(origin, KeyOrigin::Created);
    let file = dir.path().join(KEY_FILE);
    assert_eq!(mode(&file), 0o600);
    assert_eq!(std::fs::read(&file).unwrap().len(), 32);
    let before = std::fs::read(&file).unwrap();
    let (loaded, origin) = load_or_create(&source(dir.path()), true).unwrap();
    assert_eq!(origin, KeyOrigin::File);
    assert!(same_key(&loaded, &created));
    assert_eq!(std::fs::read(&file).unwrap(), before, "the key was replaced");
    // Two installs never share a key.
    let other = tempfile::tempdir().unwrap();
    let (theirs, _) = load_or_create(&source(other.path()), false).unwrap();
    assert!(!same_key(&theirs, &created));
}

#[test]
fn a_missing_key_while_credentials_exist_is_an_error_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let err = error(&source(dir.path()), true);
    assert!(err.contains("master.key") && err.contains("missing"), "{err}");
    assert!(!dir.path().join(KEY_FILE).exists());
}

#[test]
fn a_key_file_others_can_read_is_refused_and_left_alone() {
    for loose in [0o644, 0o640, 0o604, 0o620] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(KEY_FILE);
        std::fs::write(&file, bytes()).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(loose)).unwrap();
        let err = error(&source(dir.path()), false);
        assert!(err.contains("chmod 600"), "{loose:o}: {err}");
        assert_eq!(mode(&file), loose);
        assert_eq!(std::fs::read(&file).unwrap(), bytes());
    }
}

#[test]
fn a_symlinked_or_odd_key_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere");
    write_private(&target, &bytes());
    std::os::unix::fs::symlink(&target, dir.path().join(KEY_FILE)).unwrap();
    let err = error(&source(dir.path()), false);
    assert!(err.contains("symlink"), "{err}");

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(KEY_FILE)).unwrap();
    let err = error(&source(dir.path()), false);
    assert!(err.contains("not a regular file"), "{err}");
}

#[test]
fn a_key_file_of_the_wrong_length_is_refused_and_never_replaced() {
    for content in [&bytes()[..31], &[0u8; 33][..], &[][..]] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(KEY_FILE);
        write_private(&file, content);
        let err = error(&source(dir.path()), false);
        assert!(err.contains("32 bytes"), "{err}");
        assert_eq!(std::fs::read(&file).unwrap(), content);
    }
}

#[test]
fn a_key_in_the_environment_is_used_and_no_file_is_made() {
    let dir = tempfile::tempdir().unwrap();
    let mut from_env = source(dir.path());
    from_env.env = Some(HEX.to_uppercase().into());
    let (key, origin) = load_or_create(&from_env, true).unwrap();
    assert_eq!(origin, KeyOrigin::Environment);
    assert!(same_key(&key, &MasterKey::from_bytes(bytes())));
    assert!(!dir.path().join(KEY_FILE).exists());
    // A malformed value is refused without being quoted back.
    for bad in [
        &HEX[1..],
        "zz02030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        "",
    ] {
        from_env.env = Some(bad.to_string().into());
        let err = error(&from_env, false);
        assert!(err.contains("64 hexadecimal digits"), "{err}");
        assert!(bad.is_empty() || !err.contains(bad), "{err}");
    }
}

fn credentials(dir: &Path, content: &[u8], perm: u32) -> PathBuf {
    let creds = dir.join("credentials");
    std::fs::create_dir(&creds).unwrap();
    let file = creds.join(CREDENTIAL_NAME);
    std::fs::write(&file, content).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(perm)).unwrap();
    creds
}

#[test]
fn a_systemd_credential_is_used_raw_or_in_hex() {
    for (content, perm) in [
        (bytes().to_vec(), 0o400),
        (format!("{HEX}\n").into_bytes(), 0o440),
        (HEX.as_bytes().to_vec(), 0o600),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut from_creds = source(dir.path());
        from_creds.credentials_dir = Some(credentials(dir.path(), &content, perm));
        let (key, origin) = load_or_create(&from_creds, true).unwrap();
        assert_eq!(origin, KeyOrigin::Credential);
        assert!(same_key(&key, &MasterKey::from_bytes(bytes())));
        assert!(!dir.path().join(KEY_FILE).exists());
    }
}

#[test]
fn a_bad_systemd_credential_is_refused() {
    // Readable by everyone.
    let dir = tempfile::tempdir().unwrap();
    let mut from_creds = source(dir.path());
    from_creds.credentials_dir = Some(credentials(dir.path(), &bytes(), 0o404));
    assert!(error(&from_creds, false).contains("other users"));
    // The wrong length.
    let dir = tempfile::tempdir().unwrap();
    let mut from_creds = source(dir.path());
    from_creds.credentials_dir = Some(credentials(dir.path(), b"short", 0o400));
    assert!(error(&from_creds, false).contains("32 bytes"));
    // A symlink.
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("credentials");
    std::fs::create_dir(&creds).unwrap();
    let target = dir.path().join("elsewhere");
    write_private(&target, &bytes());
    std::os::unix::fs::symlink(&target, creds.join(CREDENTIAL_NAME)).unwrap();
    let mut from_creds = source(dir.path());
    from_creds.credentials_dir = Some(creds);
    assert!(error(&from_creds, false).contains("symlink"));
}

#[test]
fn a_credentials_directory_without_the_key_falls_back_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("credentials");
    std::fs::create_dir(&creds).unwrap();
    let mut from_creds = source(dir.path());
    from_creds.credentials_dir = Some(creds);
    let (_, origin) = load_or_create(&from_creds, false).unwrap();
    assert_eq!(origin, KeyOrigin::Created);
    assert!(dir.path().join(KEY_FILE).exists());
}

#[test]
fn two_supplied_keys_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut both = source(dir.path());
    both.env = Some(HEX.to_string().into());
    both.credentials_dir = Some(credentials(dir.path(), &bytes(), 0o400));
    assert!(error(&both, false).contains("both"));
}

#[test]
fn the_key_never_shows_in_debug_output() {
    let key = MasterKey::from_bytes(bytes());
    let shown = format!("{key:?}");
    assert!(!shown.contains("0, 1, 2") && !shown.contains("000102"), "{shown}");
    let mut from_env = source(Path::new("/nonexistent"));
    from_env.env = Some(HEX.to_string().into());
    let shown = format!("{from_env:?}");
    assert!(!shown.contains(HEX), "{shown}");
}

/// The review's R3: a source the operator chose that cannot be read is an
/// error, never a reason to make a key file in its place.
#[test]
fn a_key_source_that_cannot_be_read_is_refused_not_skipped() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    // `HENNERY_MASTER_KEY` set, but not text.
    let err = KeySource::from_vars(dir.path(), Some(std::ffi::OsString::from_vec(vec![0xff, 0xfe])), None)
        .expect_err("a non-UTF-8 key was taken as unset");
    assert!(format!("{err:#}").contains("64 hexadecimal digits"), "{err:#}");
    // `$CREDENTIALS_DIRECTORY` that cannot be looked into (a file, not a
    // directory: ENOTDIR).
    let not_a_dir = dir.path().join("not-a-dir");
    std::fs::write(&not_a_dir, b"x").unwrap();
    let source = KeySource::from_vars(dir.path(), None, Some(not_a_dir.into_os_string())).unwrap();
    let err = error(&source, false);
    assert!(err.contains("look for the credential"), "{err}");
    assert!(!dir.path().join(KEY_FILE).exists(), "a key file was made instead");
    // Unset, both: no source, and a key file is made.
    let source = KeySource::from_vars(dir.path(), None, None).unwrap();
    assert_eq!(load_or_create(&source, false).unwrap().1, KeyOrigin::Created);
}

#[test]
fn a_hard_linked_key_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(KEY_FILE);
    write_private(&file, &bytes());
    std::fs::hard_link(&file, dir.path().join("second-name")).unwrap();
    let err = error(&source(dir.path()), false);
    assert!(err.contains("hard links"), "{err}");
}

/// Plan 8a decision 8: the error says how to give the credentials up.
#[test]
fn a_missing_key_says_how_to_give_the_credentials_up() {
    let dir = tempfile::tempdir().unwrap();
    let err = error(&source(dir.path()), true);
    assert!(err.contains("DELETE FROM gw_credentials"), "{err}");
}
