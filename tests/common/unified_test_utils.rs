//! Unified Test Utilities for ProximaDB
//! 
//! This module provides comprehensive test utilities for all ProximaDB components:
//! - SST storage engine tests
//! - VIPER storage engine tests  
//! - Flush and compaction operations
//! - Vector operations and search
//! - Metadata handling
//! 
//! This replaces the fragmented test utilities scattered across different modules
//! and provides a consistent, reliable test infrastructure.

use anyhow::Result;
use tracing::{debug, error, info, warn};
use std::sync::{Arc, Once};
use std::path::PathBuf;
use tempfile::TempDir;
use uuid::Uuid;

// Core ProximaDB imports
use proximadb::core::{SstConfig, BloomFilterConfig, VectorRecord};
use proximadb::core::config::{WriteBufferUserConfig, ViperConfig};
use proximadb::core::config::StorageLocation;
use proximadb::proto::proximadb::{MetadataItem, Collection, CollectionConfig, StorageAssignment, CollectionStats, DistanceMetric, StorageEngine};
use proximadb::compute::distance_computation::UnifiedDistanceCompute;
use proximadb::core::hardware_capabilities::HardwareBackend;
use proximadb::storage::engines::sst::SstStorage;
use proximadb::storage::engines::viper::ViperEngine;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::traits::{FlushParameters, CompactionParameters, UnifiedStorageEngine};

static HARDWARE_INIT: Once = Once::new();

/// Setup hardware capabilities for tests (shared initialization)
pub fn setup_hardware_capabilities() {
    HARDWARE_INIT.call_once(|| {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    });
}

/// Create a unique collection ID for tests
pub fn unique_collection_id(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::new_v4().simple())
}

/// Setup test directories with proper structure
pub async fn setup_test_directories(base_path: &std::path::Path) -> Result<()> {
    use tokio::fs;
    fs::create_dir_all(base_path).await?;
    fs::create_dir_all(base_path.join("data")).await?;
    fs::create_dir_all(base_path.join("write_ahead_log")).await?;
    fs::create_dir_all(base_path.join("index")).await?;
    fs::create_dir_all(base_path.join("metadata")).await?;
    fs::create_dir_all(base_path.join("storage")).await?;
    Ok(())
}

/// Comprehensive test environment for all ProximaDB components
pub struct UnifiedTestEnvironment {
    pub collection_id: String,
    pub temp_dir: TempDir,
    pub persistent_dir: PathBuf,
    pub filesystem: Arc<FilesystemFactory>,
    pub sst_config: SstConfig,
    pub viper_config: ViperConfig,
    pub storage_locations: Vec<StorageLocation>,
}

impl UnifiedTestEnvironment {
    /// Create a new comprehensive test environment
    pub async fn new() -> Result<Self> {
        setup_hardware_capabilities();
        
        let collection_id = unique_collection_id("unified_test");
        let temp_dir = TempDir::new()?;
        
        // Create persistent directory for reliable testing
        let persistent_base = std::env::temp_dir().join("proximadb_unified_tests");
        tokio::fs::create_dir_all(&persistent_base).await?;
        let persistent_dir = persistent_base.join(&collection_id);
        tokio::fs::create_dir_all(&persistent_dir).await?;
        
        info!("🏗️ Created unified test environment: {}", persistent_dir.display());
        info!("🏗️ Collection ID: {}", collection_id);
        
        // Setup all required directories
        setup_test_directories(&persistent_dir).await?;
        
        let filesystem = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);
        
        // Create optimized configs for testing
        let sst_config = Self::create_test_sst_config(&persistent_dir);
        let viper_config = Self::create_test_viper_config(&persistent_dir);
        
        let storage_locations = vec![
            StorageLocation {
                url: format!("file://{}", persistent_dir.to_str().unwrap()),
                weight: 1,
                tags: vec!["unified_test".to_string()],
            }
        ];
        
