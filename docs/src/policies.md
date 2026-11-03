# Access Policies

A model can carry **access policies**: rules, written in
[Cedar](https://www.cedarpolicy.com), that say who may see and who may
change which parts of it. A model with no policies is unrestricted, as
before. Once a model has policies, everything they do not permit is
denied, and a `forbid` always wins over a `permit`.

The policies are stored with the model and enforced where the model is
served. Today that is `improv policy check` (to test them) and the stored
model file. `improv-server` and pg_improv enforce them in the next
releases. A local model file has no protection of its own: whoever can
read the file can read all of it, whatever its policies say. The design
is in [the design note](https://codeberg.org/gregburd/improv/src/branch/main/docs/reviews/2026-11-02-cedar-policies.md).

## A first policy

Start from a template. `improv policy template` prints policy text that you
can store as it is or edit first:

```sh
improv policy template sales.db viewer --group staff > policies.cedar
improv policy template sales.db analyst --user ann --item Product=Falcon >> policies.cedar
improv policy template sales.db editor --user ed --measure Price >> policies.cedar
echo '{"staff": {"users": ["vic", "ann"]}}' > groups.json
improv policy validate sales.db policies.cedar --groups groups.json
improv policy set sales.db policies.cedar --groups groups.json
```

`validate` checks the text against Improv's schema and the model, and
stores nothing. `set` does the same and stores the policies, replacing any
stored before. Each `set` and each `clear` moves the policies' version on.

```sh
improv policy show sales.db            # the stored text, with its version
improv policy show sales.db --groups   # the stored groups
improv policy clear sales.db           # unrestricted again
```

## Asking a question

`improv policy check` asks whether a user may do an action and names the
policies that decided. It exits 0 on allow and 1 on deny, so scripts can
test a policy set:

```text
$ improv policy check sales.db --user ann ReadCell --measure Quantity --at Time=2025 Product=Falcon
deny: ann ReadCell cell measure Quantity (101)[Time=2025, Product=Falcon]
  allow ViewModel the model by viewer:group:staff
  allow ReadMeasure measure Quantity (101) by viewer:group:staff
  allow ReadItem item 2025 (10) by viewer:group:staff
  deny  ReadItem item Falcon (21) by analyst:user:ann
```

The resource is the model (no flag), `--measure`, `--item Cat=Item`,
`--category`, `--view`, or a cell (`--measure` with `--at`). `--role`
gives the user a role, and `--policies FILE` checks a file instead of the
stored policies.

## What the actions allow

| Action | On | Allows |
|---|---|---|
| `ViewModel` | the model | Opening it: its category names and view names. Every other action needs it too. |
| `ReadMeasure` | a measure | Its name, type and dimensions. Its formula too, when every measure the formula reads is readable. |
| `ReadItem` | an item | Its name as a row or column, and its place in its category. |
| `ReadCell` | a cell | Its value. |
| `EditCell` | a cell | Setting or clearing an input cell. |
| `DefineMeasure` | the model, or a measure | Adding, redefining, formatting or dropping measures. |
| `EditStructure` | the model, a category or an item | Adding, renaming, reordering and removing categories and items. |
| `ManageViews` | the model, or a view | Saving, changing and dropping views. |
| `Admin` | the model | Setting its policies and groups. |

`Read` groups the four read actions, `Write` holds `EditCell`, and `Build`
groups the three structure actions, so a policy can say `action in
Improv::Action::"Read"`.

The rules apply together. A cell is visible only when its measure, every
item of its coordinate and the cell itself are readable. A cell is
editable only when it is visible, its measure is an input, and `EditCell`
allows it. A derived measure's cells are never edited.

## Writing policies

Policies name the model's objects **by id**, the numbers `improv list`
prints: `Improv::Measure::"101"`, `Improv::Item::"21"`,
`Improv::Category::"2"`, `Improv::View::"1"`. The model is always
`Improv::Model::"model"`. Ids do not change when a measure is renamed, so
a forbid stays on what it named. `validate` and `set` warn when a policy
names an id the model lacks, which usually means a name was written where
an id belongs.

```cedar
// Everyone in "finance", and every user with the role "auditor", reads it all.
permit (principal in Improv::Group::"finance", action in Improv::Action::"Read", resource);
permit (principal in Improv::Role::"auditor", action in Improv::Action::"Read", resource);

// Nobody but the treasurer sees Salary (measure 105) or its cells.
forbid (principal, action in Improv::Action::"Read", resource in Improv::Measure::"105")
unless { principal == Improv::User::"treasurer" };

// Input measures only, and only through the server.
permit (principal, action == Improv::Action::"ReadMeasure", resource)
when { resource.kind == "input" && context.channel == "server" };
```

A cell is `in` its measure and in each of its items, so `resource in
Improv::Measure::"105"` covers the measure and all its cells. A measure
has a `kind`, `"input"` or `"derived"`. `context.channel` is `"local"`,
`"server"` or `"postgres"`. A policy may not name a cell directly, or use
template slots (`?principal`).

**Users, roles and groups.** A user is the name the user signs in with.
Roles come from the identity source: Postgres role memberships, or later
a sign-in token's claims. Groups are stored with the policies, in a JSON
file that `set --groups` takes. A group lists its users and the groups
inside it:

```json
{"finance": {"users": ["fay"], "groups": ["treasury"]},
 "treasury": {"users": ["tom"]}}
```

An evaluation error, such as an integer overflow in a `when` clause,
denies the request even when the failing policy is a `forbid`.

## Templates

| Template | Takes | Gives |
|---|---|---|
| `viewer` | | Every read action on everything. |
| `editor` | `--measure M`… | `viewer`, plus editing the cells of those input measures. |
| `analyst` | `--item Cat=Item`… | `viewer`, except those items: their rows and cells are absent. Totals over them stay. |
| `hide-measures` | `--measure M`… | A forbid on reading those measures and their cells. Add it to another template. |
| `admin` | | Every action. |

Each takes `--user U` or `--group G`, and each policy's `@id` names the
template and the principal (`analyst:user:ann`), which is what `check`
prints.

## Totals and what they reveal

A total is computed over all of its items, hidden ones included, and is
shown with that value or not at all. It is never recomputed over the
items a user can see.

- Under `analyst`, totals over the hidden items stay visible, with their
  full values. The hidden items' rows and cells are absent.
- Under `hide-measures`, a derived measure that reads a hidden measure
  stays visible, without its formula.
- To hide a total too, forbid its measure or its cells.

So a user who sees a total and all but one of its parts can work out the
last part. `improv policy lint` lists, for one user, each readable measure
that reads something hidden from them:

```text
$ improv policy lint sales.db --user ann
measure Total (103) totals over category Product (2), including Falcon, hidden from ann: the total less the visible items is their sum
```

## Compatibility

The policies live in the model file beside the model, not in it. A model
save does not touch them. Releases before 0.13 open a file with policies,
ignore them, and keep them when they save. A Postgres model's policies
will be kept by pg_improv; `improv policy set` refuses one until then.
