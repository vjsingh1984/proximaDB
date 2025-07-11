//! Comprehensive unit tests for WAL Batch Factory

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::storage::persistence::wal::{WalBatchFactory, WalStrategyType};
    use crate::storage::WalConfig;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Create test filesystem factory with temporary directory
    async fn create_test_filesystem() -> (Arc<FilesystemFactory>, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        (filesystem, temp_dir)
    }

    /// Create test WAL config with temporary directory
    fn create_test_config(temp_dir: &TempDir) -> WalConfig {
        let mut config = WalConfig::default();
        config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];
        config
    }

    #[tokio::test]
    async fn test_create_avro_batch_strategy() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let config = create_test_config(&temp_dir);

        let strategy = WalBatchFactory::create_strategy(
            WalStrategyType::AvroBatch,
            &config,
            filesystem
        ).await.expect("Failed to create Avro batch strategy");

        assert_eq!(strategy.strategy_name(), "AvroBatch");
    }

    #[tokio::test]
    async fn test_create_bincode_batch_strategy() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let config = create_test_config(&temp_dir);

        let strategy = WalBatchFactory::create_strategy(
            WalStrategyType::BincodeBatch,
            &config,
            filesystem
        ).await.expect("Failed to create Bincode batch strategy");

        assert_eq!(strategy.strategy_name(), "BincodeBatch");
    }

    #[tokio::test]
    async fn test_create_from_config_avro() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let mut config = create_test_config(&temp_dir);
        config.strategy_type = WalStrategyType::AvroBatch;

        let strategy = WalBatchFactory::create_from_config(&config, filesystem)
            .await.expect("Failed to create strategy from config");

        assert_eq!(strategy.strategy_name(), "AvroBatch");
    }

    #[tokio::test]
    async fn test_create_from_config_bincode() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let mut config = create_test_config(&temp_dir);
        config.strategy_type = WalStrategyType::BincodeBatch;

        let strategy = WalBatchFactory::create_from_config(&config, filesystem)
            .await.expect("Failed to create strategy from config");

        assert_eq!(strategy.strategy_name(), "BincodeBatch");
    }

    #[test]
    fn test_available_strategies() {
        let strategies = WalBatchFactory::available_strategies();
        
        assert_eq!(strategies.len(), 2);
        assert!(strategies.contains(&WalStrategyType::AvroBatch));
        assert!(strategies.contains(&WalStrategyType::BincodeBatch));
    }

    #[test]
    fn test_avro_strategy_info() {
        let info = WalBatchFactory::get_strategy_info(&WalStrategyType::AvroBatch);
        
        assert_eq!(info.name, "AvroBatch");
        assert_eq!(info.serialization, "Apache Avro");
        assert!(info.schema_evolution);
        assert!(info.batch_native);
        assert!(!info.recommended_use_cases.is_empty());
        assert!(!info.description.is_empty());
    }

    #[test]
    fn test_bincode_strategy_info() {
        let info = WalBatchFactory::get_strategy_info(&WalStrategyType::BincodeBatch);
        
        assert_eq!(info.name, "BincodeBatch");
        assert_eq!(info.serialization, "Bincode (native Rust)");
        assert!(!info.schema_evolution);
        assert!(info.batch_native);
        assert!(!info.recommended_use_cases.is_empty());
        assert!(!info.description.is_empty());
    }

    #[test]
    fn test_strategy_comparison() {
        let comparison = WalBatchFactory::compare_strategies();
        
        assert!(!comparison.avro_advantages.is_empty());
        assert!(!comparison.bincode_advantages.is_empty());
        assert!(!comparison.recommendation.is_empty());
        
        // Check that Avro advantages mention schema evolution
        assert!(comparison.avro_advantages.iter().any(|adv| adv.to_lowercase().contains("schema")));
        
        // Check that Bincode advantages mention performance
        assert!(comparison.bincode_advantages.iter().any(|adv| adv.contains("performance")));
    }

    #[test]
    fn test_strategy_info_consistency() {
        let avro_info = WalBatchFactory::get_strategy_info(&WalStrategyType::AvroBatch);
        let bincode_info = WalBatchFactory::get_strategy_info(&WalStrategyType::BincodeBatch);
        
        // Both should be batch-native
        assert!(avro_info.batch_native);
        assert!(bincode_info.batch_native);
        
        // Different schema evolution capabilities
        assert!(avro_info.schema_evolution);
        assert!(!bincode_info.schema_evolution);
        
        // Both should have performance profiles
        assert!(!avro_info.performance_profile.is_empty());
        assert!(!bincode_info.performance_profile.is_empty());
    }

    #[tokio::test]
    async fn test_strategy_initialization_and_operation() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let config = create_test_config(&temp_dir);

        // Test both strategies can be created and used
        for strategy_type in &[WalStrategyType::AvroBatch, WalStrategyType::BincodeBatch] {
            let strategy = WalBatchFactory::create_strategy(
                strategy_type.clone(),
                &config,
                filesystem.clone()
            ).await.expect("Failed to create strategy");

            // Verify the strategy has a WAL behavior wrapper
            assert!(strategy.get_wal_behavior().is_some());
            
            // Verify basic operations work
            let stats = strategy.get_stats().await.expect("Failed to get stats");
            assert_eq!(stats.memory_entries, 0); // Should start empty
        }
    }

    #[tokio::test] 
    async fn test_concurrent_strategy_creation() {
        let (filesystem, temp_dir) = create_test_filesystem().await;
        let config = create_test_config(&temp_dir);

        // Create multiple strategies concurrently
        let tasks: Vec<_> = (0..5).map(|i| {
            let fs = filesystem.clone();
            let cfg = config.clone();
            let strategy_type = if i % 2 == 0 { WalStrategyType::AvroBatch } else { WalStrategyType::BincodeBatch };
            
            tokio::spawn(async move {
                WalBatchFactory::create_strategy(strategy_type, &cfg, fs).await
            })
        }).collect();

        // Wait for all to complete
        let results: Vec<_> = futures::future::join_all(tasks).await;
        
        // All should succeed
        for result in results {
            let strategy = result.expect("Task failed").expect("Strategy creation failed");
            assert!(strategy.strategy_name() == "AvroBatch" || strategy.strategy_name() == "BincodeBatch");
        }
    }

    #[test]
    fn test_strategy_selection_guidance() {
        let comparison = WalBatchFactory::compare_strategies();
        
        // The recommendation should guide users on when to use each strategy
        assert!(comparison.recommendation.to_lowercase().contains("avro"));
        assert!(comparison.recommendation.to_lowercase().contains("bincode"));
        
        // Should mention key decision factors
        let rec_lower = comparison.recommendation.to_lowercase();
        assert!(rec_lower.contains("schema") || rec_lower.contains("performance") || rec_lower.contains("rust"));
    }

    #[test]
    fn test_strategy_info_completeness() {
        for strategy_type in &[WalStrategyType::AvroBatch, WalStrategyType::BincodeBatch] {
            let info = WalBatchFactory::get_strategy_info(strategy_type);
            
            // All fields should be populated
            assert!(!info.name.is_empty());
            assert!(!info.description.is_empty());
            assert!(!info.serialization.is_empty());
            assert!(!info.performance_profile.is_empty());
            assert!(!info.recommended_use_cases.is_empty());
            
            // batch_native should always be true for new strategies
            assert!(info.batch_native);
        }
    }
}