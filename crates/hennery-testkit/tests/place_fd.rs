//! `place_fd`: a descriptor handed to a child at a fixed number is open
//! across `exec`, whether or not it already sat at that number.

use std::os::fd::AsRawFd;

fn close_on_exec(fd: i32) -> bool {
    // SAFETY: fcntl(2) reads a descriptor's flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert!(flags >= 0, "{fd} is not open");
    flags & libc::FD_CLOEXEC != 0
}

#[test]
fn a_placed_descriptor_stays_open_across_exec_even_when_already_there() {
    // std opens files close-on-exec.
    let file = std::fs::File::open("/dev/null").unwrap();
    let fd = file.as_raw_fd();
    assert!(close_on_exec(fd));
    // Already at its number: dup2 onto itself would keep the flag.
    hennery_testkit::place_fd(fd, fd).unwrap();
    assert!(!close_on_exec(fd));

    let other = std::fs::File::open("/dev/null").unwrap();
    let at = 200;
    hennery_testkit::place_fd(other.as_raw_fd(), at).unwrap();
    assert!(!close_on_exec(at));
    // SAFETY: closes the copy this test made.
    unsafe { libc::close(at) };
    assert!(hennery_testkit::place_fd(-1, at).is_err());
}

/// The class stays closed: no test calls `dup2` itself (it misses the
/// already-there case); each goes through `place_fd`.
#[test]
fn no_test_calls_dup2_directly() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut checked = 0;
    let mut found = Vec::new();
    for member in std::fs::read_dir(crates).unwrap() {
        let tests = member.unwrap().path().join("tests");
        let Ok(entries) = std::fs::read_dir(&tests) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            checked += 1;
            let source = std::fs::read_to_string(&path).unwrap();
            for (n, line) in source.lines().enumerate() {
                if line.contains(concat!("libc::", "dup2(")) {
                    found.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(checked > 10, "checked only {checked} files");
    assert!(found.is_empty(), "use hennery_testkit::place_fd:\n{}", found.join("\n"));
}
