//! Plan 8e decision 12, held by the source (the security review's finding
//! 1): the sessions module ends what is open on a revoked token only after
//! the transaction that revoked it has committed. Every cut goes through
//! `store.rs`'s `commit_then_cut`, which commits first, so no site can cut
//! before its commit, or forget to commit first, without failing here.

use std::path::Path;

/// Every Rust source of the crate, with its path, nested modules included.
fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
        }
    }
}

#[test]
fn every_cut_goes_through_commit_then_cut() {
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    assert!(files.len() > 10, "the crate's sources were not found");
    let mut cuts = Vec::new();
    for (path, text) in &files {
        for (n, line) in text.lines().enumerate() {
            if line.contains(".cut(") {
                cuts.push(format!("{path}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert_eq!(cuts.len(), 1, "a cut outside `commit_then_cut`: {cuts:#?}");
    let store = &files.iter().find(|(path, _)| path.ends_with("store.rs")).unwrap().1;
    let helper = store
        .split("fn commit_then_cut(")
        .nth(1)
        .expect("store.rs has no `commit_then_cut`");
    let body = &helper[..helper.find("\n}\n").unwrap()];
    let (commit, cut) = (body.find("tx.commit()?;").unwrap(), body.find(".cut(").unwrap());
    assert!(commit < cut, "`commit_then_cut` cuts before it commits");
    assert!(cuts[0].contains("mcp.cut(cut)"), "{cuts:?}");
}
