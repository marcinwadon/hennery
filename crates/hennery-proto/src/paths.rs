//! Path rules shared by the host's browse fence (ACP core §7) and, with
//! hats, path-rule matching (kernel spec §5.2): one definition, so the two
//! can never disagree (plan 6c, the review's A9).

/// Whether `path` is `root` or lies under it, by whole path segments:
/// `/p/acme` holds `/p/acme` and `/p/acme/x`, never `/p/acme-infra`.
///
/// Both are canonical: absolute, symlinks resolved, no `.` or `..`, no
/// trailing slash (but `/` itself). They are compared byte for byte, and
/// case is never folded: on a case-insensitive filesystem a path spelled
/// differently from its root is outside it, which fails closed.
pub fn is_within(path: &str, root: &str) -> bool {
    if root == "/" {
        return path.starts_with('/');
    }
    match path.strip_prefix(root) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}
