//! Test helper functions for HELIX engine
//!
//! This module consolidates all helper functions used across HELIX engine tests.
//! It provides utilities for:
//! - Engine creation and configuration
//! - Test data generation (vectors, collections, metadata)
//! - Filesystem and storage setup
//! - Query pattern simulation
//! - Hilbert curve utilities
//! - PCA and clustering helpers

use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::proto::proximadb_v1::{
    Collection, CollectionConfig, CollectionStats, DistanceMetric as ProtoDistanceMetric,
    StorageAssignment, StorageEngine, VectorRecord,
};
use crate::storage::engines::helix::{HelixConfig, HelixEngine};
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use crate::storage::traits::FlushParameters;

// ============================================================================
// ENGINE CREATION UTILITIES
// ============================================================================

/// Create a default HELIX engine for testing
pub async fn create_test_engine() -> Result<HelixEngine, Box<dyn std::error::Error>> {
    let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init
    HelixEngine::new().await.map_err(|e| e.into())
}

/// Create a HELIX engine with custom configuration
pub async fn create_test_engine_with_config(
    config: HelixConfig,
    temp_dir: &TempDir,
) -> Result<HelixEngine, Box<dyn std::error::Error>> {
    let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

    let path = temp_dir.path().to_str().unwrap().to_string();

    let mut fs_config = FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(FilesystemFactory::create(fs_config).await?);

    let distance_compute = Arc::new(UnifiedDistanceCompute::default());

    HelixEngine::new_with_config(config, filesystem_factory, distance_compute)
        .await
        .map_err(|e| e.into())
}

/// Create a minimal HELIX engine with filesystem and distance compute
pub async fn create_minimal_engine(
    temp_dir: &TempDir,
) -> Result<
    (
        HelixEngine,
        Arc<FilesystemFactory>,
        Arc<UnifiedDistanceCompute>,
    ),
    Box<dyn std::error::Error>,
> {
    let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

    let path = temp_dir.path().to_str().unwrap().to_string();

    let mut fs_config = FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(FilesystemFactory::create(fs_config).await?);

    let distance_compute = Arc::new(UnifiedDistanceCompute::default());

    let config = HelixConfig::default();
    let engine =
        HelixEngine::new_with_config(config, filesystem_factory.clone(), distance_compute.clone())
            .await?;

    Ok((engine, filesystem_factory, distance_compute))
}

// ============================================================================
// TEST DATA GENERATION
// ============================================================================

/// Create simple test vector records with sequential patterns
///
/// Vectors have the pattern: (i * dims + d) / 100.0 for vector i, dimension d
pub fn create_test_records(count: usize, dims: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{}", i),
            vector: (0..dims).map(|d| (i * dims + d) as f32 / 100.0).collect(),
            metadata: HashMap::from([
                (
                    "type".to_string(),
                    crate::proto::proximadb_v1::SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                            "test".to_string(),
                        )),
                    },
                ),
                (
                    "index".to_string(),
                    crate::proto::proximadb_v1::SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                            i.to_string(),
                        )),
                    },
                ),
            ]),
            timestamp: Some(i as i64),
            expires_at: None,
            source: None,
            updated_at: None,
            version: Some(1),
        })
        .collect()
}

/// Create test vectors with clustering patterns
///
/// Every 100 vectors form a cluster with similar values
pub fn create_test_vectors(count: usize, dimensions: usize) -> Vec<VectorRecord> {
    let mut records = Vec::new();

    for i in 0..count {
        // Create vectors with patterns for clustering
        let mut vector = Vec::with_capacity(dimensions);
        for d in 0..dimensions {
            // Create clusters in the data
            let cluster = i / 100; // Every 100 vectors form a cluster
            let base = cluster as f32 * 10.0;
            let noise = (i as f32 * 0.1).sin() * 0.5;
            vector.push(base + (d as f32) + noise);
        }

        records.push(VectorRecord {
            id: format!("vec_{:06}", i),
            vector,
            metadata: HashMap::new(),
            timestamp: Some(i as i64),
            expires_at: None,
            updated_at: Some(i as i64),
            version: Some(1),
            source: None,
        });
    }

    records
}

/// Generate random vector records for benchmarking
pub fn generate_random_vectors(count: usize, dims: usize, seed: u64) -> Vec<VectorRecord> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    (0..count)
        .map(|i| {
            let vector: Vec<f32> = (0..dims).map(|_| rng.gen_range(-1.0..1.0)).collect();

            VectorRecord {
                id: format!("vec_{}", i),
                vector,
                metadata: HashMap::from([
                    (
                        "type".to_string(),
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                "benchmark".to_string(),
                            )),
                        },
                    ),
                    (
                        "cluster".to_string(),
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                (i % 10).to_string(),
                            )),
                        },
                    ),
                ]),
                timestamp: Some(i as i64),
                expires_at: None,
                source: None,
                updated_at: None,
                version: Some(1),
            }
        })
        .collect()
}

