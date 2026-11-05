-- pg_improv 0.12.0 -> 0.12.1 (ALTER EXTENSION pg_improv UPDATE).
-- No catalog or function change: 0.12.1 rebuilds the same extension on EL8
-- (glibc 2.28) so it loads on RHEL 8 and 9. This script only moves the
-- recorded version.
-- First, the lock every sync worker transaction takes shared: the update
-- then holds no catalog lock while a poll reads (src/worker.rs, update_gate).
SELECT pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtext('improv.update'));
SELECT 1;
