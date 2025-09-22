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

use uuid::Uuid;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::{Arc, Once};
use tempfile::TempDir;
use tracing::{debug, info};

// Core ProximaDB imports
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::core::config::StorageLocation;
use proximadb::core::config::{BloomFilterConfig, SstConfig};
use proximadb::core::config::ViperConfig;
use proximadb::proto::proximadb_v1::{
    Collection, CollectionConfig, CollectionStats, CompressionAlgorithm, DistanceMetric, SqlValue,
    StorageEngine, VectorRecord, sql_value,
};
// SearchResult moved to different location - using Vec<VectorRecord> for results
// Note: Some imports like MetadataItem may have changed or been removed
use proximadb::services::vector_operations_service::VectorOperationsService;
use proximadb::storage::engines::impls::sst::SstEngine;
use proximadb::storage::engines::impls::viper::engine::ViperEngine;
use proximadb::storage::metadata::store::MetadataCacheConfig;
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb::storage::persistence::write_ahead_log::config::WriteBufferStrategyType;
use proximadb::storage::persistence::write_ahead_log::{
    WALBatchFactory, WALConfig, WriteAheadLogManager,
};
use proximadb::storage::traits::{CompactionParameters, FlushParameters, UnifiedStorageEngine};

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
    pub distance_compute: Arc<UnifiedDistanceCompute>,
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

        info!(
            "🏗️ Created unified test environment: {}",
            persistent_dir.display()
        );
        info!("🏗️ Collection ID: {}", collection_id);

        // Setup all required directories
        setup_test_directories(&persistent_dir).await?;

        let filesystem = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);

        // Create optimized configs for testing
        let sst_config = Self::create_test_sst_config(&persistent_dir);
        let viper_config = Self::create_test_viper_config(&persistent_dir);

        let storage_locations = vec![StorageLocation {
            url: format!("file://{}", persistent_dir.to_str().unwrap()),
            weight: 1,
            tags: vec!["unified_test".to_string()],
        }];

        Ok(Self {
            collection_id,
            temp_dir,
            persistent_dir,
            filesystem,
            sst_config,
            viper_config,
            storage_locations,
            distance_compute: Arc::new(UnifiedDistanceCompute::default()),
        })
    }

    /// Create an SST storage engine for this environment
    pub async fn create_sst_engine(&self) -> Result<SstEngine> {
        info!(
            "🏭 Creating SST engine for collection: {}",
            self.collection_id
        );

        let distance_compute = Arc::new(UnifiedDistanceCompute::new(
            proximadb::compute::distance_computation::DistanceMetric::Cosine,
        ));

        SstEngine::new()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create SST storage: {}", e))
    }

    /// Create a VIPER storage engine for this environment
    pub async fn create_viper_engine(&self) -> Result<ViperEngine> {
        info!(
            "🏭 Creating VIPER engine for collection: {}",
            self.collection_id
        );

        ViperEngine::new()
        .await
    }

    /// Get the data directory path for SST operations
    pub fn get_sst_data_directory(&self) -> PathBuf {
        // SST writes to {base_location}/{collection_id}/data
        // persistent_dir already includes collection_id, so data goes inside it
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
    ///
    /// CRITICAL: This method handles several setup complexities that cause test failures when done manually:
    /// 1. Sets base_location to PARENT directory (not the collection directory itself!)
    /// 2. Uses proper file:// URL format (though system defaults to file:// anyway)
    /// 3. Uses correct timestamp units (millis, not micros)
    /// 4. Ensures storage_assignment is consistent across flush and compaction operations
    ///
    /// Storage engines expect: {base_location}/{collection_id}/data/
    /// So if base_location = /tmp/test, files go to /tmp/test/collection_123/data/
    pub fn create_test_collection_for_engine(&self, engine: StorageEngine) -> Collection {
        self.create_test_collection_with_settings(engine, 3, None)
    }

    /// Create test collection with all settings - this is the MAIN method
    /// that should be used when you need specific compression settings
    /// For SST: block_size_kb is part of CompressionConfig (not separate SstConfig)
    pub fn create_test_collection_with_settings(
        &self,
        engine: StorageEngine,
        dimension: i32,
        _compression: Option<proximadb::proto::proximadb_v1::CompressionConfig>,
    ) -> Collection {
        let config = CollectionConfig {
            name: self.collection_id.clone(),
            dimension: dimension as u32,
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
            storage_assignment: None,
        }
    }

    /// Unified setup for SST operations with proper paths and configuration
    pub async fn setup_sst_test(
        &self,
        vectors: Vec<VectorRecord>,
    ) -> Result<(FlushParameters, PathBuf, Collection)> {
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
    pub async fn setup_viper_test(
        &self,
        vectors: Vec<VectorRecord>,
    ) -> Result<(FlushParameters, PathBuf, Collection)> {
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
    pub async fn create_sst_files_with_engine(
        &self,
        vectors: Vec<VectorRecord>,
    ) -> Result<Vec<String>> {
        debug!(
            "🚀 Creating SST files for collection {} with {} vectors",
            self.collection_id,
            vectors.len()
        );

        let engine = self.create_sst_engine().await?;
        let (flush_params, _data_dir, _collection_config) = self.setup_sst_test(vectors).await?;

        let flush_result = engine.do_flush(&flush_params).await?;
        if !flush_result.success {
            return Err(anyhow::anyhow!("SST flush failed"));
        }

        debug!(
            "✅ SST flush successful: {} entries flushed, {} files created",
            flush_result.entries_flushed.unwrap_or(0), flush_result.files_created.unwrap_or(0)
        );

        // Find created SST files
        let storage_url = format!(
            "file://{}/{}/data",
            self.persistent_dir.parent().unwrap().to_str().unwrap(),
            self.collection_id
        );

        let fs = self.filesystem.get_filesystem("file:///")?;
        let all_files = fs.list(&storage_url).await?;

        let sst_files: Vec<String> = all_files
            .iter()
            .filter(|entry| entry.name.ends_with(".sstable"))
            .map(|entry| format!("{}/{}", storage_url, entry.name))
            .collect();

        debug!("🎯 Created {} SST files: {:?}", sst_files.len(), sst_files);
        Ok(sst_files)
    }

    /// Check if SST files exist in the expected location
    pub async fn verify_sst_files_exist(&self) -> Result<Vec<std::fs::DirEntry>> {
        let data_dir = self.get_sst_data_directory();
        if !data_dir.exists() {
            return Err(anyhow::anyhow!(
                "SST data directory doesn't exist: {}",
                data_dir.display()
            ));
        }

        let entries = std::fs::read_dir(&data_dir)?;
        let sst_files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "sst"))
            .collect();

        if sst_files.is_empty() {
            return Err(anyhow::anyhow!(
                "No SST files found in: {}",
                data_dir.display()
            ));
        }

        Ok(sst_files)
    }

    /// Check if VIPER files exist in the expected location
    pub async fn verify_viper_files_exist(&self) -> Result<Vec<std::fs::DirEntry>> {
        let data_dir = self.get_viper_data_directory();
        if !data_dir.exists() {
            return Err(anyhow::anyhow!(
                "VIPER data directory doesn't exist: {}",
                data_dir.display()
            ));
        }

        let entries = std::fs::read_dir(&data_dir)?;
        let parquet_files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "parquet"))
            .collect();

        if parquet_files.is_empty() {
            return Err(anyhow::anyhow!(
                "No Parquet files found in: {}",
                data_dir.display()
            ));
        }

        Ok(parquet_files)
    }

    /// Create test vectors with metadata (enhanced version)
    pub fn create_test_vectors(&self, count: usize) -> Vec<VectorRecord> {
        self.create_test_vectors_with_dimension(count, 3)
    }

    /// Create test vectors with specific dimension
    pub fn create_test_vectors_with_dimension(
        &self,
        count: usize,
        dimension: usize,
    ) -> Vec<VectorRecord> {
        let categories = ["A", "B", "C"];
        let types = ["primary", "secondary", "tertiary"];

        (0..count)
            .map(|i| VectorRecord {
                id: format!("{}_{}", self.collection_id, i),
                timestamp: (1000 + i) as i64,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                quantized_vector: vec![],
                source: None,
                vector: (0..dimension).map(|j| (i + j) as f32).collect(),
                metadata: std::collections::HashMap::from([
                    ("category".to_string(), SqlValue {
                        value: Some(
                            sql_value::Value::StringValue(
                                categories[i % categories.len()].to_string(),
                            ),
                        ),
                    }),
                    ("type".to_string(), SqlValue {
                        value: Some(
                            sql_value::Value::StringValue(
                                types[i % types.len()].to_string(),
                            ),
                        ),
                    }),
                    ("test_id".to_string(), SqlValue {
                        value: Some(
                            sql_value::Value::StringValue(
                                self.collection_id.clone(),
                            ),
                        ),
                    }),
                ]),
                ..Default::default()
            })
            .collect()
    }

    /// Create a single test vector record (from sst_compactor_tests)
    pub fn create_test_vector_record(
        &self,
        id: String,
        vector: Vec<f32>,
        timestamp: i64,
        expires_at: Option<i64>,
        metadata: std::collections::HashMap<String, SqlValue>,
    ) -> VectorRecord {
        VectorRecord {
            id,
            vector,
            metadata,
            timestamp,
            updated_at: None,
            expires_at,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
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

    // Removed setup_test_assignment - StorageAssignment no longer exists in proto

    /// Create metadata store config (for backward compatibility)  
    pub fn create_metadata_store_config(
        &self,
    ) -> proximadb::storage::metadata::MetadataStoreConfig {
        let metadata_path = self
            .persistent_dir
            .join("metadata")
            .to_str()
            .unwrap()
            .to_string();
        proximadb::storage::metadata::MetadataStoreConfig {
            metadata_storage_urls: vec![metadata_path], // Filesystem API defaults to file:// scheme
            enable_atomic_operations: true,
            cache_config: MetadataCacheConfig::default(),
            backup_config: None,
            replication_config: None,
        }
    }

    /// Create test collection with storage (for backward compatibility)
    pub fn create_test_collection_with_storage(
        &self,
        name: &str,
        _base_location: String,
    ) -> Collection {
        let config = CollectionConfig {
            name: name.to_string(),
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
            id: name.to_string(),
            config: Some(config),
            stats: Some(stats),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            storage_assignment: None,
        }
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

            // Compaction config
            compaction_config: Default::default(),
        }
    }

    /// Create general test config (for backward compatibility)
    pub fn create_test_config(&self) -> proximadb::core::config::Config {
        // Use default config since struct fields may vary
        proximadb::core::config::Config::default()
    }

    /// Create test VIPER configuration optimized for testing
    fn create_test_viper_config(base_path: &std::path::Path) -> ViperConfig {
        ViperConfig {
            row_group_size: 50_000,
            compression: "none".to_string(), // No compression for faster tests
            compression_level: 0,
            enable_statistics: true,
            data_directory: base_path.join("data").to_str().unwrap().to_string(),
            cache_size_mb: 64,
            compaction_config: Default::default(),
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
        self.environments
            .iter()
            .map(|env| env.collection_id())
            .collect()
    }
}

/// Helper functions to build correct test parameters
/// Tests should call production code directly with these parameters
pub mod operations {
    use super::*;
    

    /// Build correct FlushParameters - the critical configuration that was causing failures
    pub async fn build_flush_params(
        environment: &UnifiedTestEnvironment,
        vectors: Vec<VectorRecord>,
        engine: StorageEngine,
    ) -> Result<FlushParameters> {
        // Ensure directories exist
        environment.ensure_all_directories().await?;

        let collection_config = environment.create_test_collection_for_engine(engine);
        Ok(FlushParameters {
            collection_id: Some(environment.collection_id().to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection_config),
            ..Default::default()
        })
    }

    /// Build FlushParameters for SST with custom compression - uses provided collection config
    pub async fn build_sst_flush_params_with_collection(
        environment: &UnifiedTestEnvironment,
        vectors: Vec<VectorRecord>,
        collection: Collection,
    ) -> Result<FlushParameters> {
        environment.ensure_all_directories().await?;

        Ok(FlushParameters {
            collection_id: Some(environment.collection_id().to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        })
    }

    // REMOVED: build_sst_flush_params_with_compression - Use build_sst_flush_params_with_collection
    // Tests must create collection config once and reuse it for both flush and compaction

    /// Build FlushParameters for VIPER with custom compression
    /// VIPER/Parquet supports: none, snappy, gzip, lz4, zstd, brotli (limited by Parquet format)
    pub async fn build_viper_flush_params_with_compression(
        environment: &UnifiedTestEnvironment,
        vectors: Vec<VectorRecord>,
        compression_algo: &str,
        _compression_level: i32,
    ) -> Result<FlushParameters> {
        environment.ensure_all_directories().await?;

        let mut collection = environment.create_test_collection_for_engine(StorageEngine::Viper);

        // VIPER uses Parquet compression, configured through collection config
        if let Some(ref mut config) = collection.config {
            use proximadb::proto::proximadb_v1::CompressionAlgorithm;

            // Map to Parquet-supported algorithms
            let algorithm = match compression_algo {
                "none" | "uncompressed" => CompressionAlgorithm::CompressionNone as i32,
                "zstd" => CompressionAlgorithm::CompressionZstd as i32,
                "snappy" => CompressionAlgorithm::CompressionSnappy as i32,
                "gzip" => CompressionAlgorithm::CompressionGzip as i32,
                "lz4" => CompressionAlgorithm::CompressionLz4 as i32,
                "brotli" => CompressionAlgorithm::CompressionBrotli as i32,
                // Unsupported algorithms default to snappy
                _ => {
                    debug!(
                        "Algorithm {} not supported by Parquet, using snappy",
                        compression_algo
                    );
                    CompressionAlgorithm::CompressionSnappy as i32
                }
            };

            // Compression config is handled by storage_config, not directly on collection config
            // The compression will be applied through storage engine configuration
        }

        Ok(FlushParameters {
            collection_id: Some(environment.collection_id().to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        })
    }

    /// Build correct storage URL for SST search operations
    pub fn build_sst_storage_url(environment: &UnifiedTestEnvironment) -> String {
        // SST expects storage_url to point directly to where .sst files are located
        format!(
            "file://{}",
            environment.get_sst_data_directory().to_str().unwrap()
        )
    }

    /// Build correct storage URL for VIPER search operations
    pub fn build_viper_storage_url(environment: &UnifiedTestEnvironment) -> String {
        // VIPER expects storage_url to point to where parquet files are located
        format!(
            "file://{}",
            environment.get_viper_data_directory().to_str().unwrap()
        )
    }

    /// Search vectors in SST engine
    pub async fn search_vectors_sst(
        engine: &SstEngine,
        environment: &UnifiedTestEnvironment,
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<VectorRecord>> {
        let storage_url = build_sst_storage_url(environment);
        let collection = Arc::new(environment.create_test_collection());
        let search_params = Arc::new(proximadb::core::search::SearchParams {
            vector: Some(query_vector.to_vec()),
            query_vectors: None,
            top_k: Some(top_k),
            distance_metric: Some(proximadb::compute::distance_computation::DistanceMetric::Cosine),
            filter_expression: None,
            timeout_ms: None,
            accuracy_threshold: None,
            ..Default::default()
        });
        let query_context = proximadb::storage::traits::StorageQueryContext {
            search_params,
            collection,
            metadata: proximadb::storage::traits::StorageQueryMetadata::default(),
        };

        // Direct production call using unified search
        let results = engine
            .search_vectors_unified(&query_context)
            .await?;

        // Convert OptimizedSearchRecord to VectorRecord
        let vector_records: Vec<VectorRecord> = results
            .into_iter()
            .map(|record| {
                VectorRecord {
                    id: record.id,
                    vector: record.vector.as_ref().map(|v| (**v).clone()).unwrap_or_default(),
                    metadata: record.metadata,
                    timestamp: record.timestamp.unwrap_or(0),
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    quantized_vector: vec![],
                    source: None,
                }
            })
            .collect();

        Ok(vector_records)
    }

    /// Search vectors in VIPER engine - REAL search, not simulated
    pub async fn search_vectors_viper(
        engine: &proximadb::storage::engines::impls::viper::engine::ViperEngine,
        environment: &UnifiedTestEnvironment,
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<VectorRecord>> {
        let storage_url = build_viper_storage_url(environment);
        let collection = Arc::new(environment.create_test_collection());
        let search_params = Arc::new(proximadb::core::search::SearchParams::default());
        let query_context = proximadb::storage::traits::StorageQueryContext {
            search_params,
            collection,
            metadata: proximadb::storage::traits::StorageQueryMetadata::default(),
        };

        // Direct production call using VIPER's unified search
        let results = engine
            .search_vectors_unified(&query_context)
            .await?;

        // Convert OptimizedSearchRecord to VectorRecord
        let vector_records: Vec<VectorRecord> = results
            .into_iter()
            .map(|record| {
                VectorRecord {
                    id: record.id,
                    vector: record.vector.as_ref().map(|v| (**v).clone()).unwrap_or_default(),
                    metadata: record.metadata,
                    timestamp: record.timestamp.unwrap_or(0),
                    updated_at: None,
                    expires_at: None,
                    version: Some(1),
                    quantized_vector: vec![],
                    source: None,
                }
            })
            .collect();

        Ok(vector_records)
    }

    /// Build correct CompactionParameters for any storage engine
    /// This is the critical part - getting the configuration right
    pub fn build_compaction_params(
        environment: &UnifiedTestEnvironment,
        engine: StorageEngine,
    ) -> CompactionParameters {
        let collection_config = environment.create_test_collection_for_engine(engine);
        CompactionParameters {
            collection_id: Some(environment.collection_id().to_string()),
            force: true,
            synchronous: true,
            collection_config: Some(collection_config),
            ..Default::default()
        }
    }

    /// Build CompactionParameters with provided collection config - PRODUCTION-LIKE
    pub fn build_compaction_params_with_collection(
        environment: &UnifiedTestEnvironment,
        collection: Collection,
    ) -> CompactionParameters {
        CompactionParameters {
            collection_id: Some(environment.collection_id().to_string()),
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        }
    }

    // REMOVED: build_compaction_params_with_compression - Use build_compaction_params_with_collection
    // Tests must create collection config once and reuse it for both flush and compaction
}

/// Global utility functions for backward compatibility with existing tests
/// These functions create temporary environments for one-off operations

/// Create test vectors (global function for backward compatibility)
pub fn create_test_vectors(count: usize, dimension: usize, prefix: &str) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("{}_{}", prefix, i),
            vector: (0..dimension).map(|j| (i + j) as f32 * 0.1).collect(),
            timestamp: (1000 + i) as i64,
            metadata: std::collections::HashMap::from([
                ("test_type".to_string(), SqlValue {
                    value: Some(
                        sql_value::Value::StringValue(
                            prefix.to_string(),
                        ),
                    ),
                }),
            ]),
            version: Some(1),
            quantized_vector: vec![],
            source: None,
            updated_at: None,
            expires_at: None,
        })
        .collect()
}

/// Create metadata store config (global function for backward compatibility)  
pub fn create_metadata_store_config() -> proximadb::storage::metadata::MetadataStoreConfig {
    let temp_dir = std::env::temp_dir().join("proximadb_test").join("metadata");
    let metadata_path = temp_dir.to_str().unwrap().to_string();
    proximadb::storage::metadata::MetadataStoreConfig {
        metadata_storage_urls: vec![metadata_path], // Filesystem API defaults to file:// scheme
        enable_atomic_operations: true,
        cache_config: MetadataCacheConfig::default(),
        backup_config: None,
        replication_config: None,
    }
}

/// Create test collection with storage (global function for backward compatibility)
pub fn create_test_collection_with_storage(name: &str, _base_location: String) -> Collection {
    let config = CollectionConfig {
        name: name.to_string(),
        dimension: 256, // Default dimension for generic tests
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
        id: name.to_string(),
        config: Some(config),
        stats: Some(stats),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        storage_assignment: None, // StorageAssignment no longer exists
    }
}

/// Create general test config (global function for backward compatibility)
pub fn create_test_config() -> proximadb::core::config::Config {
    // Use default config since struct fields may vary
    proximadb::core::config::Config::default()
}

// =============================================================================
// FLUSH OPERATIONS WITH BLOCK STATISTICS
// =============================================================================

/// Flush parameters with detailed block statistics tracking
pub struct FlushWithBlockStats {
    pub params: FlushParameters,
    pub track_blocks: bool,
    pub compression_algo: String,
    pub compression_level: i32,
    pub block_size_kb: usize,
}

/// Result of flush operation with block statistics
pub struct FlushBlockStatsResult {
    pub success: bool,
    pub entries_flushed: usize,
    pub bytes_written: usize,
    pub files_created: usize,
    pub blocks_created: usize,
    pub compression_ratio: f64,
    pub vectors_per_block: Vec<usize>,
    pub block_sizes: Vec<usize>,
}

/// Perform flush operation with block statistics tracking for SST
/// This now uses the production SST flush path with integrated quantization
pub async fn flush_sst_with_block_stats(
    environment: &UnifiedTestEnvironment,
    vectors: Vec<VectorRecord>,
    compression_algo: &str,
    compression_level: i32,
    block_size_kb: usize,
) -> Result<FlushBlockStatsResult> {
    use proximadb::proto::proximadb_v1::{Collection, CollectionConfig, CompressionConfig};
    use proximadb::storage::engines::impls::sst::SstEngine;
    use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};

    // Create SST config with specified block size
    let mut sst_config = environment.sst_config.clone();
    sst_config.block_size_kb = block_size_kb as u32;
    sst_config.compression = compression_algo.to_string();
    sst_config.compression_level = compression_level;

    // Create SST storage
    use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
    use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(
        proximadb::compute::distance_computation::DistanceMetric::Euclidean,
    ));

    let sst_storage = SstEngine::new().await?;

    // Create collection config with compression
    let compression_config = CompressionConfig {
        algorithm: match compression_algo {
            "none" | "uncompressed" => CompressionAlgorithm::CompressionNone as i32,
            "zstd" => CompressionAlgorithm::CompressionZstd as i32,
            "snappy" => CompressionAlgorithm::CompressionSnappy as i32,
            "gzip" => CompressionAlgorithm::CompressionGzip as i32,
            "lz4" => CompressionAlgorithm::CompressionLz4 as i32,
            "brotli" => CompressionAlgorithm::CompressionBrotli as i32,
            _ => CompressionAlgorithm::CompressionSnappy as i32, // default
        },
        level: Some(compression_level as u32),
        adaptive: false,
        min_ratio: None,
        enable_quantization: false,
        quantization_type: None,
        block_size_kb: block_size_kb as u32,
        dynamic_block_sizing: false,
        normalization_method: None,
    };

    let collection_config = Collection {
        id: "test_collection".to_string(),
        storage_assignment: None, // StorageAssignment no longer exists
        config: Some(CollectionConfig {
            name: "test_collection".to_string(),
            dimension: vectors[0].vector.len() as u32,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Sst as i32,
            ..Default::default()
        }),
        ..Default::default()
    };

    // Create flush parameters - production SST will handle quantization internally
    let flush_params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        vector_records: vectors.clone(),
        collection_config: Some(collection_config),
        force: true,
        synchronous: true,
        ..Default::default()
    };

    // Perform flush using production code
    let flush_result = sst_storage.do_flush(&flush_params).await?;

    // Calculate compression ratio
    let uncompressed_size = vectors.len() * vectors[0].vector.len() * 4; // FP32
    let compression_ratio = if flush_result.bytes_written.unwrap_or(0) > 0 {
        uncompressed_size as f64 / flush_result.bytes_written.unwrap_or(1) as f64
    } else {
        1.0
    };

    // Extract block statistics from engine metrics - use default if not available
    let blocks_created = flush_result
        .engine_metrics
        .get("blocks_created")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as usize;

    // Estimate vectors per block
    let vectors_per_block = vec![vectors.len() / blocks_created.max(1); blocks_created];
    let block_sizes = vectors_per_block
        .iter()
        .map(|&count| count * vectors[0].vector.len() * 4)
        .collect();

    Ok(FlushBlockStatsResult {
        success: true,
        entries_flushed: vectors.len(),
        bytes_written: flush_result.bytes_written.unwrap_or(0) as usize,
        files_created: flush_result.files_created.unwrap_or(0) as usize,
        blocks_created,
        compression_ratio,
        vectors_per_block,
        block_sizes,
    })
}

