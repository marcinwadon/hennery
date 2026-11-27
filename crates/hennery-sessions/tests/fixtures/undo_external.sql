-- Rewind independent peer messages (v50) before historical fixtures.
DROP TABLE peer_messages;
ALTER TABLE session_dispatch_queue DROP COLUMN input_origin;
-- Rewind managed workspaces (v49) before rebuilding a historical fixture.
DROP TRIGGER workspace_member_admission;
DROP TRIGGER workspace_remember_member;
DROP TRIGGER workspace_member_immutable;
DROP TRIGGER workspace_cleanup_fence;
DROP TRIGGER workspace_member_release;
DROP TRIGGER workspace_existing_users;
DROP TRIGGER workspace_session_admission;
DROP TRIGGER workspace_archive_terminal;
DROP TRIGGER workspace_archive_cleanup;
DROP TRIGGER workspace_archive_notices;
DROP TABLE session_workspace_members;
DROP TABLE session_workspaces;
DROP TABLE workspace_provisioning;
DROP TABLE workspace_profiles;
ALTER TABLE sessions DROP COLUMN managed_workspace_id;
DROP VIEW external_steering_receipts;
DROP TRIGGER external_steering_deleted;
DROP VIEW external_prompt_heads;
DROP TRIGGER external_steering_detached;
DROP TRIGGER external_steering_endpoint_end;
-- Rewind hook set acknowledgements (v47) before rebuilding a historical fixture.
ALTER TABLE sessions DROP COLUMN hook_sets_applied;
-- Rewind session files (v46) before rebuilding a historical fixture.
DROP TRIGGER artifact_session_deleted;
DROP TABLE artifact_revisions;
DROP TABLE session_artifacts;
DROP TABLE file_cache_cleanup;
-- Rewind steering (v45) before rebuilding a historical fixture.
DROP TRIGGER steering_detached;
DROP TRIGGER steering_endpoint_end;
DROP INDEX steering_deadlines;
ALTER TABLE session_dispatch_queue DROP COLUMN steering_failure;
ALTER TABLE session_dispatch_queue DROP COLUMN steering_deadline;
DROP INDEX steering_admission_request;
DROP INDEX steering_attempt_request;
ALTER TABLE sessions DROP COLUMN steering_supported;
DROP INDEX IF EXISTS events_work_last_message;
DROP INDEX IF EXISTS events_work_last_activity;
DROP TRIGGER external_scope_purge;
DROP TRIGGER external_tools_revoke;
DROP TRIGGER external_tools_deleted;
DROP TABLE external_operation_uploads;
DROP TABLE external_upload_chunks;
DROP TABLE external_uploads;
-- Test-only rollback of external sessions migrations before historical fixtures.
DROP TRIGGER external_control_revoke;
DROP TRIGGER external_control_deleted;
DROP TRIGGER external_attach_reset;
ALTER TABLE sessions DROP COLUMN external_attach_operation;
DROP TABLE external_operations;
DROP TABLE external_oauth_refresh_history;
DROP TABLE external_oauth_tokens;
DROP TABLE external_oauth_requests;
DROP TABLE external_oauth_clients;
DROP TABLE external_client_sessions;
DROP TABLE external_client_roots;
DROP TABLE external_client_hats;
DROP TABLE external_clients;
