-- Test-only rollback of sessions migration 33: navigation shortcuts.
DROP INDEX sessions_sidebar_pinned;
ALTER TABLE sessions DROP COLUMN sidebar_pinned;
-- Test-only rollback of sessions migration 32 before historical fixtures.
ALTER TABLE sessions DROP COLUMN unseen_event_id;
ALTER TABLE sessions DROP COLUMN seen_event_id;
-- Test-only rollback of sessions migration 31 before historical fixtures.
DROP TRIGGER review_delete;
DROP TABLE session_reviews;
-- Test-only rollback of sessions migration 30 before historical fixtures.
ALTER TABLE sessions DROP COLUMN project_root;
-- Test-only rollback of sessions migration 29 before historical fixtures.
ALTER TABLE turns DROP COLUMN operator_queued;
ALTER TABLE turns DROP COLUMN queue_retry_released;
DROP TRIGGER prompt_queue_failed;
DROP TRIGGER prompt_queue_insert;
DROP TRIGGER prompt_queue_update;
DROP TRIGGER prompt_queue_delete;
DROP TRIGGER delegation_endpoint_end;
DROP TRIGGER delegation_grant_end;
DROP TABLE prompt_queue_attachments;
DROP TABLE prompt_receipts;
ALTER TABLE session_dispatch_queue DROP COLUMN origin;
ALTER TABLE session_dispatch_queue DROP COLUMN image_bytes;
ALTER TABLE sessions DROP COLUMN prompt_queue_dirty;
ALTER TABLE sessions DROP COLUMN prompt_queue_held;
ALTER TABLE session_dispatch_queue DROP COLUMN failed_at;
ALTER TABLE session_dispatch_queue DROP COLUMN busy_refusals;
ALTER TABLE session_dispatch_queue DROP COLUMN retry_after;
CREATE TRIGGER delegation_endpoint_end AFTER UPDATE OF close_requested,lifecycle,hat_id ON sessions
WHEN NEW.close_requested = 1 OR NEW.lifecycle IN ('closed','deleted','failed') OR NEW.hat_id <> OLD.hat_id
BEGIN
    UPDATE session_operations SET state = CASE WHEN NEW.lifecycle = 'failed' AND child_id = NEW.id THEN 'failed' ELSE 'cancelled' END,
        code = CASE WHEN NEW.lifecycle = 'failed' AND child_id = NEW.id THEN COALESCE(NEW.failure_reason,'start_failed') ELSE 'endpoint_unavailable' END
    WHERE owner_id = NEW.owner_id AND (state IN ('accepted','queued','waiting_config') OR (state = 'awaiting_reconciliation' AND turn_id IS NULL AND tool IN ('session_spawn','session_send')))
      AND (caller_id = NEW.id OR child_id = NEW.id);
    UPDATE session_dispatch_queue SET state = 'cancelled', payload = ''
    WHERE owner_id = NEW.owner_id AND (source_id = NEW.id OR target_id = NEW.id)
      AND state IN ('queued','waiting_config');
    UPDATE session_attach_evidence SET evidence = NULL
    WHERE owner_id = NEW.owner_id AND session_id = NEW.id;
    DELETE FROM session_dispatch_queue
    WHERE owner_id = NEW.owner_id AND (source_id = NEW.id OR target_id = NEW.id) AND NEW.lifecycle = 'deleted';
    DELETE FROM session_operations
    WHERE owner_id = NEW.owner_id AND caller_id = NEW.id AND NEW.lifecycle = 'deleted';
    UPDATE session_operations SET child_id = NULL, tool = '', state = 'failed', code = 'gone',
        turn_id = NULL, pending_id = NULL, request_id = NULL, accepted_revision = 0, epoch = 0,
        validated_attach = NULL, validated_config = NULL, validated_policy = NULL
    WHERE owner_id = NEW.owner_id AND child_id = NEW.id AND NEW.lifecycle = 'deleted';
    DELETE FROM delegation_start_budgets
    WHERE owner_id = NEW.owner_id AND caller_id = NEW.id AND NEW.lifecycle = 'deleted';
    DELETE FROM session_attach_evidence WHERE owner_id = NEW.owner_id AND session_id = NEW.id AND NEW.lifecycle = 'deleted';
END;
CREATE TRIGGER delegation_grant_end AFTER UPDATE OF revision ON hat_session_tools
BEGIN
    UPDATE session_operations SET state = 'cancelled', code = 'grant_changed'
    WHERE owner_id = NEW.owner_id AND (state IN ('accepted','queued','waiting_config') OR (state = 'awaiting_reconciliation' AND turn_id IS NULL AND tool IN ('session_spawn','session_send')))
      AND caller_id IN (SELECT id FROM sessions WHERE owner_id = NEW.owner_id AND hat_id = NEW.hat_id);
    UPDATE session_dispatch_queue SET state = 'cancelled', payload = ''
    WHERE owner_id = NEW.owner_id AND accepted_revision <> NEW.revision
      AND source_id IN (SELECT id FROM sessions WHERE owner_id = NEW.owner_id AND hat_id = NEW.hat_id)
      AND state IN ('queued','waiting_config');
END;
-- Test-only rollback of sessions migrations 28/27: restore the v26 budget CHECK.
DROP TABLE runtime_catalog;
DROP TABLE delegation_limits;
DROP TRIGGER notice_endpoint_end;
ALTER TABLE delegation_budgets RENAME TO delegation_budgets_old;
CREATE TABLE delegation_budgets (
    owner_id TEXT NOT NULL, root_id TEXT NOT NULL, epoch INTEGER NOT NULL,
    wakes INTEGER NOT NULL CHECK(wakes >= 0 AND wakes <= 20),
    PRIMARY KEY(owner_id,root_id,epoch),
    FOREIGN KEY(root_id,owner_id) REFERENCES sessions(id,owner_id));
INSERT INTO delegation_budgets(owner_id,root_id,epoch,wakes)
    SELECT owner_id,root_id,epoch,wakes FROM delegation_budgets_old;
DROP TABLE delegation_budgets_old;
CREATE TRIGGER notice_endpoint_end AFTER UPDATE OF close_requested,lifecycle,hat_id ON sessions
WHEN NEW.close_requested = 1 OR NEW.lifecycle IN ('closed','deleted','failed') OR NEW.hat_id <> OLD.hat_id
BEGIN
    UPDATE delegation_notices SET state = 'operator_required',reason = 'endpoint_unavailable'
    WHERE owner_id = NEW.owner_id AND (parent_id = NEW.id OR (child_id = NEW.id AND NEW.lifecycle <> 'failed'))
      AND state IN ('parent_queued','waiting_config');
    DELETE FROM delegation_notices WHERE owner_id = NEW.owner_id
      AND (child_id = NEW.id OR parent_id = NEW.id) AND NEW.lifecycle = 'deleted';
    DELETE FROM delegation_budgets WHERE owner_id = NEW.owner_id AND root_id = NEW.id AND NEW.lifecycle = 'deleted';
END;
-- Test-only rollback of sessions migration 34: source-policy attach snapshots.
ALTER TABLE sessions DROP COLUMN mcp_source_requested;
ALTER TABLE sessions DROP COLUMN mcp_source_applied;
