-- pg_improv 0.13.0 -> 0.14.0 (ALTER EXTENSION pg_improv UPDATE).
-- No catalog or function change: 0.14.0's changes are in the new .so
-- (TLS to other Postgres servers is the client crates', and the sync
-- worker's owner check now sees a REVOKE committed mid-transaction).
-- This script only moves the recorded version.
-- First, the lock every sync worker transaction takes shared: the update
-- then holds no catalog lock while a poll reads (src/worker.rs, update_gate).
SELECT pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtext('improv.update'));
SELECT 1;
