//! Compaction and WAL configuration types

use crate::core::foundation::BaseConfig;

/// Compaction strategy for storage optimization
#[derive(Debug, Clone, Default)]
pub enum CompactionStrategy {
    /// Level-based compaction (LSM)
    #[default]
    Level,
    /// Tiered compaction
    Tiered,
    /// Size-tiered compaction
    SizeTiered,
    /// Time-based compaction
    TimeBased,
    /// No compaction
    None,
}

/// Backwards-compat alias for [`CoreCompactionConfig`].
pub type CompactionConfig = CoreCompactionConfig;

/// Compaction configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoreCompactionConfig {
    /// Compaction strategy to use
    /// Maximum number of files before compaction
    pub max_files: usize,
    /// Size threshold for compaction (MB)
    pub size_threshold_mb: usize,
    /// Time threshold for compaction (hours)
    pub time_threshold_hours: u64,
    /// Enable background compaction
    pub background_compaction: bool,
    /// Compaction thread count
    pub compaction_threads: usize,
}

impl Default for CoreCompactionConfig {
    fn default() -> Self {
        Self {
            // strategy removed -  CompactionStrategy::default(),
            max_files: 10,
            size_threshold_mb: 100,
            time_threshold_hours: 24,
            background_compaction: true,
            compaction_threads: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(4),
        }
    }
}

impl BaseConfig for CoreCompactionConfig {
    fn validate(&self) -> Result<(), String> {
        if self.max_files == 0 {
            return Err("Max files must be greater than 0".to_string());
        }
        if self.compaction_threads == 0 {
            return Err("Compaction threads must be greater than 0".to_string());
        }
        Ok(())
    }
}

/// WAL strategy type
#[derive(Debug, Clone, Default)]
pub enum WriteBufferStrategyType {
    /// Apache Avro format
    #[default]
    Avro,
    /// Bincode binary format
    Bincode,
    /// JSON format (for debugging)
    Json,
}

/// Memory table type for WAL
#[derive(Debug, Clone, Default)]
pub enum MemTableType {
    /// Hash map implementation
    HashMap,
    /// B-tree implementation
    #[default]
    BTree,
    /// Skip list implementation
    SkipList,
    /// Adaptive Radix Tree
    ART,
}

/// Synchronization mode for WAL
#[derive(Debug, Clone, Default)]
pub enum SyncMode {
    /// Synchronous writes
    Sync,
    /// Asynchronous writes
    Async,
    /// Batch synchronous writes
    #[default]
    BatchSync,
}
