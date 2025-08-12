//! Integration Test Utilities
//! 
//! Provides isolated test environments with unique collections for reliable integration testing.

use anyhow::Result;
use tracing::{debug, error, info, warn};
use std::sync::{Arc, Once};
use std::path::PathBuf;
use tempfile::TempDir;
use uuid::Uuid;

use proximadb::core::{SstConfig, BloomFilterConfig, VectorRecord};
use proximadb::core::config::WriteBufferUserConfig;
use proximadb::core::config::StorageLocation;
use proximadb::proto::proximadb::MetadataItem;
use proximadb::compute::distance_computation::UnifiedDistanceCompute;
use proximadb::core::hardware_capabilities::HardwareBackend;
// 🔴 OBSOLETE - Assignment service removed
use proximadb::storage::engines::sst::SstStorage;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::traits::UnifiedStorageEngine;

static HARDWARE_INIT: Once = Once::new();

/// Setup hardware capabilities for tests
fn setup_hardware_capabilities() {
    HARDWARE_INIT.call_once(|| {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    });
}

/// Isolated test environment with unique collection and clean state
pub struct IsolatedTestEnvironment {
    pub collection_id: String,
    pub temp_dir: TempDir,
    pub persistent_dir: PathBuf,  // NEW: Pre-assigned persistent directory
    // Assignment service removed - collections now embed storage_assignment
    pub filesystem: Arc<FilesystemFactory>,
    pub sst_config: SstConfig,
    pub storage_locations: Vec<StorageLocation>,
}

impl IsolatedTestEnvironment {
    /// Create a new isolated test environment with unique collection ID
    pub async fn new() -> Result<Self> {
        // Ensure hardware capabilities are initialized
        setup_hardware_capabilities();
        
        let collection_id = format!("test_collection_{}", Uuid::new_v4().simple());
        let temp_dir = TempDir::new()?;
        
        // Create persistent directory that won't be garbage collected
        let persistent_base = std::env::temp_dir().join("proximadb_integration_tests");
        tokio::fs::create_dir_all(&persistent_base).await?;
        let persistent_dir = persistent_base.join(&collection_id);
        tokio::fs::create_dir_all(&persistent_dir).await?;
        
        info!("🏗️ Created persistent test directory: {}", persistent_dir.display());
        info!("🏗️ Collection ID: {}", collection_id);
        
        // Create isolated assignment service (no global singleton)
        let filesystem_factory = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);
        
