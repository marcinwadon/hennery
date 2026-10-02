//! The host's half of the project picker (ACP core §7; plan 6c): the
//! enumeration of repositories (decision 5) and the browse fence with its
//! listings (decisions 3 and 4), on real directories. Every expected path
//! is canonical: macOS keeps temporary directories under `/var`, a symlink
//! to `/private/var`.

use hennery_host::projects::{BrowseError, Fence, Limits, Listing, browse, canonical, list_projects};
use hennery_proto::frames::DirEntry;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A canonical temporary directory, and its path as a string.
fn scratch() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = canonical(dir.path()).unwrap();
    (dir, path)
}

fn mkdir(path: &str) {
    std::fs::create_dir_all(path).unwrap();
}

/// A repository: a directory holding a `.git` directory.
fn repo(path: &str) {
    mkdir(&format!("{path}/.git"));
}

fn paths(roots: &[&str], limits: &Limits) -> (Vec<String>, bool) {
    let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
    let (items, partial) = list_projects(&roots, limits);
    (items.into_iter().map(|p| p.path).collect(), partial)
}

#[test]
fn repositories_are_found_under_the_roots_and_not_looked_into() {
    let (_dir, root) = scratch();
    repo(&format!("{root}/a"));
    repo(&format!("{root}/org/b"));
    repo(&format!("{root}/org/b/vendor/nested"));
    // A worktree's `.git` is a file.
    mkdir(&format!("{root}/org/wt"));
    std::fs::write(format!("{root}/org/wt/.git"), "gitdir: /elsewhere\n").unwrap();
    repo(&format!("{root}/.hidden/c"));
    repo(&format!("{root}/one/two/three/four"));
    repo(&format!("{root}/one/two/three/four-and/five"));
    mkdir(&format!("{root}/empty"));
    std::fs::write(format!("{root}/file"), "").unwrap();
    let (found, partial) = paths(&[&root, "/nonexistent/root"], &Limits::default());
    assert_eq!(
        found,
        [
            format!("{root}/a"),
            format!("{root}/one/two/three/four"),
            format!("{root}/org/b"),
            format!("{root}/org/wt"),
        ],
        "dot-dirs skipped, repositories not looked into, at most four levels down"
    );
    assert!(
        !partial,
        "the depth bound defines the enumeration; it does not make it partial"
    );
}

#[test]
fn a_root_that_is_a_repository_is_listed_and_overlapping_roots_once() {
    let (_dir, base) = scratch();
    let root = format!("{base}/r");
    repo(&root);
    repo(&format!("{base}/other"));
    let (found, _) = paths(&[&root, &base, &root], &Limits::default());
    assert_eq!(found, [format!("{base}/other"), root.clone()]);
}

#[test]
fn symlinks_are_not_followed() {
    let (_dir, base) = scratch();
    let (_outside_dir, outside) = scratch();
    repo(&format!("{outside}/secret"));
    let root = format!("{base}/root");
    mkdir(&root);
    symlink(&outside, format!("{root}/link")).unwrap();
    symlink(&root, format!("{root}/loop")).unwrap();
    assert_eq!(paths(&[&root], &Limits::default()), (vec![], false));
    // A root that is a symlink is resolved, and walked where it leads.
    let linked = format!("{base}/linked-root");
    symlink(&outside, &linked).unwrap();
    assert_eq!(
        paths(&[&linked], &Limits::default()),
        (vec![format!("{outside}/secret")], false)
    );
}

#[test]
fn each_bound_but_depth_makes_the_enumeration_partial() {
    let (_dir, root) = scratch();
    for name in ["a", "b", "c"] {
        repo(&format!("{root}/{name}"));
    }
    let per_root = Limits {
        per_root: 2,
        ..Limits::default()
    };
    assert_eq!(
        paths(&[&root], &per_root),
        (vec![format!("{root}/a"), format!("{root}/b")], true)
    );
    // The root and the first child by name: whatever order the filesystem
    // lists them in.
    let dirs = Limits {
        dirs_per_root: 2,
        ..Limits::default()
    };
    assert_eq!(paths(&[&root], &dirs), (vec![format!("{root}/a")], true));
    let budget = Limits {
        budget: Duration::ZERO,
        ..Limits::default()
    };
    assert_eq!(paths(&[&root], &budget), (vec![], true));
    let shallow = Limits {
        depth: 0,
        ..Limits::default()
    };
    assert_eq!(paths(&[&root], &shallow), (vec![], false));
}

fn names(listing: &Listing) -> Vec<(&str, bool)> {
    listing.entries.iter().map(|e| (e.name.as_str(), e.git)).collect()
}

fn fence(roots: &[&str], home: Option<&str>) -> Fence {
    let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
    Fence::new(&roots, home.map(Path::new))
}

