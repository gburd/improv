-- pg_improv 0.10.0 -> 0.11.0 (ALTER EXTENSION pg_improv UPDATE).
-- One logical replication slot and publication per database, improv_db,
-- instead of one per model (src/worker.rs): the sync worker decodes the WAL
-- once per poll and routes each change to the models bound to its table.
CREATE FUNCTION improv.sync_publication() RETURNS void STRICT SECURITY INVOKER
  LANGUAGE c AS 'MODULE_PATHNAME', 'sync_publication_wrapper';
-- improv_db publishes the union of the logical-mode models' bound tables.
SELECT improv.sync_publication();
-- Move each model's sync state to the shared slot. Its progress resets, so
-- the worker catches each model up with one full recompute from its tables
-- once it has created improv_db: nothing committed before or after the
-- update is lost, and the per-model slots are no longer read.
UPDATE improv.sync_state
   SET slot = 'improv_db', publication = 'improv_db',
       confirmed_lsn = NULL, last_applied_at = NULL;
-- The old per-model publications go now; their slots go next, dropped by the
-- worker (a slot cannot be dropped in this transaction once it has written).
DO $$
DECLARE p text;
BEGIN
  FOR p IN SELECT pubname FROM pg_catalog.pg_publication
           WHERE pubname OPERATOR(pg_catalog.~) '^improv_' AND pubname OPERATOR(pg_catalog.<>) 'improv_db'
             AND pg_catalog.substr(pubname, 8) IN (SELECT pg_catalog.lower(name) FROM improv.model)
  LOOP
    EXECUTE pg_catalog.format('DROP PUBLICATION %I', p);
  END LOOP;
END $$;
INSERT INTO improv.slot_to_drop (slot)
  SELECT slot_name FROM pg_catalog.pg_replication_slots
   WHERE database OPERATOR(pg_catalog.=) pg_catalog.current_database()
     AND slot_name OPERATOR(pg_catalog.~) '^improv_' AND slot_name OPERATOR(pg_catalog.<>) 'improv_db'
     AND pg_catalog.substr(slot_name, 8) IN (SELECT pg_catalog.lower(name) FROM improv.model)
  ON CONFLICT DO NOTHING;
