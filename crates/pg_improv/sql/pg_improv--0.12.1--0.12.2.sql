-- pg_improv 0.12.1 -> 0.12.2 (ALTER EXTENSION pg_improv UPDATE).
-- No catalog or function change: 0.12.2's change is inside the sync
-- worker (an idle worker keeps its model listing), in the new .so. This
-- script only moves the recorded version.
SELECT 1;
