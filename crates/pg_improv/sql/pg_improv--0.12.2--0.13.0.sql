-- pg_improv 0.12.2 -> 0.13.0 (ALTER EXTENSION pg_improv UPDATE).
-- Access policies (Cedar, improv_policy): a model's policies in
-- improv.policy, every change in improv.policy_audit, the functions that
-- set, show and explain them, the filtered reads, set_cells, and the
-- row-level security that applies them. No model has policies after the
-- update, so every model behaves as before until its owner sets some; no
-- existing row changes.
-- First, the lock every sync worker transaction takes shared: the update
-- then holds no catalog lock while a poll reads (src/worker.rs, update_gate).
SELECT pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtext('improv.update'));
CREATE FUNCTION improv.unrestricted(model bigint) RETURNS bool
  STRICT STABLE SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'unrestricted_wrapper';
CREATE FUNCTION improv.cell_visible(model bigint, measure bigint, dims text[]) RETURNS bool
  STRICT STABLE SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'cell_visible_wrapper';
CREATE FUNCTION improv.cell_rule(model bigint, measure bigint) RETURNS integer
  STRICT STABLE SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'cell_rule_wrapper';
CREATE FUNCTION improv.row_visible(model bigint, measure bigint, VARIADIC dims "any")
  RETURNS bool STRICT STABLE SECURITY INVOKER LANGUAGE c
  AS 'MODULE_PATHNAME', 'row_visible_wrapper';
CREATE FUNCTION improv.cell_name(model bigint, measure bigint, dim integer, slot integer)
  RETURNS text STRICT STABLE SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'cell_name_wrapper';
CREATE FUNCTION improv.item_changed() RETURNS trigger
  LANGUAGE c AS 'MODULE_PATHNAME', 'item_changed_wrapper';
CREATE FUNCTION improv.result_written() RETURNS trigger
  LANGUAGE c AS 'MODULE_PATHNAME', 'result_written_wrapper';
CREATE FUNCTION improv.set_policies(model text, policies text, groups text DEFAULT NULL)
  RETURNS bigint SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'set_policies_wrapper';
CREATE FUNCTION improv.policies(model text)
  RETURNS TABLE (version bigint, policies text, groups text, set_by text,
                 set_at timestamp with time zone)
  STRICT SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'policies_wrapper';
CREATE FUNCTION improv.explain_access(model text, action text, resource jsonb DEFAULT NULL,
                                      role text DEFAULT NULL)
  RETURNS TABLE (step int, action text, resource text, allowed bool, policies text[],
                 errors text[])
  SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'explain_access_wrapper';
CREATE FUNCTION improv.categories(model text) RETURNS TABLE (id bigint, name text)
  STRICT SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'categories_wrapper';
CREATE FUNCTION improv.items(model text) RETURNS TABLE (category text, id bigint, name text)
  STRICT SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'items_wrapper';
CREATE FUNCTION improv.measures(model text)
  RETURNS TABLE (id bigint, name text, kind text, value_type text, categories text[],
                 formula text)
  STRICT SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'measures_wrapper';
CREATE FUNCTION improv.cells(model text, measure text) RETURNS TABLE (coord jsonb, value jsonb)
  STRICT SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'cells_wrapper';
CREATE FUNCTION improv.set_cells(model text, edits jsonb) RETURNS bigint
  STRICT SECURITY INVOKER LANGUAGE c AS 'MODULE_PATHNAME', 'set_cells_wrapper';

