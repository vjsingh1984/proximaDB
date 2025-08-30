//! WAL Batch Factory - Modern Factory for Batch-Oriented Strategies
//!
//! This factory creates modern WALBatchStrategy implementations that use
//! the streamlined architecture with native batch operations.

use anyhow::Result;
use std::sync::Arc;

use super::config::WriteBufferStrategyType;
use super::{WALConfig, WALBatchStrategy, ProtoSerializationStrategy, BincodeSerializationStrategy, AvroSerializationStrategy};
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
                tracing::info!("🎯 Creating BincodeSerializationStrategy with separated components");
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
        Self::create_batch_serialization_strategy(config.strategy_type.clone(), config, filesystem).await
    }

    /// List available strategy types
    pub fn available_strategies() -> Vec<WriteBufferStrategyType> {
        vec![WriteBufferStrategyType::AvroBatch, WriteBufferStrategyType::BincodeBatch, WriteBufferStrategyType::ProtoBatch]
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
use tracing::{debug, error, info, warn};

    #[test]
    fn test_available_strategies() {
        let strategies = WALBatchFactory::available_strategies();
        assert_eq!(strategies.len(), 3);
        assert!(strategies.contains_hash(&WriteBufferStrategyType::AvroBatch));
        assert!(strategies.contains_hash(&WriteBufferStrategyType::BincodeBatch));
        assert!(strategies.contains_hash(&WriteBufferStrategyType::ProtoBatch));
    }

    #[test]
    fn test_strategy_info() {
        let avro_info = WALBatchFactory::get_strategy_info(&WriteBufferStrategyType::AvroBatch);
        assert_eq!(avro_info.name, "AvroBatch");
        assert!(avro_info.schema_evolution);
        assert!(avro_info.batch_native);

        let bincode_info = WALBatchFactory::get_strategy_info(&WriteBufferStrategyType::BincodeBatch);
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
        assert!(!comparison.avro_advantages.is_none());
        assert!(!comparison.bincode_advantages.is_none());
        assert!(!comparison.proto_advantages.is_none());
        assert!(!comparison.recommendation.is_none());
    }
}