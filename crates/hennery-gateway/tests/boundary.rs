//! The module boundary (umbrella §9, gateway spec §10): `hennery-gateway`
//! never depends on `hennery-sessions`, directly or through another crate
//! of the workspace, in any dependency table of its own. Read from the
//! manifests rather than `cargo metadata`, so it holds in a sandboxed
//! build too.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn manifest(path: &Path) -> toml::Table {
    let text = std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    text.parse().unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// The workspace's crates by name, from `[workspace.dependencies]`' paths.
fn workspace_paths(root: &Path) -> std::collections::BTreeMap<String, PathBuf> {
    let workspace = manifest(&root.join("Cargo.toml"));
    workspace["workspace"]["dependencies"]
        .as_table()
        .unwrap()
        .iter()
        .filter_map(|(name, spec)| {
            let path = spec.get("path")?.as_str()?;
            Some((name.clone(), root.join(path)))
        })
        .collect()
}

/// The workspace crates `manifest` names in `tables`, by name.
fn local_deps(
    manifest: &toml::Table,
    tables: &[&str],
    crates: &std::collections::BTreeMap<String, PathBuf>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut sections: Vec<&toml::Table> = vec![manifest];
    if let Some(targets) = manifest.get("target").and_then(|t| t.as_table()) {
        sections.extend(targets.values().filter_map(|t| t.as_table()));
    }
    for section in sections {
        for table in tables {
            let Some(deps) = section.get(*table).and_then(|t| t.as_table()) else {
                continue;
            };
            for (name, spec) in deps {
                let name = spec.get("package").and_then(|p| p.as_str()).unwrap_or(name).to_string();
                if crates.contains_key(&name) || spec.get("path").is_some() {
                    out.push(name);
                }
            }
        }
    }
    out
}

#[test]
fn the_gateway_never_depends_on_the_sessions_module() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates = workspace_paths(&root);
    assert!(crates.contains_key("hennery-sessions"), "{crates:?}");
    let gateway = manifest(&Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"));
    let mut seen = BTreeSet::new();
    let mut queue = local_deps(
        &gateway,
        &["dependencies", "dev-dependencies", "build-dependencies"],
        &crates,
    );
    assert!(queue.contains(&"hennery-kernel".to_string()), "{queue:?}");
    while let Some(name) = queue.pop() {
        assert_ne!(
            name, "hennery-sessions",
            "hennery-gateway depends on it through {seen:?}"
        );
        if !seen.insert(name.clone()) {
            continue;
        }
        let path = crates
            .get(&name)
            .unwrap_or_else(|| panic!("{name} is not in the workspace"));
        let theirs = manifest(&path.join("Cargo.toml"));
        queue.extend(local_deps(&theirs, &["dependencies", "build-dependencies"], &crates));
    }
    assert!(seen.contains("hennery-proto"), "{seen:?}");
}
