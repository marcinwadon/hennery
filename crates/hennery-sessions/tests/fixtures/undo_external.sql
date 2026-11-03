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
DROP TABLE external_clients;
