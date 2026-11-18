CREATE TABLE session_artifacts (
    owner_id TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    id TEXT NOT NULL,
    PRIMARY KEY(owner_id, id)
);
CREATE INDEX artifacts_by_session ON session_artifacts(owner_id, session_id, id);
CREATE TABLE artifact_revisions (
    owner_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    title TEXT NOT NULL,
    name TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    request_key TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    PRIMARY KEY(owner_id, artifact_id, revision),
    UNIQUE(owner_id, session_id, request_key),
    FOREIGN KEY(owner_id, artifact_id) REFERENCES session_artifacts(owner_id, id) ON DELETE CASCADE,
    FOREIGN KEY(owner_id, sha256) REFERENCES attachments(owner_id, sha256)
);
CREATE INDEX artifact_blob_references ON artifact_revisions(owner_id, sha256);
CREATE TABLE file_cache_cleanup (
    owner_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    PRIMARY KEY(owner_id,session_id)
);
CREATE TRIGGER artifact_session_deleted AFTER UPDATE OF lifecycle ON sessions
WHEN NEW.lifecycle = 'deleted' BEGIN
    DELETE FROM session_artifacts WHERE owner_id = NEW.owner_id AND session_id = NEW.id;
END;
