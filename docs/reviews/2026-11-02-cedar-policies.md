# Per-user access control with Cedar policies (2026-11-02)

**Goal.** Every front end (the egui GUI, a COSMIC GUI, the web GUI, the
TUI and the CLI) connects securely to a database that hosts Improv models,
through pg_improv or through `improv-server`, and views and edits them live.
A per-user, per-role and per-group policy decides who sees and who changes
what. Policies are written in [Cedar](https://www.cedarpolicy.com)
(`cedar-policy`, Apache-2.0) and checked by one library, `improv_policy`,
at every enforcement point.

Base: `main` at `cff0143` (0.12.2). The 0.11 owner model is
`crates/pg_improv/src/owner.rs`; the server's tokens and scopes are in
`crates/server/src/main.rs`; the C3 review is
[2026-10-25-security-review.md](2026-10-25-security-review.md).

## Decisions in one list

1. **One schema, one library.** The Cedar schema below lives in
   `crates/policy` (`improv_policy::SCHEMA`). Postgres, the server and the
   clients load the same crate. No enforcement point writes its own rules.
2. **Opt-in per model.** A model with no stored policies behaves exactly as
   in 0.12. Once a model has policies, they are enforced and the default is
   deny.
3. **Forbid wins. Errors deny.** Cedar already makes a forbid override a
   permit. Cedar also skips a policy whose evaluation errors, which can
   turn a broken forbid into an allow. Improv denies any request whose
   evaluation reports an error.
4. **Policies name things by id.** `Improv::Measure::"101"`, not
   `"Revenue"`. A rename cannot move a measure out from under a forbid.
   The CLI prints names beside ids, and its templates take names.
5. **The owner is never restricted.** In Postgres, a superuser and any
   role that may act as the model's owner (`improv.may_act_as`) bypass
   Cedar. Cedar narrows what everyone else sees and does. Nobody can lock
   the owner out.
6. **Hidden means absent.** A hidden measure, item or cell is missing from
   every response: no placeholder, no count, no different error.
7. **The authority computes.** A client under a read policy never computes
   derived values from the inputs it can see. It reads them from the
   authority (pg_improv's result tables, or the server's engine), and asks
   the authority for what-ifs.
8. **Aggregates are computed over all data.** Hiding an item hides its
   cells, not its share of a total. A policy that must hide a total hides
   the total's measure or cells. `improv policy lint` reports the totals
   and derived measures that let a reader infer something hidden.
9. **DuckDB models go through improv-server.** DuckDB has no users. A
   DuckDB-hosted model store is a later stage; the server will own the
   file and enforce the policies. The DuckDB extension stays single-user.
10. **TLS by default off the host.** storage_pg gets rustls with
    certificate verification and refuses cleartext to anything but a
    socket or loopback unless `sslmode=disable` is explicit (as today).
    The server terminates TLS itself or sits behind a proxy.

Where this disagrees with the lead's proposal: decisions 4, 7 and 8 are
additions; `ReadItem` is a ninth action (item names need their own rule,
below); Role and Group get distinct meanings (below); the server keeps its
token scopes as an outer cap; and the 0.13 scope is smaller than the whole
goal (see [What 0.13 delivers](#what-013-delivers)).

## The schema

```cedar
namespace Improv {
  // Who. A User is one authenticated identity. A Role is asserted by the
  // identity source (Postgres role membership; later, a JWT claim). A
  // Group is defined with the model's policies, by its Admin.
  entity Role in [Role];
  entity Group in [Group];
  entity User in [Group, Role];

  // What. Ids are the model's numeric ids, as strings. A model's policies
  // apply to that model only; its entity is always Model::"model".
  entity Model;
  entity Category in [Model];
  entity Item in [Category];
  entity Measure in [Model] { kind: String };   // "input" | "derived"
  entity View in [Model] { measure: Measure };
  // One cell: a measure and one item per category of the measure. Its
  // parents are the measure and the items. It has no attributes.
  entity Cell in [Measure, Item];

  type Context = { channel: String };  // "local" | "postgres" | "server"

  action Read, Write, Build;           // groups, for policies to name
  action ViewModel in [Read] appliesTo {
    principal: User, resource: Model, context: Context };
  action ReadMeasure in [Read] appliesTo {
    principal: User, resource: Measure, context: Context };
  action ReadItem in [Read] appliesTo {
    principal: User, resource: Item, context: Context };
  action ReadCell in [Read] appliesTo {
    principal: User, resource: Cell, context: Context };
  action EditCell in [Write] appliesTo {
    principal: User, resource: Cell, context: Context };
  action DefineMeasure in [Build] appliesTo {
    principal: User, resource: [Model, Measure], context: Context };
  action EditStructure in [Build] appliesTo {
    principal: User, resource: [Model, Category, Item], context: Context };
  action ManageViews in [Build] appliesTo {
    principal: User, resource: [Model, View], context: Context };
  action Admin appliesTo {
    principal: User, resource: Model, context: Context };
}
```

**What each action allows.**

| Action | Resource | Allows |
|---|---|---|
| ViewModel | Model | Opening the model: its existence, its categories' names, its views' names. |
| ReadMeasure | Measure | The measure's name, type and dimensions; its formula when every measure the formula reads is readable too. |
| ReadItem | Item | The item's name as a row or column header, and its place in its category. |
| ReadCell | Cell | The cell's value. |
| EditCell | Cell | Setting or clearing an input cell. |
| DefineMeasure | Model (new) or Measure | Adding, redefining, formatting or dropping a measure. |
| EditStructure | Model, Category or Item | Adding, renaming, reordering or removing categories and items. |
| ManageViews | Model (new) or View | Saving, changing or dropping views. |
| Admin | Model | Setting and clearing the model's policies and groups. |

**The composite rules.** An enforcement point never asks Cedar one
question for a cell. It applies these rules, so a policy author cannot
forget a step:

- A cell is **visible** when ReadMeasure allows its measure, ReadItem
  allows every item of its coordinate, and ReadCell allows the cell.
- A cell is **editable** when it is visible, its measure is an input, and
  EditCell allows it. No blind writes.
- A formula is **shown** when its measure is readable and every measure it
  reads is readable. Otherwise the measure shows without its formula.
- Every action needs ViewModel first.

**Why ReadItem.** Without it, hiding an item's cells would still show its
name as an empty row ("Project Falcon"). Item names are often the secret.

**Why cells have no attributes.** A cell's decision can then depend only
on which policy-named entities it is `in`: its measure, and those of its
items that some policy names. Two cells whose items differ only in items
no policy names get the same decision. That makes the per-cell cache
below exact, and keeps per-cell cost to a few hash lookups. To keep it
exact, a policy may not name a Cell (`set` refuses `Improv::Cell::"…"`),
and the schema gives Cells no attributes.

**Groups and roles.** Roles come from whoever authenticated the user, and
a model's Admin cannot change them. Groups are stored with the model's
policies as Cedar entities JSON (`User in Group`, `Group in Group` edges
only), so a model Admin groups users without a DBA. A stored document that
names a Role, or gives a User a Role parent, is refused.

## Where each check happens

| Path | Enforcement point | What it checks |
|---|---|---|
| psql, or any client over storage_pg | pg_improv, in the backend | Reads of result tables (row-level security calling the policy), the filtered read functions, cell edits, `define`, structure changes, `set_policies`. |
| HTTP clients (web GUI, scripts) | improv-server | Every route, before and after the store: requests are authorized, responses filtered. |
| Desktop GUI, COSMIC GUI, TUI over a server or Postgres | the server or pg_improv | Nothing locally that matters for security. The client asks for capability hints to grey out what the user cannot do. |
| CLI, TUI, GUI on a local Mentat file | none | The file is the boundary: whoever can read the file reads everything. Policies stored in a file are enforced when improv-server serves it. `improv policy check` evaluates them for testing. |
| DuckDB extension (`improv(source, spec)`) | none | Stateless and single-user: it reads what the DuckDB connection reads. |

Client hints are never security. Every authority re-checks.

## Identity mapping

| Path | Cedar User | Cedar Roles | Cedar Groups |
|---|---|---|---|
| Postgres | `current_user` (so `SET ROLE` acts as that role, as Postgres privileges do) | every role whose privileges `current_user` has (`pg_has_role(current_user, r, 'USAGE')`) | the model's stored groups |
| improv-server, tokens | the token's name | none yet | the model's stored groups |
| improv-server, later OIDC/JWT | the `sub` claim | a configured claim (`roles`) | the model's stored groups |
| `improv policy check` | `--user` | `--role` (asserted by the tester) | the model's stored groups |

The server's seam: an `Authenticator` returns an `Identity { user, roles,
scope }`. The bearer-token one exists today. A JWT one validates a
signature against a JWKS and maps claims; nothing past the `Identity`
changes. A token's scope stays an outer cap: a read token never writes,
whatever the policy says.

When the server's store is Postgres, the server connects as one service
role that may act as the model's owner, and enforces the policies itself
for its callers. pg_improv sees only the service role. Per-request `SET
ROLE` is not planned.

## Threat model

**Protected.** The confidentiality of measures, items and cells a policy
hides from a principal, and the integrity of cells, formulas, structure,
views and policies a policy does not let the principal change.

**Against whom.** Authenticated users who are not the model's owner: an
analyst, a contractor, a dashboard token. They may bypass every GUI and
use psql, curl or their own code. Also a network attacker between a client
and Postgres or the server (TLS).

**Not protected, by design:**

- Superusers, the model's owner, roles that may act as the owner, and the
  server's operator. They see everything.
- Anyone who can read a Mentat or DuckDB file, the server's token file, or
  the server's process.
- The source tables. A model bound to `sales` reads `sales` as its owner.
  Cedar governs the model's view of it. A role that can `SELECT` from
  `sales` reads it directly whatever the model's policy says. The DBA's
  grants on source tables must match the intent.
- Inference. A visible total and visible parts reveal a hidden part (total
  minus the visible ones). A derived measure reveals the measure it is
  computed from (`Bonus = Salary * 0.1`). `improv policy lint` lists these;
  it does not prevent them.
- Existence and timing. A ViewModel holder sees the model, its category
  names, and when it changes (the change notification carries a version
  number only).
- Older releases. 0.12 opens a file with policies and ignores them. For a
  file this changes nothing (see above). For Postgres the policies are in
  the database, so an old client cannot get past them.

**Denial of service.** A policy document is capped at 1 MiB and 10,000
policies. Cedar's evaluation is linear in the policy set and has no
loops.

## TLS

**Postgres (storage_pg).**

- rustls, with the `ring` provider (already in the tree, no C toolchain).
  `postgres` 0.19 takes a `MakeTlsConnect`; `tokio-postgres-rustls` if its
  version lines up, else our own connector of about 80 lines.
- `sslmode`: `disable`, `prefer` (the default), `require`, `verify-ca`,
  `verify-full`. Improv parses `verify-ca` and `verify-full` itself, since
  `postgres` 0.19 does not.
- Improv verifies the certificate whenever it uses TLS. `require` behaves
  as `verify-full`; this is stricter than libpq and the docs say so.
  `sslrootcert=FILE` trusts that CA only. Without it, the operating
  system's roots (`rustls-platform-verifier`).
- Off the host (not a Unix socket, not loopback), `prefer` never falls
  back to cleartext. Only an explicit `sslmode=disable` sends cleartext,
  as today.
- With TLS, SCRAM uses channel binding (`channel_binding=prefer`, which
  `postgres` supports).

**improv-server.**

- Behind a proxy that terminates TLS, as `server.md` documents today
  (Caddy, nginx). The server listens on loopback.
- Or itself: `--tls-cert FILE --tls-key FILE` (rustls, `tokio-rustls`,
  HTTP/1.1 over ALPN). With both set, the server refuses a plain listener
  on a non-loopback address.
- Tokens still never travel in cleartext off the host: with tokens set
  and no TLS, the server binds loopback only unless `IMPROV_ALLOW_PLAIN=1`.

## Live updates

**Postgres.** pg_improv sends `NOTIFY improv_<model>, '<version>'` when
`improv.model_version` moves and when the sync worker commits result
changes. The payload is a number and nothing else: any role may `LISTEN`
on any channel. A client keeps one extra connection that listens, and on
a notification re-reads what it shows through the filtered read path. The
re-read applies the policy, so each client sees only what it may.

**Server.** `GET /v1/changes` is Server-Sent Events. The first version
sends `{"tx": N}` per change, and the client re-reads. The second sends
the changed cells, filtered per subscriber: the server already has the
engine's delta; it keeps one policy session per subscriber and drops cells
that session may not see. A change to the policies or the structure sends
`{"reload": true}`, and the client re-reads everything.

**Mentat files.** Today's high-water polling (H9) stays. With the server
in front, its feed replaces polling.

## The model-service contract

The `web` agent is building a model-service trait in `crates/client`
(`improv_client`). Under a policy, it needs an implementation that is
remote and filtered and never holds the full input set. What that implementation must guarantee:

1. **It never computes derived values locally** when the user is
   restricted. Every derived cell, total and what-if result comes from the
   authority. Local evaluation is for an unrestricted local model only.
2. **It holds only what the user may see.** Structure (categories, visible
   items in order, visible measures, formulas when shown, views) and the
   cells of the window on screen. No call returns the whole model.
3. **Hidden is absent.** Item lists, measure lists, cell pages and totals
   ("n cells", page counts) count visible things only. A hidden cell is
   not a null, an error or a blank.
4. **Reads are windowed.** `cells(measure, page items, row range, column
   range)` returns visible cells in that window, computed by the
   authority.
5. **What-ifs go to the authority.** `what_if(edits, measures, window)`
   returns the authority's results. The authority checks each edited cell
   as for EditCell, without saving.
6. **Edits are per cell.** `edit(cells, base_version)` succeeds whole or
   fails whole. A denied edit and an edit of a cell that does not exist
   for this user get the same error.
7. **Capabilities are hints.** `can(action, resource)` greys out menus
   and cells. Every call can still fail with `Denied`, and the GUI must
   handle that on every call.
8. **Changes arrive by subscription.** `subscribe()` yields changed cells
   (already filtered) or `Reload`. On `Reload` (the policy, the structure
   or the user's groups changed) the client drops its caches.
9. **Undo is inverse edits.** A filtered client cannot snapshot the whole
   model, so undo sends the inverse edit through `edit`.
10. **Errors do not leak.** Error text from the authority is shown as is;
    the client never adds what it knows about hidden things.

## Inference and aggregates

- A total is a cell of a derived measure whose dimensions leave out the
  category it sums over. It is computed by the authority over every item,
  hidden ones included.
- A policy on the total's measure (ReadMeasure) or its cells (ReadCell
  `in Improv::Measure::"…"`) hides it. A policy on items does not.
- "Sees aggregates but not some items" therefore means: ReadItem forbids
  those items, so their rows and cells are absent; the totals stay visible
  and correct. The analyst template does exactly that.
- What that reveals: with one hidden item, the total minus the visible
  items is the hidden value. With several hidden items, their sum. `improv
  policy lint --user U` reports, for a principal, each readable derived
  measure that reads a hidden measure or aggregates over a category with
  hidden items.
- Not offered: totals over visible items only (each user would see
  different numbers, and the engine would run per user), and small-cell
  suppression. Both are possible later as a per-measure option.

## Performance targets

Measured in a release build on the canonical model and recorded in the
stage that lands each piece.

| What | Target |
|---|---|
| One `authorize` (100 policies, small entity slice) | ≤ 20 µs median |
| One cell check with the class cache, amortized | ≤ 200 ns |
| A filtered read of a 1M-cell measure | ≤ 1.2× the unfiltered read |
| Building a policy session's model view at 100k items | ≤ 20 ms |
| Parsing and validating 1,000 policies | ≤ 50 ms |

**Caching.** A session is one (policy version, model structure version,
principal, channel). It caches decisions:

- per measure for ReadMeasure and DefineMeasure;
- per item class for ReadItem: an item a policy names is its own class,
  every other item of a category shares one;
- per (measure, item class per category) for ReadCell and EditCell.

So a cell check is a few hash lookups, and the number of real Cedar
evaluations is about the number of measures plus the number of
policy-named items. The cache is exact (see "Why cells have no
attributes"); a property test checks it against uncached evaluation. A
new policy version or structure version starts a new session. The server
keeps up to 1,024 sessions (least recently used); pg_improv keeps one per
backend.

Each evaluation passes Cedar only the entities it needs: the principal
and its groups and roles, the resource and its ancestors, and every entity
a policy names. Building one `Entities` for a 100k-item model per request
would cost more than the evaluation.

## Migration from 0.12's owner model

- Nothing changes until a model's Admin stores policies. Existing models,
  grants, RLS and token scopes behave as in 0.12.
- The owner (and anyone who may act as the owner, and superusers) is
  Admin by definition and is never filtered.
- In Postgres, other roles today read results only when the owner grants
  the result tables, and see every row. With policies, pg_improv adds a
  row-level security policy to each result table, so a grant shows only
  the rows the policy allows. Cell edits and structure changes by
  non-owners go through pg_improv functions that check the policy, then
  act as the owner (as `recalc` does). Cedar can let a non-owner do, to
  this model, at most what the owner may do.
- Clearing a model's policies (`improv policy clear`, an Admin action)
  returns it to 0.12's behavior.
- pg_improv's update script adds `improv.policy` (model, version, text,
  groups, set by, set at) and `improv.policy_audit`; it changes no
  existing row.
- A Mentat file gains four `:policy/*` attributes, installed on open like
  every other attribute. Older releases ignore them and their saves keep
  them.

## Test plan

- **Unit (`improv_policy`):** the schema parses; each action's resource
  types; default deny; forbid wins; an evaluation error denies; a Cell
  literal is refused; a stored Role is refused; each template validates
  and decides as documented; group nesting.
- **Deterministic:** the canonical Time × Product model with a fixed
  policy set for four principals (viewer, editor of Price, analyst
  without WidgetB, admin); every cell's decision is a known table.
- **Property (proptest):**
  - the class cache equals uncached Cedar on every cell, for random
    models, random permit/forbid sets over random literals and random
    principals;
  - adding a forbid never shows anything new; adding a permit never hides
    anything;
  - policy text round-trips through the store.
- **CLI integration:** `policy validate`, `set`, `show`, `check`; a file
  written with policies opens in v0.12.2's `improv`, which lists and
  saves it, and the policies survive.
- **Server (stage 2):** for every route, with a hidden measure, a hidden
  item and a hidden cell, the response bodies contain none of their ids,
  names or values; lists and counts leave them out; a hidden measure's
  404 is byte-for-byte the 404 of a measure that does not exist; denied
  writes are audited.
- **pg_improv (stage 3):** `tests/privileges.sh` gains a policy section:
  direct `SELECT` on a result table, the filtered functions, edits, and
  `define` as owner, analyst, editor and PUBLIC, on every PG major CI
  runs; the NOTIFY payload carries only the version.
- **Fuzz:** the stored policy document loader (text and groups JSON).
- **Performance:** the targets above, release build, recorded in the
  stage's commit.

## Stages

Each stage lands green on its own (fmt, clippy, tests, docs, CHANGELOG).

1. **The policy crate and the CLI** (this run). `crates/policy`: schema,
   entities from a `Model`, `authorize`, sessions with the class cache,
   templates, lint. Mentat `:policy/*` attributes. `improv policy
   validate|set|show|check|template|lint`. mdBook page `policies.md`, man
   pages, CHANGELOG.
2. **improv-server enforcement.** Principal mapping, groups from the
   model, every route authorized, responses filtered, the hidden-equals-
   missing 404, `GET /v1/capabilities` for GUI hints, denied requests
   audited, OpenAPI updated.
3. **pg_improv enforcement** (after the lead reviews this note).
   `improv.policy` and `improv.policy_audit`, `set_policies`, `policies`,
   `explain_access`, filtered read functions, RLS on result tables, edit
   and define for non-owners, the update script.
4. **TLS.** storage_pg rustls and sslmode; `--tls-cert`/`--tls-key` in
   the server.
5. **Filtered clients.** The model service's remote implementations: over
   the server's API and over pg_improv's filtered functions. The GUI, TUI
   and CLI use them for any restricted user; capability hints grey out
   the GUI.
6. **Live updates.** pg NOTIFY and LISTEN; the server's SSE feed, first
   versions only, then filtered deltas.
7. **DuckDB-hosted models.** A DuckDB model store (catalog tables in the
   DuckDB file, as `improv.*` in Postgres). improv-server owns the file,
   since DuckDB allows one writing process per file; it enforces the
   policies and serves the change feed. The DuckDB extension stays
   single-user; its README says so.

### What 0.13 delivers

Proposed: stages 1 to 4. That is Cedar policies stored with Mentat and
Postgres models, enforced by pg_improv for every Postgres client and by
improv-server for every HTTP client, and TLS on both paths. A restricted
user can then use psql, curl or the web API safely.

Later (0.14): stages 5 to 7. The desktop GUI, COSMIC GUI and TUI as
restricted clients with live updates, and DuckDB-hosted models editable
in real time through the server. Until then a restricted user's GUI over
Postgres shows nothing past what RLS already allows (today: no items or
cells of a model it may not act as the owner of).

## Open questions for the maintainer

1. Is `require` meaning `verify-full` acceptable? libpq's `require`
   encrypts without checking the certificate.
2. Should a model's Admin be able to grant Admin to others through a
   policy (yes in this design), or only the owner?
3. Do totals over visible items only (per-user aggregates) matter for
   1.0, or is the lint enough?
4. Should Mentat-file policies ever be enforced by the local GUI/TUI (as
   a convenience, not security), or only by the server? This design says
   only the server.
5. Release split: stages 1 to 4 in 0.13, and 5 to 7 in 0.14?
