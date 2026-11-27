-- The legacy operator queue class also schedules independent peer messages.
-- Typed input origin, never the queue class, identifies operator authority.
ALTER TABLE session_dispatch_queue ADD COLUMN input_origin TEXT NOT NULL DEFAULT '{"kind":"operator"}';
CREATE TABLE peer_messages (
  owner_id TEXT NOT NULL, source_id TEXT NOT NULL, request_key TEXT NOT NULL,
  target_id TEXT NOT NULL, hat_id TEXT NOT NULL, revision INTEGER NOT NULL,
  epoch INTEGER NOT NULL, digest TEXT NOT NULL, operation_id TEXT NOT NULL,
  PRIMARY KEY(owner_id,source_id,request_key), UNIQUE(owner_id,operation_id),
  FOREIGN KEY(source_id,owner_id) REFERENCES sessions(id,owner_id) ON DELETE CASCADE,
  FOREIGN KEY(target_id,owner_id) REFERENCES sessions(id,owner_id) ON DELETE CASCADE
);
