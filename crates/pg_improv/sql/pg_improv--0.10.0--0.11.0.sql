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
-- Each model acts as its owner: its loads and result writes run as that
-- role, so row-level security and privileges apply as they do to it. A
-- model made before 0.11.0 is owned by the role that made it, which
-- create_model recorded as the owner of its result schema; a model with no
-- result schema left falls back to the bootstrap superuser, which is how
-- the sync worker read every model until now.
ALTER TABLE improv.model ADD COLUMN owner regrole;
UPDATE improv.model m SET owner = n.nspowner::pg_catalog.regrole
  FROM pg_catalog.pg_namespace n
 WHERE n.nspname OPERATOR(pg_catalog.=) ('improv_' OPERATOR(pg_catalog.||) pg_catalog.lower(m.name));
DO $$
DECLARE m text;
BEGIN
  FOR m IN SELECT name FROM improv.model WHERE owner IS NULL ORDER BY id LOOP
    RAISE NOTICE 'pg_improv: model % has no result schema naming the role that created it, so its owner is now the bootstrap superuser; give it another with UPDATE improv.model SET owner = ''<role>'' WHERE name = %', m, pg_catalog.quote_literal(m);
  END LOOP;
END $$;
UPDATE improv.model SET owner = '10'::pg_catalog.oid::pg_catalog.regrole WHERE owner IS NULL;
ALTER TABLE improv.model
  ALTER COLUMN owner SET DEFAULT pg_catalog.to_regrole(pg_catalog.quote_ident(CURRENT_USER)),
  ALTER COLUMN owner SET NOT NULL;
-- Who may act as a role: a superuser, or a member of it with the right to
-- SET ROLE to it (any member on PG 15), the rule ALTER TABLE ... OWNER TO
-- uses. A model reads its bound tables and writes its results as its
-- owner, so only those who may act as the owner may set it or change the
-- model: else anyone who may write the catalog could make a model read,
-- as a role that sees more, what they cannot read themselves.
CREATE FUNCTION improv.may_act_as(r pg_catalog.regrole) RETURNS boolean
LANGUAGE sql STABLE STRICT SET search_path = pg_catalog, pg_temp AS $$
SELECT pg_catalog.pg_has_role(CURRENT_USER, r::pg_catalog.oid,
  CASE WHEN pg_catalog.current_setting('server_version_num')::pg_catalog.int4
              OPERATOR(pg_catalog.>=) 160000 THEN 'SET' ELSE 'MEMBER' END)
$$;
-- A model's owner: set and changed only by those who may act as both the
-- current and the new owner. A pg_restore passes when its role may act as
-- each model's owner: a superuser, or the role owning them all.
CREATE FUNCTION improv.check_owner() RETURNS trigger LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp AS $$
BEGIN
  IF TG_OP OPERATOR(pg_catalog.=) 'UPDATE' AND NOT improv.may_act_as(OLD.owner) THEN
    RAISE EXCEPTION 'only a superuser or a member of role % may change the owner of model %',
      OLD.owner, OLD.name USING ERRCODE = 'insufficient_privilege';
  END IF;
  IF NOT improv.may_act_as(NEW.owner) THEN
    RAISE EXCEPTION 'only a superuser or a member of role % may make it a model''s owner',
      NEW.owner USING ERRCODE = 'insufficient_privilege';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER check_owner BEFORE INSERT OR UPDATE OF owner ON improv.model
  FOR EACH ROW EXECUTE FUNCTION improv.check_owner();
