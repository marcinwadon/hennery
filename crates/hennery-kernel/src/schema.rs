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
    // the start; the older ones get it with the backfill (plan 3b-ii).
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
];
