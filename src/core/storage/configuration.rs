//! Storage configuration types

use super::StorageEngine;
use crate::core::foundation::BaseConfig;
use proximadb_compression_types::CompressionConfig;

/// Unified storage configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UnifiedStorageConfig {
    /// Primary storage engine
    pub engine: StorageEngine,
    /// Compression settings
    pub compression: CompressionConfig,
    /// Data directories for storage
    pub data_dirs: Vec<std::path::PathBuf>,
    /// Maximum file size before splitting
    pub max_file_size_mb: usize,
    /// Enable write bufferging
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

        if let Some(level) = self.compression.level {
            let Some((min, max)) = self.compression.algorithm.level_range() else {
                return Err(format!(
                    "Compression level is not supported for {}",
                    self.compression.algorithm
                ));
            };
            if !(min..=max).contains(&level) {
                return Err(format!(
                    "Compression level for {} must be between {} and {}",
                    self.compression.algorithm, min, max
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_compression_types::CompressionAlgorithm;

    #[test]
    fn validates_foundation_compression_level_range() {
        let config = UnifiedStorageConfig {
            compression: CompressionConfig::zstd().with_level(22),
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_out_of_range_foundation_compression_level() {
        let config = UnifiedStorageConfig {
            compression: CompressionConfig::zstd().with_level(99),
            ..Default::default()
        };

        let err = config.validate().unwrap_err();
        assert!(err.contains("must be between 1 and 22"));
    }

    #[test]
    fn rejects_level_for_algorithm_without_level_support() {
        let config = UnifiedStorageConfig {
            compression: CompressionConfig::with_algorithm(CompressionAlgorithm::Snappy)
                .with_level(1),
            ..Default::default()
        };

        let err = config.validate().unwrap_err();
        assert!(err.contains("not supported"));
    }
}
