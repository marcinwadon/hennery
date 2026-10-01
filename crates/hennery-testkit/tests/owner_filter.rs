//! Every query filters by the owner (kernel spec §1, umbrella §7.4; plan
//! 3b-iii decision 6, 3b-ii's A10). Each SQL statement in the stores'
//! sources is prepared against a migrated `hennery.db` under an
//! authorizer, which records every table the statement touches:
//! - a table it reads, changes or deletes from must have its owner column
//!   read too (`owner_id`; `id` for `owners`), whether in a `WHERE`, a
//!   join or a subquery;
//! - one owner column at least must be compared with a parameter, not only
//!   with another owner column (all but a plain `INSERT … VALUES`);
//! - an `INSERT` must name `owner_id` among its columns.
//!
//! Reads SQLite makes on its own are not the statement's: a foreign key's
//! check of its parent (a table the statement does not name), and an
//! upsert's look-up of its conflict target (the table it inserts into, when
//! it changes nothing there and does not select from it).
//!
//! A statement is any string literal that begins with `SELECT`, `INSERT`,
//! `REPLACE`, `UPDATE`, `DELETE` or `WITH`, in any case (ED4), and the
//! rules read its text case-insensitively too, where a miss would let it
//! pass.
//!
//! A statement that does not prepare, a `{CONST}` that does not resolve,
//! or a source with fewer statements than it has fails the test too, so a
//! broken extractor cannot pass by finding nothing. And every file of the
//! workspace's `src` that holds SQL is in `SOURCES` or, with its reason, in
//! `EXEMPT`.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_sessions::store::Store;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The audited sources, and the fewest statements each holds.
const SOURCES: &[(&str, &str, usize)] = &[
    (
        "hennery-kernel/src/operator.rs",
        include_str!("../../hennery-kernel/src/operator.rs"),
        19,
    ),
    (
        "hennery-kernel/src/hosts.rs",
        include_str!("../../hennery-kernel/src/hosts.rs"),
        13,
    ),
    (
        "hennery-sessions/src/store.rs",
        include_str!("../../hennery-sessions/src/store.rs"),
        77,
    ),
];

/// Files under `crates/*/src` with SQL that the audit does not read, and
/// why (3b-iii review, A3). Any other such file fails
/// `every_file_with_sql_is_audited_or_exempt`.
const EXEMPT: &[(&str, &str)] = &[
    (
        "hennery-kernel/src/db.rs",
        "`kernel_owner`'s query finds the owner; the rest is the migrations' bookkeeping and their unit tests",
    ),
    (
        "hennery-host/src/outbox.rs",
        "the host's own database, on the host's machine, not `hennery.db`",
    ),
];