        Ok(Self {
            collection_id,
            temp_dir,
            persistent_dir,
            filesystem,
            sst_config,
            viper_config,
            storage_locations,
        })
    }
    
    /// Create an SST storage engine for this environment
    pub async fn create_sst_engine(&self) -> Result<SstStorage> {
        info!("🏭 Creating SST engine for collection: {}", self.collection_id);
        
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(
            proximadb::compute::distance_computation::DistanceMetric::Cosine
        ));
        
        SstStorage::new(
            self.sst_config.clone(),
            self.filesystem.clone(),
            distance_compute
        ).await
    }
    
    /// Create a VIPER storage engine for this environment
    pub async fn create_viper_engine(&self) -> Result<ViperEngine> {
        info!("🏭 Creating VIPER engine for collection: {}", self.collection_id);
        
        ViperEngine::from_core_config(
            self.viper_config.clone(),
            self.filesystem.clone()
        ).await
    }
    
    /// Get the data directory path for SST operations
    pub fn get_sst_data_directory(&self) -> PathBuf {
        // SST writes to {base_location}/{collection_id}/data
        self.persistent_dir.join("data")
    }
    
    /// Get the data directory path for VIPER operations
    pub fn get_viper_data_directory(&self) -> PathBuf {
        // VIPER writes to configured data directory
        PathBuf::from(&self.viper_config.data_directory)
    }
    
    /// Ensure data directories exist for operations
    pub async fn ensure_all_directories(&self) -> Result<()> {
        tokio::fs::create_dir_all(self.get_sst_data_directory()).await?;
        tokio::fs::create_dir_all(self.get_viper_data_directory()).await?;
        Ok(())
    }
    
    /// Create a test collection with proper storage assignment
    pub fn create_test_collection(&self) -> Collection {
        self.create_test_collection_for_engine(StorageEngine::Sst)
    }
    
    /// Create a test collection for specific storage engine
    pub fn create_test_collection_for_engine(&self, engine: StorageEngine) -> Collection {
        let base_path = self.persistent_dir.parent().unwrap().to_str().unwrap();
        let storage_assignment = StorageAssignment {
            base_location: format!("file://{}", base_path),
            assigned_at: chrono::Utc::now().timestamp_millis(),
        };
        
        let config = CollectionConfig {
            name: self.collection_id.clone(),
            dimension: 3,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: engine as i32,
            ..Default::default()
        };
        
        let stats = CollectionStats {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        };
        
        Collection {
            id: self.collection_id.clone(),
            config: Some(config),
            stats: Some(stats),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            storage_assignment: Some(storage_assignment),
        }
    }
    
    /// Unified setup for SST operations with proper paths and configuration
    pub async fn setup_sst_test(&self, vectors: Vec<VectorRecord>) -> Result<(FlushParameters, PathBuf, Collection)> {
        let data_dir = self.get_sst_data_directory();
        tokio::fs::create_dir_all(&data_dir).await?;
        
        let collection_config = self.create_test_collection_for_engine(StorageEngine::Sst);
        
        let flush_params = FlushParameters {
            collection_id: Some(self.collection_id.clone()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection_config.clone()),
            ..Default::default()
        };
        
        Ok((flush_params, data_dir, collection_config))
    }
    
    /// Unified setup for VIPER operations with proper paths and configuration
    pub async fn setup_viper_test(&self, vectors: Vec<VectorRecord>) -> Result<(FlushParameters, PathBuf, Collection)> {
        let data_dir = self.get_viper_data_directory();
        tokio::fs::create_dir_all(&data_dir).await?;
        
        let collection_config = self.create_test_collection_for_engine(StorageEngine::Viper);
        
        let flush_params = FlushParameters {
            collection_id: Some(self.collection_id.clone()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection_config.clone()),
            ..Default::default()
        };
        
        Ok((flush_params, data_dir, collection_config))
    }
    
    /// Create SST files using proper flush mechanism (from sst_compactor_tests)
    pub async fn create_sst_files_with_engine(&self, vectors: Vec<VectorRecord>) -> Result<Vec<String>> {
        debug!("🚀 Creating SST files for collection {} with {} vectors", self.collection_id, vectors.len());
        
        let engine = self.create_sst_engine().await?;
        let (flush_params, _data_dir, _collection_config) = self.setup_sst_test(vectors).await?;
        
        let flush_result = engine.do_flush(&flush_params).await?;
        if !flush_result.success {
            return Err(anyhow::anyhow!("SST flush failed"));
        }
        
        debug!("✅ SST flush successful: {} entries flushed, {} files created", 
               flush_result.entries_flushed, flush_result.files_created);
        
        // Find created SST files
        let storage_url = format!("file://{}/{}/data", 
                                 self.persistent_dir.parent().unwrap().to_str().unwrap(),
                                 self.collection_id);
        
        let fs = self.filesystem.get_filesystem("file:///")?;
        let all_files = fs.list(&storage_url).await?;
        
        let sst_files: Vec<String> = all_files.iter()
            .filter(|entry| entry.name.ends_with(".sst"))
            .map(|entry| format!("{}/{}", storage_url, entry.name))
            .collect();
        
        debug!("🎯 Created {} SST files: {:?}", sst_files.len(), sst_files);
        Ok(sst_files)
    }
    
    /// Check if SST files exist in the expected location
    pub async fn verify_sst_files_exist(&self) -> Result<Vec<std::fs::DirEntry>> {
        let data_dir = self.get_sst_data_directory();
        if !data_dir.exists() {
            return Err(anyhow::anyhow!("SST data directory doesn't exist: {}", data_dir.display()));
        }
        
        let entries = std::fs::read_dir(&data_dir)?;
        let sst_files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "sst"))
            .collect();
            
        if sst_files.is_empty() {
            return Err(anyhow::anyhow!("No SST files found in: {}", data_dir.display()));
        }
        
        Ok(sst_files)
    }
    
    /// Check if VIPER files exist in the expected location
    pub async fn verify_viper_files_exist(&self) -> Result<Vec<std::fs::DirEntry>> {
        let data_dir = self.get_viper_data_directory();
        if !data_dir.exists() {
            return Err(anyhow::anyhow!("VIPER data directory doesn't exist: {}", data_dir.display()));
        }
        
        let entries = std::fs::read_dir(&data_dir)?;
        let parquet_files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "parquet"))
            .collect();
            
        if parquet_files.is_empty() {
            return Err(anyhow::anyhow!("No Parquet files found in: {}", data_dir.display()));
        }
        
        Ok(parquet_files)
    }
    
    /// Create test vectors with metadata (enhanced version)
    pub fn create_test_vectors(&self, count: usize) -> Vec<VectorRecord> {
        self.create_test_vectors_with_dimension(count, 3)
    }
    
    /// Create test vectors with specific dimension
    pub fn create_test_vectors_with_dimension(&self, count: usize, dimension: usize) -> Vec<VectorRecord> {
        let categories = ["A", "B", "C"];
        let types = ["primary", "secondary", "tertiary"];
        
        (0..count).map(|i| {
            VectorRecord {
                id: Some(format!("{}_{}", self.collection_id, i)),
                timestamp: (1000 + i) as u32,
                updated_at: None,
                expires_at: None,
                distance: None,
                rank: None,
                score: None,
                version: None,
                vector: (0..dimension).map(|j| (i + j) as f32).collect(),
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            categories[i % categories.len()].to_string()
                        )),
                    },
                    MetadataItem {
                        key: "type".to_string(), 
                        value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            types[i % types.len()].to_string()
                        )),
                    },
                    MetadataItem {
                        key: "test_id".to_string(),
                        value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            self.collection_id.clone()
                        )),
                    },
                ],
                ..Default::default()
            }
        }).collect()
    }
    
    /// Create a single test vector record (from sst_compactor_tests)
    pub fn create_test_vector_record(
        &self,
        id: String,
        vector: Vec<f32>,
        timestamp: u32,
        expires_at: Option<u32>,
        metadata_items: Vec<MetadataItem>,
    ) -> VectorRecord {
        VectorRecord {
            id: Some(id),
            vector,
            metadata: metadata_items,
            timestamp,
            updated_at: None,
            expires_at,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        }
    }
    
    /// Create a query vector for testing
    pub fn create_query_vector(&self) -> Vec<f32> {
        vec![0.5, 1.5, 2.5] // Should be close to first test vector
    }
    
    /// Create a query vector with specific dimension
    pub fn create_query_vector_with_dimension(&self, dimension: usize) -> Vec<f32> {
        (0..dimension).map(|i| i as f32 * 0.1).collect()
    }
    
    /// Get collection ID
    pub fn collection_id(&self) -> &str {
        &self.collection_id
    }
    
    /// Create test SST configuration optimized for testing (from sst_compactor_tests)
    fn create_test_sst_config(base_path: &std::path::Path) -> SstConfig {
        SstConfig {
            // Level configuration
            level_count: 3,
            max_levels: 3,
            compaction_threshold: 2,
            max_files_per_level: 3,
            level_size_multiplier: 2.0,
            
            // Block settings - smaller for faster testing
            block_size_kb: 3072,
            
            // Storage type
            compaction_strategy: "leveled".to_string(),
            compression: "none".to_string(),
            compression_level: 0,
            
            // Bloom filter - enabled with conservative settings
            bloom_filter_config: Some(BloomFilterConfig {
                bits_per_key: 8,
                enabled: true,
                ..Default::default()
            }),
            decompression_cache_config: None,
            
            // Cache
            cache_size_mb: 16,
            
            // Background operations
            background_thread_count: 1, // Single thread for deterministic testing
            
            // Directories
            data_directory: base_path.join("data").to_str().unwrap().to_string(),
            
            // Memory mapping - disabled for testing
            mmap_enabled: false,
            prefetch_enabled: false,
            prefetch_size_kb: 0,
        }
    }
    
    /// Create test VIPER configuration optimized for testing
    fn create_test_viper_config(base_path: &std::path::Path) -> ViperConfig {
        ViperConfig {
            row_group_size: 50_000,
            compression: "none".to_string(), // No compression for faster tests
            compression_level: 0,
            enable_statistics: true,
            data_directory: base_path.join("viper_data").to_str().unwrap().to_string(),
            cache_size_mb: 64,
        }
    }
    
    /// Create test WriteBuffer configuration
    fn create_test_wal_config(base_path: &std::path::Path) -> WriteBufferUserConfig {
        WriteBufferUserConfig {
            write_buffer_size_mb: 2,
            memory_flush_size_bytes: 512 * 1024, // 512KB
            memtable_type: "BTree".to_string(),
            sync_mode: "perbatch".to_string(),
            write_buffer_directory: base_path.join("write_ahead_log").to_str().unwrap().to_string(),
            enable_wal: true,
            vector_count_threshold: 50,  // Small threshold for tests
        }
    }
}

