-- pg_improv 0.14.0 -> 0.15.0 (ALTER EXTENSION pg_improv UPDATE).
-- Live updates: a moved improv.model_version, and inputs applied or
-- results written (the sync worker, trigger or commit mode, recalc),
-- notify the model's listeners at commit:
-- NOTIFY improv_<model id>, '<model_version.tx>.<stale.data_gen>'.
-- data_gen is the new column below (0 for every model); the trigger is
-- the version half; the rest is in the new .so. No existing value changes.
-- First, the lock every sync worker transaction takes shared: the update
-- then holds no catalog lock while a poll reads (src/worker.rs, update_gate).
SELECT pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtext('improv.update'));
ALTER TABLE improv.stale ADD COLUMN data_gen bigint NOT NULL DEFAULT 0;
CREATE FUNCTION improv.version_moved() RETURNS trigger
  LANGUAGE c AS 'MODULE_PATHNAME', 'version_moved_wrapper';
CREATE TRIGGER version_moved AFTER INSERT OR UPDATE OF tx ON improv.model_version
  FOR EACH ROW EXECUTE FUNCTION improv.version_moved();
ALTER TABLE improv.model_version ENABLE ALWAYS TRIGGER version_moved;