/// Every string literal in `source`, in order, skipping comments and char
/// literals. Enough of Rust's lexer for these files: plain and raw
/// strings, the common escapes and line continuations.
fn string_literals(source: &str) -> Vec<String> {
    let s: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if rest.starts_with(&['/', '/']) {
            while i < s.len() && s[i] != '\n' {
                i += 1;
            }
        } else if rest.starts_with(&['/', '*']) {
            i += 2;
            while i + 1 < s.len() && !(s[i] == '*' && s[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if s[i] == 'r'
            && matches!(s.get(i + 1), Some('"' | '#'))
            && (i == 0 || !(s[i - 1].is_alphanumeric() || s[i - 1] == '_'))
        {
            let mut hashes = 0;
            i += 1;
            while s[i] == '#' {
                hashes += 1;
                i += 1;
            }
            i += 1;
            let mut lit = String::new();
            loop {
                if s[i] == '"' && s[i + 1..].iter().take(hashes).filter(|c| **c == '#').count() == hashes {
                    i += 1 + hashes;
                    break;
                }
                lit.push(s[i]);
                i += 1;
            }
            out.push(lit);
        } else if s[i] == '"' {
            i += 1;
            let mut lit = String::new();
            while s[i] != '"' {
                if s[i] == '\\' {
                    i += 1;
                    match s[i] {
                        'n' => lit.push('\n'),
                        't' => lit.push('\t'),
                        '\n' => {
                            while s[i + 1].is_whitespace() {
                                i += 1;
                            }
                        }
                        c => lit.push(c),
                    }
                } else {
                    lit.push(s[i]);
                }
                i += 1;
            }
            i += 1;
            out.push(lit);
        } else if s[i] == '\'' {
            // A char literal ('x', '\n', '"'), or a lifetime ('a).
            if s.get(i + 1) == Some(&'\\') {
                i += 2;
                while s[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if s.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// `const NAME: &str = "…";` in `source`, by name.
fn str_consts(source: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (at, _) in source.match_indices("const ") {
        let decl = &source[at + "const ".len()..];
        let Some((name, rest)) = decl.split_once(": &str = ") else {
            continue;
        };
        if !name.chars().all(|c| c.is_ascii_uppercase() || c == '_') || !rest.starts_with('"') {
            continue;
        }
        let value = string_literals(rest).into_iter().next().expect("the const's literal");
        out.insert(name.to_string(), value);
    }
    out
}

/// Whether a string literal is SQL: it begins with a statement's verb, in
/// any case, `REPLACE` among them (ED4), and goes on past it. A literal
/// that is only the word (`"update"`, `"select"`: JSON keys and values) is
/// not a statement.
fn is_sql(lit: &str) -> bool {
    let mut words = lit.split_whitespace();
    let first = words.next().unwrap_or_default();
    ["SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "REPLACE"]
        .iter()
        .any(|verb| first.eq_ignore_ascii_case(verb))
        && words.next().is_some()
}

/// `source` without its unit tests (`#[cfg(test)] mod tests`, at the end
/// of the file): their fixtures write raw rows on purpose, and are no
/// store's queries. Everything from the exact text `\n#[cfg(test)]\nmod
/// tests {` on is left out. Clippy's default `items_after_test_module`
/// lint, which CI runs with `-D warnings`, is what keeps production code
/// out of that region: an item after the test module fails the build (ED2;
/// do not allow the lint in an audited file).
fn without_tests(source: &str) -> &str {
    source.split("\n#[cfg(test)]\nmod tests {").next().unwrap_or(source)
}

/// The SQL statements in `source`, `{CONST}`s resolved; its unit tests
/// left out.
fn statements(source: &str) -> Vec<String> {
    let source = without_tests(source);
    let consts = str_consts(source);
    let mut out = Vec::new();
    for lit in string_literals(source) {
        if !is_sql(&lit) {
            continue;
        }
        let mut sql = lit.clone();
        for (name, value) in &consts {
            sql = sql.replace(&format!("{{{name}}}"), value);
        }
        assert!(!sql.contains('{'), "an unresolved constant in {lit:?}");
        out.push(sql);
    }
    out
}

/// What preparing `sql` touches: per table, the actions and the columns
/// read; and the columns it returns.
#[derive(Debug, Default)]
struct Touched {
    read: BTreeMap<String, BTreeSet<String>>,
    changed: BTreeSet<String>,
    inserted: BTreeSet<String>,
    returned: Vec<String>,
}

fn touched(conn: &rusqlite::Connection, sql: &str) -> Touched {
    let log: Arc<Mutex<Touched>> = Arc::default();
    let sink = log.clone();
    conn.authorizer(Some(move |ctx: AuthContext<'_>| {
        let mut t = sink.lock().unwrap();
        match ctx.action {
            AuthAction::Read {
                table_name,
                column_name,
            } => {
                t.read
                    .entry(table_name.to_string())
                    .or_default()
                    .insert(column_name.to_string());
            }
            AuthAction::Update { table_name, .. } | AuthAction::Delete { table_name } => {
                t.changed.insert(table_name.to_string());
            }
            AuthAction::Insert { table_name } => {
                t.inserted.insert(table_name.to_string());
            }
            _ => {}
        }
        Authorization::Allow
    }))
    .unwrap();
    let prepared = conn
        .prepare(sql)
        .map(|stmt| stmt.column_names().into_iter().map(String::from).collect());
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>).unwrap();
    let returned = prepared.unwrap_or_else(|err| panic!("{err}: {sql}"));
    let mut touched = Arc::try_unwrap(log).unwrap().into_inner().unwrap();
    touched.returned = returned;
    touched
}

/// The column that names a table's owner.
fn owner_column(table: &str) -> &'static str {
    if table == "owners" { "id" } else { "owner_id" }
}

/// Whether `sql` reads `table` itself: `FROM table` or `JOIN table`. An
/// `INSERT … SELECT` from its own target is then the statement's read, not
/// an upsert's look-up of its conflict target (3b-iii review, A2). In any
/// case (ED4): the authorizer names `table` as the schema does.
fn selects_from(sql: &str, table: &str) -> bool {
    let tokens: Vec<&str> = sql.split_whitespace().collect();
    tokens.windows(2).any(|w| {
        (w[0].eq_ignore_ascii_case("FROM") || w[0].eq_ignore_ascii_case("JOIN"))
            && w[1].trim_end_matches([')', ',']).eq_ignore_ascii_case(table)
    })
}

/// Whether `sql` names `table`, as a whole word, in any case (ED4): the
/// authorizer names it as the schema does, so `FROM EVENTS` names `events`.
fn names(sql: &str, table: &str) -> bool {
    let sql = sql.to_ascii_lowercase();
    let table = table.to_ascii_lowercase();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    sql.match_indices(&table)
        .any(|(at, _)| !word(sql[..at].chars().next_back()) && !word(sql[at + table.len()..].chars().next()))
}

/// Whether `sql` is an `INSERT`: it begins with `INSERT` or `REPLACE`, in
/// any case (ED4).
fn is_insert(sql: &str) -> bool {
    let first = sql.split_whitespace().next().unwrap_or_default();
    first.eq_ignore_ascii_case("INSERT") || first.eq_ignore_ascii_case("REPLACE")
}

/// Whether an `INSERT INTO table(…)` in `sql` names `owner_id`, in any
/// case; `REPLACE INTO` and `INSERT OR REPLACE INTO` too (ED4).
fn inserts_owner(sql: &str, table: &str) -> bool {
    let sql = sql.to_ascii_lowercase();
    let Some((_, after)) = sql.split_once(&format!(" into {}(", table.to_ascii_lowercase())) else {
        return false;
    };
    let columns = after.split(')').next().unwrap_or_default();
    columns.split(',').any(|c| c.trim() == "owner_id")
}

/// `sql` with its own comments removed (`-- … ` to end of line, `/* … */`):
/// neither can satisfy `compares_owner_with_a_parameter` (3b-iii review,
/// M2) — a commented-out comparison is not a real one.
fn without_sql_comments(sql: &str) -> String {
    let s: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == '-' && s.get(i + 1) == Some(&'-') {
            while i < s.len() && s[i] != '\n' {
                i += 1;
            }
        } else if s[i] == '/' && s.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < s.len() && !(s[i] == '*' && s[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// Whether `sql` compares an owner column with a parameter: `owner_id = ?N`
/// or `?N = owner_id`, qualified (`p.owner_id`) or not, or `id = ?N` when
/// `id` names `owners` — the statement's only table (`FROM owners`,
/// `UPDATE owners`, with no other `FROM`/`JOIN`/`UPDATE`/`INSERT INTO`), or
/// qualified with `owners` itself or an alias declared right after it
/// (`owners o`, `owners AS o`). `owners` appearing anywhere in the
/// statement is not enough on its own (3b-iii review, I1; ED3): before
/// this, a join or a subquery could tie some other table's unrelated `id`
/// to a parameter and pass, as long as the statement named `owners`
/// somewhere, even in an unconnected subquery. A join that compares two
/// owner columns to each other, not to a parameter, still does not count
/// (A1). A comma right after `owners` (`FROM owners, hosts`) is the next
/// item of an old-style join, not an alias: the comma is kept as its own
/// token so the next word is never mistaken for one (3b-iii review re-review
/// N1; ED6).
fn compares_owner_with_a_parameter(sql: &str) -> bool {
    let sql = without_sql_comments(sql);
    let tokens: Vec<&str> = sql
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')'))
        .filter(|t| !t.is_empty())
        .flat_map(|word| {
            let mut parts = Vec::new();
            for (at, part) in word.split(',').enumerate() {
                if at > 0 {
                    parts.push(",");
                }
                if !part.is_empty() {
                    parts.push(part);
                }
            }
            parts
        })
        .collect();
    // The tables this statement names (`FROM`/`JOIN`/`UPDATE`/`INSERT
    // INTO`), and any alias declared right after `owners` there. Textual,
    // like the rest of this check: it does not track a subquery's own
    // scope, only the word that follows a source keyword.
    let mut tables: BTreeSet<&str> = BTreeSet::new();
    let mut owners_aliases: BTreeSet<&str> = BTreeSet::new();
    const NOT_AN_ALIAS: &[&str] = &[
        "WHERE", "ON", "SET", "VALUES", "JOIN", "ORDER", "GROUP", "LIMIT", "AS", "SELECT", ",",
    ];
    for i in 0..tokens.len() {
        let names_a_table = matches!(tokens[i], "FROM" | "JOIN" | "UPDATE")
            || (tokens[i] == "INTO" && i > 0 && tokens[i - 1] == "INSERT");
        if !names_a_table {
            continue;
        }
        let Some(&table) = tokens.get(i + 1) else { continue };
        tables.insert(table);
        if table != "owners" {
            continue;
        }
        owners_aliases.insert(table);
        let next = tokens.get(i + 2).copied();
        let alias = if next == Some("AS") {
            tokens.get(i + 3).copied()
        } else {
            next
        };
        if let Some(alias) = alias
            && !NOT_AN_ALIAS.contains(&alias)
        {
            owners_aliases.insert(alias);
        }
    }
    let only_owners = tables.len() == 1 && tables.contains("owners");
    let column = |t: &str| {
        let (qualifier, name) = match t.rsplit_once('.') {
            Some((q, n)) => (Some(q), n),
            None => (None, t),
        };
        if name == "owner_id" {
            return true;
        }
        if name != "id" {
            return false;
        }
        match qualifier {
            Some(q) => owners_aliases.contains(q),
            None => only_owners,
        }
    };
    let parameter = |t: &str| t.len() > 1 && t.starts_with('?') && t[1..].chars().all(|c| c.is_ascii_digit());
    tokens
        .windows(3)
        .any(|w| w[1] == "=" && ((column(w[0]) && parameter(w[2])) || (parameter(w[0]) && column(w[2]))))
}

/// Why `sql` does not filter by the owner, if it does not. A read of the
/// owner column counts as filtering by it, so the column is never read for
/// its value: not returned, and not copied by an `INSERT … SELECT` (the
/// owner comes from a parameter there); and one owner column at least is
/// compared with a parameter. What this cannot see (3b-iii review, A4): a
/// filter that is wrong (`OR`, `owner_id = owner_id`); the owner column read
/// for its value under an alias (`owner_id AS o`) or as an expression in
/// the select list (`owner_id = ?1` there) beside another comparison. The
/// second-owner tests are the other half.
fn problems(conn: &rusqlite::Connection, sql: &str) -> Vec<String> {
    let t = touched(conn, sql);
    let mut out = Vec::new();
    let tables: BTreeSet<&String> = t
        .read
        .keys()
        .chain(&t.changed)
        .filter(|table| names(sql, table))
        .filter(|table| !t.inserted.contains(*table) || t.changed.contains(*table) || selects_from(sql, table))
        .collect();
    for table in &tables {
        let read = t.read.get(*table).cloned().unwrap_or_default();
        if !read.contains(owner_column(table)) {
            out.push(format!("{table} is touched without reading {}", owner_column(table)));
        }
    }
    for table in &t.inserted {
        if !inserts_owner(sql, table) {
            out.push(format!("an INSERT INTO {table} names no owner_id"));
        }
    }
    if t.returned.iter().any(|c| c == "owner_id") {
        out.push("it returns owner_id: compare the column, do not read it back".into());
    }
    let lower = sql.to_ascii_lowercase();
    if is_insert(sql)
        && let Some((_, select)) = lower.split_once("select")
        && select.split("from").next().unwrap_or_default().contains("owner_id")
    {
        out.push("its INSERT … SELECT copies owner_id: take it from a parameter".into());
    }
    let values_only = is_insert(sql) && !lower.contains("select");
    if !tables.is_empty() && !values_only && !compares_owner_with_a_parameter(sql) {
        out.push("no owner column is compared with a parameter".into());
    }
    out
}

#[test]
fn every_query_of_the_stores_filters_by_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    Operator::open(&db).unwrap();
    Hosts::open(&db).unwrap();
    Store::open(&db).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    let mut found = Vec::new();
    for (path, source, at_least) in SOURCES {
        let sql = statements(source);
        if sql.len() < *at_least {
            found.push(format!("{path}: {} statements, not {at_least}", sql.len()));
        }
        for statement in sql {
            for problem in problems(&conn, &statement) {
                found.push(format!("{path}: {problem}:\n    {statement}"));
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// Every `*.rs` under `dir`, as paths relative to `root` with `/` between
/// their parts.
fn rust_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let parts: Vec<String> = path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(parts.join("/"));
        }
    }
}

/// 3b-iii review, A3: a file with SQL cannot escape the audit by not being
/// listed. Every `*.rs` under `crates/*/src` that holds a statement is in
/// `SOURCES` or `EXEMPT`, and every `EXEMPT` file still exists.
#[test]
fn every_file_with_sql_is_audited_or_exempt() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let listed: BTreeSet<&str> = SOURCES
        .iter()
        .map(|(path, ..)| *path)
        .chain(EXEMPT.iter().map(|(path, _)| *path))
        .collect();
    let mut files = Vec::new();
    let mut members: Vec<_> = std::fs::read_dir(crates).unwrap().map(|e| e.unwrap().path()).collect();
    members.sort();
    for member in members {
        let src = member.join("src");
        if src.is_dir() {
            rust_files(crates, &src, &mut files);
        }
    }
    assert!(files.len() > 20, "found only {files:?}");
    let mut found = Vec::new();
    for path in &files {
        let source = std::fs::read_to_string(crates.join(path)).unwrap();
        let holds_sql = string_literals(without_tests(&source)).iter().any(|lit| is_sql(lit));
        if holds_sql && !listed.contains(path.as_str()) {
            found.push(format!(
                "{path} holds SQL: add it to SOURCES, or to EXEMPT with a reason"
            ));
        }
    }
    for (path, _) in EXEMPT {
        if !crates.join(path).is_file() {
            found.push(format!("{path} is in EXEMPT but no longer exists"));
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The audit itself: a statement without the owner is caught, whichever
/// way it touches a table, and one with it passes.
#[test]
fn the_audit_catches_a_query_without_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    Store::open(&db).unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    for bad in [
        "SELECT phc FROM password_credentials LIMIT 1",
        "SELECT count(*) FROM auth_sessions",
        "DELETE FROM auth_sessions WHERE expires_at <= ?1",
        "UPDATE auth_sessions SET last_step_up_at = ?2 WHERE id_hash = ?1",
        "SELECT 1 FROM owners WHERE id = ?1 AND EXISTS (SELECT 1 FROM auth_sessions WHERE id_hash = ?2)",
        "INSERT INTO settings(key, value) VALUES (?1, ?2)",
        "SELECT set_up_at FROM owners LIMIT 1",
        "SELECT owner_id FROM auth_sessions WHERE id_hash = ?1",
        "INSERT INTO session_catalog(session_id, config_options, updated_at, owner_id) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(session_id) DO UPDATE SET config_options = excluded.config_options",
        "INSERT INTO events(session_id, kind, body, ts, owner_id)
         SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM sessions WHERE id = ?1)",
        "SELECT q.answer FROM answer_queue q JOIN sessions s ON s.id = q.session_id AND s.owner_id = q.owner_id
         WHERE s.host_id = ?1",
        "INSERT INTO events(session_id, kind, body, ts, owner_id)
         SELECT session_id, kind, body, ts, ?1 FROM events WHERE event_id = ?2",
        "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, expires_at)
         SELECT ?1, owner_id, 'x', 0, 0, 0 FROM password_credentials WHERE phc = ?2",
        "SELECT s.user_agent FROM auth_sessions s JOIN password_credentials c ON c.owner_id = s.owner_id
         WHERE s.id_hash = ?1",
        // 3b-iii review I1 / ED3: `owners` appearing anywhere in the
        // statement used to make any `id`, however unrelated, count as the
        // owner comparison. Here `p` is a derived table drawing from
        // `owners.id`, not an alias of `owners` itself, and the only real
        // comparison (`ps.owner_id = p.id`) does not involve a parameter.
        "SELECT ps.phc FROM password_credentials ps, (SELECT id FROM owners WHERE set_up_at IS NOT NULL) p
         WHERE ps.owner_id = p.id AND p.id = ?1",
        // 3b-iii review M2: a comment cannot stand in for a real
        // comparison. `owner_id` is read (so rule 1 passes), but the only
        // `= ?N` naming it is commented out.
        "SELECT phc FROM password_credentials WHERE owner_id IS NOT NULL -- owner_id = ?1",
        // 3b-iii Task 4, ED3: the same gap as above, now that `hosts` also
        // has an `owner_id`. `owners` is named, but only in an unconnected
        // subquery; the real filter, `hosts.id = ?1`, is not an owner
        // comparison at all.
        "SELECT name FROM hosts WHERE id = ?1 AND owner_id IN (SELECT id FROM owners)",
        // 3b-iii Task 4, ED3 re-review N1 / ED6: a comma join. The alias
        // scan must not take `hosts`, the next table name after the comma,
        // for an alias of `owners`: `hosts.id = ?1` is not an owner
        // comparison, even though `hosts.owner_id = owners.id` (two owner
        // columns compared to each other, not to a parameter, A1) is
        // right next to it.
        "SELECT name FROM owners, hosts WHERE hosts.owner_id = owners.id AND hosts.id = ?1",
        // ED4 (3b-iii Task 3 review M3): SQL is case-insensitive, and so is
        // every rule that reads the statement's text. The authorizer names
        // tables as the schema does (`events`), whatever case the statement
        // wrote them in.
        "SELECT body FROM EVENTS WHERE event_id = ?1",
        "insert into events(session_id, kind, body, ts, owner_id)
         select session_id, kind, body, ts, ?1 from events where event_id = ?2",
        "insert into events(session_id, kind, body, ts, owner_id)
         select session_id, kind, body, ts, owner_id from events where event_id = ?1 and owner_id = ?2",
        "INSERT INTO events(session_id, kind, body, ts, owner_id)
         select ?1, ?2, ?3, ?4, ?5 where exists (select 1 from sessions where id = ?1 and owner_id = owner_id)",
        // ED4: `REPLACE` inserts.
        "REPLACE INTO session_catalog(session_id, config_options, updated_at) VALUES (?1, ?2, ?3)",
        "INSERT OR REPLACE INTO session_catalog(session_id, config_options, updated_at) VALUES (?1, ?2, ?3)",
    ] {
        assert!(!problems(&conn, bad).is_empty(), "passed: {bad}");
    }
    for good in [
        "SELECT phc FROM password_credentials WHERE owner_id = ?1",
        "DELETE FROM auth_sessions WHERE expires_at <= ?1 AND owner_id = ?2",
        "INSERT INTO settings(owner_id, key, value) VALUES (?1, ?2, ?3)
         ON CONFLICT(owner_id, key) DO UPDATE SET value = excluded.value",
        "SELECT set_up_at FROM owners WHERE id = ?1",
        "UPDATE owners SET set_up_at = ?2 WHERE id = ?1 AND set_up_at IS NULL",
        "SELECT phc FROM password_credentials WHERE ?1 = owner_id",
        "SELECT s.user_agent FROM auth_sessions s JOIN password_credentials c ON c.owner_id = s.owner_id
         WHERE s.id_hash = ?1 AND s.owner_id = ?2",
        "INSERT INTO turns(turn_id, session_id, content, created_at, owner_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(session_id, host_seq) DO NOTHING",
        "INSERT INTO session_catalog(session_id, config_options, updated_at, owner_id) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(session_id) DO UPDATE SET config_options = excluded.config_options
             WHERE session_catalog.owner_id = excluded.owner_id",
        "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, expires_at)
         SELECT ?1, ?2, 'x', 0, 0, 0 FROM password_credentials WHERE owner_id = ?2 AND phc = ?3",
        "SELECT 1",
        "select body from events where event_id = ?1 and owner_id = ?2",
        "REPLACE INTO session_catalog(session_id, config_options, updated_at, owner_id) VALUES (?1, ?2, ?3, ?4)",
        "INSERT OR REPLACE INTO session_catalog(session_id, config_options, updated_at, owner_id)
         VALUES (?1, ?2, ?3, ?4)",
    ] {
        assert_eq!(problems(&conn, good), Vec::<String>::new(), "{good}");
    }
    // ED4: a statement is found whatever the case of its verb, `REPLACE`
    // among them; a literal that is only the word is not one (`"update"`,
    // `"select"`: JSON keys and values in the host's and testkit's code).
    for sql in [
        "select 1",
        "Insert INTO t(a) VALUES (1)",
        "update t SET a = 1",
        "delete FROM t",
        "with x AS (SELECT 1) SELECT * FROM x",
        "REPLACE INTO t(a) VALUES (1)",
        "replace into t(a) values (1)",
    ] {
        assert!(is_sql(sql), "not SQL: {sql}");
    }
    for not_sql in ["update", "select", "selected rows", "Without it", "replaced by"] {
        assert!(!is_sql(not_sql), "SQL: {not_sql}");
    }
    let literals = string_literals("// \"no\"\nlet a = 'x'; let b = '\"'; f::<'a>(\"one\", r#\"two \"q\"\"#);");
    assert_eq!(literals, ["one", "two \"q\""]);
}
