-- Test-only rollback of sessions migrations 37 and 36 before historical fixtures.
DROP INDEX families_usage_root;
DROP INDEX turns_usage_state;
ALTER TABLE turns DROP COLUMN usage;
ALTER TABLE turns DROP COLUMN usage_at;
ALTER TABLE sessions DROP COLUMN usage_context;
ALTER TABLE sessions DROP COLUMN usage_cost;
ALTER TABLE sessions DROP COLUMN usage_totals;
ALTER TABLE sessions DROP COLUMN cache_retention;
ALTER TABLE sessions DROP COLUMN cache_retention_attached;
