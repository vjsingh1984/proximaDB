//! WAL Batch Factory - Modern Factory for Batch-Oriented Strategies
//!
//! This factory creates modern WALBatchStrategy implementations that use
//! the streamlined architecture with native batch operations.

use anyhow::Result;
use std::sync::Arc;

use super::config::WriteBufferStrategyType;
use super::{
    AvroSerializationStrategy, BincodeSerializationStrategy, ProtoSerializationStrategy,
    WALBatchStrategy, WALConfig,
};
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Modern factory for creating WAL batch strategies
pub struct WALBatchFactory;

impl WALBatchFactory {
    /// Create a batch serialization strategy (HIGHLY RECOMMENDED - best separation of concerns)
    ///
    /// This creates strategies using the new clean architecture with:
    /// - Separated serialization, memtable, and disk components
    /// - Direct recovery to storage engines
    /// - Parallel recovery support
    pub async fn create_batch_serialization_strategy(
        strategy_type: WriteBufferStrategyType,
        config: &WALConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Box<dyn WALBatchStrategy>> {
        match strategy_type {
            WriteBufferStrategyType::ProtoBatch => {
                tracing::info!("🎯 Creating ProtoSerializationStrategy with separated components");
                let strategy = ProtoSerializationStrategy::new(config, filesystem).await?;
                Ok(Box::new(strategy))
            }
            WriteBufferStrategyType::AvroBatch => {
                tracing::info!("🎯 Creating AvroSerializationStrategy with separated components");
                let strategy = AvroSerializationStrategy::new(config, filesystem).await?;
                Ok(Box::new(strategy))
            }
            WriteBufferStrategyType::BincodeBatch => {
                tracing::info!(
                    "🎯 Creating BincodeSerializationStrategy with separated components"
                );
                let strategy = BincodeSerializationStrategy::new(config, filesystem).await?;
                Ok(Box::new(strategy))
            }
        }
    }

    /// Create strategy with automatic type detection from config
    pub async fn create_from_config(
        config: &WALConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Box<dyn WALBatchStrategy>> {
        Self::create_batch_serialization_strategy(config.strategy_type, config, filesystem)
            .await
    }

    /// List available strategy types
    pub fn available_strategies() -> Vec<WriteBufferStrategyType> {
        vec![
            WriteBufferStrategyType::AvroBatch,
            WriteBufferStrategyType::BincodeBatch,
            WriteBufferStrategyType::ProtoBatch,
        ]
    }

    /// Get strategy information for debugging and monitoring
    pub fn get_strategy_info(strategy_type: &WriteBufferStrategyType) -> StrategyInfo {
        match strategy_type {
            WriteBufferStrategyType::AvroBatch => StrategyInfo {
                name: "AvroBatch".to_string(),
                description: "Modern Avro-based WAL strategy with schema evolution support and native batch operations".to_string(),
                serialization: "Apache Avro".to_string(),
                schema_evolution: true,
                batch_native: true,
                performance_profile: "Balanced - good for schema evolution and cross-language compatibility".to_string(),
                recommended_use_cases: vec![
                    "Production deployments requiring schema evolution".to_string(),
                    "Cross-language data exchange".to_string(),
                    "Long-term data storage with compatibility guarantees".to_string(),
                ],
            },
            WriteBufferStrategyType::BincodeBatch => StrategyInfo {
                name: "BincodeBatch".to_string(),
                description: "Modern Bincode-based WAL strategy optimized for maximum native Rust performance with native batch operations".to_string(),
                serialization: "Bincode (native Rust)".to_string(),
                schema_evolution: false,
                batch_native: true,
                performance_profile: "High Performance - optimized for native Rust throughput".to_string(),
                recommended_use_cases: vec![
                    "High-throughput Rust-only deployments".to_string(),
                    "Performance-critical applications".to_string(),
                    "Minimal serialization overhead requirements".to_string(),
                ],
            },
            WriteBufferStrategyType::ProtoBatch => StrategyInfo {
                name: "ProtoBatch".to_string(),
                description: "Modern Protocol Buffers-based WAL strategy for proto-first architecture with zero double serialization".to_string(),
                serialization: "Protocol Buffers".to_string(),
                schema_evolution: true,
                batch_native: true,
                performance_profile: "High Performance - optimized for proto-first architecture with efficient binary encoding".to_string(),
                recommended_use_cases: vec![
                    "Proto-first deployments with unified data models".to_string(),
                    "Zero double serialization requirements".to_string(),
                    "High-performance cross-language compatibility".to_string(),
                ],
            }
        }
    }

    /// Compare strategies for selection guidance
    pub fn compare_strategies() -> StrategyComparison {
        StrategyComparison {
            avro_advantages: vec![
                "Schema evolution support".to_string(),
                "Cross-language compatibility".to_string(),
                "Self-describing data format".to_string(),
                "Rich data type support".to_string(),
            ],
            bincode_advantages: vec![
                "Maximum native Rust performance".to_string(),
                "Minimal serialization overhead".to_string(),
                "Compact binary representation".to_string(),
                "Zero-copy deseralization potential".to_string(),
            ],
            proto_advantages: vec![
                "Proto-first architecture alignment".to_string(),
                "Zero double serialization".to_string(),
                "Efficient binary encoding".to_string(),
                "Strong typing with code generation".to_string(),
                "Excellent cross-language support".to_string(),
            ],
            recommendation: "Use Proto for proto-first architecture with zero double serialization. Use Avro for legacy compatibility. Use Bincode for maximum Rust-only performance.".to_string(),
        }
    }
}

/// Strategy information for debugging and monitoring
#[derive(Debug, Clone)]
pub struct StrategyInfo {
    pub name: String,
    pub description: String,
    pub serialization: String,
    pub schema_evolution: bool,
    pub batch_native: bool,
    pub performance_profile: String,
    pub recommended_use_cases: Vec<String>,
}

/// Strategy comparison for selection guidance
#[derive(Debug, Clone)]
pub struct StrategyComparison {
    pub avro_advantages: Vec<String>,
    pub bincode_advantages: Vec<String>,
    pub proto_advantages: Vec<String>,
    pub recommendation: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_available_strategies() {
        let strategies = WALBatchFactory::available_strategies();
        assert_eq!(strategies.len(), 3);
        assert!(strategies.contains(&WriteBufferStrategyType::AvroBatch));
        assert!(strategies.contains(&WriteBufferStrategyType::BincodeBatch));
        assert!(strategies.contains(&WriteBufferStrategyType::ProtoBatch));
    }

    #[test]
    fn test_strategy_info() {
        let avro_info = WALBatchFactory::get_strategy_info(&WriteBufferStrategyType::AvroBatch);
        assert_eq!(avro_info.name, "AvroBatch");
        assert!(avro_info.schema_evolution);
        assert!(avro_info.batch_native);

        let bincode_info =
            WALBatchFactory::get_strategy_info(&WriteBufferStrategyType::BincodeBatch);
        assert_eq!(bincode_info.name, "BincodeBatch");
        assert!(!bincode_info.schema_evolution);
        assert!(bincode_info.batch_native);

        let proto_info = WALBatchFactory::get_strategy_info(&WriteBufferStrategyType::ProtoBatch);
        assert_eq!(proto_info.name, "ProtoBatch");
        assert!(proto_info.schema_evolution);
        assert!(proto_info.batch_native);
    }

    #[test]
    fn test_strategy_comparison() {
        let comparison = WALBatchFactory::compare_strategies();
        assert!(!comparison.avro_advantages.is_empty());
        assert!(!comparison.bincode_advantages.is_empty());
        assert!(!comparison.proto_advantages.is_empty());
        assert!(!comparison.recommendation.is_empty());
    }

    use crate::storage::WALConfig;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn create_test_filesystem() -> (Arc<FilesystemFactory>, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::create(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );
        (filesystem, temp_dir)
    }

    fn create_test_config(temp_dir: &TempDir) -> WALConfig {
        let mut config = WALConfig::default();
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];
        config
    }

    #[tokio::test]
    async fn test_create_avro_serialization_strategy() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let config = create_test_config(&temp_dir);

        let strategy = WALBatchFactory::create_batch_serialization_strategy(
            WriteBufferStrategyType::AvroBatch,
            &config,
            filesystem,
        )
        .await
        .expect("Failed to create Avro serialization strategy");

        assert_eq!(strategy.strategy_name(), "AvroBatch");
    }