#[test]
fn a_listing_holds_the_subdirectories_sorted_with_their_git_flag() {
    let (_dir, root) = scratch();
    repo(&format!("{root}/b-repo"));
    mkdir(&format!("{root}/a-dir"));
    mkdir(&format!("{root}/.hidden"));
    std::fs::write(format!("{root}/a-file"), "").unwrap();
    let listing = browse(&fence(&[&root], None), &root, &Limits::default()).unwrap();
    assert_eq!(listing.path, root);
    assert_eq!(names(&listing), [("a-dir", false), ("b-repo", true)]);
    assert!(!listing.truncated);
    assert_eq!(listing.parent, None, "the root's parent is outside the fence");
    let inner = browse(&fence(&[&root], None), &format!("{root}/a-dir/"), &Limits::default()).unwrap();
    assert_eq!(inner.path, format!("{root}/a-dir"));
    assert_eq!(inner.parent.as_deref(), Some(root.as_str()));
}

#[test]
fn the_fence_is_the_roots_and_home_by_whole_segments() {
    let (_dir, base) = scratch();
    for name in ["u", "u2", "home", "elsewhere"] {
        mkdir(&format!("{base}/{name}"));
    }
    let fenced = fence(
        &[&format!("{base}/u"), "/nonexistent/root"],
        Some(&format!("{base}/home")),
    );
    let refused = |path: String| browse(&fenced, &path, &Limits::default()).unwrap_err();
    assert!(browse(&fenced, &format!("{base}/u"), &Limits::default()).is_ok());
    assert!(browse(&fenced, &format!("{base}/home"), &Limits::default()).is_ok());
    assert_eq!(refused(format!("{base}/u2")), BrowseError::OutsideWorkspace);
    assert_eq!(refused(format!("{base}/elsewhere")), BrowseError::OutsideWorkspace);
    assert_eq!(refused(base.clone()), BrowseError::OutsideWorkspace);
    assert_eq!(refused("/".into()), BrowseError::OutsideWorkspace);
    // No home: the roots alone.
    let roots_only = fence(&[&format!("{base}/u")], None);
    assert_eq!(
        browse(&roots_only, &format!("{base}/home"), &Limits::default()).unwrap_err(),
        BrowseError::OutsideWorkspace
    );
}

#[test]
fn a_symlink_out_of_the_fence_leads_nowhere() {
    let (_dir, root) = scratch();
    let (_outside_dir, outside) = scratch();
    mkdir(&format!("{outside}/secret"));
    std::fs::write(format!("{outside}/passwd"), "").unwrap();
    symlink(&outside, format!("{root}/link")).unwrap();
    mkdir(&format!("{root}/inner"));
    repo(&format!("{root}/inner/repo"));
    symlink(format!("{root}/inner"), format!("{root}/shortcut")).unwrap();
    symlink(format!("{root}/inner/repo"), format!("{root}/repo-link")).unwrap();
    symlink(format!("{outside}/passwd"), format!("{root}/file-link")).unwrap();
    let fenced = fence(&[&root], None);
    let listing = browse(&fenced, &root, &Limits::default()).unwrap();
    assert_eq!(
        names(&listing),
        [("inner", false), ("repo-link", true), ("shortcut", false)],
        "a link out of the fence or to a file is left out; one inside is judged by its target"
    );
    let refused = |path: String| browse(&fenced, &path, &Limits::default()).unwrap_err();
    assert_eq!(refused(format!("{root}/link")), BrowseError::OutsideWorkspace);
    assert_eq!(refused(format!("{root}/link/secret")), BrowseError::OutsideWorkspace);
    // The review's A1: existing or not, a path through the link is outside.
    assert_eq!(refused(format!("{root}/link/passwd")), BrowseError::OutsideWorkspace);
    assert_eq!(refused(format!("{root}/link/nope")), BrowseError::OutsideWorkspace);
    assert_eq!(
        refused(format!("{root}/link/nope/deeper")),
        BrowseError::OutsideWorkspace
    );
    // Through a link inside the fence, the canonical path is listed.
    let through = browse(&fenced, &format!("{root}/shortcut"), &Limits::default()).unwrap();
    assert_eq!(through.path, format!("{root}/inner"));
}

