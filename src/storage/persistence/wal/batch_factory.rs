//! WAL Batch Factory - Modern Factory for Batch-Oriented Strategies
//!
//! This factory creates modern WalBatchStrategy implementations that use
//! the streamlined architecture with native batch operations.

use anyhow::Result;
use std::sync::Arc;

use super::config::WalStrategyType;
use super::{WalConfig, WalBatchStrategy, AvroWalBatchStrategy, BincodeWalBatchStrategy};
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Modern factory for creating WAL batch strategies
pub struct WalBatchFactory;

impl WalBatchFactory {
    /// Create a modern WAL batch strategy based on configuration
    /// 
    /// This is the modern replacement for WalFactory::create_strategy that provides:
    /// - Native batch operations for better performance
    /// - Streamlined architecture using GlobalPartitionedMemtable
    /// - Unified interface across all serialization strategies
    pub async fn create_strategy(
        strategy_type: WalStrategyType,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Box<dyn WalBatchStrategy>> {
        match strategy_type {
            WalStrategyType::Avro | WalStrategyType::AvroBatch => {
                tracing::info!("🎯 Creating modern AvroWalBatchStrategy");
                let mut strategy = Box::new(AvroWalBatchStrategy::new());
                strategy.initialize(config, filesystem).await?;
                Ok(strategy)
            }
            WalStrategyType::Bincode | WalStrategyType::BincodeBatch => {
                tracing::info!("🎯 Creating modern BincodeWalBatchStrategy");
                let mut strategy = Box::new(BincodeWalBatchStrategy::new());
                strategy.initialize(config, filesystem).await?;
                Ok(strategy)
            }
        }
    }

    /// Create strategy with automatic type detection from config
    pub async fn create_from_config(
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Box<dyn WalBatchStrategy>> {
        Self::create_strategy(config.strategy_type.clone(), config, filesystem).await
    }

    /// List available strategy types
    pub fn available_strategies() -> Vec<WalStrategyType> {
        vec![WalStrategyType::Avro, WalStrategyType::Bincode, WalStrategyType::AvroBatch, WalStrategyType::BincodeBatch]
    }

    /// Get strategy information for debugging and monitoring
    pub fn get_strategy_info(strategy_type: &WalStrategyType) -> StrategyInfo {
        match strategy_type {
            WalStrategyType::Avro | WalStrategyType::AvroBatch => StrategyInfo {
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
            WalStrategyType::Bincode | WalStrategyType::BincodeBatch => StrategyInfo {
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
            }
        }
    }

    // 🚫 REMOVED: Legacy adapter no longer available - WalStrategy trait removed
    // All code must use native WalBatchStrategy with single-entry batches for individual operations
    /*
    pub async fn from_legacy_strategy(
        legacy_strategy: Box<dyn super::WalStrategy>,
    ) -> Result<Box<dyn WalBatchStrategy>> {
        // Removed - use create_strategy() for native batch strategies
    }
    */

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
            recommendation: "Use Avro for production systems requiring schema evolution. Use Bincode for maximum performance in Rust-only environments.".to_string(),
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
    pub recommendation: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_available_strategies() {
        let strategies = WalBatchFactory::available_strategies();
        assert_eq!(strategies.len(), 2);
        assert!(strategies.contains(&WalStrategyType::Avro));
        assert!(strategies.contains(&WalStrategyType::Bincode));
    }

    #[test]
    fn test_strategy_info() {
        let avro_info = WalBatchFactory::get_strategy_info(&WalStrategyType::Avro);
        assert_eq!(avro_info.name, "AvroBatch");
        assert!(avro_info.schema_evolution);
        assert!(avro_info.batch_native);

        let bincode_info = WalBatchFactory::get_strategy_info(&WalStrategyType::Bincode);
        assert_eq!(bincode_info.name, "BincodeBatch");
        assert!(!bincode_info.schema_evolution);
        assert!(bincode_info.batch_native);
    }

    #[test]
    fn test_strategy_comparison() {
        let comparison = WalBatchFactory::compare_strategies();
        assert!(!comparison.avro_advantages.is_empty());
        assert!(!comparison.bincode_advantages.is_empty());
        assert!(!comparison.recommendation.is_empty());
    }
}