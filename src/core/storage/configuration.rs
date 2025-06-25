//! Storage configuration types

use serde::{Deserialize, Serialize};
use crate::core::foundation::BaseConfig;
use super::{CompressionConfig, StorageEngine};

/// Unified storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedStorageConfig {
    /// Primary storage engine
    pub engine: StorageEngine,
    /// Compression settings
    pub compression: CompressionConfig,
    /// Data directories for storage
    pub data_dirs: Vec<std::path::PathBuf>,
    /// Maximum file size before splitting
    pub max_file_size_mb: usize,
    /// Enable write-ahead logging
    pub enable_wal: bool,
    /// Sync frequency in seconds
    pub sync_interval_secs: u64,
}

impl Default for UnifiedStorageConfig {
    fn default() -> Self {
        Self {
            engine: StorageEngine::default(),
            compression: CompressionConfig::default(),
            data_dirs: vec![std::path::PathBuf::from("./data")],
            max_file_size_mb: 256,
            enable_wal: true,
            sync_interval_secs: 30,
        }
    }
}

impl BaseConfig for UnifiedStorageConfig {
    fn validate(&self) -> Result<(), String> {
        if self.data_dirs.is_empty() {
            return Err("At least one data directory must be specified".to_string());
        }
        
        if self.max_file_size_mb == 0 {
            return Err("Max file size must be greater than 0".to_string());
        }
        
        self.compression.validate()?;
        Ok(())
    }
}