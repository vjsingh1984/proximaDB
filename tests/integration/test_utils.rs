//! Integration Test Utilities
//! 
//! Provides isolated test environments with unique collections for reliable integration testing.

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

use proximadb::core::{SstConfig, BloomFilterConfig, VectorRecord};
use proximadb::core::config::StorageLocation;
use proximadb::proto::proximadb::MetadataItem;
use proximadb::compute::unified_distance::{UnifiedDistanceCompute, HardwareBackend};
use proximadb::storage::assignment_service::{HashBasedAssignmentService, AssignmentService, set_assignment_service, get_assignment_service};
use proximadb::storage::engines::sst::SstStorage;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::traits::UnifiedStorageEngine;

/// Isolated test environment with unique collection and clean state
pub struct IsolatedTestEnvironment {
    pub collection_id: String,
    pub temp_dir: TempDir,
    pub assignment_service: Arc<HashBasedAssignmentService>,
    pub filesystem: Arc<FilesystemFactory>,
    pub sst_config: SstConfig,
    pub storage_locations: Vec<StorageLocation>,
}

impl IsolatedTestEnvironment {
    /// Create a new isolated test environment with unique collection ID
    pub async fn new() -> Result<Self> {
        let collection_id = format!("test_collection_{}", Uuid::new_v4().simple());
        let temp_dir = TempDir::new()?;
        
        // Create isolated assignment service (no global singleton)
        let assignment_service = Arc::new(HashBasedAssignmentService::new());
        
        // Create isolated filesystem
        let filesystem = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);
        
        // Create base directories
        let base_path = temp_dir.path().to_str().unwrap();
        tokio::fs::create_dir_all(format!("{}/data", base_path)).await?;
        tokio::fs::create_dir_all(format!("{}/write_buffer", base_path)).await?;
        tokio::fs::create_dir_all(format!("{}/index", base_path)).await?;
        
        // Create storage locations
        let storage_locations = vec![
            StorageLocation {
                url: format!("file://{}", base_path),
                weight: 1,
                tags: vec!["integration_test".to_string()],
            }
        ];
        
        // Create optimized SST config for testing
        let sst_config = Self::create_test_sst_config();
        
        Ok(Self {
            collection_id,
            temp_dir,
            assignment_service,
            filesystem,
            sst_config,
            storage_locations,
        })
    }
    
    /// Create an SST storage engine for this environment
    pub async fn create_sst_engine(&self) -> Result<SstStorage> {
        
        // Inject our isolated assignment service into the global singleton only once
        // If it fails, use the existing global service and add our collection to it
        if let Err(_) = set_assignment_service(self.assignment_service.clone()) {
            // Already set, add our collection to the existing global service
            println!("⚠️ Assignment service already set, adding collection to global service");
            let global_service = get_assignment_service();
            let assignment = global_service.assign_collection(
                &self.collection_id,
                &self.storage_locations,
                "hash"
            ).await?;
            
            // Ensure directories exist
            let data_path = assignment.data_url.strip_prefix("file://").unwrap_or(&assignment.data_url);
            tokio::fs::create_dir_all(data_path).await?;
        } else {
            // Successfully set our service, proceed normally
            let assignment = self.assignment_service.assign_collection(
                &self.collection_id,
                &self.storage_locations,
                "hash"
            ).await?;
            
            // Ensure directories exist
            let data_path = assignment.data_url.strip_prefix("file://").unwrap_or(&assignment.data_url);
            tokio::fs::create_dir_all(data_path).await?;
        }
        
        println!("🔧 Created isolated SST engine for collection: {}", self.collection_id);
        
        // Create distance compute for SST storage
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(proximadb::compute::distance::DistanceMetric::Cosine));
        
        // Create SST storage engine
        SstStorage::new(
            self.collection_id.clone(),
            self.sst_config.clone(),
            self.filesystem.clone(),
            distance_compute
        ).await
    }
    
    /// Create test vectors with metadata
    pub fn create_test_vectors(&self, count: usize) -> Vec<VectorRecord> {
        let categories = ["A", "B", "C"];
        let types = ["primary", "secondary", "tertiary"];
        
        (0..count).map(|i| {
            VectorRecord {
                id: Some(format!("{}_{}", self.collection_id, i)),
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
                timestamp: chrono::Utc::now().timestamp(),
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
    
    /// Get assignment service
    pub fn assignment_service(&self) -> &Arc<HashBasedAssignmentService> {
        &self.assignment_service
    }
    
    /// Create test SST configuration optimized for testing
    fn create_test_sst_config() -> SstConfig {
        SstConfig {
            // Memory settings - smaller for faster tests
            memtable_size_mb: 8,
            memory_flush_size_bytes: 512 * 1024, // 512KB
            write_buffer_size_mb: 2,
            cache_size_mb: 16,
            
            // Level configuration - fewer levels for faster tests
            level_count: 3,
            max_levels: 3,
            compaction_threshold: 2,
            max_files_per_level: 3,
            level_size_multiplier: 2.0,
            
            // Block settings - smaller for faster I/O
            block_size_kb: 8,
            
            // Storage type
            memtable_type: "skiplist".to_string(),
            compaction_strategy: "leveled".to_string(),
            compression: "none".to_string(), // No compression for faster tests
            
            // Bloom filter - enabled with conservative settings
            bloom_filter_config: Some(BloomFilterConfig {
                bits_per_key: 8,
                enabled: true,
                ..Default::default()
            }),
            
            // Background operations
            background_thread_count: 1, // Single thread for deterministic testing
            
            // Sync and persistence - immediate for testing
            sync_mode: "immediate".to_string(),
            enable_write_buffer: true,
            
            // Directories - will be set by assignment service
            write_buffer_directory: "/tmp/test_wb".to_string(), // Placeholder
            data_directory: "/tmp/test_data".to_string(), // Placeholder
            
            // Memory mapping - disabled for testing
            mmap_enabled: false,
            prefetch_enabled: false,
            prefetch_size_kb: 0,
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
//     use proximadb::compute::distance::DistanceMetric;
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
//         println!("✅ Flushed {} vectors for collection {}", 
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
//         println!("🔍 Found {} results for collection {}", 
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
//         println!("🗜️ Compacted {} entries for collection {}", 
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
        
        println!("✅ Created isolated environment: {}", env.collection_id());
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
        
        println!("✅ Created {} isolated environments with unique IDs", collection_ids.len());
        for id in collection_ids {
            println!("  - {}", id);
        }
    }
}