/// Multiple isolated environments for concurrency testing
pub struct MultiUnifiedEnvironmentTest {
    pub environments: Vec<UnifiedTestEnvironment>,
}

impl MultiUnifiedEnvironmentTest {
    /// Create multiple isolated environments
    pub async fn new(count: usize) -> Result<Self> {
        let mut environments = Vec::new();
        
        for _ in 0..count {
            environments.push(UnifiedTestEnvironment::new().await?);
        }
        
        Ok(Self { environments })
    }
    
    /// Get environment by index
    pub fn get_env(&self, index: usize) -> &UnifiedTestEnvironment {
        &self.environments[index]
    }
    
    /// Get all environment collection IDs
    pub fn collection_ids(&self) -> Vec<&str> {
        self.environments.iter().map(|env| env.collection_id()).collect()
    }
}

/// Utility functions for common test operations
pub mod operations {
    use super::*;
    use proximadb::compute::distance_computation::DistanceMetric;
    
    /// Insert vectors and flush to SST storage
    pub async fn insert_and_flush_sst(
        engine: &SstStorage,
        environment: &UnifiedTestEnvironment,
        vectors: Vec<VectorRecord>
    ) -> Result<()> {
        let (flush_params, _data_dir, _collection_config) = environment.setup_sst_test(vectors).await?;
        
        let result = engine.do_flush(&flush_params).await?;
        
        if !result.success {
            return Err(anyhow::anyhow!("SST flush failed for collection {}", environment.collection_id()));
        }
        
        debug!("✅ SST flushed {} vectors for collection {}", 
                result.entries_flushed, environment.collection_id());
        
        Ok(())
    }
    
