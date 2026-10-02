//! The managed runtime's installer (distribution spec §3.2, §9): fixture
//! sets served from loopback, never the registry or nodejs.org.

mod support;

use hennery_host::runtime::install::{self, Installed, Selection};
use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use support::{Fixture, Server, data_dir, here, quiet};

fn selection(fixture: &Fixture, skip: &[&str]) -> Selection {
    let skip: BTreeSet<String> = skip.iter().map(|s| s.to_string()).collect();
    Selection::new(&fixture.manifest, &fixture.hash(), here(), &skip).unwrap()
}

#[tokio::test]
async fn a_set_installs_with_its_runtime_and_becomes_current() {
    let server = Server::start().await;
    let fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    let (_dir, layout) = data_dir();
    let selection = selection(&fixture, &[]);
    let installed = install::install(&layout, &selection, &server.sources(), &quiet)
        .await
        .unwrap();
    let Installed::Switched { set, previous: None } = installed else {
        panic!("{installed:?}")
    };
    assert_eq!(set.id, selection.set_id());
    assert_eq!(layout.current().unwrap().unwrap(), set);
    assert_eq!(
        std::fs::read_link(layout.current_link()).unwrap(),
        std::path::Path::new("sets").join(&set.id)
    );
    // Each agent's tree under its own directory, nested package included.
    for agent in ["claude", "codex"] {
        let tree = set.path.join(agent);
        let entry = tree.join(&set.record.adapters[agent].entry);
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), format!("// {agent} 1.0.0\n"));
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&entry), 0o755);
        assert_eq!(
            mode(&tree.join(format!("node_modules/@acp/{agent}/package.json"))),
            0o644
        );
        assert!(
            tree.join(format!("node_modules/@acp/{agent}/node_modules/nested/index.js"))
                .is_file()
        );
        let cli = tree.join(format!("node_modules/@vendor/{agent}-cli-{}/cli", here().key()));
        assert_eq!(mode(&cli), 0o755, "the CLI keeps its exec bit");
        assert!(!set.record.adapters[agent].cli_skipped);
    }
    assert_eq!(
        set.node,
        layout.runtimes().join(format!("node-24.0.0-{}/bin/node", here().key()))
    );
    assert!(set.node.is_absolute() && set.path.is_absolute());
    let out = std::process::Command::new(&set.node).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "v24.0.0");
    // Nothing left over: no download, no staging directory.
    assert_eq!(std::fs::read_dir(layout.downloads()).unwrap().count(), 0);
    let sets: Vec<String> = std::fs::read_dir(layout.sets())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(sets, std::slice::from_ref(&set.id));
}

#[tokio::test]
async fn installing_the_current_set_again_downloads_nothing() {
    let server = Server::start().await;
    let fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    let (_dir, layout) = data_dir();
    let selection = selection(&fixture, &[]);
    install::install(&layout, &selection, &server.sources(), &quiet)
        .await
        .unwrap();
    let before = server.requests().len();
    let again = install::install(&layout, &selection, &server.sources(), &quiet)
        .await
        .unwrap();
    assert!(matches!(again, Installed::AlreadyCurrent(_)), "{again:?}");
    assert_eq!(server.requests().len(), before);
}

#[tokio::test]
async fn a_digest_mismatch_aborts_and_leaves_the_previous_set_current() {
    let server = Server::start().await;
    let first = Fixture::new("1.0.0");
    first.serve(&server);
    let (_dir, layout) = data_dir();
    let old = install::install(&layout, &selection(&first, &[]), &server.sources(), &quiet)
        .await
        .unwrap()
        .set()
        .clone();
    let second = Fixture::new("2.0.0");
    second.serve(&server);
    // The mirror answers other bytes for one package of the new set.
    let tampered = second.server_path("codex", "node_modules/@acp/codex");
    let mut body = second.bodies[&tampered].clone();
    let last = body.len() - 1;
    body[last] ^= 0xff;
    server.put(&tampered, body);
    let err = install::install(&layout, &selection(&second, &[]), &server.sources(), &quiet)
        .await
        .unwrap_err();
    let err = format!("{err:#}");
    assert!(
        err.contains("does not match the digest") && err.contains(&tampered),
        "{err}"
    );
    assert_eq!(layout.current().unwrap().unwrap(), old, "the old set stays current");
    assert!(layout.previous().unwrap().is_none());
    assert!(layout.set(&selection(&second, &[]).set_id()).is_err(), "no new set");
    let file = second.file("codex", "node_modules/@acp/codex");
    let digest = hennery_host::runtime::manifest::sri_sha512(&file.integrity).unwrap();
    assert!(
        !layout
            .downloads()
            .join(format!("{}.part", &hex::encode(digest)[..32]))
            .exists(),
        "the mismatched download is removed"
    );
    assert_eq!(
        server.requests().iter().filter(|(p, _)| *p == tampered).count(),
        1,
        "a mismatch is never retried"
    );
    // The old set is intact.
    assert_eq!(
        std::fs::read_to_string(old.path.join("codex/node_modules/@acp/codex/dist/index.js")).unwrap(),
        "// codex 1.0.0\n"
    );
}

