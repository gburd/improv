# Storage & Persistence

`improv_storage_mentat` persists a `Model` to an embedded, SQLite-backed
[Mentat](https://codeberg.org/gregburd/mentat) store — a Datomic-style datom
database. Categories, items, measures, and input cells are stored as
first-class entities, so the model is **queryable**, not an opaque blob.

`ModelStore::open(path)` opens (or creates) a store; pass `""` for an in-memory
database. Opening installs the schema. `save_model` writes the whole model;
`load_model` reconstructs it by querying.

## The datom schema

The schema is defined as an EDN transaction in
`improv_storage_mentat::schema::SCHEMA_EDN`. It declares these attributes.

### Category

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:category/id` | `long` | one | `unique/identity`, indexed |
| `:category/name` | `string` | one | |

### Item

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:item/id` | `long` | one | `unique/identity`, indexed |
| `:item/name` | `string` | one | |
| `:item/category` | `ref` | one | reference to a category entity |

### Measure

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:measure/id` | `long` | one | `unique/identity`, indexed |
| `:measure/name` | `string` | one | |
| `:measure/value-type` | `string` | one | the declared value type |
| `:measure/categories` | `ref` | **many** | the measure's tensor dimensions |
| `:measure/kind` | `string` | one | `"input"` or `"derived"` |
| `:measure/description` | `string` | one | optional |
| `:measure/formula` | `string` | one | JSON-serialized `Formula`, on derived measures |

### Cell (an input value)

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:cell/key` | `string` | one | `unique/identity`, indexed — synthetic `measure::coord` key |
| `:cell/measure` | `ref` | one | reference to the owning measure |
| `:cell/coord` | `string` | one | JSON-serialized `Coordinate` |
| `:cell/value-number` | `double` | one | present for numeric cells |
| `:cell/value-boolean` | `boolean` | one | present for boolean cells |
| `:cell/value-text` | `string` | one | present for text cells |

## How a model maps to datoms

- **Native attributes.** Ids, names, kinds, and value types map straight to
  native datom value types (`long`, `string`, `boolean`, `double`, `ref`).
- **Formulas and coordinates are JSON.** A `Formula` AST and a `Coordinate` are
  serialized with serde to JSON strings and stored in `:measure/formula` and
  `:cell/coord`. This is why a derived measure's formula survives a round trip
  even though the datom layer has no notion of an expression tree.
- **Typed cell values.** A cell writes exactly one of the three typed value
  attributes according to its measure's declared type. On load, the store looks
  up the owning measure's type and reads back the matching column. (`Enum` maps
  onto the numeric column, `DateTime` onto the text column.)
- **Cell identity.** Each cell is keyed by a synthetic string
  `"<measure-id>::<coord-json>"` on `:cell/key`, which is `unique/identity`.
  Re-saving the same measure+coordinate updates the cell in place.

## Save order and idempotency

`save_model` transacts in dependency order — categories, then items, then
measures, then cells — as **separate** transactions, so that lookup-refs
(for example `:item/category`, `:measure/categories`, `:cell/measure`) resolve
against already-committed entities.

Because every top-level entity is keyed by a `unique/identity` attribute
(categories/items/measures by their id, cells by their `measure::coord` key),
re-saving a model is idempotent: existing entities are updated in place rather
than duplicated.

## Load

`load_model` runs one Datalog query per entity kind against the store and
rebuilds the `Model`:

- categories and items (relinking each item to its category),
- measures (fetching the optional formula and description per measure, and the
  many-valued category set), and
- cells (reading each cell's coordinate, then its typed value by the owning
  measure's declared type).
