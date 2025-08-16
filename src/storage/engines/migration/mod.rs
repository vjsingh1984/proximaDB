// Engine Migration Utilities
// Tools for migrating data between different storage engines (VIPER ↔ SST ↔ SWIFT ↔ NOVA)

pub mod migrator;
// TODO: Implement missing modules for full migration support
// pub mod compatibility;
// pub mod validation;
// pub mod rollback;

// Re-exports
pub use migrator::{EngineMigrator, MigrationPlan, MigrationProgress, MigrationResult};
// TODO: Re-enable once modules are implemented
// pub use compatibility::{EngineCompatibilityChecker, CompatibilityReport, CompatibilityIssue};
// pub use validation::{MigrationValidator, ValidationReport, ValidationResult};
// pub use rollback::{RollbackManager, RollbackPlan, RollbackResult};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::proto::proximadb::StorageEngine as ProtoStorageEngine;
use crate::storage::traits::UnifiedStorageEngine;

/// Migration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConfig {
    /// Source engine type
    pub source_engine: ProtoStorageEngine,
    
    /// Target engine type
    pub target_engine: ProtoStorageEngine,
    
    /// Collections to migrate (empty = all)
    pub collections: Vec<String>,
    
    /// Migration strategy
    pub strategy: MigrationStrategy,
    
    /// Validation settings
    pub validation: ValidationConfig,
    
    /// Performance settings
    pub performance: PerformanceConfig,
    
    /// Rollback settings
    pub rollback: RollbackConfig,
}

/// Migration strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationStrategy {
    /// Copy all data, then switch (safest, requires 2x storage)
    CopyThenSwitch,
    
    /// Gradual migration with dual-write (balanced)
    GradualWithDualWrite,
    
    /// In-place migration (fastest, but risky)
    InPlace,
    
    /// Blue-green deployment style
    BlueGreen,
}

/// Validation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationConfig {
    /// Validate data integrity
    pub validate_data_integrity: bool,
    
    /// Validate performance characteristics
    pub validate_performance: bool,
    
    /// Sample percentage for validation (0.0-1.0)
    pub sample_percentage: f64,
    
    /// Maximum acceptable performance degradation (%)
    pub max_performance_degradation: f64,
    
    /// Timeout for validation operations (seconds)
    pub validation_timeout_seconds: u64,
}

/// Performance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Batch size for data migration
    pub batch_size: usize,
    
    /// Number of parallel workers
    pub parallel_workers: usize,
    
    /// Rate limiting (operations per second, 0 = unlimited)
    pub rate_limit_ops_per_sec: u64,
    
    /// Memory limit for migration operations (bytes)
    pub memory_limit_bytes: u64,
    
    /// Checkpoint interval (number of batches)
    pub checkpoint_interval: usize,
}

/// Rollback configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackConfig {
    /// Enable automatic rollback on failure
    pub auto_rollback_on_failure: bool,
    
    /// Maximum time to keep rollback data (hours)
    pub rollback_retention_hours: u64,
    
    /// Rollback validation requirements
    pub rollback_validation: ValidationConfig,
}

/// Migration status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationStatus {
    /// Migration is being planned
    Planning,
    
    /// Migration is in progress
    InProgress {
        progress_percent: f64,
        current_collection: String,
        estimated_completion: chrono::DateTime<chrono::Utc>,
    },
    
    /// Migration completed successfully
    Completed {
        completion_time: chrono::DateTime<chrono::Utc>,
        migrated_collections: Vec<String>,
        performance_metrics: HashMap<String, f64>,
    },
    
    /// Migration failed
    Failed {
        error: String,
        failure_time: chrono::DateTime<chrono::Utc>,
        rollback_initiated: bool,
    },
    
    /// Migration was rolled back
    RolledBack {
        rollback_time: chrono::DateTime<chrono::Utc>,
        rollback_reason: String,
    },
}

/// Migration event for monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationEvent {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_type: MigrationEventType,
    pub collection_id: Option<String>,
    pub message: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationEventType {
    Started,
    Progress,
    CollectionStarted,
    CollectionCompleted,
    ValidationStarted,
    ValidationCompleted,
    Warning,
    Error,
    Completed,
    RollbackStarted,
    RollbackCompleted,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            source_engine: ProtoStorageEngine::Viper,
            target_engine: ProtoStorageEngine::Nova,
            collections: Vec::new(),
            strategy: MigrationStrategy::CopyThenSwitch,
            validation: ValidationConfig::default(),
            performance: PerformanceConfig::default(),
            rollback: RollbackConfig::default(),
        }
    }
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            validate_data_integrity: true,
            validate_performance: true,
            sample_percentage: 0.1, // 10% sampling
            max_performance_degradation: 20.0, // 20% degradation allowed
            validation_timeout_seconds: 3600, // 1 hour
        }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            parallel_workers: 4,
            rate_limit_ops_per_sec: 0, // Unlimited
            memory_limit_bytes: 1024 * 1024 * 1024, // 1GB
            checkpoint_interval: 100, // Every 100 batches
        }
    }
}

