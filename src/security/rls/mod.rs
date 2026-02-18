//! Row-Level Security (RLS) for ProximaDB
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

pub mod policy;
pub mod service;

pub use policy::{
    Operation, RLSPolicy, RLSPolicyBuilder, SecurityPredicate, SecurityPredicateBuilder,
};
pub use service::{CollectionRLS, RLSConfig, RLSFilterResult};
