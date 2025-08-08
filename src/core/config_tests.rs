// Tests for StorageConfig with new storage locations

#[cfg(test)]
mod tests {
    use crate::core::config::{StorageConfig, StorageLocation, AssignmentConfig, BloomFilterConfig, WriteBufferUserConfig};
    
    #[test]
    fn test_storage_locations_config() {
        let config = StorageConfig {
            storage_locations: vec![
                StorageLocation {
                    url: "file:///nvme1/proximadb".to_string(),
                    weight: 1,
                    tags: vec!["fast".to_string(), "local".to_string()],
                },
                StorageLocation {
                    url: "s3://my-bucket/proximadb".to_string(),
                    weight: 2,
                    tags: vec!["cloud".to_string(), "archive".to_string()],
                },
            ],
            metadata_url: "file:///nvme1/proximadb/metadata".to_string(),
            assignment_config: AssignmentConfig::default(),
            mmap_enabled: true,
            sst_config: Default::default(),
            viper_config: Default::default(),
            wal_config: Default::default(),
            cache_size_mb: 2048,
            bloom_filter_config: Some(BloomFilterConfig {
                bits_per_key: 12,
                enabled: true,
                ..Default::default()
            }),
            filesystem_config: Default::default(),
        };
        
        let storage_urls = config.get_storage_urls();
        assert_eq!(storage_urls.len(), 2);
        assert_eq!(storage_urls[0], "file:///nvme1/proximadb");
        assert_eq!(storage_urls[1], "s3://my-bucket/proximadb");
        
        assert_eq!(config.metadata_url, "file:///nvme1/proximadb/metadata");
    }
    
    #[test]
    fn test_url_derivation() {
        let config = StorageConfig {
            storage_locations: vec![
                StorageLocation {
                    url: "file:///nvme1/proximadb".to_string(),
                    weight: 1,
                    tags: vec![],
                },
                StorageLocation {
                    url: "s3://bucket/proximadb/".to_string(), // With trailing slash
                    weight: 1,
                    tags: vec![],
                },
            ],
            metadata_url: "file:///fast-ssd/metadata".to_string(),
            viper_config: Default::default(),
            wal_config: Default::default(),
            ..Default::default()
        };
        
        let write_buffer_urls = config.get_write_buffer_urls();
        assert_eq!(write_buffer_urls.len(), 2);
        assert_eq!(write_buffer_urls[0], "file:///nvme1/proximadb/wal");
        assert_eq!(write_buffer_urls[1], "s3://bucket/proximadb/wal"); // Trailing slash handled
        
        let data_urls = config.get_data_urls();
        assert_eq!(data_urls.len(), 2);
        assert_eq!(data_urls[0], "file:///nvme1/proximadb/data");
        assert_eq!(data_urls[1], "s3://bucket/proximadb/data");
        
        let index_urls = config.get_index_urls();
        assert_eq!(index_urls.len(), 2);
        assert_eq!(index_urls[0], "file:///nvme1/proximadb/index");
        assert_eq!(index_urls[1], "s3://bucket/proximadb/index");
    }
    
    #[test]
    fn test_heterogeneous_storage() {
        let config = StorageConfig {
            storage_locations: vec![
                StorageLocation {
                    url: "file:///local/proximadb".to_string(),
                    weight: 1,
                    tags: vec!["local".to_string()],
                },
                StorageLocation {
                    url: "s3://aws-bucket/proximadb".to_string(),
                    weight: 2,
                    tags: vec!["cloud".to_string(), "aws".to_string()],
                },
                StorageLocation {
                    url: "gs://gcp-bucket/proximadb".to_string(),
                    weight: 2,
                    tags: vec!["cloud".to_string(), "gcp".to_string()],
                },
                StorageLocation {
                    url: "adls://azure-account.dfs.core.windows.net/container/proximadb".to_string(),
                    weight: 2,
                    tags: vec!["cloud".to_string(), "azure".to_string()],
                },
            ],
            metadata_url: "file:///fast-ssd/metadata".to_string(),
            viper_config: Default::default(),
            wal_config: Default::default(),
            ..Default::default()
        };
        
        let urls = config.get_storage_urls();
        assert_eq!(urls.len(), 4);
        assert!(urls[0].starts_with("file://"));
        assert!(urls[1].starts_with("s3://"));
        assert!(urls[2].starts_with("gs://"));
        assert!(urls[3].starts_with("adls://"));
        
        // WAL URLs should be derived correctly for each
        let write_buffer_urls = config.get_write_buffer_urls();
        assert_eq!(write_buffer_urls.len(), 4);
        assert_eq!(write_buffer_urls[0], "file:///local/proximadb/wal");
        assert_eq!(write_buffer_urls[1], "s3://aws-bucket/proximadb/wal");
        assert_eq!(write_buffer_urls[2], "gs://gcp-bucket/proximadb/wal");
        assert_eq!(write_buffer_urls[3], "adls://azure-account.dfs.core.windows.net/container/proximadb/wal");
    }
    