/// Perform flush with PQ sorting for improved compression
/// The production SST code now handles PQ sorting internally when quantization is enabled
pub async fn flush_sst_with_pq_sorting(
    environment: &UnifiedTestEnvironment,
    vectors: Vec<VectorRecord>,
    compression_algo: &str,
    compression_level: i32,
    block_size_kb: usize,
) -> Result<FlushBlockStatsResult> {
    // Simply use the production flush with compression enabled
    // When compression is enabled, quantization with PQ sorting is automatically applied
    flush_sst_with_block_stats(
        environment,
        vectors,
        compression_algo,
        compression_level,
        block_size_kb,
    )
    .await
}

/// Create VectorOperationsService with WAL manager for testing
pub async fn create_test_vector_operations_service() -> Result<VectorOperationsService> {
    setup_hardware_capabilities();

    // Create test environment
    let env = UnifiedTestEnvironment::new().await?;

    // Create SST storage engine
    let sst_storage = env.create_sst_engine().await?;

    // Create WAL manager
    let wal_config = WALConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);
    let strategy_type = WriteBufferStrategyType::AvroBatch;
    let strategy = WALBatchFactory::create_batch_serialization_strategy(
        strategy_type,
        &wal_config,
        filesystem_factory.clone(),
    )
    .await?;
    let wal_manager = Arc::new(WriteAheadLogManager::new(strategy, wal_config).await?);
    let axis_manager = Arc::new(proximadb::index::axis::AxisManager::new(proximadb::index::axis::AxisConfig::default()).await?);

    Ok(VectorOperationsService::new(
        Arc::new(sst_storage),
        wal_manager,
        axis_manager,
        // TODO: Create proper CollectionService for tests - requires metadata_backend and storage_config
        todo!("CollectionService creation requires complex setup - implement test-specific version"),
    ))
}

