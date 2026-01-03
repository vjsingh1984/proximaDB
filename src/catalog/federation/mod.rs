//! # Catalog Federation Module - PRODUCTION READY
//!
//! Provides a unified view across internal and external catalogs, enabling:
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
