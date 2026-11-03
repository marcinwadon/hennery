-- Test-only rollback of sessions migrations 39 and 38 before historical fixtures.
DROP TABLE external_oauth_refresh_history;
DROP TABLE external_oauth_tokens;
DROP TABLE external_oauth_requests;
DROP TABLE external_oauth_clients;
DROP TABLE external_client_sessions;
DROP TABLE external_client_roots;
DROP TABLE external_clients;
