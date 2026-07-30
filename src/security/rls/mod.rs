//! Row-Level Security (RLS) and ABAC row-level enforcement for ProximaDB.
//!
//! ## ABAC enforcement substrate (TD-FOUNDATION-3, behind `abac-policy`)
//!
//! The [`abac_adapter`] module provides the `AbacEnforcer` — the service-facing
//! API that resolves a subject's authorization, compiles it to a
//! `FilterExpression` or a per-row predicate, and feeds it to the read
//! primitives (`scan_records_filtered`, `unified_search_native`). The substrate
//! (crates/control/proximadb-abac) provides the attribute authority, policy-epoch
//! cache keying, and the non-optional `AuthorizedReadContext`.
//!
//! ## Legacy RLS (inert on the read path)
//!
//! Implements collection-level data isolation through security predicates
//! that filter records based on user context. RLS policies are evaluated
//! at query time and converted to metadata filters.
//!
//! ## Features
//! - Owner-based isolation (users see only their own data)
//! - Role-based access control with metadata field matching
//! - Department/tenant isolation
//! - Time-based access expiry
//! - Composite predicates (AND/OR combinations)
//!
//! ## Integration
//! RLS integrates with the collection service to automatically apply
//! security filters before search operations reach the storage engine.

pub mod filter_lattice;
pub mod policy;
pub mod service;

pub use filter_lattice::{SecurityFilter, unsatisfiable_expression};
pub use policy::{
    Operation, RLSPolicy, RLSPolicyBuilder, SecurityPredicate, SecurityPredicateBuilder,
};
pub use service::{CollectionRLS, RLSConfig, RLSFilterResult};

#[cfg(feature = "abac-policy")]
mod abac_adapter;
// FA-c Phase 2: the service-facing enforcement API, re-exported so the
// read-serving services (DmlService, …) can name the enforcer without depending
// on the private adapter module's internals.
#[cfg(feature = "abac-policy")]
pub use abac_adapter::{AbacEnforcer, AbacScanResult};

#[cfg(all(test, feature = "abac-policy"))]
mod abac_enforcement_tests;
