//! Property tests for the coordinate <-> dataflow-key codec.
//!
//! * `encode_coord(decode_coord(k)) == k` and `decode(encode(c)) == c` for
//!   arbitrary well-formed keys (sorted, unique category ids).
//! * `project_key` keeps exactly the requested categories and no others.

use improv_engine::{decode_coord, encode_coord, project_key, CoordKey};
use proptest::prelude::*;

/// A well-formed CoordKey: sorted by category id, with unique category ids.
/// This matches `encode_coord`'s output (a `BTreeMap` iterated in order), so it
/// is the correct domain for the round-trip property.
fn arb_coord_key() -> impl Strategy<Value = CoordKey> {
    // Unique category ids, then attach an arbitrary item id to each.
    (
        prop::collection::hash_set(0u32..100, 0..8),
        prop::collection::vec(0u32..1000, 8),
    )
        .prop_map(|(cats, items)| {
            let mut k: CoordKey = cats.into_iter().zip(items).collect();
            k.sort();
            k
        })
}

proptest! {
    #[test]
    fn decode_encode_round_trip(k in arb_coord_key()) {
        // k -> Coordinate -> k must be the identity on well-formed keys.
        let c = decode_coord(&k);
        prop_assert_eq!(encode_coord(&c), k);
    }

    #[test]
    fn encode_decode_round_trip(k in arb_coord_key()) {
        // Coordinate -> k -> Coordinate is the identity.
        let c = decode_coord(&k);
        let round = decode_coord(&encode_coord(&c));
        prop_assert_eq!(c, round);
    }

    /// project_key retains exactly the pairs whose category is in `keep`.
    #[test]
    fn project_keeps_requested_categories(
        k in arb_coord_key(),
        keep in prop::collection::vec(0u32..100, 0..6),
    ) {
        let projected = project_key(&k, &keep);
        // Every projected pair's category is in keep and was in the original.
        for &(cat, item) in &projected {
            prop_assert!(keep.contains(&cat));
            prop_assert!(k.contains(&(cat, item)));
        }
        // Every original pair with a kept category survives (nothing dropped
        // that should stay). Since categories are unique, this is exact.
        for &(cat, item) in &k {
            if keep.contains(&cat) {
                prop_assert!(projected.contains(&(cat, item)));
            }
        }
        // Result stays sorted / a subsequence: projection preserves order.
        prop_assert!(projected.windows(2).all(|w| w[0] <= w[1]));
    }
}
