ALTER TABLE sessions ADD COLUMN managed_workspace_id TEXT;
-- Workspace ownership survives session transcript deletion. A workspace is a
-- host-local resource, not a directory owned exclusively by its first session.
CREATE TABLE session_workspaces (
    owner_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    id TEXT NOT NULL,
    root TEXT NOT NULL,
    repository TEXT NOT NULL,
    profile_source TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL,
    base_commit TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_attempt INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL DEFAULT 'ready'
        CHECK(state IN ('preparing','ready','cleanup_pending','cleanup_running','blocked','failed','removed')),
    PRIMARY KEY(owner_id,id),
    UNIQUE(owner_id,host_id,root)
);
CREATE TABLE session_workspace_members (
    owner_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    PRIMARY KEY(owner_id,session_id),
    FOREIGN KEY(owner_id,workspace_id) REFERENCES session_workspaces(owner_id,id),
    FOREIGN KEY(session_id,owner_id) REFERENCES sessions(id,owner_id)
);
CREATE TRIGGER workspace_member_admission BEFORE INSERT ON session_workspace_members
WHEN NOT EXISTS (
    SELECT 1 FROM session_workspaces w JOIN sessions s
      ON s.owner_id=w.owner_id AND s.host_id=w.host_id
    WHERE w.owner_id=NEW.owner_id AND w.id=NEW.workspace_id
      AND s.id=NEW.session_id AND s.lifecycle NOT IN ('archived','deleted')
      AND w.state IN ('preparing','ready','cleanup_pending','blocked','failed')
)
BEGIN SELECT RAISE(ABORT,'workspace_unavailable'); END;
CREATE TRIGGER workspace_remember_member AFTER INSERT ON session_workspace_members
BEGIN
    UPDATE sessions SET managed_workspace_id=NEW.workspace_id WHERE owner_id=NEW.owner_id AND id=NEW.session_id;
END;
CREATE TRIGGER workspace_member_immutable BEFORE UPDATE ON session_workspace_members
BEGIN SELECT RAISE(ABORT,'workspace_membership_immutable'); END;
CREATE TRIGGER workspace_cleanup_fence BEFORE UPDATE OF state ON session_workspaces
WHEN NEW.state IN ('cleanup_running','removed') AND EXISTS (
    SELECT 1 FROM session_workspace_members m
    WHERE m.owner_id=NEW.owner_id AND m.workspace_id=NEW.id
)
BEGIN SELECT RAISE(ABORT,'workspace_in_use'); END;
CREATE TRIGGER workspace_member_release AFTER UPDATE OF lifecycle ON sessions
WHEN NEW.lifecycle IN ('archived','deleted')
BEGIN
    DELETE FROM session_workspace_members
    WHERE owner_id=NEW.owner_id AND session_id=NEW.id;
END;
CREATE TRIGGER workspace_existing_users AFTER INSERT ON session_workspaces
BEGIN
    INSERT INTO session_workspace_members(owner_id,workspace_id,session_id)
    SELECT NEW.owner_id,NEW.id,s.id FROM sessions s
    WHERE s.owner_id=NEW.owner_id AND s.host_id=NEW.host_id
      AND s.lifecycle NOT IN ('archived','deleted')
      AND (s.cwd=NEW.root OR substr(s.cwd,1,length(NEW.root)+1)=NEW.root || '/');
END;
-- All canonical cwd admissions, including clones and explicit subdirectories,
-- acquire membership in the same transaction that creates the session.
CREATE TRIGGER workspace_session_admission AFTER INSERT ON sessions
WHEN NEW.lifecycle NOT IN ('archived','deleted')
BEGIN
    INSERT INTO session_workspace_members(owner_id,workspace_id,session_id)
    SELECT NEW.owner_id,w.id,NEW.id FROM session_workspaces w
    WHERE w.owner_id=NEW.owner_id AND w.host_id=NEW.host_id
      AND w.state <> 'removed'
      AND (NEW.cwd=w.root OR substr(NEW.cwd,1,length(w.root)+1)=w.root || '/')
    ORDER BY length(w.root) DESC LIMIT 1;
END;
CREATE TRIGGER workspace_archive_terminal BEFORE UPDATE OF lifecycle ON sessions
WHEN OLD.lifecycle='archived' AND NEW.lifecycle NOT IN ('archived','deleted')
BEGIN SELECT RAISE(ABORT,'session_archived'); END;
CREATE TRIGGER workspace_archive_cleanup BEFORE UPDATE OF lifecycle ON sessions
WHEN NEW.lifecycle IN ('archived','deleted')
BEGIN
    UPDATE session_workspaces SET state='cleanup_pending'
    WHERE owner_id=NEW.owner_id AND id IN (
        SELECT workspace_id FROM session_workspace_members WHERE owner_id=NEW.owner_id AND session_id=NEW.id
    ) AND state NOT IN ('cleanup_running','removed');
END;
CREATE TABLE workspace_provisioning (
    owner_id TEXT NOT NULL, id TEXT NOT NULL, host_id TEXT NOT NULL, source TEXT NOT NULL,
    last_attempt INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY(owner_id,id)
);
CREATE TRIGGER workspace_archive_notices AFTER UPDATE OF lifecycle ON sessions
WHEN NEW.lifecycle='archived'
BEGIN
    UPDATE delegation_notices SET state='operator_required',reason='endpoint_unavailable'
    WHERE owner_id=NEW.owner_id AND (parent_id=NEW.id OR child_id=NEW.id)
      AND state IN ('parent_queued','waiting_config');
END;
CREATE TABLE workspace_profiles (
    owner_id TEXT NOT NULL,host_id TEXT NOT NULL,source TEXT NOT NULL,
    files TEXT NOT NULL DEFAULT '[]',command TEXT,
    PRIMARY KEY(owner_id,host_id,source)
);