    #[test]
    fn test_assignment_config() {
        let config = StorageConfig {
            storage_locations: vec![
                StorageLocation {
                    url: "file:///disk1".to_string(),
                    weight: 1,
                    tags: vec![],
                },
            ],
            metadata_url: "file:///disk1/metadata".to_string(),
            assignment_config: AssignmentConfig {
                strategy: "hash".to_string(),
                affinity: true,
            },
            viper_config: Default::default(),
            wal_config: Default::default(),
            ..Default::default()
        };
        
        assert_eq!(config.assignment_config.strategy, "hash");
        assert_eq!(config.assignment_config.affinity, true);
    }
    
    #[test]
    fn test_default_storage_config() {
        let config = StorageConfig::default();
        
        // Should have default storage locations
        assert!(!config.storage_locations.is_empty());
        
        // Should have proper metadata URL
        assert!(!config.metadata_url.is_empty());
        assert!(config.metadata_url.starts_with("file://"));
        
        // Assignment config should default to hash with affinity
        assert_eq!(config.assignment_config.strategy, "hash");
        assert_eq!(config.assignment_config.affinity, true);
    }
    
    #[test]
    fn test_wal_config_values() {
        // Test with custom values that should be used instead of defaults
        let wal_config = WriteBufferUserConfig {
            write_buffer_size_mb: 8192,  // 8GB
            memory_flush_size_bytes: 16777216,  // 16MB
            vector_count_threshold: 100_000,  // 100k vectors
            memtable_type: "BTree".to_string(),
            sync_mode: "PerBatch".to_string(),
            write_buffer_directory: "./test_wal".to_string(),
            enable_wal: true,
        };
        
        // Verify the values are set correctly
        assert_eq!(wal_config.write_buffer_size_mb, 8192);
        assert_eq!(wal_config.memory_flush_size_bytes, 16777216); // 16MB not 2MB!
        assert_eq!(wal_config.memtable_type, "BTree");
        assert_eq!(wal_config.sync_mode, "PerBatch");
        assert_eq!(wal_config.write_buffer_directory, "./test_wal");
        assert!(wal_config.enable_wal);
    }
    
    #[test]
    fn test_wal_config_from_toml() {
        // Test loading from TOML string
        let toml_str = r#"
            write_buffer_size_mb = 4096
            memory_flush_size_bytes = 33554432  # 32MB
            vector_count_threshold = 10000
            memtable_type = "SkipList"
            sync_mode = "Periodic"
            write_buffer_directory = "/tmp/wal"
            enable_wal = false
        "#;
        
        let wal_config: WriteBufferUserConfig = toml::from_str(toml_str).unwrap();
        
        assert_eq!(wal_config.write_buffer_size_mb, 4096);
        assert_eq!(wal_config.memory_flush_size_bytes, 33554432); // 32MB
        assert_eq!(wal_config.vector_count_threshold, 10000);
        assert_eq!(wal_config.memtable_type, "SkipList");
        assert_eq!(wal_config.sync_mode, "Periodic");
        assert_eq!(wal_config.write_buffer_directory, "/tmp/wal");
        assert!(!wal_config.enable_wal);
    }
}