#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

// The UniFFI bindings generator entrypoint: a build-time codegen tool, not
// part of the shipped library, so it carries no runtime coverage.
#[cfg_attr(coverage_nightly, coverage(off))]
fn main() {
    uniffi::uniffi_bindgen_main()
}