impl Default for RollbackConfig {
    fn default() -> Self {
        Self {
            auto_rollback_on_failure: true,
            rollback_retention_hours: 72, // 3 days
            rollback_validation: ValidationConfig {
                validate_data_integrity: true,
                validate_performance: false, // Skip performance validation on rollback
                sample_percentage: 0.05, // 5% sampling for rollback
                max_performance_degradation: 100.0, // Accept any performance for rollback
                validation_timeout_seconds: 1800, // 30 minutes
            },
        }
    }
}

/// Utility functions for engine migration
pub mod utils {
    use super::*;
    
    /// Check if migration is supported between two engines
    pub fn is_migration_supported(
        source: ProtoStorageEngine,
        target: ProtoStorageEngine,
    ) -> bool {
        match (source, target) {
            // Same engine - no migration needed
            (a, b) if a == b => false,
            
            // All engines support migration to/from each other
            (ProtoStorageEngine::Viper, _) => true,
            (ProtoStorageEngine::Sst, _) => true,
            (ProtoStorageEngine::Swift, _) => true,
            (ProtoStorageEngine::Nova, _) => true,
            
            // From any engine to VIPER/SST/SWIFT/NOVA
            (_, ProtoStorageEngine::Viper) => true,
            (_, ProtoStorageEngine::Sst) => true,
            (_, ProtoStorageEngine::Swift) => true,
            (_, ProtoStorageEngine::Nova) => true,
            
            // Unsupported engines
            _ => false,
        }
    }
    
    /// Get recommended migration strategy for engine pair
    pub fn recommend_migration_strategy(
        source: ProtoStorageEngine,
        target: ProtoStorageEngine,
        data_size_gb: f64,
    ) -> MigrationStrategy {
        match (source, target) {
            // Columnar to columnar (VIPER ↔ NOVA) - fast migration
            (ProtoStorageEngine::Viper, ProtoStorageEngine::Nova) |
            (ProtoStorageEngine::Nova, ProtoStorageEngine::Viper) => {
                if data_size_gb < 100.0 {
                    MigrationStrategy::CopyThenSwitch
                } else {
                    MigrationStrategy::GradualWithDualWrite
                }
            }
            
            // Row-based to row-based (SST ↔ SWIFT) - medium complexity
            (ProtoStorageEngine::Sst, ProtoStorageEngine::Swift) |
            (ProtoStorageEngine::Swift, ProtoStorageEngine::Sst) => {
                MigrationStrategy::GradualWithDualWrite
            }
            
            // Cross-paradigm migrations (columnar ↔ row-based) - careful approach
            _ => {
                if data_size_gb < 50.0 {
                    MigrationStrategy::CopyThenSwitch
                } else {
                    MigrationStrategy::BlueGreen
                }
            }
        }
    }
    
    /// Estimate migration time
    pub fn estimate_migration_time(
        source: ProtoStorageEngine,
        target: ProtoStorageEngine,
        data_size_gb: f64,
        config: &PerformanceConfig,
    ) -> chrono::Duration {
        // Base throughput estimates (GB/hour)
        let throughput = match (source, target) {
            // Same paradigm migrations
            (ProtoStorageEngine::Viper, ProtoStorageEngine::Nova) |
            (ProtoStorageEngine::Nova, ProtoStorageEngine::Viper) => 50.0,
            
            (ProtoStorageEngine::Sst, ProtoStorageEngine::Swift) |
            (ProtoStorageEngine::Swift, ProtoStorageEngine::Sst) => 40.0,
            
            // Cross-paradigm migrations
            _ => 20.0,
        };
        
        // Adjust for parallelization
        let adjusted_throughput = throughput * (config.parallel_workers as f64).sqrt();
        
        // Adjust for rate limiting
        let final_throughput = if config.rate_limit_ops_per_sec > 0 {
            // Estimate ops/GB and apply rate limit
            let estimated_ops_per_gb = 10000.0; // Rough estimate
            let max_throughput_from_rate_limit = 
                config.rate_limit_ops_per_sec as f64 / estimated_ops_per_gb * 3600.0; // per hour
            
            adjusted_throughput.min(max_throughput_from_rate_limit)
        } else {
            adjusted_throughput
        };
        
        let hours = (data_size_gb / final_throughput).max(0.1); // Minimum 6 minutes
        chrono::Duration::milliseconds((hours * 3600.0 * 1000.0) as i64)
    }
    
