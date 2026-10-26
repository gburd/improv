-- pg_improv 0.11.0 -> 0.12.0 (ALTER EXTENSION pg_improv UPDATE).
-- A measure's number format (H1-22): how the GUI, TUI and CLI show its
-- numbers, a JSON NumberFormat ({"decimals": 2, "grouping": true}); NULL is
-- the General form, as every measure was before. Presentation only: the
-- extension never reads it, and the result tables keep every bit.
ALTER TABLE improv.measure ADD COLUMN format jsonb;
