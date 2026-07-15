//! `proximablocks` hermetic sub-leaves hoisted from the root crate
//! (root-crate decomposition, engines extraction). These files have no `crate::`
//! deps (only foundation/horizontal crates), so they move verbatim. Root keeps
//! per-file re-export shims.

pub mod header_metadata;
pub mod per_column_alignment;