#[tokio::test]
async fn an_interrupted_download_resumes_where_it_stopped() {
    let server = Server::start().await;
    let fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    let path = fixture.server_path("claude", "node_modules/@acp/claude");
    let half = fixture.bodies[&path].len() / 2;
    server.cut_once(&path, half);
    let (_dir, layout) = data_dir();
    install::install(&layout, &selection(&fixture, &[]), &server.sources(), &quiet)
        .await
        .unwrap();
    let asked: Vec<Option<String>> = server
        .requests()
        .into_iter()
        .filter(|(p, _)| *p == path)
        .map(|(_, range)| range)
        .collect();
    assert_eq!(asked, [None, Some(format!("bytes={half}-"))]);
}

#[tokio::test]
async fn a_download_left_by_an_earlier_run_resumes() {
    let server = Server::start().await;
    let fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    let path = fixture.server_path("codex", "node_modules/@acp/codex");
    let body = fixture.bodies[&path].clone();
    let (_dir, layout) = data_dir();
    std::fs::create_dir_all(layout.downloads()).unwrap();
    let third = body.len() / 3;
    std::fs::write(layout.downloads().join(support::part_name(&body)), &body[..third]).unwrap();
    install::install(&layout, &selection(&fixture, &[]), &server.sources(), &quiet)
        .await
        .unwrap();
    let asked: Vec<Option<String>> = server
        .requests()
        .into_iter()
        .filter(|(p, _)| *p == path)
        .map(|(_, range)| range)
        .collect();
    assert_eq!(asked, [Some(format!("bytes={third}-"))]);
}

#[tokio::test]
async fn a_download_longer_than_pinned_is_cut_off() {
    let server = Server::start().await;
    let fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    let path = fixture.server_path("claude", "node_modules/@acp/claude");
    let mut body = fixture.bodies[&path].clone();
    body.extend_from_slice(&[0u8; 4096]);
    server.put(&path, body);
    let (_dir, layout) = data_dir();
    let err = install::install(&layout, &selection(&fixture, &[]), &server.sources(), &quiet)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("more than the"), "{err:#}");
    assert!(layout.current().unwrap().is_none());
    // Refused outright: not asked again.
    assert_eq!(server.requests().iter().filter(|(p, _)| *p == path).count(), 1);
}

/// A mirror may redirect to https, never down to plain http elsewhere: the
/// redirect is refused before anything connects to it (a documentation
/// address, so this test stays offline either way).
#[tokio::test]
async fn a_redirect_down_to_plain_http_is_refused() {
    let server = Server::start().await;
    let fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    let node = format!("/node/v24.0.0/node-v24.0.0-{}.tar.gz", here().key());
    server.redirect(&node, "http://203.0.113.1/node.tar.gz");
    let (_dir, layout) = data_dir();
    let err = install::install(&layout, &selection(&fixture, &[]), &server.sources(), &quiet)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("a redirect away from https"), "{err:#}");
    assert!(layout.current().unwrap().is_none());
}