-- As src/policy.rs creates them.
CREATE TABLE improv.policy (
    model_id bigint PRIMARY KEY REFERENCES improv.model(id) ON DELETE CASCADE,
    version  bigint NOT NULL,
    text     text,
    groups   text,
    set_by   text NOT NULL,
    set_at   timestamptz NOT NULL
);
CREATE TABLE improv.policy_audit (
    model_id bigint NOT NULL,
    model    text NOT NULL,
    at       timestamptz NOT NULL,
    who      text NOT NULL,
    action   text NOT NULL,
    resource text NOT NULL,
    detail   jsonb
);
CREATE INDEX policy_audit_model ON improv.policy_audit (model_id, at);
ALTER TABLE improv.policy ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.policy FOR SELECT USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
CREATE POLICY write ON improv.policy USING (false);
ALTER TABLE improv.policy_audit ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.policy_audit FOR SELECT USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
CREATE POLICY write ON improv.policy_audit USING (false);
SELECT pg_catalog.pg_extension_config_dump('improv.policy', '');
SELECT pg_catalog.pg_extension_config_dump('improv.policy_audit', '');
ALTER POLICY read ON improv.category USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.measure USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.binding USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.category_binding USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.view USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.lambda USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.sync_state USING (improv.unrestricted(model_id));
CREATE TABLE improv.stale (
    model_id  bigint PRIMARY KEY REFERENCES improv.model(id) ON DELETE CASCADE,
    items     bigint NOT NULL DEFAULT 0,
    gen       bigint NOT NULL DEFAULT 1,
    clean_gen bigint
);
INSERT INTO improv.stale (model_id) SELECT id FROM improv.model;
ALTER TABLE improv.stale ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.stale FOR SELECT USING (true);
CREATE POLICY write ON improv.stale USING (false);
CREATE TRIGGER stale_items AFTER UPDATE ON improv.item
  REFERENCING OLD TABLE AS improv_old NEW TABLE AS improv_new
  FOR EACH STATEMENT EXECUTE FUNCTION improv.item_changed();
CREATE TRIGGER stale_items_del AFTER DELETE ON improv.item REFERENCING OLD TABLE AS improv_old
  FOR EACH STATEMENT EXECUTE FUNCTION improv.item_changed();
CREATE TRIGGER stale_items_trn AFTER TRUNCATE ON improv.item
  FOR EACH STATEMENT EXECUTE FUNCTION improv.item_changed();
CREATE TRIGGER stale_items_ins AFTER INSERT ON improv.item REFERENCING NEW TABLE AS improv_new
  FOR EACH STATEMENT EXECUTE FUNCTION improv.item_changed();
-- Also under session_replication_role = replica (a logical subscriber, a
-- restore with triggers disabled): a missed rename would leak.
ALTER TABLE improv.item ENABLE ALWAYS TRIGGER stale_items, ENABLE ALWAYS TRIGGER stale_items_del,
  ENABLE ALWAYS TRIGGER stale_items_trn, ENABLE ALWAYS TRIGGER stale_items_ins;

-- The grants a fresh install makes.
GRANT SELECT ON improv.policy, improv.policy_audit TO improv_user;
GRANT EXECUTE ON FUNCTION improv.unrestricted(bigint), improv.cell_visible(bigint, bigint, text[]),
  improv.cell_rule(bigint, bigint), improv.row_visible(bigint, bigint, "any"),
  improv.cell_name(bigint, bigint, integer, integer),
  improv.set_policies(text, text, text), improv.policies(text),
  improv.explain_access(text, text, jsonb, text), improv.categories(text), improv.items(text),
  improv.measures(text), improv.cells(text, text), improv.set_cells(text, jsonb) TO improv_user;

-- improv_reader, as a fresh install makes it (src/owner.rs, "roles").
DO $$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname OPERATOR(pg_catalog.=) 'improv_reader') THEN
    CREATE ROLE improv_reader NOLOGIN;
  END IF;
  IF NOT EXISTS (SELECT FROM pg_catalog.pg_auth_members a
                 WHERE a.roleid OPERATOR(pg_catalog.=) 'improv_reader'::pg_catalog.regrole
                   AND a.member OPERATOR(pg_catalog.=) 'improv_admin'::pg_catalog.regrole
                   AND a.admin_option) THEN
    GRANT improv_reader TO improv_admin WITH ADMIN OPTION;
  END IF;
END $$;
GRANT USAGE ON SCHEMA improv TO improv_reader;
GRANT SELECT ON improv.model, improv.models TO improv_reader;
GRANT EXECUTE ON FUNCTION improv.categories(text), improv.items(text), improv.measures(text),
  improv.cells(text, text), improv.set_cells(text, jsonb), improv.define(text, text, boolean),
  improv.drop_measure(text, text), improv.explain_access(text, text, jsonb, text),
  improv.policies(text), improv.cell_rule(bigint, bigint),
  improv.cell_name(bigint, bigint, integer, integer), improv.row_visible(bigint, bigint, "any"), improv.cell_visible(bigint, bigint, text[]),
  improv.unrestricted(bigint), improv.may_act_as(regrole) TO improv_reader;
