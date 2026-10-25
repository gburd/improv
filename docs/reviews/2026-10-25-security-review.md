# C3 security review: pg_improv and improv-server (2026-10-25)

The road-to-1.0 plan's C3: review pg_improv and the HTTP server for SQL built
from strings, `SECURITY INVOKER`/`DEFINER` paths, `search_path`, the install
and update scripts, pgspot findings, and the server's auth, scopes, limits
and CORS. Every high is fixed before 1.0; every medium is fixed or accepted
with a reason.

Base: `main` at `19211d7` (v0.11.0). The earlier review,
[2026-10-04-security-scalability.md](2026-10-04-security-scalability.md)
(S1–S14), covered external functions, the sandbox, SQL refresh queries and
the server's first hardening; its fixes are not repeated here. The threat
model is the same: anything stored in a model is untrusted, a token holder
is semi-trusted, an anonymous client is untrusted. For Postgres it adds: a
role with `improv_user` (or `EXECUTE` and `USAGE` granted another way) is a
semi-trusted model builder, and must not be able to read, through a model,
anything its own grants do not let it read.

## Method

- **Generated SQL:** the 0.11.0 install script (`cargo pgrx schema`, 21
  functions) and the four update scripts in `crates/pg_improv/sql/`, read in
  full, and run through pgspot 0.9.3 with and without CI's ignores.
- **String-built SQL:** every `format!` that builds a statement then run,
  in pg_improv, storage_pg and pg_model (about 30 production sites, 75
  interpolated values), traced from source to statement. Each value is
  quoted (`quote_ident`, `format('%I')`, `regclass::text`), a constant, or
  checked against a fixed list. Working notes: the rescue copy's
  `c3/sql-quoting.md`.
- **Privileges, tested live:** a PG 18.6 cluster with the release build,
  three roles: `alice` (`improv_user`, owns model `m`, may read `sales`),
  `bob` (`improv_user`, not alice), `eve` (no grants). Each check below was
  run as that role; the script is the rescue copy's `ops/c3-priv.sh`.
- **`unsafe`:** every site in `crates/` (56), each read for its invariant.
- **The server:** `crates/server/src/main.rs` read end to end.

## Findings

