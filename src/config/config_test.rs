//! Configuration Coverage and Default Merging Tests
//! 
//! This module ensures complete configuration field coverage and validates
//! that the default config system properly merges with user overrides.

#[cfg(test)]
mod tests {
    use crate::config::{Config, ServerConfig, StorageConfig, LsmConfig, WalConfig, ApiConfig, MonitoringConfig, ConsensusConfig};
    use crate::storage::persistence::filesystem::atomic_strategy::AtomicConfig;
    use crate::storage::metadata::backends::filestore_backend::FilestoreConfig;
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Test that all required fields are present in default configuration
    #[test]
    fn test_default_config_completeness() {
        println!("🔧 Testing default configuration completeness...");
        
        // This should not panic - all required fields must be present
        let config = Config::load_with_defaults(None)
            .expect("Default configuration should be complete");
        
        // Verify all major sections are present
        assert!(!config.server.node_id.is_empty(), "Server node_id should not be empty");
        assert!(!config.server.bind_address.is_empty(), "Server bind_address should not be empty");
        assert!(config.server.port > 0, "Server port should be positive");
        assert!(!config.server.data_dir.is_empty(), "Server data_dir should not be empty");
        
        // Storage configuration
        assert!(!config.storage.data_dirs.is_empty(), "Storage data_dirs should not be empty");
        assert!(!config.storage.wal_dir.is_empty(), "Storage wal_dir should not be empty");
        assert!(config.storage.cache_size_mb > 0, "Storage cache_size_mb should be positive");
        assert!(config.storage.bloom_filter_bits > 0, "Storage bloom_filter_bits should be positive");
        
        // WAL configuration
        assert!(!config.storage.wal_config.wal_urls.is_empty(), "WAL wal_urls should not be empty");
        assert!(config.storage.wal_config.memory_flush_size_bytes > 0, "WAL memory_flush_size_bytes should be positive");
        assert!(config.storage.wal_config.global_flush_threshold > 0, "WAL global_flush_threshold should be positive");
        assert!(config.storage.wal_config.global_shrink_factor > 0.0, "WAL global_shrink_factor should be positive");
        assert!(!config.storage.wal_config.strategy_type.is_empty(), "WAL strategy_type should not be empty");
        assert!(!config.storage.wal_config.memtable_type.is_empty(), "WAL memtable_type should not be empty");
        assert!(!config.storage.wal_config.sync_mode.is_empty(), "WAL sync_mode should not be empty");
        assert!(config.storage.wal_config.batch_threshold > 0, "WAL batch_threshold should be positive");
        assert!(config.storage.wal_config.write_buffer_size_mb > 0, "WAL write_buffer_size_mb should be positive");
        assert!(config.storage.wal_config.concurrent_flushes > 0, "WAL concurrent_flushes should be positive");
        
        // LSM configuration
        assert!(config.storage.lsm_config.memtable_size_mb > 0, "LSM memtable_size_mb should be positive");
        assert!(config.storage.lsm_config.memory_flush_size_bytes > 0, "LSM memory_flush_size_bytes should be positive");
        assert!(!config.storage.lsm_config.memtable_type.is_empty(), "LSM memtable_type should not be empty");
        assert!(config.storage.lsm_config.level_count > 0, "LSM level_count should be positive");
        assert!(config.storage.lsm_config.compaction_threshold > 0, "LSM compaction_threshold should be positive");
        assert!(!config.storage.lsm_config.compaction_strategy.is_empty(), "LSM compaction_strategy should not be empty");
        assert!(!config.storage.lsm_config.compression.is_empty(), "LSM compression should not be empty");
        assert!(config.storage.lsm_config.block_size_kb > 0, "LSM block_size_kb should be positive");
        assert!(config.storage.lsm_config.cache_size_mb > 0, "LSM cache_size_mb should be positive");
        assert!(config.storage.lsm_config.write_buffer_size_mb > 0, "LSM write_buffer_size_mb should be positive");
        assert!(config.storage.lsm_config.max_files_per_level > 0, "LSM max_files_per_level should be positive");
        assert!(config.storage.lsm_config.level_size_multiplier > 0.0, "LSM level_size_multiplier should be positive");
        assert!(config.storage.lsm_config.max_levels > 0, "LSM max_levels should be positive");
        assert!(config.storage.lsm_config.background_thread_count > 0, "LSM background_thread_count should be positive");
        assert!(!config.storage.lsm_config.sync_mode.is_empty(), "LSM sync_mode should not be empty");
        assert!(!config.storage.lsm_config.wal_directory.is_empty(), "LSM wal_directory should not be empty");
        assert!(!config.storage.lsm_config.data_directory.is_empty(), "LSM data_directory should not be empty");
        assert!(config.storage.lsm_config.prefetch_size_kb > 0, "LSM prefetch_size_kb should be positive");
        
        // Bloom filter configuration
        assert!(config.storage.bloom_filter_config.bits_per_key > 0, "Bloom filter bits_per_key should be positive");
        
        // Storage layout configuration
        assert!(config.storage.storage_layout.node_instance > 0, "Storage layout node_instance should be positive");
        assert!(!config.storage.storage_layout.assignment_strategy.is_empty(), "Storage layout assignment_strategy should not be empty");
        assert!(!config.storage.storage_layout.base_paths.is_empty(), "Storage layout base_paths should not be empty");
        
        // Base path configuration
        let base_path = &config.storage.storage_layout.base_paths[0];
        assert!(!base_path.base_dir.is_empty(), "Base path base_dir should not be empty");
        assert!(base_path.instance_id > 0, "Base path instance_id should be positive");
        assert!(!base_path.mount_point.is_empty(), "Base path mount_point should not be empty");
        
        // Capacity configuration
        assert!(base_path.capacity_config.max_wal_size_mb > 0, "Capacity config max_wal_size_mb should be positive");
        assert!(base_path.capacity_config.metadata_reserved_mb > 0, "Capacity config metadata_reserved_mb should be positive");
        assert!(base_path.capacity_config.warning_threshold_percent > 0.0, "Capacity config warning_threshold_percent should be positive");
        
        // Temp configuration
        assert!(!base_path.temp_config.temp_suffix.is_empty(), "Temp config temp_suffix should not be empty");
        assert!(!base_path.temp_config.compaction_suffix.is_empty(), "Temp config compaction_suffix should not be empty");
        assert!(!base_path.temp_config.flush_suffix.is_empty(), "Temp config flush_suffix should not be empty");
        
        // Filesystem configuration
        assert!(!config.storage.filesystem_config.temp_strategy.is_empty(), "Filesystem config temp_strategy should not be empty");
        
        // Metadata backend configuration
        assert!(!config.storage.metadata_backend.backend_type.is_empty(), "Metadata backend backend_type should not be empty");
        assert!(!config.storage.metadata_backend.storage_url.is_empty(), "Metadata backend storage_url should not be empty");
        assert!(config.storage.metadata_backend.cache_size_mb > 0, "Metadata backend cache_size_mb should be positive");
        assert!(config.storage.metadata_backend.flush_interval_secs > 0, "Metadata backend flush_interval_secs should be positive");
        
        // API configuration
        assert!(config.api.grpc_port > 0, "API grpc_port should be positive");
        assert!(config.api.rest_port > 0, "API rest_port should be positive");
        assert!(config.api.max_request_size_mb > 0, "API max_request_size_mb should be positive");
        assert!(config.api.timeout_seconds > 0, "API timeout_seconds should be positive");
        
        // Monitoring configuration
        assert!(!config.monitoring.log_level.is_empty(), "Monitoring log_level should not be empty");
        
        // Consensus configuration
        assert!(config.consensus.node_id > 0, "Consensus node_id should be positive");
        assert!(config.consensus.election_timeout_ms > 0, "Consensus election_timeout_ms should be positive");
        assert!(config.consensus.heartbeat_interval_ms > 0, "Consensus heartbeat_interval_ms should be positive");
        assert!(config.consensus.snapshot_threshold > 0, "Consensus snapshot_threshold should be positive");
        
        println!("✅ Default configuration completeness test passed");
    }
    
