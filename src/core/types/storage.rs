//! Shared storage-related types used across the codebase.
//!
//! This module contains types that are shared between multiple architectural layers
//! to prevent circular dependencies and promote code reuse.

use serde::{Deserialize, Serialize};

/// Storage engine type identifier.
///
/// This enum defines the available storage engines in ProximaDB. It is shared
/// across the storage and index layers to prevent circular dependencies.
///
/// ## Architecture Note
///
/// This type was originally defined in `index/axis::eventlog` but was moved here
/// to resolve the TD-CROSS-LAYER technical debt where the storage layer imported
/// from the index layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StorageEngineType {
    /// Sorted String Table engine.
    SST,
    /// VIPER engine for high-throughput writes.
    VIPER,
    /// NOVA engine for large-scale batch operations.
    NOVA,
    /// RAPTOR engine for columnar analytics.
    RAPTOR,
    /// SWIFT engine for low-latency reads.
    SWIFT,
    /// HELIX engine for time-series workloads.
    HELIX,
    /// TST (Ternary Search Tree) engine for text indexing.
    TST,
}

impl StorageEngineType {
    /// Get the display name of the storage engine.
    pub fn as_str(self) -> &'static str {
        match self {
            StorageEngineType::SST => "SST",
            StorageEngineType::VIPER => "VIPER",
            StorageEngineType::NOVA => "NOVA",
            StorageEngineType::RAPTOR => "RAPTOR",
            StorageEngineType::SWIFT => "SWIFT",
            StorageEngineType::HELIX => "HELIX",
            StorageEngineType::TST => "TST",
        }
    }

    /// Parse a string into a StorageEngineType.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "SST" => Some(StorageEngineType::SST),
            "VIPER" => Some(StorageEngineType::VIPER),
            "NOVA" => Some(StorageEngineType::NOVA),
            "RAPTOR" => Some(StorageEngineType::RAPTOR),
            "SWIFT" => Some(StorageEngineType::SWIFT),
            "HELIX" => Some(StorageEngineType::HELIX),
            "TST" => Some(StorageEngineType::TST),
            _ => None,
        }
    }
}

impl std::fmt::Display for StorageEngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for StorageEngineType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str(s).ok_or_else(|| format!("Unknown storage engine type: {}", s))
    }
}
