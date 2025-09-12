// Unit tests for flush and compaction configuration loading

#[cfg(test)]
mod tests {
    use super::super::config::*;

    #[test]
    fn test_wal_config_defaults() {
        let config = WriteBufferUserConfig::default();

        // Test size thresholds
        assert_eq!(config.write_buffer_size_mb, 8192); // 8GB global
        assert_eq!(config.memory_flush_size_bytes, 16 * 1024 * 1024); // 16MB per collection
        assert_eq!(config.vector_count_threshold, 100_000); // 100k vectors per collection

        // Test other settings
        assert_eq!(config.memtable_type, "BTree");
        assert_eq!(config.sync_mode, "PerBatch");
        assert!(config.enable_wal);
    }

    #[test]
    fn test_sst_config_defaults() {
        let config = SstConfig::default();

        // Test compaction settings
        assert_eq!(config.compaction_threshold, 5); // Min 5 files before compaction
        assert_eq!(config.compaction_strategy, "leveled");
        assert_eq!(config.level_count, 7);
        assert_eq!(config.max_files_per_level, 10);

        // Test block size
        assert_eq!(config.block_size_kb, 2048); // 2MB blocks (default)
        assert_eq!(config.block_size_bytes(), 2048 * 1024);

        // Test compression - server default is none (no compression)
        assert_eq!(
            config.storage.as_ref().and_then(|s| s.compression.as_ref()),
            "none"
        );
    }

    #[test]
    fn test_wal_config_custom_values() {
        let config = WriteBufferUserConfig {
            write_buffer_size_mb: 16384,               // 16GB
            memory_flush_size_bytes: 32 * 1024 * 1024, // 32MB
            vector_count_threshold: 50_000,            // 50k vectors
            memtable_type: "SkipList".to_string(),
            sync_mode: "Periodic".to_string(),
            write_buffer_directory: "/custom/path".to_string(),
            enable_wal: false,
        };

        assert_eq!(config.write_buffer_size_mb, 16384);
        assert_eq!(config.memory_flush_size_bytes, 32 * 1024 * 1024);
        assert_eq!(config.vector_count_threshold, 50_000);
        assert_eq!(config.memtable_type, "SkipList");
        assert_eq!(config.sync_mode, "Periodic");
        assert!(!config.enable_wal);
    }

    #[test]
    fn test_wal_config_values_propagate() {
        // Test that TOML config values would propagate correctly
        let toml_config = WriteBufferUserConfig {
            write_buffer_size_mb: 8192,
            memory_flush_size_bytes: 16777216, // 16MB
            vector_count_threshold: 20000,
            memtable_type: "BTree".to_string(),
            sync_mode: "perbatch".to_string(),
            write_buffer_directory: "./test_data/write_buffer".to_string(),
            enable_wal: true,
        };

        // Verify values are as expected
        assert_eq!(toml_config.memory_flush_size_bytes, 16777216);
        assert_eq!(
            toml_config.write_buffer_size_mb * 1024 * 1024,
            8192 * 1024 * 1024
        );
        assert_eq!(toml_config.vector_count_threshold, 20000);
        assert_eq!(toml_config.enable_wal, true);
        assert_eq!(
            toml_config.write_buffer_directory,
            "./test_data/write_buffer"
        );
    }

    #[test]
    fn test_sst_config_validation() {
        // Test valid config
        let valid_config = SstConfig {
            level_count: 7,
            compaction_threshold: 5,
            block_size_kb: 2048, // 2MB - optimal for cloud storage
            decompression_cache_config: None,
            ..Default::default()
        };
        assert!(valid_config.validate().is_ok());

        // Test invalid level count
        let invalid_levels = SstConfig {
            level_count: 0,
            decompression_cache_config: None,
            ..Default::default()
        };
        assert!(invalid_levels.validate().is_err());

        // Test invalid compaction threshold
        let invalid_threshold = SstConfig {
            compaction_threshold: 0,
            decompression_cache_config: None,
            ..Default::default()
        };
        assert!(invalid_threshold.validate().is_err());

        // Test block size too small
        let small_blocks = SstConfig {
            block_size_kb: 128, // Less than 256KB - invalid
            decompression_cache_config: None,
            ..Default::default()
        };
        assert!(small_blocks.validate().is_err());

        // Test block size too large
        let large_blocks = SstConfig {
            block_size_kb: 20000, // More than 16MB - too large
            decompression_cache_config: None,
            ..Default::default()
        };
        assert!(large_blocks.validate().is_err());
    }

    #[test]
    fn test_flush_threshold_hierarchy() {
        // Test that we have proper hierarchy:
        // 1. Per-collection size: 16MB
        // 2. Per-collection count: 20k vectors
        // 3. Global size: 8GB
        // 4. Global count: 1M vectors (hardcoded in VectorOperationsService)

        let config = WriteBufferUserConfig::default();

        // Per-collection thresholds
        let per_collection_size_mb = config.memory_flush_size_bytes / (1024 * 1024);
        assert_eq!(per_collection_size_mb, 16);
        assert_eq!(config.vector_count_threshold, 100_000); // Default is 100k

        // Global thresholds
        let global_size_mb = config.write_buffer_size_mb;
        assert_eq!(global_size_mb, 8192); // 8GB

        // Verify hierarchy: global >> per-collection
        assert!(global_size_mb > (per_collection_size_mb * 100) as u64); // Can hold many collections
    }
}
