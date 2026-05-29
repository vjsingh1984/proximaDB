//! Discovery refinement passes (Phase 8 F1).
//!
//! `dedup` is the keystone pass (S3). `recluster` / `re_embed` / `quality` /
//! `trajectory` are later-phase stubs handled as identity passes by the
//! executor until implemented.

pub mod dedup;
