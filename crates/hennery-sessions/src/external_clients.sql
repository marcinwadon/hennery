CREATE TABLE external_clients (
 owner_id TEXT NOT NULL, id TEXT NOT NULL, label TEXT NOT NULL, hat_id TEXT NOT NULL,
 token_hash TEXT NOT NULL UNIQUE, read_metadata INTEGER NOT NULL CHECK(read_metadata IN (0,1)),
 existing_sessions INTEGER NOT NULL CHECK(existing_sessions IN (0,1)),
 revision INTEGER NOT NULL CHECK(revision>0), expires_at INTEGER NOT NULL, revoked_at INTEGER,
 PRIMARY KEY(owner_id,id),
 FOREIGN KEY(hat_id,owner_id) REFERENCES hats(id,owner_id) ON DELETE CASCADE
);
CREATE TABLE external_client_roots (
 owner_id TEXT NOT NULL, client_id TEXT NOT NULL, host_id TEXT NOT NULL, root TEXT NOT NULL,
 PRIMARY KEY(owner_id,client_id,host_id,root),
 FOREIGN KEY(owner_id,client_id) REFERENCES external_clients(owner_id,id) ON DELETE CASCADE
);
-- Reserved for atomic creation/provenance in the next slice. No API can adopt
-- a session merely by inserting an ID from an external tool argument.
CREATE TABLE external_client_sessions (
 owner_id TEXT NOT NULL, client_id TEXT NOT NULL, session_id TEXT NOT NULL,
 PRIMARY KEY(owner_id,client_id,session_id),
 FOREIGN KEY(owner_id,client_id) REFERENCES external_clients(owner_id,id) ON DELETE CASCADE
);
