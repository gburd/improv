-- pg_improv 0.14.0 -> 0.15.0 (ALTER EXTENSION pg_improv UPDATE).
-- Live updates: a moved improv.model_version, and result rows written by
-- the sync worker, trigger or commit mode or recalc, notify the model's
-- listeners at commit: NOTIFY improv_<model id>, '<model_version.tx>'.
-- The trigger below is the first half; the rest is in the new .so. No
-- existing row changes.
-- First, the lock every sync worker transaction takes shared: the update
-- then holds no catalog lock while a poll reads (src/worker.rs, update_gate).
SELECT pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtext('improv.update'));
CREATE FUNCTION improv.version_moved() RETURNS trigger
  LANGUAGE c AS 'MODULE_PATHNAME', 'version_moved_wrapper';
CREATE TRIGGER version_moved AFTER INSERT OR UPDATE OF tx ON improv.model_version
  FOR EACH ROW EXECUTE FUNCTION improv.version_moved();
ALTER TABLE improv.model_version ENABLE ALWAYS TRIGGER version_moved;