#[tokio::test]
async fn an_update_keeps_the_previous_set_and_rollback_holds_the_host_on_it() {
    let server = Server::start().await;
    let (one, two) = (Fixture::new("1.0.0"), Fixture::new("2.0.0"));
    one.serve(&server);
    two.serve(&server);
    let (_dir, layout) = data_dir();
    let (a, b) = (selection(&one, &[]), selection(&two, &[]));
    install::install(&layout, &a, &server.sources(), &quiet).await.unwrap();
    let switched = install::install(&layout, &b, &server.sources(), &quiet).await.unwrap();
    assert!(matches!(&switched, Installed::Switched { previous: Some(p), .. } if *p == a.set_id()));
    assert_eq!(layout.previous().unwrap().unwrap().id, a.set_id());
    assert!(!layout.held());

    let back = install::rollback(&layout, &quiet).await.unwrap();
    assert_eq!(
        (back.from.as_str(), back.to.id.as_str()),
        (b.set_id().as_str(), a.set_id().as_str())
    );
    assert_eq!(layout.current().unwrap().unwrap().id, a.set_id());
    assert_eq!(layout.previous().unwrap().unwrap().id, b.set_id());
    assert!(layout.held(), "a rollback holds the host on its set");
    // A hold names its set: one left over for another set holds nothing.
    let hold = std::fs::read_to_string(layout.hold_file()).unwrap();
    std::fs::write(layout.hold_file(), b.set_id()).unwrap();
    assert!(!layout.held());
    std::fs::write(layout.hold_file(), hold).unwrap();

    // Updating again returns to the pinned set, from disk, and lifts the hold.
    let before = server.requests().len();
    install::install(&layout, &b, &server.sources(), &quiet).await.unwrap();
    assert_eq!(server.requests().len(), before, "the set was still there");
    assert_eq!(layout.current().unwrap().unwrap().id, b.set_id());
    assert_eq!(layout.previous().unwrap().unwrap().id, a.set_id());
    assert!(!layout.held());
}

/// A `current` this binary cannot read (another layout: a later release's,
/// after a downgrade) never stops an install, and a directory under the
/// pinned id that is not a complete set is moved aside and rebuilt.
#[tokio::test]
async fn an_unreadable_current_set_does_not_stop_an_install() {
    let server = Server::start().await;
    let fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    let (_dir, layout) = data_dir();
    let selection = selection(&fixture, &[]);
    let other = "0123456789abcdef0123456789abcdef";
    for (id, record) in [
        (other, format!("{{\"layout\": 0, \"id\": \"{other}\"}}")),
        (selection.set_id().as_str(), "torn".to_string()),
    ] {
        std::fs::create_dir_all(layout.sets().join(id)).unwrap();
        std::fs::write(layout.sets().join(id).join("hennery-set.json"), record).unwrap();
    }
    std::os::unix::fs::symlink(std::path::Path::new("sets").join(other), layout.current_link()).unwrap();
    assert!(layout.current().is_err() && layout.current_id().as_deref() == Some(other));
    let installed = install::install(&layout, &selection, &server.sources(), &quiet)
        .await
        .unwrap();
    assert!(
        matches!(&installed, Installed::Switched { previous: Some(p), .. } if p == other),
        "{installed:?}"
    );
    assert_eq!(layout.current().unwrap().unwrap().id, selection.set_id());
    assert_eq!(layout.previous_id().as_deref(), Some(other));
    // Rolling back to a set this binary cannot read is refused.
    assert!(install::rollback(&layout, &quiet).await.is_err());
    assert_eq!(layout.current_id(), Some(selection.set_id()));
}

#[tokio::test]
async fn rollback_without_a_previous_set_changes_nothing() {
    let server = Server::start().await;
    let fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    let (_dir, layout) = data_dir();
    assert!(install::rollback(&layout, &quiet).await.is_err());
    install::install(&layout, &selection(&fixture, &[]), &server.sources(), &quiet)
        .await
        .unwrap();
    let err = install::rollback(&layout, &quiet).await.unwrap_err();
    assert!(err.to_string().contains("no previous adapter set"), "{err}");
    assert!(!layout.held());
}

#[tokio::test]
async fn sets_beyond_previous_are_collected_unless_a_host_holds_them() {
    let server = Server::start().await;
    let fixtures: Vec<Fixture> = ["1.0.0", "2.0.0", "3.0.0", "4.0.0"]
        .iter()
        .map(|v| Fixture::new(v))
        .collect();
    for f in &fixtures {
        f.serve(&server);
    }
    let (_dir, layout) = data_dir();
    let ids: Vec<String> = fixtures.iter().map(|f| selection(f, &[]).set_id()).collect();
    let first = install::install(&layout, &selection(&fixtures[0], &[]), &server.sources(), &quiet)
        .await
        .unwrap()
        .set()
        .clone();
    // A host launching from the first set holds it.
    let held = install::hold_in_use(&layout, &first).unwrap();
    for f in &fixtures[1..3] {
        install::install(&layout, &selection(f, &[]), &server.sources(), &quiet)
            .await
            .unwrap();
    }
    assert!(layout.set(&ids[0]).is_ok(), "a held set survives");
    assert!(layout.set(&ids[1]).is_ok(), "previous");
    assert!(layout.set(&ids[2]).is_ok(), "current");
    drop(held);
    install::install(&layout, &selection(&fixtures[3], &[]), &server.sources(), &quiet)
        .await
        .unwrap();
    assert!(layout.set(&ids[0]).is_err(), "released, it is collected");
    assert!(layout.set(&ids[1]).is_err());
    assert!(layout.set(&ids[2]).is_ok() && layout.set(&ids[3]).is_ok());
    let left: Vec<String> = std::fs::read_dir(layout.sets())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
}

