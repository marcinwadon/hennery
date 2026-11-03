-- Existing metadata grants never acquire transcript or execution authority.
ALTER TABLE external_clients ADD COLUMN read_transcript INTEGER NOT NULL DEFAULT 0 CHECK(read_transcript IN (0,1));
ALTER TABLE external_clients ADD COLUMN control_sessions INTEGER NOT NULL DEFAULT 0 CHECK(control_sessions IN (0,1));
ALTER TABLE sessions ADD COLUMN external_attach_operation TEXT;
CREATE TRIGGER external_attach_reset AFTER UPDATE OF lifecycle ON sessions WHEN NEW.lifecycle='starting'
BEGIN
 UPDATE sessions SET external_attach_operation=NULL WHERE owner_id=NEW.owner_id AND id=NEW.id;
END;
CREATE TABLE external_operations (
 position INTEGER PRIMARY KEY AUTOINCREMENT,
 owner_id TEXT NOT NULL, id TEXT NOT NULL, client_id TEXT NOT NULL, revision INTEGER NOT NULL,
 request_key_hash TEXT NOT NULL, digest TEXT NOT NULL, tool TEXT NOT NULL,
 session_id TEXT NOT NULL, turn_id TEXT, request_id TEXT, intent_event INTEGER,
 state TEXT NOT NULL CHECK(state IN ('queued','running','uncertain','completed','failed','cancelled')),
 args TEXT NOT NULL, result TEXT, code TEXT, created_at INTEGER NOT NULL,
 UNIQUE(owner_id,id), UNIQUE(owner_id,client_id,request_key_hash),
 FOREIGN KEY(owner_id,client_id) REFERENCES external_clients(owner_id,id) ON DELETE CASCADE
);
CREATE INDEX external_operations_session ON external_operations(owner_id,session_id,position);
CREATE INDEX external_operations_state ON external_operations(owner_id,state,position);
CREATE TRIGGER external_control_revoke AFTER UPDATE OF revision,revoked_at ON external_clients
BEGIN
 UPDATE external_operations SET state='cancelled',code='grant_changed',args='{}'
 WHERE owner_id=NEW.owner_id AND client_id=NEW.id AND state='queued';
END;
CREATE TRIGGER external_control_deleted AFTER UPDATE OF lifecycle ON sessions WHEN NEW.lifecycle='deleted'
BEGIN
 UPDATE external_operations SET state='failed',code='gone',args='{}',result=NULL,session_id='',turn_id=NULL,request_id=NULL
 WHERE owner_id=NEW.owner_id AND session_id=NEW.id;
END;
