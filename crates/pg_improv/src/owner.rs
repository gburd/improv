//! Each model acts as its owner (`improv.model.owner`, the role that called
//! `create_model`): every read of its bound tables and every write of its
//! result tables runs as that role, so row-level security and privileges
//! apply to a model exactly as they apply to its owner, whichever process
//! runs it: the logical sync worker (the bootstrap superuser), a writer's
//! trigger or commit, a `define` or a `recalc`. Its catalog rows
//! (`improv.*`) are read and written by whoever runs it, or, in a writer's
//! trigger or commit, by the bootstrap superuser ([`as_catalog`]): a writer
//! needs no privilege on `improv` to write a bound table. Row-level security
//! on the catalog (below) lets only a role that may act as the owner change
//! the model, so no one makes it read as the owner what they cannot.
//!
//! [`as_role`] switches with `SetUserIdAndSecContext`, as Postgres does for
//! work run on a table owner's behalf (`REFRESH MATERIALIZED VIEW`,
//! `ANALYZE`). `SET ROLE` would need the session user to be a member of the
//! owner: the worker's bootstrap superuser is, a writer firing a trigger is
//! not. `SECURITY_RESTRICTED_OPERATION` forbids `SET ROLE`, temporary
//! tables and other state that outlives the operation (cursors `WITH HOLD`,
//! `LISTEN`), so a function the owner controls (an RLS policy, a column
//! default) cannot take over the session it runs in, `search_path` is
//! pinned to `pg_catalog, pg_temp`, so no schema can shadow a name the
//! generated SQL uses, and `row_security` is pinned on, since with
//! it off (as pg_dump and pg_restore set it) a policy that applies to the
//! owner makes its reads fail instead of filtering them. All of it is
//! restored afterwards, on error too.

use pgrx::prelude::*;

use super::Res;

/// The owner of model `mid`; `None` before `ALTER EXTENSION … UPDATE` to
/// 0.11.0 added the column, when a model acts as its caller (as before).
pub(crate) fn owner_of(mid: i64) -> Res<Option<pg_sys::Oid>> {
    // Through to_jsonb, so a catalog without the column reads NULL.
    let (owner, exists) = Spi::get_two_with_args::<pg_sys::Oid, bool>(
        "SELECT o, EXISTS (SELECT 1 FROM pg_roles WHERE oid = o) \
         FROM (SELECT (to_jsonb(m) ->> 'owner')::regrole::oid AS o \
               FROM improv.model m WHERE m.id = $1) s",
        &[mid.into()],
    )?;
    match (owner, exists) {
        (Some(o), Some(false)) => Err(format!(
            "the owner of improv model {mid} (role oid {}) no longer exists: give the model \
             another (as a superuser) with UPDATE improv.model SET owner = '<role>' \
             WHERE id = {mid}",
            o.to_u32()
        )
        .into()),
        (o, _) => Ok(o),
    }
}

/// Run `f` as model `mid`'s owner.
pub(crate) fn as_owner<R>(mid: i64, f: impl FnOnce() -> Res<R>) -> Res<R> {
    as_role(owner_of(mid)?, f)
}

/// Run `f` as the bootstrap superuser: catalog work only, never a read of
/// a bound table or a write of a result table.
pub(crate) fn as_catalog<R>(f: impl FnOnce() -> Res<R>) -> Res<R> {
    as_role(Some(pg_sys::BOOTSTRAP_SUPERUSERID.into()), f)
}

/// Run `f` as `role` (as the current role when `None`), restricted, with
/// `search_path = pg_catalog, pg_temp` and `row_security = on`.
pub(crate) fn as_role<R>(role: Option<pg_sys::Oid>, f: impl FnOnce() -> Res<R>) -> Res<R> {
    let Some(role) = role else { return f() };
    let (mut saved, mut ctx) = (pg_sys::InvalidOid, 0);
    // SAFETY: reads and writes of the backend's identity and settings, on
    // the backend thread, in a live transaction; the `finally` restores both
    // on success and on error.
    let nest = unsafe {
        pg_sys::GetUserIdAndSecContext(&mut saved, &mut ctx);
        let nest = pg_sys::NewGUCNestLevel();
        for (name, value) in [
            (c"search_path", c"pg_catalog, pg_temp"),
            (c"row_security", c"on"),
        ] {
            pg_sys::set_config_option(
                name.as_ptr(),
                value.as_ptr(),
                pg_sys::GucContext::PGC_USERSET,
                pg_sys::GucSource::PGC_S_SESSION,
                pg_sys::GucAction::GUC_ACTION_SAVE,
                true,
                0,
                false,
            );
        }
        let restricted =
            pg_sys::SECURITY_LOCAL_USERID_CHANGE | pg_sys::SECURITY_RESTRICTED_OPERATION;
        pg_sys::SetUserIdAndSecContext(role, ctx | restricted as i32);
        nest
    };
    let f = std::panic::AssertUnwindSafe(f);
    PgTryBuilder::new(move || {
        // The whole wrapper, not its field: closures capture disjointly.
        let f = f;
        (f.0)()
    })
    .finally(move || {
        // SAFETY: undoes the switch above, innermost first.
        unsafe {
            pg_sys::AtEOXact_GUC(false, nest);
            pg_sys::SetUserIdAndSecContext(saved, ctx);
        }
    })
    .execute()
}

extension_sql!(
    r#"
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
-- may use schema improv (names and formulas: status, what_if and the GUI
-- read them), and changed only by those who may act as its owner. Input
-- cells (improv.cell) and a model's other state (improv.meta: scenario
-- overrides, SQL sources) are data, read only by them too. A superuser, the
-- sync worker and a writer's trigger or commit (which reads the catalog as
-- the bootstrap superuser) bypass it.
ALTER TABLE improv.model ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.model FOR SELECT USING (true);
CREATE POLICY write ON improv.model USING (improv.may_act_as(owner));
ALTER TABLE improv.category ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.category FOR SELECT USING (true);
CREATE POLICY write ON improv.category USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
ALTER TABLE improv.item ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.item FOR SELECT USING (true);
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
"#,
    name = "check_owner",
    requires = ["dump_catalog", "l2_logical"]
);

/// Whether row-level security filters what `role` reads from `rel` (a
/// policy applies to it, it neither owns the table nor bypasses RLS).
pub(crate) fn rls_applies(rel: pg_sys::Oid, role: pg_sys::Oid) -> bool {
    // SAFETY: a catalog lookup; `noError` makes it return, not raise.
    let r = unsafe { pg_sys::check_enable_rls(rel, role, true) };
    r == pg_sys::CheckEnableRlsResult::RLS_ENABLED as i32
}