/// Generate clustered vectors (for testing clustering effectiveness)
pub fn generate_clustered_vectors(
    num_clusters: usize,
    vectors_per_cluster: usize,
    dims: usize,
) -> Vec<VectorRecord> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    let mut all_vectors = Vec::new();

    for cluster_id in 0..num_clusters {
        // Generate cluster center
        let center: Vec<f32> = (0..dims).map(|_| rng.gen_range(-10.0..10.0)).collect();

        // Generate vectors around center
        for i in 0..vectors_per_cluster {
            let mut vector = center.clone();
            for v in &mut vector {
                *v += rng.gen_range(-0.5..0.5); // Small noise around center
            }

            all_vectors.push(VectorRecord {
                id: format!("cluster_{}_vec_{}", cluster_id, i),
                vector,
                metadata: HashMap::from([(
                    "cluster_id".to_string(),
                    crate::proto::proximadb_v1::SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                            cluster_id.to_string(),
                        )),
                    },
                )]),
                timestamp: Some((cluster_id * vectors_per_cluster + i) as i64),
                expires_at: None,
                source: None,
                updated_at: None,
                version: Some(1),
            });
        }
    }

    all_vectors
}

// ============================================================================
// COLLECTION AND CONFIGURATION UTILITIES
// ============================================================================

/// Create a test collection configuration
pub fn create_test_collection(
    collection_id: &str,
    dimension: usize,
    base_path: &str,
) -> Collection {
    let collection_config = CollectionConfig {
        name: collection_id.to_string(),
        dimension: dimension as u32,
        distance_metric: Some(ProtoDistanceMetric::Euclidean as i32),
        storage_engine: Some(StorageEngine::Helix as i32),
        ..Default::default()
    };

    Collection {
        id: collection_id.to_string(),
        config: Some(collection_config),
        stats: Some(CollectionStats {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: format!("{}/helix", base_path),
            backup_paths: vec![],
            engine: StorageEngine::Helix as i32,
            engine_config: HashMap::new(),
            base_location: base_path.to_string(),
            assigned_at: 0,
        }),
        created_at: 0,
        updated_at: 0,
    }
}

/// Create flush parameters for testing
pub fn create_flush_params(
    collection_id: &str,
    vectors: Vec<VectorRecord>,
    collection: Collection,
) -> FlushParameters {
    FlushParameters {
        collection_id: Some(collection_id.to_string()),
        vector_records: vectors,
        force: false,
        synchronous: true,
        collection_config: Some(collection),
        hints: HashMap::new(),
        timeout_ms: None,
        trigger_compaction: false,
        batch_ids: vec![],
        estimated_size: 0,
    }
}

/// Create flush parameters with custom options
pub fn create_flush_params_with_options(
    collection_id: &str,
    vectors: Vec<VectorRecord>,
    collection: Collection,
    force: bool,
    trigger_compaction: bool,
    estimated_size: usize,
) -> FlushParameters {
    FlushParameters {
        collection_id: Some(collection_id.to_string()),
        vector_records: vectors,
        force,
        synchronous: true,
        collection_config: Some(collection),
        hints: HashMap::new(),
        timeout_ms: Some(5000),
        trigger_compaction,
        batch_ids: vec![],
        estimated_size,
    }
}

// ============================================================================
// FILESYSTEM AND STORAGE UTILITIES
// ============================================================================

/// Create a temporary directory and filesystem factory
pub async fn create_test_filesystem()
-> Result<(TempDir, Arc<FilesystemFactory>), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let path = temp_dir.path().to_str().unwrap().to_string();

    let mut fs_config = FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(FilesystemFactory::create(fs_config).await?);

    Ok((temp_dir, filesystem_factory))
}

/// Create a temporary directory with custom path
pub fn create_temp_dir() -> Result<TempDir, Box<dyn std::error::Error>> {
    TempDir::new().map_err(|e| e.into())
}

/// Get path string from temp directory
pub fn get_temp_path(temp_dir: &TempDir) -> String {
    temp_dir.path().to_str().unwrap().to_string()
}

// ============================================================================
// HILBERT CURVE UTILITIES
// ============================================================================

/// Create diverse vectors for Hilbert key testing (avoiding uniform values)
///
/// Returns vectors with different patterns to ensure distinct Hilbert keys
pub fn create_diverse_vectors(count: usize, dims: usize) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            (0..dims)
                .map(|d| {
                    // Create diverse patterns
                    let pattern = (i * dims + d) as f32;
                    (pattern.sin() + pattern.cos()) / 2.0
                })
                .collect()
        })
        .collect()
}