-- Row-level security on the catalog: a model's rows are read by anyone who
-- may use schema improv (names and formulas: status reads them), and
-- changed only by those who may act as its owner. Its items (improv.item:
-- the bound tables' keys), input cells (improv.cell) and other state
-- (improv.meta: scenario overrides, SQL sources) are data, read only by
-- them too. A superuser, the sync worker and a writer's trigger or commit
-- (which reads the catalog as the bootstrap superuser) bypass it.
ALTER TABLE improv.model ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.model FOR SELECT USING (true);
CREATE POLICY write ON improv.model USING (improv.may_act_as(owner));
ALTER TABLE improv.category ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.category FOR SELECT USING (true);
CREATE POLICY write ON improv.category USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.item ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.item FOR SELECT USING (improv.may_act_as((SELECT m.owner FROM improv.model m WHERE m.id OPERATOR(pg_catalog.=) model_id)));
CREATE POLICY write ON improv.item USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.measure ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.measure FOR SELECT USING (true);
CREATE POLICY write ON improv.measure USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.binding ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.binding FOR SELECT USING (true);
CREATE POLICY write ON improv.binding USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.cell ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.cell FOR SELECT USING (improv.may_act_as((SELECT m.owner FROM improv.model m WHERE m.id OPERATOR(pg_catalog.=) model_id)));
CREATE POLICY write ON improv.cell USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.view ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.view FOR SELECT USING (true);
CREATE POLICY write ON improv.view USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.meta ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.meta FOR SELECT USING (improv.may_act_as((SELECT m.owner FROM improv.model m WHERE m.id OPERATOR(pg_catalog.=) model_id)));
CREATE POLICY write ON improv.meta USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.model_version ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.model_version FOR SELECT USING (true);
CREATE POLICY write ON improv.model_version USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.category_binding ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.category_binding FOR SELECT USING (true);
CREATE POLICY write ON improv.category_binding USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.lambda ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.lambda FOR SELECT USING (true);
CREATE POLICY write ON improv.lambda USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.sync_state ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.sync_state FOR SELECT USING (true);
CREATE POLICY write ON improv.sync_state USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
-- The owner now writes the result tables, which the sync worker made as the
-- bootstrap superuser (logical mode) and trigger mode as the writing role.
DO $$
DECLARE t record;
BEGIN
  FOR t IN SELECT c.oid::pg_catalog.regclass AS rel, m.owner
             FROM improv.model m
             JOIN improv.measure d ON d.model_id OPERATOR(pg_catalog.=) m.id
                                  AND d.kind OPERATOR(pg_catalog.=) 'derived'
             JOIN pg_catalog.pg_namespace n
               ON n.nspname OPERATOR(pg_catalog.=) ('improv_' OPERATOR(pg_catalog.||) pg_catalog.lower(m.name))
             JOIN pg_catalog.pg_class c ON c.relnamespace OPERATOR(pg_catalog.=) n.oid
                                       AND c.relname OPERATOR(pg_catalog.=) pg_catalog.lower(d.name)
            WHERE c.relowner OPERATOR(pg_catalog.<>) m.owner::pg_catalog.oid
  LOOP
    EXECUTE pg_catalog.format('ALTER TABLE %s OWNER TO %s', t.rel, t.owner);
  END LOOP;
END $$;

-- Commit mode's opt-in row capture (improv.commit_capture = 'rows').
CREATE FUNCTION improv.commit_capture() RETURNS trigger
  LANGUAGE c AS 'MODULE_PATHNAME', 'commit_capture_wrapper';

-- The roles (see the README's Privileges). Roles are cluster-wide: made here
-- when missing (another database's install, or the DBA, may have made them),
-- and kept by DROP EXTENSION. improv_user builds models: schema USAGE, the
-- API (EXECUTE: PUBLIC has it too, unless the DBA revokes it), the catalog
-- DML that row-level security confines to the models it may act as the
-- owner of, and improv.models. No UPDATE on improv.sync_state (the
-- worker's progress) and nothing on improv.slot_to_drop (the worker drops
-- the slots it names). improv_admin enrolls builders (ADMIN OPTION on
-- improv_user) and acts on no one else's model: acting on a model is acting
-- as its owner, so acting on every model would be acting as every owner,
-- superusers included. Only a superuser does that.
DO $$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname OPERATOR(pg_catalog.=) 'improv_user') THEN
    CREATE ROLE improv_user NOLOGIN;
  END IF;
  IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname OPERATOR(pg_catalog.=) 'improv_admin') THEN
    CREATE ROLE improv_admin NOLOGIN;
    GRANT improv_user TO improv_admin WITH ADMIN OPTION;
  END IF;
END $$;
-- Every model, for anyone who may read improv.model (whose read policy is
-- true): may_change says whether the caller may act as its owner.
CREATE VIEW improv.models WITH (security_invoker = true) AS
SELECT m.name, m.owner, m.sync_mode, improv.may_act_as(m.owner) AS may_change,
       'improv_' OPERATOR(pg_catalog.||) pg_catalog.lower(m.name) AS result_schema
FROM improv.model m;
GRANT USAGE ON SCHEMA improv TO improv_user;
GRANT SELECT, INSERT, UPDATE, DELETE ON improv.model, improv.category, improv.item,
  improv.measure, improv.binding, improv.category_binding, improv.cell, improv.view,
  improv.meta, improv.model_version, improv.lambda TO improv_user;
GRANT SELECT, INSERT, DELETE ON improv.sync_state TO improv_user;
GRANT USAGE ON SEQUENCE improv.model_id_seq TO improv_user;
GRANT SELECT ON improv.models TO improv_user;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA improv TO improv_user;
