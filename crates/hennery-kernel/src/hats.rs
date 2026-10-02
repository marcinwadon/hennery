//! Hats (kernel spec §5; umbrella §8): the isolation boundary a session
//! belongs to. A hat is `{id, name, colour}`; every host has a default hat,
//! and per-host path rules (`prefix → hat`) pick another for the paths
//! under them. The registry keeps them beside the hosts, on the same
//! connection (`Hosts`), so a host and its default hat always change
//! together. Every query names the database's owner (kernel spec §1).
//!
//! Resolution (kernel spec §5.2) is a pure function of a host's rules, its
//! default hat and a canonical path (`resolve`): rules match by path
//! segment, the longest prefix wins, and no match means the host's default
//! hat. Canonicalisation itself happens on the host, where the filesystem
//! is; here a path is only ever checked to be in canonical form
//! (`is_canonical`), never trusted to be.

use crate::hosts::{Hosts, is_format_char, is_valid_display_field};
use crate::secret::random_bytes;
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use std::collections::{BTreeSet, HashMap};

/// The colour a hat gets when none is given: slate, neither a warning nor
/// a brand.
pub const DEFAULT_COLOUR: &str = "#64748b";

/// The most path rules one host has (plan 5a decision 6).
pub const MAX_RULES: usize = 256;

/// The longest path accepted, in bytes: Linux's `PATH_MAX`.
pub const MAX_PATH: usize = 4096;

/// The `settings` key naming the hat a newly paired host gets (plan 5a
/// decision 2).
pub(crate) const DEFAULT_HAT_KEY: &str = "default_hat_id";

/// One hat, as `GET /api/hats` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatRecord {
    pub id: String,
    pub name: String,
    /// `#rrggbb`, lowercase.
    pub colour: String,
    pub created_at: i64,
    /// The hat newly paired hosts get as their default (kernel spec §4.1).
    pub default_for_new_hosts: bool,
}

/// One stored path rule of a host (kernel spec §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRule {
    pub id: String,
    /// Canonical: absolute, no `.` or `..`, no trailing slash.
    pub prefix: String,
    pub hat_id: String,
    /// The host resolved the prefix when the rule was saved. An unverified
    /// rule is stored as typed, normalised lexically (kernel spec §5.2).
    pub verified: bool,
}

/// A rule to store, its prefix already in canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRule {
    pub prefix: String,
    pub hat_id: String,
    pub verified: bool,
}

/// Which hat a path resolves to on a host, and the rule that decided it
/// (`None`: no rule matched, so the host's default hat).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub hat_id: String,
    pub rule_id: Option<String>,
}

/// The outcome of `Hosts::create_hat` and `Hosts::update_hat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HatChange {
    Done(HatRecord),
    NotFound,
    /// Another hat of the owner has this name, in any case.
    NameTaken,
    /// The request is not acceptable (why); nothing changed.
    Invalid(String),
}

/// The outcome of `Hosts::replace_path_rules`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RulesChange {
    /// The host's rules now, longest prefix first.
    Done(Vec<PathRule>),
    HostNotFound,
    Invalid(String),
}

/// The outcome of `Hosts::update_host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostChange {
    Done,
    NotFound,
    Invalid(String),
}

/// A hat's colour as given, if it is `#rrggbb`: returned lowercase. The
/// frontend applies it as a CSS custom property, so nothing else may pass.
pub fn parse_colour(input: &str) -> Option<String> {
    let hex = input.strip_prefix('#')?;
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("#{}", hex.to_ascii_lowercase()))
}

/// Why `name` cannot name a hat, if it cannot: the rule for host names (1
/// to 64 printable characters once trimmed, no invisible format characters).
pub fn hat_name_problem(name: &str) -> Option<String> {
    (!is_valid_display_field(name)).then(|| "a hat's name must be 1 to 64 printable characters".to_string())
}

