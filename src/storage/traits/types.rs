//! Parameter types for storage operations.
//!
//! This module contains configuration and parameter types used
//! throughout the storage layer for flush, compaction, and other operations.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::proto::proximadb_v1::Collection;

/// Flexible flush parameters that work for all storage engines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlushParameters {
    /// Target collection (None means global flush for engines that support it)
    pub collection_id: Option<String>,

    /// Force immediate flush regardless of thresholds
    pub force: bool,

    /// Wait for completion before returning
    pub synchronous: bool,

    /// Engine-specific hints
    pub hints: HashMap<String, serde_json::Value>,

    /// Maximum time to wait for operation
    pub timeout_ms: Option<u64>,

    /// Canonical records to flush (provided by FlushCoordinator from WAL).
    /// Protocol adapters (REST, gRPC, Arrow, pgwire) convert to `ProximaRecord` before
    /// inserting into WAL/flush path. `VectorRecord` no longer crosses this boundary.
    pub vector_records: Vec<proximadb_records::ProximaRecord>,

    /// Whether to trigger compaction after flush
    pub trigger_compaction: bool,

    /// Batch IDs involved in this flush operation (for coordination)
    pub batch_ids: Vec<crate::storage::persistence::write_ahead_log::BatchId>,

    /// Collection configuration to avoid redundant lookups
    pub collection_config: Option<Collection>,

    /// Estimated size in bytes for metrics tracking
    pub estimated_size: usize,
}

impl FlushParameters {
    /// Resolve the target collection id from the explicit parameter or cached config.
    pub fn get_collection_id(&self) -> Result<String> {
        self.collection_id
            .clone()
            .or_else(|| {
                self.collection_config
                    .as_ref()
                    .map(|collection| collection.id.clone())
            })
            .ok_or_else(|| anyhow!("No collection_id provided in flush parameters"))
    }

    /// Resolve the collection data directory from cached config or an engine hint.
    pub fn get_data_dir(&self) -> Result<String> {
        if let Some(collection_config) = &self.collection_config
            && let Some(storage_assignment) = &collection_config.storage_assignment
        {
            return Ok(format!(
                "{}/{}/data",
                storage_assignment.base_location, collection_config.id
            ));
        }

        self.hints
            .get("data_dir")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("Flush parameters require collection_config or a data_dir hint"))
    }
}

/// Flexible compaction parameters that work for all storage engines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactionParameters {
    /// Target collection (None means global compaction for engines that support it)
    pub collection_id: Option<String>,

    /// Force compaction regardless of thresholds
    pub force: bool,

    /// Wait for completion before returning
    pub synchronous: bool,

    /// Engine-specific hints (e.g., target level for LSM, cluster hints for VIPER)
    pub hints: HashMap<String, serde_json::Value>,

    /// Maximum time to wait for operation
    pub timeout_ms: Option<u64>,

    /// Priority level for the operation
    pub priority: OperationPriority,

    /// Collection configuration to avoid redundant lookups
    pub collection_config: Option<Collection>,

    /// Estimated input size in bytes for metrics tracking
    pub estimated_input_size: usize,
}

impl CompactionParameters {
    /// Resolve the target collection id from the explicit parameter or cached config.
    pub fn get_collection_id(&self) -> Result<String> {
        self.collection_id
            .clone()
            .or_else(|| {
                self.collection_config
                    .as_ref()
                    .map(|collection| collection.id.clone())
            })
            .ok_or_else(|| {
                anyhow!("Compaction parameters require a collection_id or collection_config")
            })
    }

    /// Resolve the collection data directory from cached config or an engine hint.
    pub fn get_data_dir(&self) -> Result<String> {
        if let Some(collection_config) = &self.collection_config
            && let Some(storage_assignment) = &collection_config.storage_assignment
        {
            return Ok(format!(
                "{}/{}/data",
                storage_assignment.base_location, collection_config.id
            ));
        }

        self.hints
            .get("data_dir")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow!("Compaction parameters require collection_config or a data_dir hint")
            })
    }
}

