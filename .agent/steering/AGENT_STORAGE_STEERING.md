# AGENT_STORAGE_STEERING.md
Storage Steering Document for Improv (Mentat Datom Store)

## 1. Purpose

This document defines the storage architecture for Improv (`improv_storage_mentat`):

- The embedded Mentat datom store as the persistence substrate
- The canonical Mentat schema (categories, items, measures, cells)
- Coordinate and formula serialization
- Save / load round-trip
- Integration with the engine and interfaces
- Performance considerations

This is the authoritative guide for all storage development.

---

## 2. Substrate: the Embedded Mentat Fork

Improv persists models to the **embedded SQLite Mentat fork** — a Datomic-style
datom store — **not** to EDN files, JSON files, or an external database. This is
a hard requirement from the source design.

- Dependency: `mentat = { path = "../mentat" }`, a **sibling path dependency**.
- CI clones it next to the checkout from
  `https://codeberg.org/gregburd/mentat.git` at branch **`improv-base`** (env
  `MENTAT_REPO` / `MENTAT_REF`). When Improv needs newer Mentat behavior, commit
  and push it to `improv-base` in the mentat repo first, then bump the ref here.
- Mentat is backed by SQLite, giving durable, transactional, cross-platform
  persistence for free.

### 2.1 Why Datoms

A datom is a single immutable fact `[entity attribute value tx]`. Storing the
model as datoms yields:

- **Facts, not mutable state** — an append-only, auditable record
- **Queryable structure** — the model can be reconstructed by query
- **Transactional writes** — Mentat/SQLite provide ACID guarantees
- **History** — the transaction log preserves how the model evolved

---

## 3. Canonical Mentat Schema

The schema below is the authoritative storage model. Attributes use Mentat's
`:db/ident`, `:db/valueType`, `:db/cardinality`, and `:db/unique`.

### 3.1 Category

```clojure
{:db/ident :category/id
 :db/valueType :db.type/long
 :db/cardinality :db.cardinality/one
 :db/unique :db.unique/identity}

{:db/ident :category/name
 :db/valueType :db.type/string
 :db/cardinality :db.cardinality/one}
```

### 3.2 Item

```clojure
{:db/ident :item/id
 :db/valueType :db.type/long
 :db/cardinality :db.cardinality/one
 :db/unique :db.unique/identity}

{:db/ident :item/name
 :db/valueType :db.type/string
 :db/cardinality :db.cardinality/one}

{:db/ident :item/category
 :db/valueType :db.type/ref
 :db/cardinality :db.cardinality/one}
```

### 3.3 Measure

```clojure
{:db/ident :measure/id
 :db/valueType :db.type/long
 :db/cardinality :db.cardinality/one
 :db/unique :db.unique/identity}

{:db/ident :measure/name
 :db/valueType :db.type/string
 :db/cardinality :db.cardinality/one}

{:db/ident :measure/value-type
 :db/valueType :db.type/keyword
 :db/cardinality :db.cardinality/one}
;; :value-type/number | :value-type/boolean | :value-type/text
;;   | :value-type/datetime | :value-type/enum

{:db/ident :measure/categories
 :db/valueType :db.type/ref
 :db/cardinality :db.cardinality/many}
;; refs to category entities — the measure's dimensionality

{:db/ident :measure/kind
 :db/valueType :db.type/keyword
 :db/cardinality :db.cardinality/one}
;; :measure-kind/input | :measure-kind/derived

{:db/ident :measure/description
 :db/valueType :db.type/string
 :db/cardinality :db.cardinality/one}

{:db/ident :measure/formula
 :db/valueType :db.type/string
 :db/cardinality :db.cardinality/one}
;; serialized formula AST (see §5)
```

### 3.4 Cell (input value)

