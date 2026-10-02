//! The gateway's tables in `hennery.db` (gateway spec §2), migrated as a
//! component of their own (`db::migrate_component`), beside the kernel's
//! and the sessions store's. Plan 8a makes the first three, plan 8d the
//! session tokens; later plans add standalone clients, OAuth clients and
//! stdio servers.
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

pub(crate) const MIGRATIONS: &[&str] = &[
    "
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
    ",
    // Plan 8d: one token per session (gateway spec §3.1), minted and
    // revoked inside the sessions store's own transactions (lane L1). The
    // session id is an opaque value with no foreign key: the gateway never
    // reads session tables (lane L6). The host and the hat are the
    // owner's, as everywhere else; a hat's tokens go with its purge
    // (`tokens::purge_hat_in`) before the hat row can be deleted. Only the
    // token's SHA-256 is stored.
    "
    CREATE TABLE gw_session_tokens (
        session_id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        host_id TEXT NOT NULL,
        hat_id TEXT NOT NULL,
        token_hash TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        last_used_at INTEGER,
        revoked_at INTEGER,
        FOREIGN KEY (host_id, owner_id) REFERENCES hosts(id, owner_id),
        FOREIGN KEY (hat_id, owner_id) REFERENCES hats(id, owner_id));
    CREATE INDEX gw_session_tokens_by_host ON gw_session_tokens(owner_id, host_id);
    CREATE INDEX gw_session_tokens_by_hat ON gw_session_tokens(owner_id, hat_id);
    ",
];