    #[tokio::test]
    async fn test_create_bincode_serialization_strategy() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let config = create_test_config(&temp_dir);

        let strategy = WALBatchFactory::create_batch_serialization_strategy(
            WriteBufferStrategyType::BincodeBatch,
            &config,
            filesystem,
        )
        .await
        .expect("Failed to create Bincode serialization strategy");

        assert_eq!(strategy.strategy_name(), "BincodeBatch");
    }

    #[tokio::test]
    async fn test_create_proto_serialization_strategy() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let config = create_test_config(&temp_dir);

        let strategy = WALBatchFactory::create_batch_serialization_strategy(
            WriteBufferStrategyType::ProtoBatch,
            &config,
            filesystem,
        )
        .await
        .expect("Failed to create Proto serialization strategy");

        assert_eq!(strategy.strategy_name(), "ProtoBatch");
    }

    #[tokio::test]
    async fn test_create_from_config_avro() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let mut config = create_test_config(&temp_dir);
        config.strategy_type = WriteBufferStrategyType::AvroBatch;

        let strategy = WALBatchFactory::create_from_config(&config, filesystem)
            .await
            .expect("Failed to create strategy from config");

        assert_eq!(strategy.strategy_name(), "AvroBatch");
    }

    #[tokio::test]
    async fn test_create_from_config_bincode() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let mut config = create_test_config(&temp_dir);
        config.strategy_type = WriteBufferStrategyType::BincodeBatch;

        let strategy = WALBatchFactory::create_from_config(&config, filesystem)
            .await
            .expect("Failed to create strategy from config");

        assert_eq!(strategy.strategy_name(), "BincodeBatch");
    }

    #[test]
    fn test_avro_strategy_info() {
        let info = WALBatchFactory::get_strategy_info(&WriteBufferStrategyType::AvroBatch);

        assert_eq!(info.name, "AvroBatch");
        assert_eq!(info.serialization, "Apache Avro");
        assert!(info.schema_evolution);
        assert!(info.batch_native);
        assert!(!info.recommended_use_cases.is_empty());
        assert!(!info.description.is_empty());
    }

    #[test]
    fn test_bincode_strategy_info() {
        let info = WALBatchFactory::get_strategy_info(&WriteBufferStrategyType::BincodeBatch);

        assert_eq!(info.name, "BincodeBatch");
        assert_eq!(info.serialization, "Bincode (native Rust)");
        assert!(!info.schema_evolution);
        assert!(info.batch_native);
        assert!(!info.recommended_use_cases.is_empty());
        assert!(!info.description.is_empty());
    }

    #[test]
    fn test_strategy_info_consistency() {
        let avro_info = WALBatchFactory::get_strategy_info(&WriteBufferStrategyType::AvroBatch);
        let bincode_info =
            WALBatchFactory::get_strategy_info(&WriteBufferStrategyType::BincodeBatch);

        assert!(avro_info.batch_native);
        assert!(bincode_info.batch_native);

        assert!(avro_info.schema_evolution);
        assert!(!bincode_info.schema_evolution);

        assert!(!avro_info.performance_profile.is_empty());
        assert!(!bincode_info.performance_profile.is_empty());
    }

    #[tokio::test]
    async fn test_serialization_strategy_initialization() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let config = create_test_config(&temp_dir);

        for strategy_type in &[
            WriteBufferStrategyType::AvroBatch,
            WriteBufferStrategyType::BincodeBatch,
            WriteBufferStrategyType::ProtoBatch,
        ] {
            let strategy = WALBatchFactory::create_batch_serialization_strategy(
                strategy_type.clone(),
                &config,
                filesystem.clone(),
            )
            .await
            .expect("Failed to create serialization strategy");

            assert!(strategy.get_wal_behavior().is_none());

            let stats = strategy.get_stats().await.expect("Failed to get stats");
            assert_eq!(stats.memory_entries, 0);
        }
    }

    #[tokio::test]
    async fn test_concurrent_strategy_creation() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let config = create_test_config(&temp_dir);

        let tasks: Vec<_> = (0..5)
            .map(|i| {
                let fs = filesystem.clone();
                let cfg = config.clone();
                let strategy_type = if i % 2 == 0 {
                    WriteBufferStrategyType::AvroBatch
                } else {
                    WriteBufferStrategyType::BincodeBatch
                };

                tokio::spawn(async move {
                    WALBatchFactory::create_batch_serialization_strategy(strategy_type, &cfg, fs)
                        .await
                })
            })
            .collect();

        let results: Vec<_> = futures::future::join_all(tasks).await;

        for result in results {
            let strategy = result
                .expect("Task failed")
                .expect("Strategy creation failed");
            let name = strategy.strategy_name();
            assert!(name == "AvroBatch" || name == "BincodeBatch" || name == "ProtoBatch");
        }
    }

    #[test]
    fn test_strategy_selection_guidance() {
        let comparison = WALBatchFactory::compare_strategies();

        assert!(comparison.recommendation.to_lowercase().contains("avro"));
        assert!(comparison.recommendation.to_lowercase().contains("bincode"));

        let rec_lower = comparison.recommendation.to_lowercase();
        assert!(
            rec_lower.contains("schema")
                || rec_lower.contains("performance")
                || rec_lower.contains("rust")
        );
    }

    #[test]
    fn test_strategy_info_completeness() {
        for strategy_type in &[
            WriteBufferStrategyType::AvroBatch,
            WriteBufferStrategyType::BincodeBatch,
        ] {
            let info = WALBatchFactory::get_strategy_info(strategy_type);

            assert!(!info.name.is_empty());
            assert!(!info.description.is_empty());
            assert!(!info.serialization.is_empty());
            assert!(!info.performance_profile.is_empty());
            assert!(!info.recommended_use_cases.is_empty());

            assert!(info.batch_native);
        }
    }
}