/// `path` in canonical form by its text alone (kernel spec §5.2's
/// unverified rule): absolute, empty and `.` segments dropped, no trailing
/// slash. Refuses a relative path, a `..` segment (the review's O1: by its
/// text alone it can disagree with the filesystem when a parent is a
/// symlink), a NUL or other control character, and a path longer than
/// `MAX_PATH`. Symlinks are not resolved: only the host can do that.
pub fn normalize_lexically(path: &str) -> Result<String, String> {
    if !path.starts_with('/') {
        return Err("a path must be absolute".into());
    }
    if path.len() > MAX_PATH {
        return Err(format!("a path must be at most {MAX_PATH} bytes"));
    }
    if path.chars().any(char::is_control) {
        return Err("a path must not hold control characters".into());
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => return Err("a path must not hold `..`; give it as the host resolves it".into()),
            other => segments.push(other),
        }
    }
    Ok(format!("/{}", segments.join("/")))
}

/// Whether `path` is already in the form `normalize_lexically` gives. A
/// path from a host is checked with this before it is matched against any
/// rule: the host is authenticated, but its answer is still its own words.
pub fn is_canonical(path: &str) -> bool {
    normalize_lexically(path).is_ok_and(|normal| normal == path)
}

/// Whether the rule `prefix` covers `path`: it is the path, or a parent of
/// it by whole segments (`/p/acme` covers `/p/acme/x`, never
/// `/p/acme-infra`). Both are canonical.
///
/// The one segment rule, shared with the host's browse fence
/// (`hennery_proto::paths::is_within`, plan 6c's A9), so the fence and the
/// hat rules cannot disagree.
pub fn covers(prefix: &str, path: &str) -> bool {
    hennery_proto::paths::is_within(path, prefix)
}

/// The hat `path` (canonical) resolves to among a host's `rules`, with
/// `default_hat` when no rule covers it (kernel spec §5.2): the longest
/// covering prefix wins. Two rules never share a prefix on one host.
pub fn resolve(rules: &[PathRule], default_hat: &str, path: &str) -> Resolution {
    rules
        .iter()
        .filter(|rule| covers(&rule.prefix, path))
        .max_by_key(|rule| rule.prefix.len())
        .map(|rule| Resolution {
            hat_id: rule.hat_id.clone(),
            rule_id: Some(rule.id.clone()),
        })
        .unwrap_or_else(|| Resolution {
            hat_id: default_hat.to_string(),
            rule_id: None,
        })
}

fn new_id(kind: &str) -> String {
    format!("{kind}-{}", hex::encode(random_bytes::<8>()))
}

fn read_rule(r: &rusqlite::Row<'_>) -> rusqlite::Result<PathRule> {
    Ok(PathRule {
        id: r.get(0)?,
        prefix: r.get(1)?,
        hat_id: r.get(2)?,
        verified: r.get(3)?,
    })
}

