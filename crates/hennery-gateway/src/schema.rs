//! The gateway's tables in `hennery.db` (gateway spec §2), migrated as a
//! component of their own (`db::migrate_component`), beside the kernel's
//! and the sessions store's. Plan 8a makes the first three, plan 8d the
//! session tokens, plan 8f the OAuth clients; later plans add standalone
//! clients and stdio servers.
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
    // Plan 8f: OAuth (gateway spec §4, §7). A connection learns when it was
    // last checked, why its latest Connect failed, and the protected-resource
    // document's `resource` when it is not the URL (found, or accepted). Its
    // OAuth client is one row: the client its grant was made with (or, with
    // no grant yet, the one the next Connect uses), pinned to the
    // authorization server it was first used with, and a pre-registered
    // client saved while a grant is live, which replaces it only when a
    // Connect with it completes (G-7). Secrets are sealed (`crypto`), each
    // under a field of its own; whether one is stored is a column apart, so
    // a list never reads a ciphertext (plan 8a decision 3).
    "
    ALTER TABLE gw_connections ADD COLUMN checked_at INTEGER;
    ALTER TABLE gw_connections ADD COLUMN oauth_error_code TEXT;
    ALTER TABLE gw_connections ADD COLUMN oauth_error_message TEXT;
    ALTER TABLE gw_connections ADD COLUMN oauth_error_at INTEGER;
    ALTER TABLE gw_connections ADD COLUMN resource_mismatch TEXT;
    ALTER TABLE gw_connections ADD COLUMN accepted_resource TEXT;
    CREATE TABLE gw_oauth_clients (
        connection_id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL,
        client_id TEXT NOT NULL,
        has_client_secret INTEGER NOT NULL CHECK (has_client_secret IN (0, 1)),
        client_secret_ciphertext BLOB,
        key_version INTEGER,
        token_endpoint_auth_method TEXT
            CHECK (token_endpoint_auth_method IN ('none', 'client_secret_basic', 'client_secret_post')),
        issuer TEXT,
        authorization_endpoint TEXT,
        token_endpoint TEXT,
        redirect_uri TEXT,
        scopes TEXT,
        resource TEXT,
        resource_param_accepted INTEGER NOT NULL DEFAULT 1 CHECK (resource_param_accepted IN (0, 1)),
        registered_at INTEGER NOT NULL,
        pending_client_id TEXT,
        pending_has_secret INTEGER NOT NULL DEFAULT 0 CHECK (pending_has_secret IN (0, 1)),
        pending_secret_ciphertext BLOB,
        pending_issuer TEXT,
        pending_token_endpoint TEXT,
        FOREIGN KEY (connection_id, owner_id) REFERENCES gw_connections(id, owner_id));
    ",
];
