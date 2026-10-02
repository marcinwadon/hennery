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
//! `REPLACE`, `UPDATE`, `DELETE` or `WITH`, in any case (ED4), and every
//! rule reads its text in any case too.
//!
//! A statement that does not prepare, a `{CONST}` that does not resolve,
//! or a source with fewer statements than it has fails the test too, so a
//! broken extractor cannot pass by finding nothing. And every file of the
//! workspace's `src` that holds SQL is in `SOURCES` or, with its reason, in
//! `EXEMPT`.

use hennery_gateway::store::GatewayStore;
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
        27,
    ),
    (
        "hennery-kernel/src/passkeys.rs",
        include_str!("../../hennery-kernel/src/passkeys.rs"),
        9,
    ),
    (
        "hennery-kernel/src/hats.rs",
        include_str!("../../hennery-kernel/src/hats.rs"),
        20,
    ),
    (
        "hennery-kernel/src/hosts.rs",
        include_str!("../../hennery-kernel/src/hosts.rs"),
        16,
    ),
    (
        "hennery-kernel/src/push.rs",
        include_str!("../../hennery-kernel/src/push.rs"),
        20,
    ),
    (
        "hennery-kernel/src/recents.rs",
        include_str!("../../hennery-kernel/src/recents.rs"),
        3,
    ),
    (
        "hennery-sessions/src/store.rs",
        include_str!("../../hennery-sessions/src/store.rs"),
        146,
    ),
    (
        "hennery-gateway/src/store.rs",
        include_str!("../../hennery-gateway/src/store.rs"),
        GATEWAY_STATEMENTS,
    ),
    // Plan 8d: session tokens, and the proxy's reads and writes.
    (
        "hennery-gateway/src/tokens.rs",
        include_str!("../../hennery-gateway/src/tokens.rs"),
        4,
    ),
    (
        "hennery-gateway/src/scope.rs",
        include_str!("../../hennery-gateway/src/scope.rs"),
        5,
    ),
];

/// The gateway store's statements (plan 8a), apart from the list above so
/// that other lanes' changes to it stay apart from this one.
const GATEWAY_STATEMENTS: usize = 23;

/// Files under `crates/*/src` with SQL that the audit does not read, and
/// why (3b-iii review, A3). Any other such file fails
/// `every_file_with_sql_is_audited_or_exempt`.
const EXEMPT: &[(&str, &str)] = &[
    (
        "hennery-kernel/src/db.rs",
        "`kernel_owner`'s query finds the owner; the rest is the migrations' bookkeeping and their unit tests",
    ),
    (
        "hennery-sessions/src/shared_files.rs",
        "attachment files are shared by hash across owners: whether any owner's row still names one, before its file \
         is removed (plan 9a A6); an existence read, no data, no write",
    ),
    (
        "hennery-host/src/outbox.rs",
        "the host's own database, on the host's machine, not `hennery.db`",
    ),
    (
        "hennery-host/src/agent_home.rs",
        "the host's own registry of agent homes (plan 9d B1), on the host's machine, not `hennery.db`",
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

/// `const NAME: &str = "…";` in `source`, by name, the literal on the
/// declaration's line or, wrapped by rustfmt, on the next.
fn str_consts(source: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (at, _) in source.match_indices("const ") {
        let decl = &source[at + "const ".len()..];
        let Some((name, rest)) = decl.split_once(": &str =") else {
            continue;
        };
        let rest = rest.trim_start();
        if !name.chars().all(|c| c.is_ascii_uppercase() || c == '_') || !rest.starts_with('"') {
            continue;
        }
        let value = string_literals(rest).into_iter().next().expect("the const's literal");
        out.insert(name.to_string(), value);
    }
    out
}

/// Whether a string literal is SQL: it begins with a statement's verb,
/// `REPLACE` among them (ED4). In uppercase the verb alone is enough, so a
/// fragment such as `"SELECT "`, the start of a query put together piece by
/// piece, is found (and fails to prepare, or makes A3 flag its file). In
/// any other case, another word must follow: a lone lowercase `"update"` or
/// `"select"` is a JSON key or value, not a statement.
fn is_sql(lit: &str) -> bool {
    const VERBS: &[&str] = &["SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "REPLACE"];
    let mut words = lit.split_whitespace();
    let first = words.next().unwrap_or_default();
    VERBS.contains(&first) || (VERBS.iter().any(|verb| first.eq_ignore_ascii_case(verb)) && words.next().is_some())
}

/// Whether `word` is one of `sql`'s words (runs of letters, digits and
/// `_`), in any case: `from_seq` holds no `from`.
fn has_word(sql: &str, word: &str) -> bool {
    sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|w| w.eq_ignore_ascii_case(word))
}

/// What follows the first whole word `word` of `sql`, if it has one.
fn after_word<'a>(sql: &'a str, word: &str) -> Option<&'a str> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut start = None;
    for (at, c) in sql.char_indices().chain([(sql.len(), ' ')]) {
        match (ident(c), start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                if sql[from..at].eq_ignore_ascii_case(word) {
                    return Some(&sql[at..]);
                }
                start = None;
            }
            _ => {}
        }
    }
    None
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