/// A host's rules, longest prefix first, inside the caller's lock.
pub(crate) fn rules_of(conn: &rusqlite::Connection, owner: &str, host_id: &str) -> Result<Vec<PathRule>> {
    let mut stmt = conn.prepare(
        "SELECT id, prefix, hat_id, verified FROM hat_path_rules
         WHERE host_id = ?1 AND owner_id = ?2 ORDER BY length(prefix) DESC, prefix",
    )?;
    let rows = stmt.query_map([host_id, owner], read_rule)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Whether `hat_id` is one of `owner`'s hats.
fn hat_exists(conn: &rusqlite::Connection, owner: &str, hat_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM hats WHERE id = ?1 AND owner_id = ?2",
            [hat_id, owner],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn hats_of(conn: &rusqlite::Connection, owner: &str) -> Result<Vec<HatRecord>> {
    let mut stmt = conn.prepare(
        "SELECT h.id, h.name, h.colour, h.created_at, h.id = s.value FROM hats h
         LEFT JOIN settings s ON s.owner_id = h.owner_id AND s.key = ?2
         WHERE h.owner_id = ?1 ORDER BY h.created_at, h.id",
    )?;
    let rows = stmt.query_map([owner, DEFAULT_HAT_KEY], |r| {
        Ok(HatRecord {
            id: r.get(0)?,
            name: r.get(1)?,
            colour: r.get(2)?,
            created_at: r.get(3)?,
            default_for_new_hosts: r.get::<_, Option<bool>>(4)?.unwrap_or(false),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Whether another of the owner's hats than `except` is named `name`,
/// ignoring case (plan 5a decision 4).
fn name_taken(hats: &[HatRecord], name: &str, except: Option<&str>) -> bool {
    let name = name.trim().to_lowercase();
    hats.iter()
        .any(|hat| Some(hat.id.as_str()) != except && hat.name.to_lowercase() == name)
}

impl Hosts {
    /// Every hat of the owner, oldest first.
    pub fn hats(&self) -> Result<Vec<HatRecord>> {
        hats_of(&self.conn(), self.owner_id())
    }

    pub fn hat(&self, hat_id: &str) -> Result<Option<HatRecord>> {
        Ok(self.hats()?.into_iter().find(|hat| hat.id == hat_id))
    }

    /// The hat newly paired hosts get as their default.
    pub fn default_hat_for_new_hosts(&self) -> Result<String> {
        Ok(self.conn().query_row(
            "SELECT value FROM settings WHERE owner_id = ?1 AND key = ?2",
            [self.owner_id(), DEFAULT_HAT_KEY],
            |r| r.get(0),
        )?)
    }

    /// A new hat, `colour` defaulting to `DEFAULT_COLOUR`.
    pub fn create_hat(&self, name: &str, colour: Option<&str>, now: i64) -> Result<HatChange> {
        if let Some(problem) = hat_name_problem(name) {
            return Ok(HatChange::Invalid(problem));
        }
        let Some(colour) = parse_colour(colour.unwrap_or(DEFAULT_COLOUR)) else {
            return Ok(HatChange::Invalid("a hat's colour must be #rrggbb".into()));
        };
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if name_taken(&hats_of(&tx, self.owner_id())?, name, None) {
            return Ok(HatChange::NameTaken);
        }
        let id = new_id("hat");
        tx.execute(
            "INSERT INTO hats(id, owner_id, name, colour, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, self.owner_id(), name.trim(), colour, now],
        )?;
        let created = hats_of(&tx, self.owner_id())?.into_iter().find(|hat| hat.id == id);
        tx.commit()?;
        Ok(created.map_or(HatChange::NotFound, HatChange::Done))
    }

    /// Rename a hat, change its colour, or make it the one new hosts get.
    /// A hat stops being that default only by another taking its place, so
    /// `default_for_new_hosts` is `Some(true)` or absent.
    pub fn update_hat(
        &self,
        hat_id: &str,
        name: Option<&str>,
        colour: Option<&str>,
        default_for_new_hosts: Option<bool>,
    ) -> Result<HatChange> {
        if let Some(problem) = name.and_then(hat_name_problem) {
            return Ok(HatChange::Invalid(problem));
        }
        let colour = match colour.map(parse_colour) {
            Some(None) => return Ok(HatChange::Invalid("a hat's colour must be #rrggbb".into())),
            Some(Some(colour)) => Some(colour),
            None => None,
        };
        if default_for_new_hosts == Some(false) {
            return Ok(HatChange::Invalid(
                "make another hat the default for new hosts instead".into(),
            ));
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let hats = hats_of(&tx, self.owner_id())?;
        if !hats.iter().any(|hat| hat.id == hat_id) {
            return Ok(HatChange::NotFound);
        }
        if let Some(name) = name {
            if name_taken(&hats, name, Some(hat_id)) {
                return Ok(HatChange::NameTaken);
            }
            tx.execute(
                "UPDATE hats SET name = ?2 WHERE id = ?1 AND owner_id = ?3",
                params![hat_id, name.trim(), self.owner_id()],
            )?;
        }
        if let Some(colour) = colour {
            tx.execute(
                "UPDATE hats SET colour = ?2 WHERE id = ?1 AND owner_id = ?3",
                params![hat_id, colour, self.owner_id()],
            )?;
        }
        if default_for_new_hosts == Some(true) {
            // Otherwise the transaction rolls back: the name and colour
            // changes above don't apply either.
            let moved = tx.execute(
                "UPDATE settings SET value = ?1 WHERE owner_id = ?2 AND key = ?3",
                params![hat_id, self.owner_id(), DEFAULT_HAT_KEY],
            )?;
            anyhow::ensure!(moved == 1, "the owner has no default hat setting to move");
        }
        let updated = hats_of(&tx, self.owner_id())?.into_iter().find(|hat| hat.id == hat_id);
        tx.commit()?;
        Ok(updated.map_or(HatChange::NotFound, HatChange::Done))
    }

    /// A host's path rules, longest prefix first; `None` for an unknown
    /// host.
    pub fn path_rules(&self, host_id: &str) -> Result<Option<Vec<PathRule>>> {
        let conn = self.conn();
        if !host_exists(&conn, self.owner_id(), host_id)? {
            return Ok(None);
        }
        Ok(Some(rules_of(&conn, self.owner_id(), host_id)?))
    }

    /// Replace a host's path rules with `rules`, in one transaction (kernel
    /// spec §8: the full set). Each prefix must already be canonical and
    /// each hat the owner's; no two rules share a prefix. `/` is refused:
    /// it is what the host's default hat is for (the review's O2). So is a
    /// prefix with an invisible format character, which could display as
    /// another path (O6). A rule whose prefix was there before keeps its id.
    pub fn replace_path_rules(&self, host_id: &str, rules: &[NewRule]) -> Result<RulesChange> {
        if rules.len() > MAX_RULES {
            return Ok(RulesChange::Invalid(format!(
                "a host has at most {MAX_RULES} path rules"
            )));
        }
        let mut seen = BTreeSet::new();
        for rule in rules {
            if !is_canonical(&rule.prefix) {
                return Ok(RulesChange::Invalid(format!(
                    "{:?} is not an absolute path in canonical form",
                    rule.prefix
                )));
            }
            if rule.prefix == "/" {
                return Ok(RulesChange::Invalid(
                    "a rule for / is the host's default hat: change that instead".into(),
                ));
            }
            if rule.prefix.chars().any(is_format_char) {
                return Ok(RulesChange::Invalid(format!(
                    "{:?} holds an invisible format character",
                    rule.prefix
                )));
            }
            if !seen.insert(rule.prefix.as_str()) {
                return Ok(RulesChange::Invalid(format!("two rules for {:?}", rule.prefix)));
            }
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if !host_exists(&tx, self.owner_id(), host_id)? {
            return Ok(RulesChange::HostNotFound);
        }
        for rule in rules {
            if !hat_exists(&tx, self.owner_id(), &rule.hat_id)? {
                return Ok(RulesChange::Invalid(format!("no hat {:?}", rule.hat_id)));
            }
        }
        let kept: HashMap<String, String> = rules_of(&tx, self.owner_id(), host_id)?
            .into_iter()
            .map(|rule| (rule.prefix, rule.id))
            .collect();
        tx.execute(
            "DELETE FROM hat_path_rules WHERE host_id = ?1 AND owner_id = ?2",
            [host_id, self.owner_id()],
        )?;
        for rule in rules {
            let id = kept.get(&rule.prefix).cloned().unwrap_or_else(|| new_id("rule"));
            tx.execute(
                "INSERT INTO hat_path_rules(id, owner_id, host_id, prefix, hat_id, verified)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, self.owner_id(), host_id, rule.prefix, rule.hat_id, rule.verified],
            )?;
        }
        let stored = rules_of(&tx, self.owner_id(), host_id)?;
        tx.commit()?;
        Ok(RulesChange::Done(stored))
    }

    /// The hat `path` resolves to on `host_id` (kernel spec §5.2), from
    /// one read of the host's default hat and its rules; `None` for an
    /// unknown host. `path` must be canonical: anything else is an error,
    /// never a guess.
    pub fn resolve_hat(&self, host_id: &str, path: &str) -> Result<Option<Resolution>> {
        anyhow::ensure!(is_canonical(path), "not a canonical path: {path:?}");
        let conn = self.conn();
        let Some(default_hat) = host_default_hat(&conn, self.owner_id(), host_id)? else {
            return Ok(None);
        };
        let rules = rules_of(&conn, self.owner_id(), host_id)?;
        Ok(Some(resolve(&rules, &default_hat, path)))
    }

    /// Whether one of the host's rules names a hat other than its default:
    /// rules only. This is half of whether the host is mixed (umbrella
    /// §8.5), which the gateway's fallback needs; the other half is a live
    /// session of another hat than the default (a default changed since it
    /// started), which only the sessions module knows (the review's A1).
    /// `false` for an unknown host.
    pub fn rules_name_other_hats(&self, host_id: &str) -> Result<bool> {
        let conn = self.conn();
        let Some(default_hat) = host_default_hat(&conn, self.owner_id(), host_id)? else {
            return Ok(false);
        };
        Ok(rules_of(&conn, self.owner_id(), host_id)?
            .iter()
            .any(|rule| rule.hat_id != default_hat))
    }

    /// Rename a host, or change its default hat (kernel spec §4.3), in one
    /// transaction. The hat must be the owner's.
    pub fn update_host(&self, host_id: &str, name: Option<&str>, default_hat_id: Option<&str>) -> Result<HostChange> {
        if let Some(name) = name
            && !is_valid_display_field(name)
        {
            return Ok(HostChange::Invalid(
                "a host's name must be 1 to 64 printable characters".into(),
            ));
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if !host_exists(&tx, self.owner_id(), host_id)? {
            return Ok(HostChange::NotFound);
        }
        if let Some(name) = name {
            tx.execute(
                "UPDATE hosts SET name = ?2 WHERE id = ?1 AND owner_id = ?3",
                params![host_id, name.trim(), self.owner_id()],
            )?;
        }
        if let Some(hat_id) = default_hat_id {
            if !hat_exists(&tx, self.owner_id(), hat_id)? {
                return Ok(HostChange::Invalid(format!("no hat {hat_id:?}")));
            }
            tx.execute(
                "UPDATE hosts SET default_hat_id = ?2 WHERE id = ?1 AND owner_id = ?3",
                params![host_id, hat_id, self.owner_id()],
            )?;
        }
        tx.commit()?;
        Ok(HostChange::Done)
    }
}

fn host_exists(conn: &rusqlite::Connection, owner: &str, host_id: &str) -> Result<bool> {
    Ok(host_default_hat(conn, owner, host_id)?.is_some())
}

pub(crate) fn host_default_hat(conn: &rusqlite::Connection, owner: &str, host_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT default_hat_id FROM hosts WHERE id = ?1 AND owner_id = ?2",
            [host_id, owner],
            |r| r.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, prefix: &str, hat: &str) -> PathRule {
        PathRule {
            id: id.into(),
            prefix: prefix.into(),
            hat_id: hat.into(),
            verified: true,
        }
    }

    /// Kernel spec §11: segment match, longest prefix, default fallback.
    #[test]
    fn resolution_matches_whole_segments_and_the_longest_prefix_wins() {
        let rules = [
            rule("r-acme", "/p/acme", "hat-acme"),
            rule("r-secret", "/p/acme/secret", "hat-secret"),
            rule("r-root", "/srv", "hat-srv"),
        ];
        let cases = [
            ("/p/acme", "hat-acme", Some("r-acme")),
            ("/p/acme/x", "hat-acme", Some("r-acme")),
            ("/p/acme-infra", "hat-default", None),
            ("/p/acme-infra/x", "hat-default", None),
            ("/p/acme/secret", "hat-secret", Some("r-secret")),
            ("/p/acme/secret/deep/er", "hat-secret", Some("r-secret")),
            ("/p/acme/secretive", "hat-acme", Some("r-acme")),
            ("/p", "hat-default", None),
            ("/", "hat-default", None),
            ("/srv/x", "hat-srv", Some("r-root")),
        ];
        for (path, hat, rule_id) in cases {
            let got = resolve(&rules, "hat-default", path);
            assert_eq!((got.hat_id.as_str(), got.rule_id.as_deref()), (hat, rule_id), "{path}");
        }
        // The order the rules come in does not matter.
        let mut reversed = rules.clone();
        reversed.reverse();
        assert_eq!(
            resolve(&reversed, "hat-default", "/p/acme/secret/x").hat_id,
            "hat-secret"
        );
    }

    #[test]
    fn a_rule_for_the_root_covers_every_path() {
        let rules = [rule("r", "/", "hat-root"), rule("r-p", "/p", "hat-p")];
        assert_eq!(resolve(&rules, "d", "/x").hat_id, "hat-root");
        assert_eq!(resolve(&rules, "d", "/").hat_id, "hat-root");
        assert_eq!(resolve(&rules, "d", "/p/x").hat_id, "hat-p");
        assert_eq!(resolve(&rules, "d", "/pq").hat_id, "hat-root");
    }

    /// The review's O1: a `..` is refused, never applied by its text.
    #[test]
    fn lexical_normalisation_drops_dots_and_slashes_and_refuses_what_is_not_a_path() {
        let cases = [
            ("/", "/"),
            ("//", "/"),
            ("/p/acme/", "/p/acme"),
            ("/p//acme", "/p/acme"),
            ("/p/./acme", "/p/acme"),
            ("/p/acme/...", "/p/acme/..."),
            ("/p/..acme", "/p/..acme"),
            ("/p/ac me", "/p/ac me"),
        ];
        for (input, normal) in cases {
            assert_eq!(normalize_lexically(input).as_deref(), Ok(normal), "{input:?}");
        }
        for bad in [
            "",
            "p/acme",
            "~/p",
            "./p",
            "/p/\0x",
            "/p/\nx",
            "/p/x/../acme",
            "/../p",
            "/p/acme/..",
        ] {
            assert!(normalize_lexically(bad).is_err(), "{bad:?}");
        }
        assert!(normalize_lexically(&format!("/{}", "a".repeat(MAX_PATH))).is_err());
        assert!(normalize_lexically(&format!("/{}", "a".repeat(MAX_PATH - 1))).is_ok());
    }

    #[test]
    fn only_a_path_in_canonical_form_is_canonical() {
        for good in ["/", "/p", "/p/acme", "/p/acme/..."] {
            assert!(is_canonical(good), "{good:?}");
        }
        for bad in ["", "p", "/p/", "//p", "/p/./x", "/p/../x", "/p/..", "/p\0"] {
            assert!(!is_canonical(bad), "{bad:?}");
        }
    }

    /// Plan 5a decision 3: a database as 3c left it gets one default hat
    /// per owner, named in `settings`, and every host keeps everything it
    /// had and gains that hat as its default.
    #[test]
    fn the_migration_gives_every_host_its_owners_default_hat() {
        use crate::schema::{COMPONENT, MIGRATIONS};
        let mut conn = crate::db::open_in_memory().unwrap();
        crate::db::migrate_component(&mut conn, COMPONENT, &MIGRATIONS[..5]).unwrap();
        let owner: String = conn.query_row("SELECT id FROM owners", [], |r| r.get(0)).unwrap();
        conn.execute(
            "INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, capabilities, created_at,
                               last_seen_at, revoked_at)
             VALUES ('host-old', ?1, 'laptop', 'aa', 'linux', '0.0.0', '[\"park\"]', 1, 2, 3)",
            [&owner],
        )
        .unwrap();
        crate::db::migrate_component(&mut conn, COMPONENT, MIGRATIONS).unwrap();

        let hats: Vec<(String, String, String)> = conn
            .prepare("SELECT id, owner_id, name FROM hats")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(hats.len(), 1, "{hats:?}");
        let (hat, hat_owner, name) = &hats[0];
        assert_eq!((hat_owner.as_str(), name.as_str()), (owner.as_str(), "Personal"));
        let default: String = conn
            .query_row(
                "SELECT value FROM settings WHERE owner_id = ?1 AND key = 'default_hat_id'",
                [&owner],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(&default, hat);
        let host: (String, String, String, String, i64, i64, i64) = conn
            .query_row(
                "SELECT owner_id, public_key, capabilities, default_hat_id, created_at, last_seen_at, revoked_at
                 FROM hosts WHERE id = 'host-old'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(host, (owner, "aa".into(), "[\"park\"]".into(), hat.clone(), 1, 2, 3));
        // The rebuilt table keeps its constraints: a default hat that is
        // not a hat is refused.
        assert!(
            conn.execute("UPDATE hosts SET default_hat_id = 'hat-nope' WHERE id = 'host-old'", [])
                .is_err()
        );
    }

    /// The review's A3: a host's default hat, and a rule's host and hat,
    /// are the owner's own, by the schema itself, whatever a statement
    /// forgets. The colour and the verified flag are checked there too
    /// (O4).
    #[test]
    fn the_schema_keeps_hosts_hats_and_rules_within_one_owner() {
        let mut conn = crate::db::open_in_memory().unwrap();
        let owner = crate::db::kernel_owner(&mut conn).unwrap();
        let mine: String = conn.query_row("SELECT id FROM hats", [], |r| r.get(0)).unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO owners(id, created_at) VALUES ('owner-other', 1);
             INSERT INTO hats(id, owner_id, name, colour, created_at) VALUES ('hat-other', 'owner-other', 'x', '#000000', 1);
             INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, default_hat_id, created_at)
                 VALUES ('host-1', '{owner}', 'laptop', 'aa', 'linux', '0.0.0', '{mine}', 1);"
        ))
        .unwrap();
        let refused = [
            "UPDATE hosts SET default_hat_id = 'hat-other' WHERE id = 'host-1'".to_string(),
            format!(
                "INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, default_hat_id, created_at)
                 VALUES ('host-2', '{owner}', 'laptop', 'bb', 'linux', '0.0.0', 'hat-other', 1)"
            ),
            format!(
                "INSERT INTO hat_path_rules(id, owner_id, host_id, prefix, hat_id, verified)
                 VALUES ('rule-1', '{owner}', 'host-1', '/p', 'hat-other', 1)"
            ),
            "INSERT INTO hat_path_rules(id, owner_id, host_id, prefix, hat_id, verified)
             VALUES ('rule-2', 'owner-other', 'host-1', '/p', 'hat-other', 1)"
                .to_string(),
            format!("UPDATE hats SET colour = 'red' WHERE id = '{mine}'"),
            format!("UPDATE hats SET colour = '#ABCDEF' WHERE id = '{mine}'"),
            format!(
                "INSERT INTO hat_path_rules(id, owner_id, host_id, prefix, hat_id, verified)
                 VALUES ('rule-3', '{owner}', 'host-1', '/p', '{mine}', 2)"
            ),
        ];
        for sql in &refused {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }
        // The control: the same rule, all the owner's, is stored.
        conn.execute(
            &format!(
                "INSERT INTO hat_path_rules(id, owner_id, host_id, prefix, hat_id, verified)
                 VALUES ('rule-4', '{owner}', 'host-1', '/p', '{mine}', 1)"
            ),
            [],
        )
        .unwrap();
    }

    #[test]
    fn a_colour_is_six_hex_digits_and_comes_back_lowercase() {
        assert_eq!(parse_colour("#A1b2C3").as_deref(), Some("#a1b2c3"));
        for bad in ["a1b2c3", "#a1b2c", "#a1b2c3d", "#g1b2c3", "red", "#a1b2c3;x", ""] {
            assert_eq!(parse_colour(bad), None, "{bad:?}");
        }
    }
}