    /// Insert vectors and flush to VIPER storage
    pub async fn insert_and_flush_viper(
        engine: &ViperEngine,
        environment: &UnifiedTestEnvironment,
        vectors: Vec<VectorRecord>
    ) -> Result<()> {
        let (flush_params, _data_dir, _collection_config) = environment.setup_viper_test(vectors).await?;
        
        let result = engine.do_flush(&flush_params).await?;
        
        if !result.success {
            return Err(anyhow::anyhow!("VIPER flush failed for collection {}", environment.collection_id()));
        }
        
        debug!("✅ VIPER flushed {} vectors for collection {}", 
                result.entries_flushed, environment.collection_id());
        
        Ok(())
    }
    
    /// Search vectors using SST engine
    pub async fn search_vectors_sst(
        engine: &SstStorage,
        environment: &UnifiedTestEnvironment,
        query_vector: &[f32],
        top_k: usize
    ) -> Result<Vec<proximadb::core::search::SearchResult>> {
        let storage_url = format!("file://{}/data", environment.persistent_dir.to_str().unwrap());
        
        let results = engine.search_vectors_unified(
            environment.collection_id(),
            &storage_url,
            query_vector,
            top_k,
            &DistanceMetric::Cosine,
            None,
            true,
            true
        ).await?;
        
        debug!("🔍 SST search found {} results for collection {}", 
                results.len(), environment.collection_id());
        
        Ok(results)
    }
    