/// Operation priority levels for storage operations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum OperationPriority {
    Low = 0,
    #[default]
    Medium = 1,
    High = 2,
    Critical = 3,
}

/// Performance tier hint for storage engines.
///
/// ## Purpose:
///
/// PerformanceTier provides hints to storage engines about data temperature,
/// enabling intelligent tiering decisions for optimal cost/performance balance.
///
/// ## Tiering Strategy:
///
/// - **Hot**: Memory/NVMe SSD, uncompressed or lightly compressed
/// - **Warm**: SSD with moderate compression (ZSTD level 3)
/// - **Cold**: HDD/Cloud with heavy compression (ZSTD level 9)
/// - **Archive**: Glacier/Archive with maximum compression (ZSTD level 19)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PerformanceTier {
    /// Hot data - keep in memory/SSD, optimize for latency
    /// Target: <1ms latency, highest cost
    Hot,

    /// Warm data - balance between latency and cost
    /// Target: <10ms latency, moderate cost
    Warm,

    /// Cold data - optimize for cost
    /// Target: <100ms latency, lowest cost
    Cold,

    /// Archive data - lowest cost, longest retrieval time
    /// Target: Minutes to hours, archival storage
    Archive,

    /// Default tier (system decides)
    #[default]
    Auto,
}

/// Storage engine strategy enumeration for polymorphic engine selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StorageEngineStrategy {
    /// SST engine - hybrid columnar with ProximaBlocks
    #[default]
    Sst,

    /// VIPER engine - columnar Parquet with advanced quantization
    Viper,

    /// HELIX engine - time-series optimized storage
    Helix,

    /// NOVA engine - next-gen columnar with integrated quantization
    Nova,

    /// SWIFT engine - hierarchical superblock architecture
    Swift,

    /// RAPTOR engine - experimental parallel tiered storage
    Raptor,

    /// Hybrid engine - combines row and column optimized paths
    Hybrid,

    /// TST engine - time-series optimized storage
    TimeSeries,

    /// CEDAR engine - unified multimodal storage
    Cedar,

    /// CHRONO engine - temporal data storage
    Chrono,
}

/// Backwards-compat **format** alias for [`StorageEngineStrategy`] (engines →
/// formats convergence). Variants are reached through the alias, e.g.
/// `StorageFormatStrategy::Sst`. New code may use this name;
/// `StorageEngineStrategy` remains during the migration window (see
/// `docs/12-design/NAMING_CONVENTIONS.adoc`).
pub type StorageFormatStrategy = StorageEngineStrategy;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_format_strategy_alias_is_interchangeable() {
        // The format alias names the same type — default + variants match.
        assert_eq!(StorageFormatStrategy::default(), StorageEngineStrategy::Sst);
        let s: StorageFormatStrategy = StorageEngineStrategy::Viper;
        assert_eq!(s, StorageEngineStrategy::Viper);
        // And serializes identically (same type, no wire change).
        assert_eq!(
            serde_json::to_string(&StorageFormatStrategy::Helix).unwrap(),
            serde_json::to_string(&StorageEngineStrategy::Helix).unwrap()
        );
    }

    #[test]
    fn test_flush_parameters_default() {
        let params = FlushParameters::default();
        assert!(params.collection_id.is_none());
        assert!(!params.force);
        assert!(!params.synchronous);
    }

    #[test]
    fn test_compaction_parameters_default() {
        let params = CompactionParameters::default();
        assert!(params.collection_id.is_none());
        assert!(!params.force);
        assert_eq!(params.priority, OperationPriority::Medium);
    }

    #[test]
    fn test_operation_priority_ordering() {
        assert!(OperationPriority::Critical > OperationPriority::High);
        assert!(OperationPriority::High > OperationPriority::Medium);
        assert!(OperationPriority::Medium > OperationPriority::Low);
    }

    #[test]
    fn test_performance_tier_default() {
        let tier = PerformanceTier::default();
        assert_eq!(tier, PerformanceTier::Auto);
    }
}
