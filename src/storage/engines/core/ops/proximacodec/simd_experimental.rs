//! Experimental SIMD codec wiring.
//! Enabled via `--features simd-experimental`.
//! We include the archived prototype directly so it can be exercised without
//! renaming the original backup file.

// SAFETY: this is opt-in and sourced from the archived prototype.
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/storage/engines/core/ops/proximacodec/archive/simd.rs.bak2"
));