/// Generate Hilbert keys for a set of vectors
pub fn generate_hilbert_keys(vectors: &[VectorRecord]) -> Vec<u64> {
    use crate::storage::engines::helix::clustering::compute_hilbert_key;
    vectors
        .iter()
        .map(|v| compute_hilbert_key(&v.vector))
        .collect()
}

// ============================================================================
// CLUSTERING UTILITIES
// ============================================================================

/// Create a query pattern tracker with simulated access patterns
pub fn create_query_tracker_with_patterns(
    hot_ids: Vec<&str>,
    hot_access_count: usize,
    cold_ids: Vec<&str>,
    cold_access_count: usize,
) -> crate::storage::engines::helix::clustering::QueryPatternTracker {
    use crate::storage::engines::helix::clustering::QueryPatternTracker;

    let mut tracker = QueryPatternTracker::default();

    // Record hot access patterns
    for id in hot_ids {
        for _ in 0..hot_access_count {
            tracker.record_access(id, 100);
        }
    }

    // Record cold access patterns
    for id in cold_ids {
        for _ in 0..cold_access_count {
            tracker.record_access(id, 200);
        }
    }

    tracker
}

// ============================================================================
// ZONE MAP UTILITIES
// ============================================================================

/// Create zone map builder with test vectors
pub fn create_zone_map_builder(
    vectors_per_block: usize,
) -> crate::storage::engines::helix::zone_maps::ZoneMapBuilder {
    use crate::storage::engines::helix::zone_maps::ZoneMapBuilder;
    ZoneMapBuilder::new(vectors_per_block)
}

// ============================================================================
// PCA UTILITIES
// ============================================================================

/// Train a PCA model with test vectors
pub fn train_test_pca_model(
    vectors: &[VectorRecord],
    n_components: usize,
) -> Result<
    crate::storage::engines::helix::pca_impl::EnhancedPCAModel,
    Box<dyn std::error::Error>,
> {
    use crate::storage::engines::helix::pca_impl::EnhancedPCAModel;
    EnhancedPCAModel::train(vectors, n_components).map_err(|e| e.into())
}

// ============================================================================
// SETUP AND TEARDOWN UTILITIES
// ============================================================================

/// Initialize hardware capabilities (idempotent)
pub fn init_hardware() {
    let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init
}

/// Setup a complete test environment with engine, filesystem, and collection
pub async fn setup_test_environment(
    collection_id: &str,
    dimension: usize,
) -> Result<(HelixEngine, TempDir, Collection), Box<dyn std::error::Error>> {
    init_hardware();

    let temp_dir = create_temp_dir()?;
    let path = get_temp_path(&temp_dir);

    let mut fs_config = FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(FilesystemFactory::create(fs_config).await?);

    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    let config = HelixConfig::default();

    let engine = HelixEngine::new_with_config(config, filesystem_factory, distance_compute).await?;
    let collection = create_test_collection(collection_id, dimension, &path);

    Ok((engine, temp_dir, collection))
}

/// Setup test environment with custom HELIX configuration
pub async fn setup_test_environment_with_config(
    collection_id: &str,
    dimension: usize,
    config: HelixConfig,
) -> Result<(HelixEngine, TempDir, Collection), Box<dyn std::error::Error>> {
    init_hardware();

    let temp_dir = create_temp_dir()?;
    let path = get_temp_path(&temp_dir);

    let mut fs_config = FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(FilesystemFactory::create(fs_config).await?);

    let distance_compute = Arc::new(UnifiedDistanceCompute::default());

    let engine = HelixEngine::new_with_config(config, filesystem_factory, distance_compute).await?;
    let collection = create_test_collection(collection_id, dimension, &path);

    Ok((engine, temp_dir, collection))
}

// ============================================================================
// SSTABLE METADATA UTILITIES
// ============================================================================

/// Create mock SSTable metadata for testing
pub fn create_mock_sstable_metadata(
    path: PathBuf,
    level: usize,
    hilbert_range: Option<(u64, u64)>,
    num_vectors: usize,
) -> crate::storage::engines::helix::SStableMetadata {
    use crate::storage::engines::helix::SStableMetadata;

    SStableMetadata {
        path,
        level,
        hilbert_range,
        num_vectors,
        size_bytes: (num_vectors * 512) as u64, // Approximate size
        created_at: chrono::Utc::now(),
        blocks: vec![],
        bloom_filter: None,
    }
}

