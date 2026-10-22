//! The scale gate's reference probe (`scale-gate.py`): a fixed workload that
//! uses none of Improv's code, so its time measures only the machine and the
//! compiler. Each probe's time is divided by it, which cancels most of the
//! difference between one runner and another. The mix is the engine's kind
//! of work: ordered maps with cache-missing lookups, sorting, hash
//! aggregation, and many small allocations.
//! Build: rustc -C opt-level=3 --edition 2021 reference.rs; prints `ms <ms>`.
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

fn lcg(s: &mut u64) -> u64 {
    *s = s
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *s >> 11
}

fn main() {
    let t = Instant::now();
    let mut s = 42u64;
    let mut check = 0u64;
    // Ordered map: 1M random keys (larger than the L2), then 2M lookups.
    let mut m = BTreeMap::new();
    for i in 0..1_000_000u64 {
        m.insert(lcg(&mut s) % 4_000_000, i);
    }
    for _ in 0..2_000_000 {
        if let Some(v) = m.get(&(lcg(&mut s) % 4_000_000)) {
            check = check.wrapping_add(*v);
        }
    }
    drop(m);
    // Sort 4M (key, value) pairs.
    let mut v: Vec<(u64, f64)> = (0..4_000_000).map(|i| (lcg(&mut s), i as f64)).collect();
    v.sort_unstable_by_key(|p| p.0);
    check = check.wrapping_add(v[v.len() / 2].0);
    // Hash aggregation: 3M rows into 50k groups.
    let mut g: HashMap<u64, f64> = HashMap::new();
    for (k, x) in v.iter().take(3_000_000) {
        *g.entry(k % 50_000).or_insert(0.0) += x;
    }
    check = check.wrapping_add(g.len() as u64);
    drop(v);
    // Small allocations: 1M short strings, formatted and dropped.
    let mut names: Vec<String> = Vec::with_capacity(1_000_000);
    for i in 0..1_000_000 {
        names.push(format!("item{i}"));
    }
    check = check.wrapping_add(names.iter().map(|n| n.len() as u64).sum::<u64>());
    let ms = t.elapsed().as_secs_f64() * 1e3;
    // The checksum keeps the optimizer from dropping the work.
    eprintln!("checksum {check}");
    println!("ms {ms:.3}");
}
