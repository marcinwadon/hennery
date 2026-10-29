CREATE TABLE external_oauth_clients (
 owner_id TEXT NOT NULL, id TEXT NOT NULL, metadata TEXT NOT NULL, expires_at INTEGER NOT NULL,
 PRIMARY KEY(owner_id,id)
);
CREATE TABLE external_oauth_requests (
 owner_id TEXT NOT NULL, id TEXT NOT NULL, client_id TEXT NOT NULL, client_name TEXT NOT NULL,
 redirect_uri TEXT NOT NULL, resource TEXT NOT NULL, scope TEXT NOT NULL, state TEXT,
 challenge TEXT NOT NULL, expires_at INTEGER NOT NULL, code_hash TEXT, code_expires_at INTEGER,
 grant_id TEXT, consumed INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(owner_id,id)
);
CREATE UNIQUE INDEX external_oauth_code ON external_oauth_requests(owner_id,code_hash);
CREATE TABLE external_oauth_tokens (
 owner_id TEXT NOT NULL, grant_id TEXT NOT NULL, client_id TEXT NOT NULL, resource TEXT NOT NULL,
 scope TEXT NOT NULL, access_hash TEXT NOT NULL, access_expires_at INTEGER NOT NULL,
 refresh_hash TEXT, expires_at INTEGER NOT NULL,
 PRIMARY KEY(owner_id,grant_id),
 UNIQUE(owner_id,access_hash), UNIQUE(owner_id,refresh_hash),
 FOREIGN KEY(owner_id,grant_id) REFERENCES external_clients(owner_id,id) ON DELETE CASCADE
);
CREATE TABLE external_oauth_refresh_history (
 owner_id TEXT NOT NULL, token_hash TEXT NOT NULL, grant_id TEXT NOT NULL, expires_at INTEGER NOT NULL,
 PRIMARY KEY(owner_id,token_hash),
 FOREIGN KEY(owner_id,grant_id) REFERENCES external_clients(owner_id,id) ON DELETE CASCADE
);
