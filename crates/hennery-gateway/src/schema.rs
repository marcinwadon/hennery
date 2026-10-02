//! The gateway's tables in `hennery.db` (gateway spec §2), migrated as a
//! component of their own (`db::migrate_component`), beside the kernel's
//! and the sessions store's. Plan 8a makes the first three; later plans add
//! the session tokens, standalone clients, OAuth clients and stdio servers.
//!
//! Every table carries `owner_id` (lane L6). A connection's hat and a
//! mount's host are the owner's by composite foreign keys (plan 5a decision
//! 3), and a credential's or mount's connection likewise. None cascades:
//! the store deletes a connection's credential and mounts itself, and a hat
//! with connections cannot be deleted until the gateway's purge has run
//! (kernel spec §5.5). The `CHECK`s name every value the spec has, OAuth
//! kinds included: these tables will have children, and a table with
//! children cannot be rebuilt by this runner to widen one later.

/// The gateway's component name in `schema_versions`.
pub(crate) const COMPONENT: &str = "gateway";

pub(crate) const MIGRATIONS: &[&str] = &["
    CREATE TABLE gw_connections (
        id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        slug TEXT NOT NULL,
        label TEXT NOT NULL,
        url TEXT NOT NULL,
        hat_id TEXT NOT NULL,
        cred_kind TEXT NOT NULL CHECK (cred_kind IN ('none', 'static', 'oauth_dcr', 'oauth_client')),
        static_header TEXT NOT NULL DEFAULT 'Authorization',
        static_prefix TEXT NOT NULL DEFAULT 'Bearer ',
        tool_allowlist TEXT,
        internal_network INTEGER NOT NULL CHECK (internal_network IN (0, 1)),
        status TEXT NOT NULL DEFAULT 'not_connected'
            CHECK (status IN ('not_connected', 'ok', 'needs_auth', 'error')),
        status_note TEXT,
        account_label TEXT,
        status_at INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        UNIQUE (owner_id, slug),
        UNIQUE (id, owner_id),
        FOREIGN KEY (hat_id, owner_id) REFERENCES hats(id, owner_id));
    CREATE INDEX gw_connections_by_hat ON gw_connections(owner_id, hat_id);
    CREATE TABLE gw_credentials (
        connection_id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL,
        key_version INTEGER NOT NULL,
        ciphertext BLOB NOT NULL,
        expires_at INTEGER,
        updated_at INTEGER NOT NULL,
        FOREIGN KEY (connection_id, owner_id) REFERENCES gw_connections(id, owner_id));
    CREATE TABLE gw_mounts (
        connection_id TEXT NOT NULL,
        host_id TEXT NOT NULL,
        owner_id TEXT NOT NULL,
        PRIMARY KEY (connection_id, host_id),
        FOREIGN KEY (connection_id, owner_id) REFERENCES gw_connections(id, owner_id),
        FOREIGN KEY (host_id, owner_id) REFERENCES hosts(id, owner_id));
    CREATE INDEX gw_mounts_by_host ON gw_mounts(owner_id, host_id);
    "];
