-- pg_improv 0.12.1 -> 0.12.2 (ALTER EXTENSION pg_improv UPDATE).
-- No catalog or function change: 0.12.2's change is inside the sync
-- worker (an idle worker keeps its model listing), in the new .so. This
-- script only moves the recorded version.
-- First, the lock every sync worker transaction takes shared: the update
-- then holds no catalog lock while a poll reads (src/worker.rs, update_gate).
SELECT pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtext('improv.update'));
SELECT 1;
