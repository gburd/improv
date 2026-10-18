-- pg_improv 0.9.0 -> 0.10.0 (ALTER EXTENSION pg_improv UPDATE).
-- The `commit` sync mode (src/commit.rs): one recompute per writing
-- transaction, before it commits, with nothing on the user's tables.
ALTER TABLE improv.model DROP CONSTRAINT model_sync_mode_check;
ALTER TABLE improv.model ADD CONSTRAINT model_sync_mode_check
  CHECK (sync_mode IN ('logical', 'trigger', 'commit', 'manual'));
-- Each backend caches the relations commit-mode models bind; these
-- triggers invalidate every backend's copy when the catalog changes.
CREATE FUNCTION improv.commit_catalog_changed() RETURNS trigger
  LANGUAGE c AS 'MODULE_PATHNAME', 'commit_catalog_changed_wrapper';
CREATE TRIGGER commit_sync AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON improv.model
  FOR EACH STATEMENT EXECUTE FUNCTION improv.commit_catalog_changed();
CREATE TRIGGER commit_sync AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON improv.binding
  FOR EACH STATEMENT EXECUTE FUNCTION improv.commit_catalog_changed();
