//! Comprehensive tests for config and config_loader modules
//! Target: 80%+ coverage for configuration handling

use proximadb::core::config::{
    ApiConfig, Config, ConsensusConfig, MonitoringConfig, ServerConfig, SstConfig, StorageConfig,
    StorageLocation,
};
use std::env;
use std::io::Write;
use tempfile::{NamedTempFile, TempDir};

#[test]
fn test_default_config() {
    let config = Config::default();

    // Test default server config
    assert_eq!(config.server.node_id, "node-1");
    assert_eq!(config.server.bind_address, "127.0.0.1");
    assert_eq!(config.server.port, 5678);

    // Test default storage config
    assert!(!config.storage.storage_locations.is_empty());
    assert!(config.storage.metadata_url.contains("metadata"));
    assert_eq!(config.storage.cache_size_mb, 512);
    assert!(config.storage.mmap_enabled);

    // Test default SST config - sst_config is Option<SstConfig>
    if let Some(ref sst_config) = config.storage.sst_config {
        assert_eq!(sst_config.level_count, 7);
        assert_eq!(sst_config.compaction_threshold, 3);
        // Note: memtable_size_mb and enable_write_ahead_log fields no longer exist
    }

    // Test default API config
    assert_eq!(config.api.rest_port, 5678);
    assert_eq!(config.api.grpc_port, 5679);
    assert_eq!(config.api.max_request_size_mb, 100);
    assert_eq!(config.api.timeout_seconds, 30);
}

#[test]
fn test_config_from_toml() {
    let toml_content = r#"
[server]
node_id = "test-node"
bind_address = "127.0.0.1"
port = 5678
data_dir = "/custom/data"

[storage]
storage_locations = [{url = "file:///custom/storage", weight = 1, tags = ["primary"]}]
metadata_url = "file:///custom/metadata"
cache_size_mb = 512
mmap_enabled = true

[storage.sst_config]
memtable_size_mb = 128
level_count = 5
compaction_threshold = 3
enable_write_ahead_log = true
write_ahead_log_directory = "/custom/write_ahead_log"
data_directory = "/custom/sst_data"

[api]
rest_port = 8080
grpc_port = 9090
max_request_size_mb = 200
timeout_seconds = 60

[consensus]
node_id = "test-consensus-node"
cluster_peers = []
election_timeout_ms = 300
heartbeat_interval_ms = 100

[monitoring]
metrics_enabled = true
log_level = "debug"
"#;

    let config: Config = toml::from_str(toml_content).unwrap();

    assert_eq!(config.server.node_id, "test-node");
    assert_eq!(config.server.bind_address, "127.0.0.1");
    assert_eq!(config.storage.cache_size_mb, 512);
    assert!(config.storage.mmap_enabled);

    // Check SST config if present
    if let Some(ref sst_config) = config.storage.sst_config {
        assert_eq!(sst_config.level_count, 5);
        // Note: memtable_size_mb and enable_write_ahead_log fields no longer exist
    }

    assert_eq!(config.api.rest_port, 8080);
    assert_eq!(config.api.grpc_port, 9090);
    assert!(config.monitoring.metrics_enabled);
}

/*
#[test]
fn test_config_loader_from_file() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("test_config.toml");

    let config_content = r#"
[storage]
engine = "viper"
data_dir = "./test_data"

[collections]
default_distance_metric = "manhattan"

[network]
rest_port = 7777
"#;

    std::fs::write(&config_path, config_content).unwrap();

    let loader = ConfigLoader::new();
    let config = loader.load_from_file(config_path.to_str().unwrap()).unwrap();

    assert_eq!(config.storage.engine, "viper");
    assert_eq!(config.storage.data_dir, "./test_data");
    assert_eq!(config.collections.default_distance_metric, "manhattan");
    assert_eq!(config.network.rest_port, 7777);
}*/

/*#[test]
fn test_config_loader_from_env() {
    // Save current env vars
    let saved_engine = env::var("PROXIMADB_STORAGE_ENGINE").ok();
    let saved_port = env::var("PROXIMADB_NETWORK_REST_PORT").ok();

    // Set test env vars
    env::set_var("PROXIMADB_STORAGE_ENGINE", "lsm");
    env::set_var("PROXIMADB_NETWORK_REST_PORT", "9999");
    env::set_var("PROXIMADB_STORAGE_MAX_MEMORY_USAGE", "4294967296");
    env::set_var("PROXIMADB_COLLECTIONS_ENABLE_AUTO_ID", "false");

    let loader = ConfigLoader::new();
    let config = loader.load_from_env().unwrap();

    assert_eq!(config.storage.engine, "lsm");
    assert_eq!(config.network.rest_port, 9999);
    assert_eq!(config.storage.max_memory_usage, 4294967296);
    assert!(!config.collections.enable_auto_id);

    // Restore env vars
    match saved_engine {
        Some(val) => env::set_var("PROXIMADB_STORAGE_ENGINE", val),
        None => env::remove_var("PROXIMADB_STORAGE_ENGINE"),
    }
    match saved_port {
        Some(val) => env::set_var("PROXIMADB_NETWORK_REST_PORT", val),
        None => env::remove_var("PROXIMADB_NETWORK_REST_PORT"),
    }
    env::remove_var("PROXIMADB_STORAGE_MAX_MEMORY_USAGE");
    env::remove_var("PROXIMADB_COLLECTIONS_ENABLE_AUTO_ID");
}*/