#[test]
fn only_a_path_inside_the_fence_says_why_it_cannot_be_listed() {
    let (_dir, root) = scratch();
    std::fs::write(format!("{root}/file"), "").unwrap();
    let fenced = fence(&[&root], None);
    let refused = |path: String| browse(&fenced, &path, &Limits::default()).unwrap_err();
    assert_eq!(refused(format!("{root}/missing")), BrowseError::PathNotFound);
    assert_eq!(refused(format!("{root}/missing/deeper")), BrowseError::PathNotFound);
    assert_eq!(refused(format!("{root}/file")), BrowseError::NotADirectory);
    assert_eq!(refused(format!("{root}/file/below")), BrowseError::NotADirectory);
    assert_eq!(
        refused("/nonexistent/hennery/path".into()),
        BrowseError::OutsideWorkspace
    );
    assert_eq!(refused("/etc/hennery-missing".into()), BrowseError::OutsideWorkspace);
}

#[test]
fn a_path_must_be_absolute_and_plain() {
    let (_dir, root) = scratch();
    mkdir(&format!("{root}/a"));
    let fenced = fence(&[&root], None);
    for bad in [
        "a".to_string(),
        "".into(),
        "~/a".into(),
        format!("{root}/a/.."),
        format!("{root}/./a"),
        format!("{root}//a"),
        format!("{root}/a/../a"),
        format!("{root}/a\n"),
        "//".into(),
        format!("/{}", "x".repeat(4096)),
    ] {
        assert_eq!(
            browse(&fenced, &bad, &Limits::default()).unwrap_err(),
            BrowseError::Invalid,
            "{bad:?}"
        );
    }
}

#[test]
fn an_unreadable_directory_inside_the_fence_is_permission_denied() {
    // SAFETY: geteuid(2) has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // root reads every directory
    }
    let (_dir, root) = scratch();
    let locked = format!("{root}/locked");
    mkdir(&format!("{locked}/inner"));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let fenced = fence(&[&root], None);
    let listed = browse(&fenced, &locked, &Limits::default());
    let below = browse(&fenced, &format!("{locked}/inner"), &Limits::default());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(listed.unwrap_err(), BrowseError::PermissionDenied);
    assert_eq!(below.unwrap_err(), BrowseError::PermissionDenied);
}

#[test]
fn a_listing_is_capped() {
    let (_dir, root) = scratch();
    for name in ["c", "a", "b", "d"] {
        mkdir(&format!("{root}/{name}"));
    }
    let fenced = fence(&[&root], None);
    let few = Limits {
        entries: 2,
        ..Limits::default()
    };
    let listing = browse(&fenced, &root, &few).unwrap();
    assert_eq!(
        listing.entries,
        [
            DirEntry {
                name: "a".into(),
                git: false
            },
            DirEntry {
                name: "b".into(),
                git: false
            }
        ]
    );
    assert!(listing.truncated);
    let short_read = Limits {
        entries_read: 1,
        ..Limits::default()
    };
    let listing = browse(&fenced, &root, &short_read).unwrap();
    assert!(listing.truncated && listing.entries.len() <= 1);
}

// The probes on blocking threads, bounded (the review's A4).

use hennery_host::outbox::Outbox;
use hennery_host::projects::Probes;
use hennery_host::uplink::Uplink;
use hennery_proto::frames::HostFrame;

#[tokio::test]
async fn probes_answer_through_the_uplink() {
    let (_dir, root) = scratch();
    let (_home_dir, home) = scratch();
    repo(&format!("{root}/a"));
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let probes = Probes::default();
    probes.list(
        &uplink,
        "l1".into(),
        vec![PathBuf::from(&root)],
        Some(PathBuf::from(&home)),
    );
    let reply = replies.recv().await.unwrap();
    let HostFrame::Projects {
        request_id,
        items,
        partial,
        home: reported,
    } = reply
    else {
        panic!("expected projects, got {reply:?}");
    };
    assert_eq!(request_id, "l1");
    assert_eq!(
        items.into_iter().map(|p| p.path).collect::<Vec<_>>(),
        [format!("{root}/a")]
    );
    assert!(!partial);
    assert_eq!(reported, Some(home.clone()));
    probes.browse(&uplink, "b1".into(), "/".into(), vec![PathBuf::from(&root)], None);
    let reply = replies.recv().await.unwrap();
    let HostFrame::Error { request_id, code, .. } = reply else {
        panic!("expected an error, got {reply:?}");
    };
    assert_eq!((request_id.as_str(), code.as_str()), ("b1", "outside_workspace"));
}

#[tokio::test]
async fn a_probe_past_the_hosts_bounds_is_answered_busy() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let probes = Probes::with_capacity(Limits::default(), 0, 0);
    probes.list(&uplink, "l1".into(), vec![], None);
    probes.browse(&uplink, "b1".into(), "/".into(), vec![], None);
    for expected in ["l1", "b1"] {
        let reply = replies.recv().await.unwrap();
        let HostFrame::Error { request_id, code, .. } = reply else {
            panic!("expected busy, got {reply:?}");
        };
        assert_eq!((request_id.as_str(), code.as_str()), (expected, "busy"));
    }
}
