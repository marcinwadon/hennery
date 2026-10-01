//! The project picker on the host (ACP core §7; plan 6c): the workspace
//! roots, from `host.toml` or `--workspace-root`, and the user's home.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// The most workspace roots a host takes (decision 6).
pub const MAX_ROOTS: usize = 32;

/// The host user's home directory: `$HOME`, if it is set and absolute (the
/// review's A10). Without one, `~/` roots are refused and the browse fence
/// is the workspace roots alone.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// The workspace roots to use (decision 6): `given` (the `--workspace-root`
/// flags) if there are any, else `configured` (`host.toml`). Flags replace
/// the file's list, as `--listen` replaces `config.toml`'s (kernel spec §2).
///
/// Each root is absolute, or `~` / `~/…`, expanded against `home`; at most
/// `MAX_ROOTS`; duplicates are dropped. A root is not checked for
/// existence: a missing one is tolerated (ACP core §7).
pub fn workspace_roots(given: &[String], configured: &[String], home: Option<&Path>) -> Result<Vec<PathBuf>> {
    let raw = if given.is_empty() { configured } else { given };
    if raw.len() > MAX_ROOTS {
        bail!("{} workspace roots; at most {MAX_ROOTS} are supported", raw.len());
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in raw {
        let path = if root == "~" || root.starts_with("~/") {
            let Some(home) = home else {
                bail!("workspace root {root:?} needs $HOME, which is unset or not absolute");
            };
            match root.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => home.to_path_buf(),
            }
        } else {
            PathBuf::from(root)
        };
        if !path.is_absolute() {
            bail!("workspace root {root:?} is not absolute; give /… or ~/…");
        }
        if path.to_str().is_none() {
            bail!("workspace root {root:?} is not valid UTF-8");
        }
        if !roots.contains(&path) {
            roots.push(path);
        }
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(roots: &[&str]) -> Vec<String> {
        roots.iter().map(|r| r.to_string()).collect()
    }

    #[test]
    fn the_flags_replace_the_files_roots() {
        let home = Path::new("/home/u");
        let file = strings(&["/srv/a", "~/src"]);
        assert_eq!(
            workspace_roots(&[], &file, Some(home)).unwrap(),
            [PathBuf::from("/srv/a"), PathBuf::from("/home/u/src")]
        );
        assert_eq!(
            workspace_roots(&strings(&["/p"]), &file, Some(home)).unwrap(),
            [PathBuf::from("/p")]
        );
        assert!(workspace_roots(&[], &[], Some(home)).unwrap().is_empty());
    }

    #[test]
    fn a_root_is_absolute_or_under_home() {
        let home = Path::new("/home/u");
        assert_eq!(
            workspace_roots(&strings(&["~", "~/", "/p", "/p"]), &[], Some(home)).unwrap(),
            [PathBuf::from("/home/u"), PathBuf::from("/p")],
            "`~/` is `~` again, so it is dropped as a duplicate"
        );
        for bad in ["src", "./src", "", "~other/src"] {
            let err = workspace_roots(&strings(&[bad]), &[], Some(home)).unwrap_err();
            assert!(err.to_string().contains("not absolute"), "{bad:?}: {err}");
        }
        let err = workspace_roots(&strings(&["~/src"]), &[], None).unwrap_err();
        assert!(err.to_string().contains("$HOME"), "{err}");
        assert_eq!(
            workspace_roots(&strings(&["/p"]), &[], None).unwrap(),
            [PathBuf::from("/p")]
        );
    }

    #[test]
    fn at_most_max_roots_are_taken() {
        let many: Vec<String> = (0..=MAX_ROOTS).map(|n| format!("/r{n}")).collect();
        let err = workspace_roots(&many, &[], None).unwrap_err();
        assert!(err.to_string().contains("at most"), "{err}");
        assert_eq!(workspace_roots(&many[1..], &[], None).unwrap().len(), MAX_ROOTS);
    }
}
