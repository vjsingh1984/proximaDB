//! # Catalog Federation Module — FRAMEWORK ONLY, NOT WIRED
//!
//! This header said "PRODUCTION READY". It is not, and was not: [`ExternalCatalog`]
//! has **zero implementations** and [`FederatedCatalog`] has **zero construction
//! sites** anywhere in the tree. Meanwhile the five external adapters that should
//! implement it (`hive`, `glue`, `unity`, `polaris`, `iceberg`) implement the
//! internal `Catalog` trait instead — the one that demands identity minting they
//! cannot honour. Two halves of a federation feature, neither connected to the
//! other, neither reachable at runtime.
//!
//! Connecting them is tracked in TD-CAT-8. Until then, treat everything below as
//! a design, not a capability. A doc claiming a capability the code does not have
//! is the same defect as a silent fallback: a reader cannot tell working from
//! aspirational.
//!
//! The framework provides, once wired:
//! - Seamless table discovery across ProximaDB internal and external sources
//! - Constraint support awareness per catalog/format type
//! - Cross-catalog query resolution
//! - External catalog registration (Iceberg, Delta, Hive, etc.)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        FEDERATED CATALOG                                 │
//! │  ┌─────────────────────────────────────────────────────────────────────┐│
//! │  │                    Unified Table Resolution                         ││
//! │  │  catalog.namespace.table → resolve to internal or external          ││
//! │  └─────────────────────────────────────────────────────────────────────┘│
//! │                                  │                                       │
//! │           ┌──────────────────────┼──────────────────────┐               │
//! │           ▼                      ▼                      ▼               │
//! │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐         │
//! │  │    INTERNAL     │  │   ICEBERG       │  │    DELTA        │         │
//! │  │  Full Constraints│  │ Partial Support │  │ Partial Support │         │
//! │  │  PK/FK/UNIQUE/  │  │  PK, NOT NULL   │  │  PK, NOT NULL   │         │
//! │  │  CHECK/NOT NULL │  │                 │  │                 │         │
//! │  └─────────────────┘  └─────────────────┘  └─────────────────┘         │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

pub mod external;
pub mod federated_catalog;

// Re-exports
pub use external::{ExternalCatalog, ExternalCatalogConfig, ExternalCatalogType};
pub use federated_catalog::{
    ConstraintSupport, FederatedCatalog, FederatedCatalogConfig, FederatedTableInfo,
};
