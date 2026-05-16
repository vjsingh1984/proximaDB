//! Storage engine type identifier shared across storage and index layers.

use serde::{Deserialize, Serialize};

/// Identifies the available storage engines in ProximaDB.
///
/// Shared across storage and index layers to avoid circular dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StorageEngineType {
    SST,
    VIPER,
    NOVA,
    RAPTOR,
    SWIFT,
    HELIX,
    TST,
}

impl StorageEngineType {
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

    pub fn from_name(s: &str) -> Option<Self> {
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
        Self::from_name(s).ok_or_else(|| format!("Unknown storage engine type: {}", s))
    }
}