        // Create isolated filesystem
        let filesystem = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);
        
        // Create base directories in BOTH temp and persistent locations
        let temp_base_path = temp_dir.path().to_str().unwrap();
        let persistent_base_path = persistent_dir.to_str().unwrap();
        
        // Use persistent directory for actual storage
        let base_path = persistent_base_path;
        tokio::fs::create_dir_all(format!("{}/data", base_path)).await?;
        tokio::fs::create_dir_all(format!("{}/write_ahead_log", base_path)).await?;
        tokio::fs::create_dir_all(format!("{}/index", base_path)).await?;
        
        info!("📁 Created storage directories:");
        info!("   - Data: {}/data", base_path);
        info!("   - WAL: {}/write_ahead_log", base_path);
        info!("   - Index: {}/index", base_path);
        
        // Create storage locations pointing to persistent directory
        let storage_locations = vec![
            StorageLocation {
                url: format!("file://{}", base_path),
                weight: 1,
                tags: vec!["integration_test".to_string()],
            }
        ];
        
        info!("🔗 Storage location URL: file://{}", base_path);
        
        // Create optimized SST config for testing
        let sst_config = Self::create_test_sst_config();
        
        Ok(Self {
            collection_id,
            temp_dir,
            persistent_dir,
            filesystem,
            sst_config,
            storage_locations,
        })
    }
    
    /// Create an SST storage engine for this environment
    pub async fn create_sst_engine(&self) -> Result<SstStorage> {
        
        // Assignment service removed - create directories directly
        // Collections now embed storage_assignment
        let data_path = self.persistent_dir.join("data");
        
        info!("🏭 Creating SST engine with data path: {}", data_path.display());
        tokio::fs::create_dir_all(&data_path).await?;
        
        debug!("🔧 Created isolated SST engine for collection: {}", self.collection_id);
        
        // Create distance compute for SST storage
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(proximadb::compute::distance_computation::DistanceMetric::Cosine));
        
        // Create SST storage engine
        SstStorage::new(
            self.sst_config.clone(),
            self.filesystem.clone(),
            distance_compute
        ).await
    }
    
    /// Create a test collection with embedded storage assignment
    pub fn create_test_collection(&self) -> proximadb::proto::proximadb::Collection {
        use proximadb::proto::proximadb::{Collection, CollectionConfig, StorageAssignment, CollectionStats, DistanceMetric, StorageEngine};
        
        // FIXED: Use persistent_dir instead of temp_dir for storage assignment
        let base_path = self.persistent_dir.to_str().unwrap();
        let storage_assignment = StorageAssignment {
            base_location: format!("file://{}", base_path),
            assigned_at: chrono::Utc::now().timestamp_millis(),
        };
        
        let config = CollectionConfig {
            name: self.collection_id.clone(),
            dimension: 3,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Sst as i32,
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
    
    /// Create test vectors with metadata
    pub fn create_test_vectors(&self, count: usize) -> Vec<VectorRecord> {
        let categories = ["A", "B", "C"];
        let types = ["primary", "secondary", "tertiary"];
        
        (0..count).map(|i| {
            VectorRecord {
                id: Some(format!("{}_{}", self.collection_id, i)),
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                distance: None,
                rank: None,
                score: None,
                version: None,
                vector: vec![i as f32, (i + 1) as f32, (i + 2) as f32],
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
    
    /// Create a query vector for testing
    pub fn create_query_vector(&self) -> Vec<f32> {
        vec![0.5, 1.5, 2.5] // Should be close to first test vector
    }
    
    /// Get collection ID
    pub fn collection_id(&self) -> &str {
        &self.collection_id
    }
    
    // Assignment service removed - collections now embed storage_assignment
    
    /// Create test SST configuration optimized for testing
    fn create_test_sst_config() -> SstConfig {
        SstConfig {
            // Level configuration - fewer levels for faster tests
            level_count: 3,
            max_levels: 3,
            compaction_threshold: 2,
            max_files_per_level: 3,
            level_size_multiplier: 2.0,
            
            // Block settings - smaller for faster I/O
            block_size_kb: 8,
            
            // Storage type
            compaction_strategy: "leveled".to_string(),
            compression: "none".to_string(), // No compression for faster tests
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
            
            // Directories - will be set by assignment service
            data_directory: "/tmp/test-data".to_string(), // Placeholder
            
            // Memory mapping - disabled for testing
            mmap_enabled: false,
            prefetch_enabled: false,
            prefetch_size_kb: 0,
        }
    }
    
    /// Create test WriteBuffer configuration
    fn create_test_wal_config() -> WriteBufferUserConfig {
        WriteBufferUserConfig {
            write_buffer_size_mb: 2,
            memory_flush_size_bytes: 512 * 1024, // 512KB
            memtable_type: "BTree".to_string(),
            sync_mode: "perbatch".to_string(),
            write_buffer_directory: "/tmp/test-wb".to_string(), // Placeholder
            enable_wal: true,
            vector_count_threshold: 50,  // Small threshold for tests
        }
    }
}

/// Multiple isolated environments for concurrency testing
pub struct MultiEnvironmentTest {
    pub environments: Vec<IsolatedTestEnvironment>,
}

impl MultiEnvironmentTest {
    /// Create multiple isolated environments
    pub async fn new(count: usize) -> Result<Self> {
        let mut environments = Vec::new();
        
        for _ in 0..count {
            environments.push(IsolatedTestEnvironment::new().await?);
        }
        
        Ok(Self { environments })
    }
    
    /// Get environment by index
    pub fn get_env(&self, index: usize) -> &IsolatedTestEnvironment {
        &self.environments[index]
    }
    
    /// Get all environment collection IDs
    pub fn collection_ids(&self) -> Vec<&str> {
        self.environments.iter().map(|env| env.collection_id()).collect()
    }
}

/// Utility functions for common test operations
// TODO: Re-enable once SST compilation issues are resolved
// pub mod test_operations {
//     use super::*;
//     use proximadb::storage::traits::{FlushParameters, CompactionParameters};
//     use proximadb::compute::distance_computation::DistanceMetric;
//     
//     /// Insert vectors and flush to storage
//     pub async fn insert_and_flush(
//         engine: &SstStorage,
//         environment: &IsolatedTestEnvironment,
//         vectors: Vec<VectorRecord>
//     ) -> Result<()> {
//         let flush_params = FlushParameters {
//             collection_id: Some(environment.collection_id().to_string()),
//             vector_records: vectors,
//             force: true,
//             synchronous: true,
//             ..Default::default()
//         };
//         
//         let result = engine.do_flush(&flush_params).await?;
//         
//         if !result.success {
//             return Err(anyhow::anyhow!("Flush failed for collection {}", environment.collection_id()));
//         }
//         
//         debug!("✅ Flushed {} vectors for collection {}", 
//                 result.entries_flushed, environment.collection_id());
//         
//         Ok(())
//     }
//     
//     /// Search vectors and return results
//     pub async fn search_vectors(
//         engine: &SstStorage,
//         environment: &IsolatedTestEnvironment,
//         query_vector: &[f32],
//         top_k: usize
//     ) -> Result<Vec<proximadb::core::VectorSearchResult>> {
//         let results = engine.search_vectors_unified(
//             environment.collection_id(),
//             query_vector,
//             top_k,
//             &DistanceMetric::Cosine,
//             None,
//             true,
//             true
//         ).await?;
//         
//         debug!("🔍 Found {} results for collection {}", 
//                 results.len(), environment.collection_id());
//         
//         Ok(results)
//     }
//     
//     /// Compact storage and return results
//     pub async fn compact_storage(
//         engine: &mut SstStorage,
//         environment: &IsolatedTestEnvironment
//     ) -> Result<()> {
//         let compact_params = CompactionParameters {
//             collection_id: Some(environment.collection_id().to_string()),
//             force: true,
//             synchronous: true,
//             ..Default::default()
//         };
//         
//         let result = engine.compact(compact_params).await?;
//         
//         if !result.success {
//             return Err(anyhow::anyhow!("Compaction failed for collection {}", environment.collection_id()));
//         }
//         
//         debug!("🗜️ Compacted {} entries for collection {}", 
//                 result.entries_processed, environment.collection_id());
//         
//         Ok(())
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_isolated_environment_creation() {
        let env = IsolatedTestEnvironment::new().await.unwrap();
        
        // Verify unique collection ID
        assert!(env.collection_id().starts_with("test_collection_"));
        assert!(env.collection_id().len() > 20); // UUID should make it long
        
        // Verify storage locations
        assert_eq!(env.storage_locations.len(), 1);
        assert!(env.storage_locations[0].url.starts_with("file://"));
        
        debug!("✅ Created isolated environment: {}", env.collection_id());
    }
    
    #[tokio::test]
    async fn test_multiple_environments_isolation() {
        let multi_env = MultiEnvironmentTest::new(3).await.unwrap();
        let collection_ids = multi_env.collection_ids();
        
        // Verify all collection IDs are unique
        for i in 0..collection_ids.len() {
            for j in (i+1)..collection_ids.len() {
                assert_ne!(collection_ids[i], collection_ids[j]);
            }
        }
        
        debug!("✅ Created {} isolated environments with unique IDs", collection_ids.len());
        for id in collection_ids {
            debug!("  - {}", id);
        }
    }
}