//! Embedding-precision rollout — runtime helpers (PR 7+ of
//! `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc`).
//!
//! PR 7 lands the hardware capability probe; later PRs add boundary +
//! query downconvert (Q10, Q15) and PQ-codebook lifecycle (PR 10).

pub mod boundary;
pub mod hw_capability;
pub mod query_downconvert;