```clojure
{:db/ident :cell/measure
 :db/valueType :db.type/ref
 :db/cardinality :db.cardinality/one}

{:db/ident :cell/coord
 :db/valueType :db.type/string
 :db/cardinality :db.cardinality/one}
;; serialized Coordinate (see §4)

;; one value attribute per cell, chosen by :measure/value-type
{:db/ident :cell/value-number   :db/valueType :db.type/double  :db/cardinality :db.cardinality/one}
{:db/ident :cell/value-boolean  :db/valueType :db.type/boolean :db/cardinality :db.cardinality/one}
{:db/ident :cell/value-text     :db/valueType :db.type/string  :db/cardinality :db.cardinality/one}
{:db/ident :cell/value-datetime :db/valueType :db.type/instant :db/cardinality :db.cardinality/one}
{:db/ident :cell/value-enum     :db/valueType :db.type/long    :db/cardinality :db.cardinality/one}

{:db/ident :cell/error-kind
 :db/valueType :db.type/keyword
 :db/cardinality :db.cardinality/one}

{:db/ident :cell/error-source-measure
 :db/valueType :db.type/ref
 :db/cardinality :db.cardinality/one}
```

Keep one value attribute per cell based on `:measure/value-type`, and enforce
that consistency in code.

> **Only input cells are stored.** Derived values are never persisted; the
> engine recomputes them from formulas + inputs on load. A `View` entity type is
> a documented later addition, not part of the v1 schema.

---

## 4. Coordinate Serialization

`:cell/coord` stores a stable JSON string. The name-based form is canonical for
readability and external tooling:

```json
{ "Time": "2025", "Product": "Widget A" }
```

On load, resolve category/item **names** back to their ids. (An id-based form —
`{"category_ids": [1,2], "item_ids": [10,20]}` — is acceptable internally but
less debuggable.)

---

## 5. Formula Serialization

`:measure/formula` stores the serialized `Formula` AST as JSON, using measure and
category **names** (resolved to ids on load):

```json
{
  "expr": {
    "BinaryOp": {
      "op": "Mul",
      "left":  { "Ref": { "measure": "Price",    "dim": { "by": ["Product"] } } },
      "right": { "Ref": { "measure": "Quantity", "dim": { "by": ["Time", "Product"] } } }
    }
  }
}
```

The serialized format must round-trip losslessly with the in-memory
`core_model::Formula` (a tested property).

---

## 6. Save / Load

### 6.1 Load

`MentatStore::load_model()`:

1. Query categories, items, measures, cells
2. Reconstruct `core_model::Model` (resolve coord/formula names → ids)
3. Hand the model to the engine to build its collections

### 6.2 Save

`MentatStore::save_model(&model)`:

1. Serialize categories, items, measures, and input cells to datoms
2. Transact them into Mentat (atomic)

### 6.3 Autosave

Interfaces (TUI, later GUI) autosave via Mentat transactions on edit. Because
writes are datoms in a transaction log, saves are naturally incremental and
transactional.

---

## 7. Integration

### 7.1 With the Engine

Storage provides the loaded `Model`; the engine owns all derived computation.
Storage never stores derived values.

### 7.2 With Interfaces

- **CLI** reads/writes categories, items, measures, and cells over a
  Mentat-backed store (`init`, `add-category`, `add-item`, `add-measure`, `set`,
  `list`, `show`, `export`).
- **TUI / server** load a model, evaluate, and persist edits back.

---

## 8. Export Formats (interface-level)

Distinct from the canonical Mentat persistence, interfaces may **export** a
computed view for interchange (e.g. CSV). Export is a read-only projection of
engine output, not a storage backend. Additional export targets (Parquet, JSON)
are optional future conveniences.

---

## 9. Performance Considerations

- **Load** — query only what is needed; resolve names once and cache id maps.
- **Save** — rely on Mentat transactions; only changed datoms are written.
- **Memory** — intern category and measure names shared across many cells.

---

## 10. Definition of Success

Storage succeeds when models save and reopen faithfully across Linux, macOS, and
Windows; the save↔load round-trip is lossless (tested); and the on-disk record
is a durable, queryable datom store.

---

## 11. Document Index

Part of the full steering set:

- `AGENT_MASTER_STEERING.md`
- `AGENT_GUI_STEERING.md`
- `AGENT_ENGINE_STEERING.md`
- `AGENT_STORAGE_STEERING.md`
- `AGENT_FORMULA_LANGUAGE.md`
- `AGENT_DATABASE_CONNECTIVITY.md`
- `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md`
- `STEERING_SYSTEM_OVERVIEW.md`
