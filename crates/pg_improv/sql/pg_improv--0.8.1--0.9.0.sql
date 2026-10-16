-- pg_improv 0.8.1 -> 0.9.0 (ALTER EXTENSION pg_improv UPDATE).
-- The catalog rows are the user's models: register them so pg_dump keeps
-- them (before 0.9.0 a dump restored every model empty). See
-- `dump_catalog` in src/lib.rs and `l2_logical` in src/worker.rs.
SELECT pg_catalog.pg_extension_config_dump('improv.model', '');
SELECT pg_catalog.pg_extension_config_dump('improv.model_id_seq', '');
SELECT pg_catalog.pg_extension_config_dump('improv.category', '');
SELECT pg_catalog.pg_extension_config_dump('improv.item', '');
SELECT pg_catalog.pg_extension_config_dump('improv.measure', '');
SELECT pg_catalog.pg_extension_config_dump('improv.binding', '');
SELECT pg_catalog.pg_extension_config_dump('improv.category_binding', '');
SELECT pg_catalog.pg_extension_config_dump('improv.cell', '');
SELECT pg_catalog.pg_extension_config_dump('improv.view', '');
SELECT pg_catalog.pg_extension_config_dump('improv.meta', '');
SELECT pg_catalog.pg_extension_config_dump('improv.model_version', '');
SELECT pg_catalog.pg_extension_config_dump('improv.sync_state', '');
-- mint_items runs with a pinned search_path (pgspot PS005), as a fresh
-- 0.9.0 install defines it.
ALTER FUNCTION improv.mint_items(bigint, bigint[], text[])
  SET search_path = pg_catalog, pg_temp;
