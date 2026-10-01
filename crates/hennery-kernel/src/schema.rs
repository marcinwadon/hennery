//! The kernel's tables in `hennery.db` (kernel spec §1.1), migrated as one
//! component beside the sessions store (`db::migrate_component`). Every
//! part of the kernel that opens the database runs the same list, so
//! whichever opens it first creates them all.

/// The kernel's component name in `schema_versions`.
pub(crate) const COMPONENT: &str = "kernel";

pub(crate) const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE hosts (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        public_key TEXT NOT NULL UNIQUE,
        platform TEXT NOT NULL,
        host_version TEXT NOT NULL,
        capabilities TEXT NOT NULL DEFAULT '[]',
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked_at INTEGER);
    CREATE TABLE pairing_codes (
        code_hash TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        used_at INTEGER);
    ",
    // Operator auth (kernel spec §3). The new tables carry `owner_id` from
    // the start; the older ones get it with the backfill (plan 3b-iii).
    "
    CREATE TABLE owners (
        id TEXT PRIMARY KEY,
        contact TEXT,
        created_at INTEGER NOT NULL);
    CREATE TABLE password_credentials (
        owner_id TEXT PRIMARY KEY REFERENCES owners(id),
        phc TEXT NOT NULL,
        updated_at INTEGER NOT NULL);
    CREATE TABLE auth_sessions (
        id_hash TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        user_agent TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER NOT NULL,
        last_step_up_at INTEGER,
        expires_at INTEGER NOT NULL);
    CREATE TABLE settings (
        owner_id TEXT NOT NULL REFERENCES owners(id),
        key TEXT NOT NULL,
        value TEXT NOT NULL,
        PRIMARY KEY (owner_id, key));
    ",
    // `owner_id` everywhere (plan 3b-iii decision 1): the owner exists from
    // the first start, before setup, so what is written then (`up`'s
    // pairing code and host) has an owner to carry. `set_up_at` marks the
    // setup that completes it; an owner already here was made by setup.
    "
    ALTER TABLE owners ADD COLUMN set_up_at INTEGER;
    UPDATE owners SET set_up_at = created_at;
    INSERT INTO owners(id, created_at)
        SELECT 'owner-' || lower(hex(randomblob(8))), unixepoch()
        WHERE NOT EXISTS (SELECT 1 FROM owners);
    ",
    // `owner_id` on the tables from before operator auth, filled with the
    // database's owner (plan 3b-iii decision 4). Rebuilt: SQLite cannot add
    // a column that is both NOT NULL and a foreign key, and nothing
    // references either table.
    "
    CREATE TABLE hosts_owned (
        id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        name TEXT NOT NULL,
        public_key TEXT NOT NULL UNIQUE,
        platform TEXT NOT NULL,
        host_version TEXT NOT NULL,
        capabilities TEXT NOT NULL DEFAULT '[]',
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked_at INTEGER);
    INSERT INTO hosts_owned(id, owner_id, name, public_key, platform, host_version, capabilities,
                            created_at, last_seen_at, revoked_at)
        SELECT id, (SELECT id FROM owners ORDER BY created_at, id LIMIT 1), name, public_key, platform,
               host_version, capabilities, created_at, last_seen_at, revoked_at
        FROM hosts;
    DROP TABLE hosts;
    ALTER TABLE hosts_owned RENAME TO hosts;
    CREATE TABLE pairing_codes_owned (
        code_hash TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        used_at INTEGER);
    INSERT INTO pairing_codes_owned(code_hash, owner_id, created_at, expires_at, used_at)
        SELECT code_hash, (SELECT id FROM owners ORDER BY created_at, id LIMIT 1), created_at, expires_at,
               used_at
        FROM pairing_codes;
    DROP TABLE pairing_codes;
    ALTER TABLE pairing_codes_owned RENAME TO pairing_codes;
    ",
    // Passkeys (kernel spec §1.1, §3.2; plan 3c decision 3). `credential`
    // is `webauthn-rs`'s `Passkey` as JSON. `credential_id` (hex) is
    // unique across owners: one credential, one account. `sign_count` is
    // the authenticator's counter as last accepted, kept beside the JSON
    // so a login can compare and set it in one statement (decision 6).
    "
    CREATE TABLE passkeys (
        id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        credential_id TEXT NOT NULL UNIQUE,
        credential TEXT NOT NULL,
        sign_count INTEGER NOT NULL,
        label TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        last_used_at INTEGER);
    ",
    // Hats (kernel spec §5.1; plan 5a decisions 1 to 3). Every owner gets
    // its default hat here, before setup, as it got its owner row: `up`
    // pairs its host before setup, and no host is ever without a default
    // hat. The `default_hat_id` setting names the hat new hosts get.
    // `hosts` is rebuilt (as in migration 4) to gain `default_hat_id`, NOT
    // NULL, filled with its owner's default hat. A host's default hat and a
    // rule's host and hat are the same owner's, by composite foreign keys
    // (the review's A3). Rules are made last: they reference the rebuilt
    // `hosts`.
    "
    CREATE TABLE hats (
        id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        name TEXT NOT NULL,
        colour TEXT NOT NULL CHECK (colour GLOB '#[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
        created_at INTEGER NOT NULL,
        UNIQUE (id, owner_id));
    INSERT INTO hats(id, owner_id, name, colour, created_at)
        SELECT 'hat-' || lower(hex(randomblob(8))), id, 'Personal', '#64748b', unixepoch() FROM owners;
    INSERT INTO settings(owner_id, key, value)
        SELECT owner_id, 'default_hat_id', id FROM hats;
    CREATE TABLE hosts_hatted (
        id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        name TEXT NOT NULL,
        public_key TEXT NOT NULL UNIQUE,
        platform TEXT NOT NULL,
        host_version TEXT NOT NULL,
        capabilities TEXT NOT NULL DEFAULT '[]',
        default_hat_id TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked_at INTEGER,
        UNIQUE (id, owner_id),
        FOREIGN KEY (default_hat_id, owner_id) REFERENCES hats(id, owner_id));
    INSERT INTO hosts_hatted(id, owner_id, name, public_key, platform, host_version, capabilities,
                             default_hat_id, created_at, last_seen_at, revoked_at)
        SELECT h.id, h.owner_id, h.name, h.public_key, h.platform, h.host_version, h.capabilities,
               (SELECT s.value FROM settings s WHERE s.owner_id = h.owner_id AND s.key = 'default_hat_id'),
               h.created_at, h.last_seen_at, h.revoked_at
        FROM hosts h;
    DROP TABLE hosts;
    ALTER TABLE hosts_hatted RENAME TO hosts;
    CREATE TABLE hat_path_rules (
        id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        host_id TEXT NOT NULL,
        prefix TEXT NOT NULL,
        hat_id TEXT NOT NULL,
        verified INTEGER NOT NULL CHECK (verified IN (0, 1)),
        UNIQUE (host_id, prefix),
        FOREIGN KEY (host_id, owner_id) REFERENCES hosts(id, owner_id),
        FOREIGN KEY (hat_id, owner_id) REFERENCES hats(id, owner_id));
    ",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::Hosts;
    use crate::operator::Operator;
    use crate::secret::sha256_hex;
    use rusqlite::{Connection, OptionalExtension};

    const NOW: i64 = 1_800_000_000;
    const PASSWORD: &str = "correct horse battery";
    /// RFC 8032's first test key: a valid Ed25519 public key.
    const HOST_KEY: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    fn kernel_version(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT version FROM schema_versions WHERE component = ?1",
            [COMPONENT],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn has_table(conn: &Connection, table: &str) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .unwrap()
        .is_some()
    }

    /// Plan 3c's deferred minor: a database at kernel version 4 (before
    /// passkeys), set up, signed in, with a host and a pairing code, keeps
    /// every row through migration 5, and its owner can keep passkeys.
    #[test]
    fn a_v4_database_upgrades_to_passkeys_keeping_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let token = "a".repeat(64);
        let owner: String = {
            let mut conn = Connection::open(&db).unwrap();
            crate::db::migrate_component(&mut conn, COMPONENT, &MIGRATIONS[..4]).unwrap();
            assert_eq!(kernel_version(&conn), 4);
            assert!(!has_table(&conn, "passkeys"));
            let owner: String = conn.query_row("SELECT id FROM owners", [], |r| r.get(0)).unwrap();
            let phc = password_auth::generate_hash(PASSWORD);
            let code = sha256_hex(b"AAAAAAAA");
            conn.execute_batch(&format!(
                "
                UPDATE owners SET set_up_at = {NOW};
                INSERT INTO password_credentials VALUES ('{owner}', '{phc}', {NOW});
                INSERT INTO settings VALUES ('{owner}', 'public_url', 'https://hennery.example');
                INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, expires_at)
                    VALUES ('{}', '{owner}', 'test', {NOW}, {NOW}, {});
                INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, created_at)
                    VALUES ('host-old', '{owner}', 'laptop', '{HOST_KEY}', 'macos-aarch64', '0.0.0', {NOW});
                INSERT INTO pairing_codes VALUES ('{code}', '{owner}', {NOW}, {}, NULL);
                ",
                sha256_hex(token.as_bytes()),
                NOW + 3600,
                NOW + 3600,
            ))
            .unwrap();
            owner
        };

        let operator = Operator::open(&db).unwrap();
        assert_eq!(operator.owner_id(), owner);
        assert!(operator.verify_password(PASSWORD).unwrap().is_some());
        assert!(operator.authenticate(&token, NOW).unwrap().is_some());
        let hosts = Hosts::open(&db).unwrap();
        let listed: Vec<String> = hosts.list().unwrap().into_iter().map(|h| h.id).collect();
        assert_eq!(listed, ["host-old"]);

        let conn = Connection::open(&db).unwrap();
        assert_eq!(kernel_version(&conn), MIGRATIONS.len() as i64);
        let codes: Vec<String> = conn
            .prepare("SELECT owner_id FROM pairing_codes")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(codes, [owner.as_str()]);
        let public_url: String = conn
            .query_row(
                "SELECT value FROM settings WHERE owner_id = ?1 AND key = 'public_url'",
                [&owner],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(public_url, "https://hennery.example");

        conn.execute(
            "INSERT INTO passkeys(id, owner_id, credential_id, credential, sign_count, label, created_at)
             VALUES ('passkey-0000000000000001', ?1, 'c0ffee', '{}', 0, 'laptop', ?2)",
            rusqlite::params![owner, NOW],
        )
        .unwrap();
        let passkeys: Vec<(String, String)> = operator
            .passkeys()
            .unwrap()
            .into_iter()
            .map(|p| (p.id, p.label))
            .collect();
        assert_eq!(
            passkeys,
            [("passkey-0000000000000001".to_string(), "laptop".to_string())]
        );
    }
}