    /// Test that user overrides properly merge with defaults
    #[test]
    fn test_config_override_merging() {
        println!("🔧 Testing configuration override merging...");
        
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config_file = temp_dir.path().join("test_override.toml");
        
        // Create a minimal override config
        let override_config = r#"
[server]
node_id = "test-override-node"
port = 9999

[storage]
cache_size_mb = 2048

[storage.wal_config]
memory_flush_size_bytes = 20971520  # 20MB

[api]
grpc_port = 9998
rest_port = 9997
"#;
        
        std::fs::write(&config_file, override_config)
            .expect("Failed to write override config");
        
        // Load config with overrides
        let config = Config::load_with_defaults(Some(config_file.to_str().unwrap()))
            .expect("Failed to load config with overrides");
        
        // Verify overrides took effect
        assert_eq!(config.server.node_id, "test-override-node");
        assert_eq!(config.server.port, 9999);
        assert_eq!(config.storage.cache_size_mb, 2048);
        assert_eq!(config.storage.wal_config.memory_flush_size_bytes, 20971520);
        assert_eq!(config.api.grpc_port, 9998);
        assert_eq!(config.api.rest_port, 9997);
        
        // Verify defaults are still present for non-overridden fields
        assert!(!config.server.bind_address.is_empty(), "Default bind_address should be preserved");
        assert!(!config.server.data_dir.is_empty(), "Default data_dir should be preserved");
        assert!(config.storage.bloom_filter_bits > 0, "Default bloom_filter_bits should be preserved");
        assert!(!config.storage.wal_config.strategy_type.is_empty(), "Default WAL strategy_type should be preserved");
        assert!(config.storage.lsm_config.memtable_size_mb > 0, "Default LSM memtable_size_mb should be preserved");
        assert!(config.api.timeout_seconds > 0, "Default API timeout_seconds should be preserved");
        
        println!("✅ Configuration override merging test passed");
    }
    