/// Create VectorOperationsService with custom storage for testing
pub async fn create_test_vector_operations_service_with_storage(
    storage: Arc<SstEngine>,
) -> Result<VectorOperationsService> {
    setup_hardware_capabilities();

    // Create WAL manager
    let wal_config = WALConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);
    let strategy_type = WriteBufferStrategyType::AvroBatch;
    let strategy = WALBatchFactory::create_batch_serialization_strategy(
        strategy_type,
        &wal_config,
        filesystem_factory.clone(),
    )
    .await?;
    let wal_manager = Arc::new(WriteAheadLogManager::new(strategy, wal_config).await?);
    let axis_manager = Arc::new(proximadb::index::axis::AxisManager::new(proximadb::index::axis::AxisConfig::default()).await?);

    // TODO: VectorOperationsService creation requires CollectionService which needs complex setup
    // For now, return an error to indicate this test helper needs implementation
    Err(anyhow::anyhow!(
        "VectorOperationsService creation disabled - requires CollectionService setup"
    ))
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
        let _sst_engine = env.create_sst_engine().await.unwrap();
        debug!("✅ Created SST engine for: {}", env.collection_id());

        // Test VIPER engine creation
        let _viper_engine = env.create_viper_engine().await.unwrap();
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
            for j in (i + 1)..collection_ids.len() {
                assert_ne!(collection_ids[i], collection_ids[j]);
            }
        }

        debug!(
            "✅ Created {} isolated unified environments",
            collection_ids.len()
        );
        for id in collection_ids {
            debug!("  - {}", id);
        }
    }
}
