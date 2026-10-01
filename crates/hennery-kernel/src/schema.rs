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
];