| ID | Area | Severity | Finding | Status |
|---|---|---|---|---|
| C3-0 | engine, pg_improv | **medium** | Text results had no size limit except in `REPT` and `TEXTJOIN`: `SUBSTITUTE` and `&` could square a string's length per step. `SUBSTITUTE(SUBSTITUTE(REPT("x", 30000), "x", REPT("y", 30000)), "y", REPT("z", 30000))` passed 6 GB and aborted. Any role that may `CREATE FUNCTION ... LANGUAGE improv` (a TRUSTED language: an ordinary user with CREATE on a schema) could exhaust a Postgres backend's memory with one call; the same formula through `define` or the server's model. | **fixed** (`fix(engine): no text result passes 32,767 characters`): Excel's limit on every text result (`builtins::MAX_TEXT`, enforced in `ok_text` and `&`; `SUBSTITUTE` and `TEXTJOIN` size the result before building it). The formula is now `#VALUE!` in 0.00 s and 18 MB. |
| C3-1 | pg_model | low | `quote_literal` doubled quotes only, so a backslash would end the literal early under `standard_conforming_strings = off`. Its one caller (`trigger.rs:462`) passes a model name `validate_name` restricts to `[A-Za-z0-9_]`, so no input reaches it today. | **fixed** (`fix(pg_model): quote_literal reads the same whatever standard_conforming_strings is`): writes `E'...'` with doubled backslashes, as Postgres's `quote_literal()` does; checked against PG 18 with the setting on and off. |
| C3-2 | extfn | low | Four `unsafe` blocks (`pre_exec`, `getrlimit`/`setrlimit`, the process-group `kill`) had no SAFETY comment. Their invariants hold. | **fixed** (`docs(extfn): SAFETY comments on the pre_exec, rlimit and kill blocks`). |
| C3-3 | pg_improv | low | `bind_category`/`bind_measure` accept a table the model's owner may not read: they check only that the column exists. Nothing is read: `recalc`, trigger mode and the worker all refuse as the owner (`permission denied for table sales`; the worker's `last_error` names the columns). But in logical mode the table joins publication `improv_db` until the model is dropped, and logical decoding then carries its rows (the worker discards them unapplied). | **accepted**: no data reaches the builder, and failing at bind time would break a builder who binds before the DBA's GRANT. An earlier error with a hint is a usability improvement for 0.12. |
| C3-4 | pg_improv | info | Without `REVOKE`, `PUBLIC` keeps `EXECUTE` on every `improv.*` function (Postgres's default), but `USAGE` on schema `improv` is only `improv_user`'s, so `eve` gets `permission denied for schema improv` on every call. The README's Privileges section says the same. | no change |
| C3-5 | pg_improv | info | `improv.on_change()` and `improv.commit_catalog_changed()`, the trigger functions, are executable by `improv_user`; called directly they fail (`PgTrigger::from_fcinfo failed: NotTrigger`). | no change |
| C3-6 | server | info | `GET /v1/model` (Read scope) returns the whole stored model, external function bodies and SQL-refresh queries included. A Read token is a model reader by design, and the model holds no credentials (SQL refresh takes its source at run time, never from the model). The route is marked `x-improv-unstable` (J2). | no change |
| C3-7 | server | info | A token file readable by group or others only warns. | no change: the operator chose the file's mode; refusing would be a surprise on a single-user host. |

No high finding was found, and no high or medium finding remains open. No
`SECURITY DEFINER` function exists.

## pgspot

`ci/pg_improv.sh` runs `pgspot --ignore PS001 --ignore PS016` on the install
script and every update script; all five are clean (0 errors, 0 warnings).
Without the ignores the install script has three PS001 (unqualified `=` in
the `CHECK (… IN (…))` constraints on `model.sync_mode`, `measure.kind`,
`binding.agg`) and one PS016 (`now()` in `sync_state`'s column default).
Those run when a row is written by the extension's own functions, under a
search path the function pins (or the catalog's owner's), never in a
caller-controlled context, and Postgres resolves both through `pg_catalog`
first; the ignore is sound. Every function body in the scripts
schema-qualifies (`improv.`, `pg_catalog.`, `OPERATOR(pg_catalog.=)`).

## Privileges

Functions (from the generated install script and `pg_proc` on the live
cluster). None is `SECURITY DEFINER`; `prosecdef` is false for all 21.

| Function | Security | `search_path` | Who may change a model through it |
|---|---|---|---|
| `create_model`, `bind_category`, `bind_measure`, `define`, `drop_measure`, `drop_model`, `set_sync_mode`, `enable_logical`, `recalc` (2), `what_if` | INVOKER | per operation: [`as_role`](../../crates/pg_improv/src/owner.rs) pins `pg_catalog, pg_temp` while acting as the owner | a role that may act as the model's owner (`improv.may_act_as`: superuser, or SET-member on PG 16+, member on PG 15) |
| `status`, `sync_publication` | INVOKER | — | `status` reads only; `sync_publication` must own publication `improv_db` (bob: `must be owner of publication improv_db`) |
| `mint_items` (plpgsql) | INVOKER | `pg_catalog, pg_temp` | inserts as the caller, so `improv.item`'s RLS applies (bob into alice's model: `new row violates row-level security policy`) |
| `may_act_as` (sql), `check_owner` (trigger) | INVOKER | `pg_catalog, pg_temp` | — |
| `on_change`, `commit_capture`, `commit_catalog_changed` (triggers) | INVOKER | per operation (`as_catalog`/`as_owner`) | fail outside a trigger |
| `plimprov_call_handler`, `plimprov_validator` | — | — | `LANGUAGE improv` is TRUSTED; bodies are formulas, `SQL(...)`/`CALL(...)` are unknown functions there (checked: "unknown function: SQL"), the depth limit applies (61 levels refused), and text results are capped at 32,767 characters (C3-0) |

Tables, sequences and views (`relacl` on the live cluster). `PUBLIC` has
nothing on any of them; the extension's owner (the installing superuser)
has everything.

| Object | `improv_user` | Row-level security |
|---|---|---|
| `model` | SELECT, INSERT, UPDATE, DELETE | read: everyone; write: `may_act_as(owner)`; `check_owner` trigger guards `owner` |
| `category`, `measure`, `binding`, `category_binding`, `view`, `model_version`, `lambda` | SELECT, INSERT, UPDATE, DELETE | read: everyone; write: `may_act_as(model's owner)` |
| `item`, `cell`, `meta` | SELECT, INSERT, UPDATE, DELETE | read and write: `may_act_as(model's owner)` (they are data) |
| `sync_state` | SELECT, INSERT, DELETE (no UPDATE: the worker's progress) | read: everyone; write: `may_act_as(model's owner)` |
| `slot_to_drop` | none | — |
| `model_id_seq` | USAGE | — |
| `models` (view, `security_invoker`) | SELECT | through `model`'s policies |
| schema `improv` | USAGE | — |
| result schemas `improv_<model>` | none: owned by the model's owner, who grants them | — |

Checked live, as bob against alice's model: reading `improv.item` returns 0
rows; `UPDATE improv.model SET owner = 'bob'`, `DELETE FROM
improv.sync_state` and `UPDATE improv.measure` change 0 rows; `define(...,
replace => true)`, `drop_model` and `what_if` are refused with "only a
superuser or a member of its owner, alice, may change it" (what_if: "needs
to act as its owner"); `create_model('M')` (a name whose result schema
exists) is refused rather than adopting alice's schema; a model name like
`x"; DROP TABLE sales; --` is refused by `validate_name`. As eve, every call
and every read of `improv_m` is refused until alice grants the result table,
after which eve reads it like any table.

## search_path

No `SECURITY DEFINER` function exists, so no caller's search path is ever
used with elevated rights. The three SQL/plpgsql functions pin `search_path
= pg_catalog, pg_temp`. Every privileged operation in Rust goes through
`owner::as_role`, which pins the same search path and `row_security = on`
under `SECURITY_RESTRICTED_OPERATION` and restores both on error. The
string-built SQL names every relation qualified (`improv.*`, `regclass::text`,
`format('%I.%I')`), and a relation name computed as `regclass::text` is
computed in the same role and search path that runs its statement.

## String-built SQL

All 75 interpolated values in production code are quoted (45), constants
(27), or checked against a fixed list (3: a binding's `agg`, through
`pm::agg_sql`, `live::Agg::parse` and storage_pg's `agg_call`). A column type
used in a cast is checked against an allowed list and then quoted
(`delta.rs:272`, `:350`). Nothing user-supplied is interpolated unquoted.
The pg_model builders take an already-quoted relation and say so; every
caller passes one. `duckdb_model`'s `improv(source, model)` passes the
user's source text into DuckDB unquoted by design: it is the user's own
query, run with the user's own connection.

## unsafe

56 sites: pg_improv 50 (FFI with Postgres: hooks, callbacks, the call
handler, signals, syscache, identity switching), duckdb_improv 5 (the C API
entry point and vector slices), extfn 4 (sandbox). Each states its
invariant in a SAFETY comment or a `# Safety` doc section, except:

- the four extfn blocks, now commented (C3-2);
- `unsafe extern "C-unwind"` declarations and `#[pg_guard]` callbacks
  (`worker.rs:508` `die`, `commit.rs` `relcache_callback`,
  `process_utility`, `subxact_callback`, `xact_callback`), whose contract is
  Postgres's hook or callback signature, documented where they are
  registered (`commit.rs:118`: "_PG_init runs once per process ... the
  hooks chain to whatever was installed before them");
- `duckdb_improv`'s `extension_entrypoint`, whose contract is the
  `duckdb_entrypoint_c_api` macro's.

The invariants were checked: identity and GUC switches are undone in a
`finally` (`owner.rs`), syscache tuples are released before return
(`language.rs`), argument reads match the compiled signature's types
(`language.rs`, `arg`), and DuckDB slices are taken at the vector's length.

## The server

- **Auth:** every route but `/health` and `/openapi.json` goes through
  `require_token` (`main.rs:594-606`, from the one `routes()` table, for both
  `/v1/...` and the legacy path). Tokens are compared in constant time
  (`ct_eq`, no early exit; `find_token` checks every token). With no token
  configured the server refuses to bind a non-loopback address unless
  `IMPROV_ALLOW_OPEN=1`.
- **Scopes:** the only write route is `POST /measures/:id/cells` (Write);
  `POST /measures/:id/eval` is Read and checks Write itself for
  `persist: true`. No route installs functions, SQL sources or formulas.
- **Limits:** 1 MiB body (`DefaultBodyLimit`), 30 s per request, 64 in
  flight (one global semaphore), a 10 s header-read timeout (slow-loris),
  10,000 edits per `/eval`, and a per-caller token bucket
  (`IMPROV_RATE_LIMIT`; at most 10,000 tracked callers, idle ones forgotten).
- **Static files (`--static`):** the root is canonicalized at startup; each
  request's path is joined, canonicalized and refused unless it is still
  under the root, so `..` and symlinks out are 404. Percent-encoding is not
  decoded.
- **CORS:** off unless `IMPROV_CORS_ORIGINS` lists origins; an origin is
  echoed only on an exact match, with `Vary: Origin`; no credentials header.
- **Errors:** internal errors (which can carry the store's path) go to the
  log with a request id; the client gets the id only.
- **Audit:** every write and every refused write attempt is logged with the
  caller's token name, never the secret.