#[tokio::test]
async fn a_runtime_no_set_names_is_collected() {
    let server = Server::start().await;
    let old = Fixture::new("1.0.0");
    let newer = [
        Fixture::with_node("2.0.0", "24.0.1", "echo v24.0.1"),
        Fixture::with_node("3.0.0", "24.0.1", "echo v24.0.1"),
    ];
    old.serve(&server);
    for f in &newer {
        f.serve(&server);
    }
    let (_dir, layout) = data_dir();
    let runtime = |v: &str| layout.runtimes().join(format!("node-{v}-{}", here().key()));
    install::install(&layout, &selection(&old, &[]), &server.sources(), &quiet)
        .await
        .unwrap();
    install::install(&layout, &selection(&newer[0], &[]), &server.sources(), &quiet)
        .await
        .unwrap();
    assert!(runtime("24.0.0").exists(), "previous still names it");
    install::install(&layout, &selection(&newer[1], &[]), &server.sources(), &quiet)
        .await
        .unwrap();
    assert!(!runtime("24.0.0").exists());
    assert!(runtime("24.0.1").join("bin/node").exists());
}

#[tokio::test]
async fn too_little_free_space_is_refused_before_any_download() {
    let server = Server::start().await;
    let mut fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    for files in fixture
        .manifest
        .adapters
        .get_mut("claude")
        .unwrap()
        .platforms
        .values_mut()
    {
        files[0].unpacked_size = 1 << 60;
    }
    let (_dir, layout) = data_dir();
    let err = install::install(&layout, &selection(&fixture, &[]), &server.sources(), &quiet)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("MB free"), "{err}");
    assert!(server.requests().is_empty(), "{:?}", server.requests());
}

#[tokio::test]
async fn a_node_that_does_not_run_here_is_refused() {
    let server = Server::start().await;
    let fixture = Fixture::with_node("1.0.0", "24.0.0", "echo v99.0.0");
    fixture.serve(&server);
    let (_dir, layout) = data_dir();
    let err = install::install(&layout, &selection(&fixture, &[]), &server.sources(), &quiet)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not run here"), "{err}");
    assert!(layout.current().unwrap().is_none());
    assert_eq!(
        std::fs::read_dir(layout.runtimes()).unwrap().count(),
        1,
        "only its staging"
    );
}

#[tokio::test]
async fn a_skipped_cli_is_neither_downloaded_nor_installed() {
    let server = Server::start().await;
    let fixture = Fixture::new("1.0.0");
    fixture.serve(&server);
    let (_dir, layout) = data_dir();
    let selection = selection(&fixture, &["claude"]);
    let set = install::install(&layout, &selection, &server.sources(), &quiet)
        .await
        .unwrap()
        .set()
        .clone();
    let cli = format!("node_modules/@vendor/claude-cli-{}", here().key());
    assert!(!set.path.join("claude").join(&cli).exists());
    assert!(set.record.adapters["claude"].cli_skipped);
    assert!(
        set.path
            .join(format!("codex/node_modules/@vendor/codex-cli-{}", here().key()))
            .exists()
    );
    assert!(!server.requests().iter().any(|(p, _)| p.contains("claude-cli")));
}

#[test]
fn an_entry_that_needs_an_install_script_is_refused() {
    let mut fixture = Fixture::new("1.0.0");
    for files in fixture
        .manifest
        .adapters
        .get_mut("codex")
        .unwrap()
        .platforms
        .values_mut()
    {
        files[1].install_script = true;
    }
    let err = Selection::new(&fixture.manifest, &fixture.hash(), here(), &BTreeSet::new()).unwrap_err();
    assert!(err.to_string().contains("install script"), "{err}");
}