/// Create multiple mock SSTables for testing pruning
pub fn create_mock_sstables(
    count: usize,
) -> Vec<crate::storage::engines::helix::SStableMetadata> {
    (0..count)
        .map(|i| {
            let start = i as u64 * 1000;
            let end = start + 1000;
            create_mock_sstable_metadata(
                PathBuf::from(format!("test{}.helix", i)),
                1,
                Some((start, end)),
                100,
            )
        })
        .collect()
}

// ============================================================================
// DISTANCE COMPUTATION UTILITIES
// ============================================================================

/// Create a default distance compute engine
pub fn create_distance_compute() -> Arc<UnifiedDistanceCompute> {
    Arc::new(UnifiedDistanceCompute::default())
}

// ============================================================================
// CONFIGURATION HELPERS
// ============================================================================

/// Create HELIX config optimized for compaction testing
pub fn create_compaction_test_config() -> HelixConfig {
    let mut config = HelixConfig::default();
    config.level0_file_num_compaction_trigger = 2;
    // config.max_bytes_for_level_base = 10_000_000; // 10MB // Field not found
    // config.level0_slowdown_writes_trigger = 8; // Field not found
    config
}

/// Create HELIX config optimized for PCA testing
pub fn create_pca_test_config(pca_dimensions: usize) -> HelixConfig {
    let mut config = HelixConfig::default();
    config.pca_dimensions = pca_dimensions;
    // config.pca_training_sample_size = 100; // Field not found
    config
}

/// Create HELIX config with liquid clustering enabled
pub fn create_liquid_clustering_config() -> HelixConfig {
    let mut config = HelixConfig::default();
    config.enable_liquid_clustering = true;
    config
}

/// Create HELIX config for zone map testing
pub fn create_zone_map_config(vectors_per_block: usize) -> HelixConfig {
    let mut config = HelixConfig::default();
    config.proxima_block_size = vectors_per_block;
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::traits::UnifiedStorageFormat;

    #[test]
    fn test_create_test_records() {
        let records = create_test_records(10, 4);
        assert_eq!(records.len(), 10);
        assert_eq!(records[0].id, "vec_0");
        assert_eq!(records[0].vector.len(), 4);
    }

    #[test]
    fn test_create_test_vectors() {
        let vectors = create_test_vectors(250, 64);
        assert_eq!(vectors.len(), 250);
        assert_eq!(vectors[0].vector.len(), 64);

        // Check clustering pattern
        let cluster_0_vec = &vectors[0].vector;
        let cluster_1_vec = &vectors[100].vector;

        // First dimension should differ by approximately 10.0 between clusters
        let diff = (cluster_1_vec[0] - cluster_0_vec[0]).abs();
        assert!(
            diff > 9.0 && diff < 11.0,
            "Clustering pattern not working: diff = {}",
            diff
        );
    }

    #[test]
    fn test_generate_random_vectors() {
        let vectors = generate_random_vectors(100, 32, 42);
        assert_eq!(vectors.len(), 100);
        assert_eq!(vectors[0].vector.len(), 32);

        // Values should be in range [-1.0, 1.0]
        for v in &vectors[0].vector {
            assert!(*v >= -1.0 && *v <= 1.0);
        }
    }

    #[test]
    fn test_generate_clustered_vectors() {
        let vectors = generate_clustered_vectors(5, 20, 16);
        assert_eq!(vectors.len(), 100); // 5 clusters * 20 vectors
        assert_eq!(vectors[0].vector.len(), 16);

        // Check metadata
        assert!(vectors[0].metadata.contains_key("cluster_id"));
    }

    #[test]
    fn test_create_test_collection() {
        let collection = create_test_collection("test_coll", 128, "/tmp");
        assert_eq!(collection.id, "test_coll");
        assert_eq!(collection.config.as_ref().unwrap().dimension, 128);
    }

    #[test]
    fn test_create_diverse_vectors() {
        let vectors = create_diverse_vectors(10, 4);
        assert_eq!(vectors.len(), 10);
        assert_eq!(vectors[0].len(), 4);

        // Check that vectors are diverse (not all same)
        assert_ne!(vectors[0], vectors[1]);
    }

    #[tokio::test]
    async fn test_create_test_engine() {
        let result = create_test_engine().await;
        assert!(result.is_ok());
        let engine = result.unwrap();
        assert_eq!(engine.engine_name(), "helix");
    }

    #[tokio::test]
    async fn test_setup_test_environment() {
        let result = setup_test_environment("test_collection", 128).await;
        assert!(result.is_ok());

        let (engine, _temp_dir, collection) = result.unwrap();
        assert_eq!(engine.engine_name(), "helix");
        assert_eq!(collection.id, "test_collection");
        assert_eq!(collection.config.as_ref().unwrap().dimension, 128);
    }
}
