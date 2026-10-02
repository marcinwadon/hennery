//! The `gen-view` binary's exit statuses, run in a scratch directory of
//! its own, never the repository: `--check` passes its flag and its
//! status on (`codegen::run`'s own tests cover each status).

use hennery_view::codegen::{EXIT_OK, EXIT_STALE};
use std::path::PathBuf;
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("gen-view-bin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn status(root: &PathBuf, args: &[&str]) -> i32 {
    Command::new(env!("CARGO_BIN_EXE_gen-view"))
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
        .status
        .code()
        .unwrap()
}

#[test]
fn the_binary_checks_with_check_and_writes_without() {
    let root = scratch("flag");
    // Nothing there: `--check` is stale, and writes nothing.
    assert_eq!(status(&root, &["--check"]), i32::from(EXIT_STALE));
    assert!(!root.join(hennery_view::codegen::TS_PATH).exists());
    // Without it, it writes; then `--check` is up to date.
    assert_eq!(status(&root, &[]), i32::from(EXIT_OK));
    assert_eq!(status(&root, &["--check"]), i32::from(EXIT_OK));
    std::fs::remove_dir_all(&root).unwrap();
}
