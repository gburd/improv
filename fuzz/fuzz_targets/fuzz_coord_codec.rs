//! Fuzz the coordinate codec: derive a CoordKey from arbitrary bytes, then
//! assert encode/decode never panic and round-trip.
//!
//! Round-trip identity (`encode(decode(k)) == k`) only holds for well-formed
//! keys: sorted by category id, one entry per category. `encode_coord` always
//! emits such keys (it iterates a BTreeMap), so we normalize the arbitrary key
//! to that canonical form before asserting the round trip.
#![no_main]

use improv_engine::{decode_coord, encode_coord, project_key, CoordKey};
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeMap;

fuzz_target!(|raw: CoordKey| {
    // Canonicalize: dedup by category (last wins), sorted -> matches encode's
    // output domain.
    let canon: CoordKey = raw
        .iter()
        .copied()
        .collect::<BTreeMap<u32, u32>>()
        .into_iter()
        .collect();

    let coord = decode_coord(&canon);
    let reencoded = encode_coord(&coord);
    assert_eq!(reencoded, canon, "encode(decode(k)) == k for canonical k");

    // decode(encode(c)) == c as well.
    let round = decode_coord(&reencoded);
    assert_eq!(round, coord);

    // project_key must not panic and must yield a subset of the input.
    let keep: Vec<u32> = canon.iter().map(|(c, _)| *c).take(2).collect();
    let projected = project_key(&canon, &keep);
    for pair in &projected {
        assert!(canon.contains(pair));
    }
});