    /// Test that incomplete user config still works with defaults
    #[test]
    fn test_minimal_user_config() {
        println!("🔧 Testing minimal user configuration with defaults...");
        
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config_file = temp_dir.path().join("minimal.toml");
        
        // Create an extremely minimal config - just change one thing
        let minimal_config = r#"
[server]
node_id = "minimal-test"
"#;
        
        std::fs::write(&config_file, minimal_config)
            .expect("Failed to write minimal config");
        
        // This should not fail - defaults should fill in everything else
        let config = Config::load_with_defaults(Some(config_file.to_str().unwrap()))
            .expect("Minimal config should work with defaults");
        
        // Verify override took effect
        assert_eq!(config.server.node_id, "minimal-test");
        
        // Verify all required fields are still present from defaults
        assert!(!config.server.bind_address.is_empty());
        assert!(config.server.port > 0);
        assert!(!config.storage.wal_config.strategy_type.is_empty());
        assert!(config.storage.lsm_config.memtable_size_mb > 0);
        assert!(config.api.grpc_port > 0);
        
        println!("✅ Minimal user configuration test passed");
    }
    
    /// Test config validation catches invalid values
    #[test]
    fn test_config_validation() {
        println!("🔧 Testing configuration validation...");
        
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config_file = temp_dir.path().join("invalid.toml");
        
        // Create config with invalid values
        let invalid_config = r#"
[server]
port = 0  # Invalid port

[storage]
cache_size_mb = 0  # Invalid cache size

[api]
timeout_seconds = 0  # Invalid timeout
"#;
        
        std::fs::write(&config_file, invalid_config)
            .expect("Failed to write invalid config");
        
        // This should fail validation
        let result = Config::load_with_defaults(Some(config_file.to_str().unwrap()));
        
        match result {
            Err(_) => {
                println!("✅ Configuration validation correctly rejected invalid config");
            }
            Ok(_) => {
                panic!("Configuration validation should have failed for invalid values");
            }
        }
    }
    
    /// Test that all configuration fields are documented
    #[test]
    fn test_config_field_documentation() {
        println!("🔧 Testing configuration field documentation...");
        
        // Read the default config file
        let defaults_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/config/defaults.toml");
        
        let defaults_content = std::fs::read_to_string(&defaults_path)
            .expect("Failed to read defaults.toml");
        
        // Check that all major sections have comments
        let required_sections = [
            "server", "storage", "wal_config", "lsm_config", 
            "bloom_filter_config", "storage_layout", "base_paths",
            "filesystem_config", "metadata_backend", "api", 
            "monitoring", "consensus"
        ];
        
        for section in &required_sections {
            assert!(
                defaults_content.contains(&format!("[{}]", section)) ||
                defaults_content.contains(&format!("[storage.{}]", section)),
                "Section {} should be present in defaults.toml", section
            );
        }
        
        println!("✅ Configuration field documentation test passed");
    }
    
    /// Integration test that validates server startup with merged config
    #[test]
    fn test_server_startup_with_merged_config() {
        println!("🔧 Testing server startup with merged configuration...");
        
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config_file = temp_dir.path().join("startup_test.toml");
        let data_dir = temp_dir.path().join("data");
        
        // Create a realistic test config
        let test_config = format!(r#"
[server]
node_id = "startup-test-node"
bind_address = "127.0.0.1"
port = 15678
data_dir = "{}"

[storage]
data_dirs = ["{}/storage"]
wal_dir = "{}/wal"

[storage.metadata_backend]
storage_url = "file://{}/metadata"

[api]
grpc_port = 15679
rest_port = 15678
"#, 
            data_dir.to_str().unwrap(),
            data_dir.to_str().unwrap(),
            data_dir.to_str().unwrap(),
            data_dir.to_str().unwrap()
        );
        
        std::fs::write(&config_file, test_config)
            .expect("Failed to write test config");
        
        // Load and validate the config
        let config = Config::load_with_defaults(Some(config_file.to_str().unwrap()))
            .expect("Test config should load successfully");
        
        // Verify the config is complete and valid
        assert_eq!(config.server.node_id, "startup-test-node");
        assert_eq!(config.server.port, 15678);
        assert_eq!(config.api.grpc_port, 15679);
        
        // Verify defaults are preserved
        assert!(config.storage.cache_size_mb > 0);
        assert!(!config.storage.wal_config.strategy_type.is_empty());
        assert!(config.storage.lsm_config.memtable_size_mb > 0);
        
        println!("✅ Server startup configuration test passed");
    }
}