//! The Improv-on-Mentat datom schema, as an EDN transaction.
//!
//! Mirrors IMPROV.txt "Mentat schema (steering version)".

pub const SCHEMA_EDN: &str = r#"[
 ;; CATEGORY
 {:db/ident :category/id
  :db/valueType :db.type/long
  :db/cardinality :db.cardinality/one
  :db/unique :db.unique/identity
  :db/index true}
 {:db/ident :category/name
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}

 ;; ITEM
 {:db/ident :item/id
  :db/valueType :db.type/long
  :db/cardinality :db.cardinality/one
  :db/unique :db.unique/identity
  :db/index true}
 {:db/ident :item/name
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}
 {:db/ident :item/category
  :db/valueType :db.type/ref
  :db/cardinality :db.cardinality/one}

 ;; MEASURE
 {:db/ident :measure/id
  :db/valueType :db.type/long
  :db/cardinality :db.cardinality/one
  :db/unique :db.unique/identity
  :db/index true}
 {:db/ident :measure/name
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}
 {:db/ident :measure/value-type
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}
 {:db/ident :measure/categories
  :db/valueType :db.type/ref
  :db/cardinality :db.cardinality/many}
 {:db/ident :measure/kind
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}
 {:db/ident :measure/description
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}
 {:db/ident :measure/formula
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}
 {:db/ident :measure/sql-source
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}

 ;; CELL (input value). Keyed by a synthetic unique string measure+coord.
 {:db/ident :cell/key
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one
  :db/unique :db.unique/identity
  :db/index true}
 {:db/ident :cell/measure
  :db/valueType :db.type/ref
  :db/cardinality :db.cardinality/one}
 {:db/ident :cell/coord
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}
 {:db/ident :cell/value-number
  :db/valueType :db.type/double
  :db/cardinality :db.cardinality/one}
 {:db/ident :cell/value-boolean
  :db/valueType :db.type/boolean
  :db/cardinality :db.cardinality/one}
 {:db/ident :cell/value-text
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}

 ;; VIEW (saved pivot layout). Stored whole as a JSON blob, keyed by id.
 {:db/ident :view/id
  :db/valueType :db.type/long
  :db/cardinality :db.cardinality/one
  :db/unique :db.unique/identity
  :db/index true}
 {:db/ident :view/json
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}

 ;; MODEL META (singleton). JSON blobs for maps without a natural per-entity
 ;; datom shape: external function defs and external-call measures.
 {:db/ident :meta/singleton
  :db/valueType :db.type/long
  :db/cardinality :db.cardinality/one
  :db/unique :db.unique/identity
  :db/index true}
 {:db/ident :meta/external-fns
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}
 {:db/ident :meta/external-calls
  :db/valueType :db.type/string
  :db/cardinality :db.cardinality/one}
]"#;