/*#[test]
fn test_config_loader_precedence() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("precedence_test.toml");

    // File config
    let file_content = r#"
[storage]
engine = "viper"
data_dir = "./file_data"

[network]
rest_port = 5555
"#;
    std::fs::write(&config_path, file_content).unwrap();

    // Set env var that should override file
    env::set_var("PROXIMADB_STORAGE_ENGINE", "lsm");

    let loader = ConfigLoader::new();
    let config = loader.load_with_precedence(Some(config_path.to_str().unwrap())).unwrap();

    // Env var should override file
    assert_eq!(config.storage.engine, "lsm");
    // File value should be used when no env var
    assert_eq!(config.storage.data_dir, "./file_data");
    assert_eq!(config.network.rest_port, 5555);

    env::remove_var("PROXIMADB_STORAGE_ENGINE");
}*/

/*#[test]
fn test_config_validation() {
    let mut config = Config::default();

    // Valid config should pass
    assert!(config.validate().is_ok());

    // Invalid storage engine
    config.storage.engine = "invalid_engine".to_string();
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid storage engine"));

    // Reset and test invalid distance metric
    config = Config::default();
    config.collections.default_distance_metric = "invalid_metric".to_string();
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid distance metric"));

    // Test invalid port
    config = Config::default();
    config.network.rest_port = 0;
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid REST port"));

    // Test port conflict
    config = Config::default();
    config.network.rest_port = 8080;
    config.network.grpc_port = 8080;
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("REST and gRPC ports cannot be the same"));
}*/

/*#[test]
fn test_storage_config_methods() {
    let config = StorageConfig::default();

    // Test path methods
    assert_eq!(config.data_path(), std::path::Path::new("./data"));
    assert_eq!(config.wal_path(), std::path::Path::new("./wal"));
    assert_eq!(config.cache_path(), std::path::Path::new("./cache"));

    // Test with custom paths
    let mut custom_config = StorageConfig::default();
    custom_config.data_dir = "/custom/data".to_string();
    custom_config.wal_dir = "/custom/wal".to_string();

    assert_eq!(custom_config.data_path(), std::path::Path::new("/custom/data"));
    assert_eq!(custom_config.wal_path(), std::path::Path::new("/custom/wal"));
}*/

/*#[test]
fn test_config_error_handling() {
    let loader = ConfigLoader::new();

    // Test loading non-existent file
    let result = loader.load_from_file("/non/existent/path.toml");
    assert!(result.is_err());
    match result.unwrap_err() {
        ConfigError::IoError(_) => (),
        _ => panic!("Expected IoError"),
    }

    // Test loading invalid TOML
    let temp_file = NamedTempFile::new().unwrap();
    writeln!(temp_file.as_file(), "invalid toml content").unwrap();

    let result = loader.load_from_file(temp_file.path().to_str().unwrap());
    assert!(result.is_err());
    match result.unwrap_err() {
        ConfigError::ParseError(_) => (),
        _ => panic!("Expected ParseError"),
    }
}*/

/*#[test]
fn test_collections_config_defaults() {
    let config = CollectionsConfig::default();

    assert_eq!(config.default_distance_metric, "cosine");
    assert!(config.enable_auto_id);
    assert_eq!(config.default_shard_count, 1);
    assert_eq!(config.default_replication_factor, 1);
    assert_eq!(config.max_vectors_per_collection, None);
    assert_eq!(config.max_dimension, None);
}*/

/*#[test]
fn test_network_config_tls() {
    let mut config = NetworkConfig::default();

    // No TLS by default
    assert_eq!(config.enable_tls, None);
    assert_eq!(config.tls_cert_path, None);
    assert_eq!(config.tls_key_path, None);

    // Enable TLS
    config.enable_tls = Some(true);
    config.tls_cert_path = Some("/path/to/cert.pem".to_string());
    config.tls_key_path = Some("/path/to/key.pem".to_string());

    // Should validate TLS config when enabled
    assert!(config.enable_tls.unwrap());
    assert_eq!(config.tls_cert_path.as_ref().unwrap(), "/path/to/cert.pem");
}*/

/*#[test]
fn test_config_serialization() {
    let config = Config::default();

    // Serialize to TOML
    let toml_str = toml::to_string(&config).unwrap();
    assert!(toml_str.contains("[storage]"));
    assert!(toml_str.contains("[collections]"));
    assert!(toml_str.contains("[network]"));

    // Deserialize back
    let deserialized: Config = toml::from_str(&toml_str).unwrap();
    assert_eq!(deserialized.storage.engine, config.storage.engine);
    assert_eq!(deserialized.collections.default_distance_metric, config.collections.default_distance_metric);
    assert_eq!(deserialized.network.rest_port, config.network.rest_port);
}*/

/*#[test]
fn test_environment_variable_parsing() {
    // Test boolean parsing
    env::set_var("TEST_BOOL_TRUE", "true");
    env::set_var("TEST_BOOL_FALSE", "false");
    env::set_var("TEST_BOOL_1", "1");
    env::set_var("TEST_BOOL_0", "0");

    assert_eq!(env::var("TEST_BOOL_TRUE").unwrap().parse::<bool>().unwrap(), true);
    assert_eq!(env::var("TEST_BOOL_FALSE").unwrap().parse::<bool>().unwrap(), false);
    assert_eq!(env::var("TEST_BOOL_1").unwrap(), "1");
    assert_eq!(env::var("TEST_BOOL_0").unwrap(), "0");

    // Test number parsing
    env::set_var("TEST_NUM", "42");
    assert_eq!(env::var("TEST_NUM").unwrap().parse::<u16>().unwrap(), 42);

    // Cleanup
    env::remove_var("TEST_BOOL_TRUE");
    env::remove_var("TEST_BOOL_FALSE");
    env::remove_var("TEST_BOOL_1");
    env::remove_var("TEST_BOOL_0");
    env::remove_var("TEST_NUM");
}*/