    /// Get engine display name
    pub fn engine_display_name(engine: ProtoStorageEngine) -> &'static str {
        match engine {
            ProtoStorageEngine::Viper => "VIPER (Columnar Analytics)",
            ProtoStorageEngine::Sst => "SST (Row-based OLTP)",
            ProtoStorageEngine::Swift => "SWIFT (Instant Fast Traversal)",
            ProtoStorageEngine::Nova => "NOVA (Optimized Vector Analytics)",
            ProtoStorageEngine::Mmap => "MMAP (Memory-mapped)",
            ProtoStorageEngine::Hybrid => "Hybrid (Multi-engine)",
            _ => "Unknown Engine",
        }
    }
    
    /// Validate migration configuration
    pub fn validate_migration_config(config: &MigrationConfig) -> Result<()> {
        // Check engine support
        if !is_migration_supported(config.source_engine, config.target_engine) {
            return Err(anyhow::anyhow!(
                "Migration from {} to {} is not supported",
                engine_display_name(config.source_engine),
                engine_display_name(config.target_engine)
            ));
        }
        
        // Validate performance config
        if config.performance.batch_size == 0 {
            return Err(anyhow::anyhow!("Batch size must be greater than 0"));
        }
        
        if config.performance.parallel_workers == 0 {
            return Err(anyhow::anyhow!("Number of parallel workers must be greater than 0"));
        }
        
        // Validate validation config
        if config.validation.sample_percentage < 0.0 || config.validation.sample_percentage > 1.0 {
            return Err(anyhow::anyhow!("Sample percentage must be between 0.0 and 1.0"));
        }
        
        if config.validation.max_performance_degradation < 0.0 {
            return Err(anyhow::anyhow!("Maximum performance degradation must be non-negative"));
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::utils::*;
    
    #[test]
    fn test_migration_support() {
        // Same engine - no migration needed
        assert!(!is_migration_supported(ProtoStorageEngine::Viper, ProtoStorageEngine::Viper));
        
        // Supported migrations
        assert!(is_migration_supported(ProtoStorageEngine::Viper, ProtoStorageEngine::Nova));
        assert!(is_migration_supported(ProtoStorageEngine::Sst, ProtoStorageEngine::Swift));
        assert!(is_migration_supported(ProtoStorageEngine::Swift, ProtoStorageEngine::Nova));
        
        // Reverse directions
        assert!(is_migration_supported(ProtoStorageEngine::Nova, ProtoStorageEngine::Viper));
        assert!(is_migration_supported(ProtoStorageEngine::Swift, ProtoStorageEngine::Sst));
    }
    
    #[test]
    fn test_migration_strategy_recommendation() {
        // Small data - copy then switch
        let strategy = recommend_migration_strategy(
            ProtoStorageEngine::Viper,
            ProtoStorageEngine::Nova,
            10.0, // 10GB
        );
        assert!(matches!(strategy, MigrationStrategy::CopyThenSwitch));
        
        // Large data - gradual migration
        let strategy = recommend_migration_strategy(
            ProtoStorageEngine::Viper,
            ProtoStorageEngine::Nova,
            500.0, // 500GB
        );
        assert!(matches!(strategy, MigrationStrategy::GradualWithDualWrite));
        
        // Cross-paradigm - blue-green for large data
        let strategy = recommend_migration_strategy(
            ProtoStorageEngine::Viper,
            ProtoStorageEngine::Sst,
            100.0, // 100GB
        );
        assert!(matches!(strategy, MigrationStrategy::BlueGreen));
    }
    
    #[test]
    fn test_migration_time_estimation() {
        let config = PerformanceConfig::default();
        
        let duration = estimate_migration_time(
            ProtoStorageEngine::Viper,
            ProtoStorageEngine::Nova,
            100.0, // 100GB
            &config,
        );
        
        // Should be reasonable (between 1 hour and 10 hours)
        assert!(duration.num_hours() >= 1);
        assert!(duration.num_hours() <= 10);
    }
    
    #[test]
    fn test_migration_config_validation() {
        let mut config = MigrationConfig::default();
        
        // Valid config should pass
        assert!(validate_migration_config(&config).is_ok());
        
        // Invalid batch size
        config.performance.batch_size = 0;
        assert!(validate_migration_config(&config).is_err());
        
        // Reset and test invalid sample percentage
        config = MigrationConfig::default();
        config.validation.sample_percentage = 1.5;
        assert!(validate_migration_config(&config).is_err());
    }
    
    #[test]
    fn test_engine_display_names() {
        assert_eq!(engine_display_name(ProtoStorageEngine::Viper), "VIPER (Columnar Analytics)");
        assert_eq!(engine_display_name(ProtoStorageEngine::Swift), "SWIFT (Instant Fast Traversal)");
        assert_eq!(engine_display_name(ProtoStorageEngine::Nova), "NOVA (Optimized Vector Analytics)");
    }
}