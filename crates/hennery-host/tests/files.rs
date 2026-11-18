use hennery_host::files;
use std::os::unix::fs::symlink;

#[test]
fn reads_are_confined_to_regular_files_under_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    std::fs::create_dir(cwd.join("reports")).unwrap();
    std::fs::write(cwd.join("reports/report.md"), b"# Report").unwrap();
    let root = cwd.to_str().unwrap();
    assert_eq!(
        files::read(root, "reports/report.md").unwrap(),
        ("report.md".into(), b"# Report".to_vec())
    );
    assert!(files::read(root, "../outside").is_err());
    assert!(files::read(root, "/etc/passwd").is_err());
    assert!(files::read(root, "reports").is_err());
    symlink("/etc/passwd", cwd.join("secret")).unwrap();
    symlink("reports", cwd.join("linked")).unwrap();
    assert!(files::read(root, "secret").is_err());
    assert!(files::read(root, "linked/report.md").is_err());
    let fifo = cwd.join("pipe");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(files::read(root, "pipe").is_err());
}

#[test]
fn growth_and_oversized_files_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let file = std::fs::File::create(cwd.join("large")).unwrap();
    file.set_len(files::MAX_FILE as u64 + 1).unwrap();
    assert!(files::read(cwd.to_str().unwrap(), "large").is_err());
}

#[test]
fn staging_is_private_session_scoped_and_detects_changed_cached_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let a = files::stage(dir.path(), "session/a", "input.csv", "eA==").unwrap();
    let b = files::stage(dir.path(), "session/b", "input.csv", "eA==").unwrap();
    assert_ne!(a, b);
    assert_eq!(std::fs::read(&a).unwrap(), b"x");
    assert_eq!(std::fs::metadata(&a).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(files::stage(dir.path(), "session/a", "input.csv", "eA==").unwrap(), a);
    std::fs::write(&a, b"changed").unwrap();
    assert!(files::stage(dir.path(), "session/a", "input.csv", "eA==").is_err());
    assert!(files::stage(dir.path(), "s", "../escape", "eA==").is_err());
}

#[test]
fn cleanup_removes_only_the_named_session_cache_and_never_follows_a_link() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let a = files::stage(root.path(), "session-a", "input.txt", "eA==").unwrap();
    let b = files::stage(root.path(), "session-b", "input.txt", "eA==").unwrap();
    files::forget(root.path(), "session-a").unwrap();
    assert!(!std::path::Path::new(&a).exists());
    assert!(std::path::Path::new(&b).exists());
    files::forget(root.path(), "session-a").unwrap();
    let session = std::path::Path::new(&b).parent().unwrap().parent().unwrap();
    std::fs::remove_dir_all(session).unwrap();
    std::fs::write(outside.path().join("keep"), b"keep").unwrap();
    symlink(outside.path(), session).unwrap();
    files::forget(root.path(), "session-b").unwrap();
    assert_eq!(std::fs::read(outside.path().join("keep")).unwrap(), b"keep");
}
