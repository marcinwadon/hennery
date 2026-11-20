-- Test-only rollback of the ACP v2 projection migration before historical fixtures.
DROP TRIGGER acp_view_scrub;
DROP TABLE acp_view_refs;
DROP TABLE acp_view_entities;
ALTER TABLE sessions DROP COLUMN background_state;
ALTER TABLE sessions DROP COLUMN background_info;
ALTER TABLE sessions DROP COLUMN acp_protocol;
ALTER TABLE sessions DROP COLUMN acp_fallback;