    /// Compact SST storage
    pub async fn compact_sst_storage(
        engine: &mut SstStorage,
        environment: &UnifiedTestEnvironment
    ) -> Result<()> {
        let collection_config = environment.create_test_collection_for_engine(StorageEngine::Sst);
        let compact_params = CompactionParameters {
            collection_id: Some(environment.collection_id().to_string()),
            force: true,
            synchronous: true,
            collection_config: Some(collection_config),
            ..Default::default()
        };
        
        let result = engine.compact(compact_params).await?;
        
        if !result.success {
            return Err(anyhow::anyhow!("SST compaction failed for collection {}", environment.collection_id()));
        }
        
        debug!("🗜️ SST compacted {} entries for collection {}", 
                result.entries_processed, environment.collection_id());
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_unified_environment_creation() {
        let env = UnifiedTestEnvironment::new().await.unwrap();
        
        // Verify unique collection ID
        assert!(env.collection_id().starts_with("unified_test_"));
        assert!(env.collection_id().len() > 20); // UUID should make it long
        
        // Verify storage locations
        assert_eq!(env.storage_locations.len(), 1);
        assert!(env.storage_locations[0].url.starts_with("file://"));
        
        debug!("✅ Created unified environment: {}", env.collection_id());
    }
    
    #[tokio::test]
    async fn test_sst_and_viper_creation() {
        let env = UnifiedTestEnvironment::new().await.unwrap();
        
        // Test SST engine creation
        let sst_engine = env.create_sst_engine().await.unwrap();
        debug!("✅ Created SST engine for: {}", env.collection_id());
        
        // Test VIPER engine creation  
        let viper_engine = env.create_viper_engine().await.unwrap();
        debug!("✅ Created VIPER engine for: {}", env.collection_id());
        
        // Test directory creation
        env.ensure_all_directories().await.unwrap();
        assert!(env.get_sst_data_directory().exists());
        assert!(env.get_viper_data_directory().exists());
    }
    
    #[tokio::test]
    async fn test_multi_environment_isolation() {
        let multi_env = MultiUnifiedEnvironmentTest::new(3).await.unwrap();
        let collection_ids = multi_env.collection_ids();
        
        // Verify all collection IDs are unique
        for i in 0..collection_ids.len() {
            for j in (i+1)..collection_ids.len() {
                assert_ne!(collection_ids[i], collection_ids[j]);
            }
        }
        
        debug!("✅ Created {} isolated unified environments", collection_ids.len());
        for id in collection_ids {
            debug!("  - {}", id);
        }
    }
}