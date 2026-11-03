-- Only external principals own these bounded staging objects. Images enter
-- canonical attachment storage through ordinary prompt admission.
ALTER TABLE external_operations ADD COLUMN source_session_id TEXT;
CREATE TABLE external_uploads (
 owner_id TEXT NOT NULL, id TEXT NOT NULL, client_id TEXT NOT NULL,
 session_id TEXT NOT NULL, mime TEXT NOT NULL, sha256 TEXT NOT NULL,
 size INTEGER NOT NULL CHECK(size>0 AND size<=5242880),
 received INTEGER NOT NULL DEFAULT 0, sealed INTEGER NOT NULL DEFAULT 0,
 expires_at INTEGER NOT NULL,
 PRIMARY KEY(owner_id,id),
 FOREIGN KEY(owner_id,client_id) REFERENCES external_clients(owner_id,id) ON DELETE CASCADE
);
CREATE TABLE external_upload_chunks (
 owner_id TEXT NOT NULL, upload_id TEXT NOT NULL, offset INTEGER NOT NULL,
 digest TEXT NOT NULL, data BLOB NOT NULL,
 PRIMARY KEY(owner_id,upload_id,offset),
 FOREIGN KEY(owner_id,upload_id) REFERENCES external_uploads(owner_id,id) ON DELETE CASCADE
);
CREATE TABLE external_operation_uploads (
 owner_id TEXT NOT NULL, operation_id TEXT NOT NULL, upload_id TEXT NOT NULL,
 PRIMARY KEY(owner_id,operation_id,upload_id),
 FOREIGN KEY(owner_id,operation_id) REFERENCES external_operations(owner_id,id) ON DELETE CASCADE,
 FOREIGN KEY(owner_id,upload_id) REFERENCES external_uploads(owner_id,id) ON DELETE CASCADE
);
CREATE INDEX external_uploads_expiry ON external_uploads(owner_id,expires_at);
CREATE TRIGGER external_tools_revoke AFTER UPDATE OF revision,revoked_at ON external_clients
BEGIN
 DELETE FROM external_uploads WHERE owner_id=NEW.owner_id AND client_id=NEW.id;
END;
CREATE TRIGGER external_tools_deleted AFTER UPDATE OF lifecycle ON sessions WHEN NEW.lifecycle='deleted'
BEGIN
 DELETE FROM external_uploads WHERE owner_id=NEW.owner_id AND session_id=NEW.id;
 UPDATE external_operations SET state='failed',code='gone',args='{}',result=NULL,source_session_id=NULL
 WHERE owner_id=NEW.owner_id AND source_session_id=NEW.id AND request_id IS NULL AND state IN ('queued','running','uncertain');
 UPDATE external_operations SET source_session_id=NULL WHERE owner_id=NEW.owner_id AND (session_id=NEW.id OR source_session_id=NEW.id);
END;
