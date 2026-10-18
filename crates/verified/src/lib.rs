//! Small, pure integer kernels of the Improv engine, with Verus specifications
//! and machine-checked proofs.
//!
//! Each kernel lives in a `verus! { ... }` block: a `spec fn` says what it
//! means, the executable function `ensures` it, and `cargo-verus verify -p
//! improv_verified` (see `scripts/verify.sh`) checks the proof. Under plain
//! cargo the specs and proofs are erased and the functions are ordinary Rust,
//! which is what the engine calls: the proven code is the code that runs.
//!
//! Floating-point arithmetic is out of scope (Verus does not reason about
//! `f64`), so every kernel takes and returns integers; the engine does the
//! float range checks and truncation before calling in.

pub mod bits;
pub mod gcd;
pub mod keys;
pub mod radix;
