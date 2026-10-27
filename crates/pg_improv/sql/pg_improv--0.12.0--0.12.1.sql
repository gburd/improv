-- pg_improv 0.12.0 -> 0.12.1 (ALTER EXTENSION pg_improv UPDATE).
-- No catalog or function change: 0.12.1 rebuilds the same extension on EL8
-- (glibc 2.28) so it loads on RHEL 8 and 9. This script only moves the
-- recorded version.
SELECT 1;
