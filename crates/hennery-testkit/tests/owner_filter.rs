//! Every query filters by the owner (kernel spec §1, umbrella §7.4; plan
//! 3b-iii decision 6, 3b-ii's A10). Each SQL statement in the stores'
//! sources is prepared against a migrated `hennery.db` under an
//! authorizer, which records every table the statement touches:
//! - a table it reads, changes or deletes from must have its owner column
//!   read too (`owner_id`; `id` for `owners`), whether in a `WHERE`, a
//!   join or a subquery;
//! - an `INSERT` must name `owner_id` among its columns.
//!
//! A statement that does not prepare, a `{CONST}` that does not resolve,
//! or a source with fewer statements than it has fails the test too, so a
//! broken extractor cannot pass by finding nothing.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_sessions::store::Store;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// The audited sources, and the fewest statements each holds.
const SOURCES: &[(&str, &str, usize)] = &[(
    "hennery-kernel/src/operator.rs",
    include_str!("../../hennery-kernel/src/operator.rs"),
    17,
)];

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

/// The SQL statements in `source`, `{CONST}`s resolved.
fn statements(source: &str) -> Vec<String> {
    let consts = str_consts(source);
    let mut out = Vec::new();
    for lit in string_literals(source) {
        let first = lit.split_whitespace().next().unwrap_or_default();
        if !matches!(first, "SELECT" | "INSERT" | "UPDATE" | "DELETE" | "WITH") {
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

/// Whether an `INSERT INTO table(…)` in `sql` names `owner_id`.
fn inserts_owner(sql: &str, table: &str) -> bool {
    let Some((_, after)) = sql.split_once(&format!("INSERT INTO {table}(")) else {
        return false;
    };
    let columns = after.split(')').next().unwrap_or_default();
    columns.split(',').any(|c| c.trim() == "owner_id")
}

/// Whether `sql` compares an owner column with a parameter: `owner_id = ?N`
/// or `?N = owner_id`, qualified (`p.owner_id`) or not, or `id = ?N` when it
/// names `owners`. A join that compares two owner columns ties the rows to
/// each other, not to the owner (3b-iii review, A1).
fn compares_owner_with_a_parameter(sql: &str) -> bool {
    let tokens: Vec<&str> = sql
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ','))
        .filter(|t| !t.is_empty())
        .collect();
    let names_owners = tokens.contains(&"owners");
    let column = |t: &str| {
        let name = t.rsplit('.').next().unwrap_or(t);
        name == "owner_id" || (names_owners && name == "id")
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
    let tables: BTreeSet<&String> = t.read.keys().chain(&t.changed).collect();
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
    if sql.trim_start().starts_with("INSERT")
        && let Some((_, select)) = sql.split_once("SELECT")
        && select.split("FROM").next().unwrap_or_default().contains("owner_id")
    {
        out.push("its INSERT … SELECT copies owner_id: take it from a parameter".into());
    }
    let values_only = sql.trim_start().starts_with("INSERT") && !sql.contains("SELECT");
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
        assert!(
            sql.len() >= *at_least,
            "{path}: {} statements, not {at_least}",
            sql.len()
        );
        for statement in sql {
            for problem in problems(&conn, &statement) {
                found.push(format!("{path}: {problem}:\n    {statement}"));
            }
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
    Operator::open(&db).unwrap();
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
        "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, expires_at)
         SELECT ?1, owner_id, 'x', 0, 0, 0 FROM password_credentials WHERE phc = ?2",
        "SELECT s.user_agent FROM auth_sessions s JOIN password_credentials c ON c.owner_id = s.owner_id
         WHERE s.id_hash = ?1",
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
        "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, expires_at)
         SELECT ?1, ?2, 'x', 0, 0, 0 FROM password_credentials WHERE owner_id = ?2 AND phc = ?3",
        "SELECT 1",
    ] {
        assert_eq!(problems(&conn, good), Vec::<String>::new(), "{good}");
    }
    let literals = string_literals("// \"no\"\nlet a = 'x'; let b = '\"'; f::<'a>(\"one\", r#\"two \"q\"\"#);");
    assert_eq!(literals, ["one", "two \"q\""]);
}
