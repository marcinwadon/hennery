-- rebuild-with-foreign-keys-off
-- Preserve all dependent OAuth credentials, receipts, sessions and uploads.
-- A grant no longer belongs to one anchor hat: purging one selected hat must
-- not erase unrelated authority, nor expand the remaining selection.
CREATE TABLE external_clients_new (
 owner_id TEXT NOT NULL, id TEXT NOT NULL, label TEXT NOT NULL, hat_id TEXT,
 token_hash TEXT NOT NULL UNIQUE, read_metadata INTEGER NOT NULL CHECK(read_metadata IN (0,1)),
 existing_sessions INTEGER NOT NULL CHECK(existing_sessions IN (0,1)),
 revision INTEGER NOT NULL CHECK(revision>0), expires_at INTEGER NOT NULL, revoked_at INTEGER,
 read_transcript INTEGER NOT NULL DEFAULT 0 CHECK(read_transcript IN (0,1)),
 control_sessions INTEGER NOT NULL DEFAULT 0 CHECK(control_sessions IN (0,1)),
 all_hats INTEGER NOT NULL DEFAULT 0 CHECK(all_hats IN (0,1)),
 all_hosts INTEGER NOT NULL DEFAULT 0 CHECK(all_hosts IN (0,1)),
 PRIMARY KEY(owner_id,id)
);
INSERT INTO external_clients_new
 SELECT owner_id,id,label,hat_id,token_hash,read_metadata,existing_sessions,
 revision,expires_at,revoked_at,read_transcript,control_sessions,0,0 FROM external_clients;
DROP TABLE external_clients;
ALTER TABLE external_clients_new RENAME TO external_clients;
CREATE TABLE external_client_hats (
 owner_id TEXT NOT NULL, client_id TEXT NOT NULL, hat_id TEXT NOT NULL,
 PRIMARY KEY(owner_id,client_id,hat_id),
 FOREIGN KEY(owner_id,client_id) REFERENCES external_clients(owner_id,id) ON DELETE CASCADE,
 FOREIGN KEY(hat_id,owner_id) REFERENCES hats(id,owner_id) ON DELETE CASCADE
);
INSERT INTO external_client_hats SELECT owner_id,id,hat_id FROM external_clients WHERE hat_id IS NOT NULL;
ALTER TABLE external_operations ADD COLUMN target_hat_id TEXT;
UPDATE external_operations SET target_hat_id=(SELECT c.hat_id FROM external_clients c
 WHERE c.owner_id=external_operations.owner_id AND c.id=external_operations.client_id)
 WHERE tool IN ('session_create','session_clone') AND session_id<>'' AND code IS NOT 'gone';
-- A queued create has no session for the ordinary purge hooks to enumerate.
-- Clean it at the freeze gate; existing sessions use their normal deletion
-- hooks, including sessions already reassigned to a different live hat.
UPDATE external_operations SET state='failed',code='gone',args='{}',result=NULL,
 session_id='',turn_id=NULL,request_id=NULL,source_session_id=NULL,target_hat_id=NULL
 WHERE EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id=external_operations.owner_id AND p.hat_id=external_operations.target_hat_id)
 AND NOT EXISTS(SELECT 1 FROM sessions s WHERE s.owner_id=external_operations.owner_id AND s.id=external_operations.session_id);
CREATE TRIGGER external_scope_purge AFTER INSERT ON purged_hats
BEGIN
 UPDATE external_operations SET state='failed',code='gone',args='{}',result=NULL,
  session_id='',turn_id=NULL,request_id=NULL,source_session_id=NULL,target_hat_id=NULL
  WHERE owner_id=NEW.owner_id AND target_hat_id=NEW.hat_id
  AND NOT EXISTS(SELECT 1 FROM sessions s WHERE s.owner_id=NEW.owner_id AND s.id=external_operations.session_id);
END;
CREATE TRIGGER external_control_revoke AFTER UPDATE OF revision,revoked_at ON external_clients
BEGIN
 UPDATE external_operations SET state='cancelled',code='grant_changed',args='{}'
 WHERE owner_id=NEW.owner_id AND client_id=NEW.id AND state='queued';
END;
CREATE TRIGGER external_tools_revoke AFTER UPDATE OF revision,revoked_at ON external_clients
BEGIN
 DELETE FROM external_uploads WHERE owner_id=NEW.owner_id AND client_id=NEW.id;
END;
