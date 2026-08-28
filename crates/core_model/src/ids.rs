//! Opaque, stable identifiers and human-readable names.

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        pub struct $name(pub u32);
    };
}

id_type!(
    /// Identifies a category (a model dimension).
    CategoryId
);
id_type!(
    /// Identifies an item (a member of a category).
    ItemId
);
id_type!(
    /// Identifies a measure (an input or derived variable).
    MeasureId
);
id_type!(
    /// Identifies a saved view (pivot/grid layout).
    ViewId
);
id_type!(
    /// Identifies a what-if scenario (an input-override overlay).
    ScenarioId
);

/// A human-readable name. Unique within its scope; renamable without breaking
/// formulas (formulas bind to ids, not names).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Name(pub String);

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Name(s.to_string())
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
