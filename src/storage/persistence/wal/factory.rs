// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! WAL Factory - Abstract Factory Pattern for Strategy Creation

use anyhow::Result;
use std::sync::Arc;

use super::config::WalStrategyType;
use super::{WalConfig, WalStrategy};
use crate::storage::persistence::filesystem::FilesystemFactory;

// Re-exports for factory users (avoid duplicate import)
// pub use super::config::WalStrategyType;  // Already imported above

/// Abstract factory for creating WAL strategies
pub struct WalFactory;

impl WalFactory {
    /// Create a WAL strategy based on configuration
    pub async fn create_strategy(
        strategy_type: WalStrategyType,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Box<dyn WalStrategy>> {
        match strategy_type {
            WalStrategyType::Avro => {
                let mut strategy = Box::new(super::avro::AvroWalStrategy::new());
                strategy.initialize(config, filesystem).await?;
                Ok(strategy)
            }
            WalStrategyType::Bincode => {
                let mut strategy = Box::new(super::bincode::BincodeWalStrategy::new());
                strategy.initialize(config, filesystem).await?;
                Ok(strategy)
            }
        }
    }

    /// Create strategy with automatic type detection from config
    pub async fn create_from_config(
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Box<dyn WalStrategy>> {
        Self::create_strategy(config.strategy_type.clone(), config, filesystem).await
    }

    /// List available strategy types
    pub fn available_strategies() -> Vec<WalStrategyType> {
        vec![WalStrategyType::Avro, WalStrategyType::Bincode]
    }

    /// Get strategy information
    pub fn strategy_info(strategy_type: &WalStrategyType) -> StrategyInfo {
        match strategy_type {
            WalStrategyType::Avro => StrategyInfo {
                name: "Avro",
                description:
                    "Schema evolution support, cross-language compatibility, built-in compression",
                features: vec![
                    "Schema evolution",
                    "Cross-language support",
                    "Built-in compression",
                    "Field-level compatibility",
                ],
                performance_profile: PerformanceProfile {
                    write_speed: PerformanceRating::Good,
                    read_speed: PerformanceRating::Good,
                    compression_ratio: PerformanceRating::Excellent,
                    memory_usage: PerformanceRating::Good,
                    cpu_usage: PerformanceRating::Good,
                },
                use_cases: vec![
                    "Long-term data storage",
                    "Cross-language environments",
                    "Schema evolution requirements",
                    "Data analytics pipelines",
                ],
            },
            WalStrategyType::Bincode => StrategyInfo {
                name: "Bincode",
                description: "Native Rust serialization, maximum performance, minimal overhead",
                features: vec![
                    "Zero-copy deserialization",
                    "Native Rust performance",
                    "Minimal CPU overhead",
                    "Compact binary format",
                ],
                performance_profile: PerformanceProfile {
                    write_speed: PerformanceRating::Excellent,
                    read_speed: PerformanceRating::Excellent,
                    compression_ratio: PerformanceRating::Good,
                    memory_usage: PerformanceRating::Excellent,
                    cpu_usage: PerformanceRating::Excellent,
                },
                use_cases: vec![
                    "High-frequency trading",
                    "Real-time analytics",
                    "Performance-critical applications",
                    "Rust-only environments",
                ],
            },
        }
    }
}

/// Strategy information for decision making
#[derive(Debug, Clone)]
pub struct StrategyInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub features: Vec<&'static str>,
    pub performance_profile: PerformanceProfile,
    pub use_cases: Vec<&'static str>,
}

/// Performance profile for strategy comparison
#[derive(Debug, Clone)]
pub struct PerformanceProfile {
    pub write_speed: PerformanceRating,
    pub read_speed: PerformanceRating,
    pub compression_ratio: PerformanceRating,
    pub memory_usage: PerformanceRating,
    pub cpu_usage: PerformanceRating,
}

/// Performance rating scale
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PerformanceRating {
    Poor,
    Fair,
    Good,
    Excellent,
}

impl std::fmt::Display for PerformanceRating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Poor => write!(f, "Poor"),
            Self::Fair => write!(f, "Fair"),
            Self::Good => write!(f, "Good"),
            Self::Excellent => write!(f, "Excellent"),
        }
    }
}

/// Strategy selection helper
pub struct StrategySelector;

impl StrategySelector {
    /// Recommend strategy based on workload characteristics
    pub fn recommend_strategy(workload: &WorkloadCharacteristics) -> WalStrategyType {
        match workload {
            WorkloadCharacteristics {
                write_heavy: true,
                read_heavy: false,
                schema_evolution: false,
                cross_language: false,
                ..
            } => WalStrategyType::Bincode, // High-write performance

            WorkloadCharacteristics {
                schema_evolution: true,
                ..
            } => WalStrategyType::Avro, // Schema evolution needed

            WorkloadCharacteristics {
                cross_language: true,
                ..
            } => WalStrategyType::Avro, // Cross-language compatibility

            WorkloadCharacteristics {
                storage_efficiency: true,
                ..
            } => WalStrategyType::Avro, // Better compression

            _ => WalStrategyType::Avro, // Safe default
        }
    }

    /// Get strategy comparison matrix
    pub fn compare_strategies() -> StrategyComparison {
        StrategyComparison {
            avro: WalFactory::strategy_info(&WalStrategyType::Avro),
            bincode: WalFactory::strategy_info(&WalStrategyType::Bincode),
        }
    }
}

/// Workload characteristics for strategy selection
#[derive(Debug, Clone)]
pub struct WorkloadCharacteristics {
    pub write_heavy: bool,
    pub read_heavy: bool,
    pub schema_evolution: bool,
    pub cross_language: bool,
    pub storage_efficiency: bool,
    pub latency_sensitive: bool,
    pub throughput_critical: bool,
}

impl Default for WorkloadCharacteristics {
    fn default() -> Self {
        Self {
            write_heavy: false,
            read_heavy: false,
            schema_evolution: false,
            cross_language: false,
            storage_efficiency: false,
            latency_sensitive: false,
            throughput_critical: false,
        }
    }
}

/// Strategy comparison result
#[derive(Debug, Clone)]
pub struct StrategyComparison {
    pub avro: StrategyInfo,
    pub bincode: StrategyInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strategy_recommendation() {
        let workload = WorkloadCharacteristics {
            write_heavy: true,
            schema_evolution: false,
            cross_language: false,
            ..Default::default()
        };

        assert_eq!(
            StrategySelector::recommend_strategy(&workload),
            WalStrategyType::Bincode
        );

        let workload = WorkloadCharacteristics {
            schema_evolution: true,
            ..Default::default()
        };

        assert_eq!(
            StrategySelector::recommend_strategy(&workload),
            WalStrategyType::Avro
        );
    }

    #[test]
    fn test_available_strategies() {
        let strategies = WalFactory::available_strategies();
        assert_eq!(strategies.len(), 2);
        assert!(strategies.contains(&WalStrategyType::Avro));
        assert!(strategies.contains(&WalStrategyType::Bincode));
    }

    #[test]
    fn test_strategy_info() {
        let avro_info = WalFactory::strategy_info(&WalStrategyType::Avro);
        assert_eq!(avro_info.name, "Avro");
        assert!(avro_info.features.contains(&"Schema evolution"));

        let bincode_info = WalFactory::strategy_info(&WalStrategyType::Bincode);
        assert_eq!(bincode_info.name, "Bincode");
        assert!(bincode_info.features.contains(&"Zero-copy deserialization"));
    }
}
