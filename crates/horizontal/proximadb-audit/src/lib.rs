//! ProximaDB Audit — audit correlation analysis + cross-tenant guard.
//! Extracted from root src/audit/ (TD-DECOMP-16). logger.rs + storage.rs
//! stay root-side (depend on root storage/auth types).

pub mod correlation;
pub mod cross_tenant_guard;
pub mod types;