/// The audit's connection to the migrated `db`, with SQLite's legacy
/// double-quoted string literals turned off (3b-iii final re-review, N1).
/// With them on, a `"…"` that names nothing falls back to a string, and
/// its text, which the textual rules keep as a quoted name, could declare
/// an alias of `owners` (`" from owners h "`). Off, `"…"` is always a
/// name, and anything else fails to prepare, so it fails the audit.
fn audit_connection(db: &Path) -> rusqlite::Connection {
    use rusqlite::config::DbConfig;
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false).unwrap();
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false).unwrap();
    conn
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

/// The select list of every `SELECT` in `sql`, in order: what follows
/// that word, up to the next whole word `FROM` (or the end). A subquery's
/// list is its own: an outer list ends at the subquery's `FROM` at the
/// latest, before any `WHERE` of it (3b-iii final review, I2).
fn select_lists(sql: &str) -> Vec<&str> {
    let mut lists = Vec::new();
    let mut rest = sql;
    while let Some(list) = after_word(rest, "select") {
        lists.push(after_word(list, "from").map_or(list, |tail| &list[..list.len() - tail.len()]));
        rest = list;
    }
    lists
}

/// Whether `sql` orders, groups or partitions by `owner_id`: the column, as
/// a whole word, in the list after `ORDER BY`, `GROUP BY` or `PARTITION BY`,
/// up to the next word that ends that list. Read there, the column filters
/// nothing (3b-iii final review, I2).
fn orders_by_owner(sql: &str) -> bool {
    const ENDS_THE_LIST: &str =
        "limit offset having window order union except intersect where from select and or on join returning values set";
    let words: Vec<String> = sql
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    words.windows(2).enumerate().any(|(at, w)| {
        matches!(w[0].as_str(), "order" | "group" | "partition")
            && w[1] == "by"
            && words[at + 2..]
                .iter()
                .take_while(|word| !ENDS_THE_LIST.split(' ').any(|end| end == *word))
                .any(|word| word == "owner_id")
    })
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

/// `sql` as the textual rules read it: its own comments removed (`-- … `
/// to end of line, `/* … */`), since a commented-out comparison is not a
/// real one (3b-iii review, M2); and each string literal (`'…'`, `''` its
/// escape) emptied to `''`, since its text is a value, not the statement's
/// own words. A comment's opening inside a literal opens nothing, and a
/// literal's `from owners h` declares no table and no alias (3b-iii final
/// review, I1). A quoted name (`"…"`, `` `…` ``, `[…]`) is a name: it is
/// kept as it is, and a comment's opening inside it opens nothing either.
fn without_sql_comments(sql: &str) -> String {
    let s: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if matches!(s[i], '\'' | '"' | '`' | '[') {
            let quote = s[i];
            let close = if quote == '[' { ']' } else { quote };
            let start = i;
            i += 1;
            // To the closing quote; a doubled one (`''`, `""`) is its escape.
            while i < s.len() {
                if s[i] == close && !(close != ']' && s.get(i + 1) == Some(&close)) {
                    break;
                }
                i += if s[i] == close { 2 } else { 1 };
            }
            i += 1;
            if quote == '\'' {
                out.push_str("''");
            } else {
                out.extend(&s[start..i.min(s.len())]);
            }
        } else if s[i] == '-' && s.get(i + 1) == Some(&'-') {
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
/// N1; ED6). Keywords and names are read in any case (ED4): a `from hosts`
/// missed for its case would leave `owners`, named only in a subquery, as
/// the statement's only table, and `hosts`' own `id` would count.
fn compares_owner_with_a_parameter(sql: &str) -> bool {
    let sql = without_sql_comments(sql).to_ascii_lowercase();
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
    // The tables this statement names, after `FROM`, `JOIN`, `UPDATE` or
    // `INTO` (which ends `INSERT INTO`, `REPLACE INTO` and `INSERT OR
    // REPLACE INTO` alike), and any alias declared right after `owners`
    // there. Textual, like the rest of this check: it does not track a
    // subquery's own scope, only the word that follows a source keyword.
    let mut tables: BTreeSet<&str> = BTreeSet::new();
    let mut owners_aliases: BTreeSet<&str> = BTreeSet::new();
    const NOT_AN_ALIAS: &[&str] = &[
        "where", "on", "set", "values", "join", "order", "group", "limit", "as", "select", ",",
    ];
    for i in 0..tokens.len() {
        let names_a_table = matches!(tokens[i], "from" | "join" | "update" | "into");
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
        let alias = if next == Some("as") {
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
/// owner column counts as filtering by it, so `owner_id` is never read for
/// its value or its order: not returned, not in any select list, the
/// outer one or a subquery's (so not copied by an `INSERT … SELECT`
/// either: the owner comes from a parameter there), and not ordered,
/// grouped or partitioned by; and one owner column at least is compared
/// with a parameter. Rule 1 is per table and A1 per statement, so without
/// those, a second table whose owner column is only read to order it, or
/// returned by a subquery, passed beside another table's comparison
/// (3b-iii final review, I2). What this cannot see (3b-iii review, A4): a
/// filter that is wrong rather than missing (`OR`, `owner_id = owner_id`,
/// `owner_id <> ''`); `owners`' own `id` read for its value or its order
/// (`EXISTS (SELECT id FROM owners)`), since only `owner_id` is matched
/// there; `owner_id` later in an ordering expression than its first
/// keyword (`ORDER BY 1 AND owner_id`), since the ordering list ends at
/// `AND`, `OR` and the like. The second-owner tests are the other half.
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
    // The textual rules read the statement without its comments and the
    // text of its string literals (I1).
    let code = without_sql_comments(sql);
    // `SELECT` and `FROM` as whole words, in any case: a column such as
    // `from_seq` must not cut a select list short. An `INSERT`'s first list
    // is what it inserts.
    for (at, list) in select_lists(&code).into_iter().enumerate() {
        if !has_word(list, "owner_id") {
            continue;
        }
        if at == 0 && is_insert(&code) {
            out.push("its INSERT … SELECT copies owner_id: take it from a parameter".into());
        } else {
            out.push("a select list reads owner_id: compare the column, do not read it back".into());
        }
    }
    if orders_by_owner(&code) {
        out.push("it orders or groups by owner_id: compare the column instead".into());
    }
    let values_only = is_insert(&code) && !has_word(&code, "select");
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
    GatewayStore::open(&db).unwrap();
    let conn = audit_connection(&db);
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
    let conn = audit_connection(&db);
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
        // 3b-iii final review I1: ED3's hosts case again, with a comment's
        // opening inside a string literal. Stripped as a comment, `--`
        // took `FROM hosts` with it, and `/*` everything up to a real
        // `*/`: `owners`, from the subquery, was then the only table left.
        "SELECT name, '--' FROM hosts
         WHERE id = ?1 AND owner_id IN (SELECT id FROM owners)",
        "SELECT name, '/*' FROM hosts /**/ WHERE id = ?1 AND owner_id IN (SELECT id FROM owners)",
        // The same class: a literal's text read as the statement's own.
        // Here it declared `h` an alias of `owners`, so `hosts`' own
        // `h.id = ?1` counted as the owner comparison.
        "SELECT name, ' from owners h ' FROM hosts h WHERE h.id = ?1 AND owner_id IN (SELECT id FROM owners)",
        // 3b-iii final review I2: a second table's owner column read only
        // to order or group it, or returned by a subquery's select list,
        // while another table carries the comparison with a parameter.
        // `auth_sessions`, `sessions` and `turns` are filtered by nothing.
        "SELECT phc FROM password_credentials
         WHERE owner_id = ?1 AND EXISTS (SELECT 1 FROM auth_sessions WHERE id_hash = ?2 ORDER BY owner_id)",
        "SELECT phc FROM password_credentials
         WHERE owner_id = ?1 AND EXISTS (SELECT 1 FROM auth_sessions WHERE id_hash = ?2 GROUP BY owner_id)",
        "SELECT q.answer FROM answer_queue q JOIN sessions s ON s.id = q.session_id
         WHERE q.owner_id = ?1 ORDER BY s.owner_id",
        "SELECT body FROM events WHERE owner_id = ?1
         AND session_id IN (SELECT id FROM sessions WHERE owner_id IN (SELECT owner_id FROM turns WHERE turn_id = ?2))",
        // Every select list is read now, the outer one too: the owner
        // column under an alias, or as an expression, beside another
        // comparison (A4's two select-list cases until then).
        "SELECT phc, owner_id AS o FROM password_credentials WHERE owner_id = ?1",
        "SELECT q.answer, s.owner_id = ?2 FROM answer_queue q JOIN sessions s ON s.id = q.session_id
         WHERE q.owner_id = ?1",
        // ED4 (3b-iii Task 3 review M3): SQL is case-insensitive, and so is
        // every rule that reads the statement's text. The authorizer names
        // tables as the schema does (`events`), whatever case the statement
        // wrote them in.
        "SELECT body FROM EVENTS WHERE event_id = ?1",
        // ED3's hosts case with its outer keywords in lowercase: the A1
        // rule must not miss `from hosts`, take `owners` (named only in the
        // subquery) for the statement's only table, and so count `hosts`'
        // own `id = ?1` as the owner comparison.
        "select name from hosts where id = ?1 and owner_id in (SELECT id FROM owners)",
        "insert into events(session_id, kind, body, ts, owner_id)
         select session_id, kind, body, ts, ?1 from events where event_id = ?2",
        "insert into events(session_id, kind, body, ts, owner_id)
         select session_id, kind, body, ts, owner_id from events where event_id = ?1 and owner_id = ?2",
        "INSERT INTO events(session_id, kind, body, ts, owner_id)
         select ?1, ?2, ?3, ?4, ?5 where exists (select 1 from sessions where id = ?1 and owner_id = owner_id)",
        // M1 (Task 5 review): `SELECT` and `FROM` are whole words. A select
        // list with `from_seq` in it still copies `owner_id`.
        "INSERT INTO events(session_id, kind, body, ts, owner_id)
         SELECT session_id AS from_seq, kind, body, ts, owner_id FROM events WHERE event_id = ?1 AND owner_id = ?2",
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
        "select set_up_at from Owners o where O.ID = ?1",
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
        "SELECT ",
        "DELETE",
    ] {
        assert!(is_sql(sql), "not SQL: {sql}");
    }
    for not_sql in ["update", "select", "selected rows", "Without it", "replaced by"] {
        assert!(!is_sql(not_sql), "SQL: {not_sql}");
    }
    let literals = string_literals("// \"no\"\nlet a = 'x'; let b = '\"'; f::<'a>(\"one\", r#\"two \"q\"\"#);");
    assert_eq!(literals, ["one", "two \"q\""]);
    // I1: a string literal's text is emptied, `''` and all; a quoted name
    // is kept; a comment's opening opens nothing inside either.
    assert_eq!(
        without_sql_comments("SELECT 'it''s -- x', \"a--b\", [c/*d] -- e\nFROM t /* f */"),
        "SELECT '', \"a--b\", [c/*d] \nFROM t "
    );
    // N1 (final re-review): the double-quoted twin of the literal-alias case
    // does not prepare on the audit's connection, so it fails the audit
    // (`touched` panics); with SQLite's default it was a string, and passed.
    // A double-quoted name still prepares.
    let double_quoted = "SELECT name, \" from owners h \" FROM hosts h
         WHERE h.id = ?1 AND owner_id IN (SELECT id FROM owners)";
    assert!(conn.prepare(double_quoted).is_err(), "prepared: {double_quoted}");
    assert!(conn.prepare("SELECT \"name\" FROM \"hosts\"").is_ok());
}
