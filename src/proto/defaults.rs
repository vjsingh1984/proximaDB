//! Proto Defaults Module
//!
//! This module provides default values for optional proto fields as documented in the proto files.
//! These defaults are applied when fields are not provided by the client.

use super::proximadb_v1::*;

/// Apply smart defaults to CollectionConfig based on proto comments
pub fn apply_collection_config_defaults(config: &mut CollectionConfig) {
    // Default distance_metric to COSINE
    if config.distance_metric.is_none() {
        config.distance_metric = Some(DistanceMetric::Cosine as i32);
    }

    // Default storage_engine to SST (primary production engine)
    if config.storage_engine.is_none() {
        config.storage_engine = Some(StorageEngine::Sst as i32);
    }

    // Default primary_index to empty string (no primary index)
    if config.primary_index.is_none() {
        config.primary_index = Some(String::new());
    }

    // Default auto_index_selection to true
    if config.auto_index_selection.is_none() {
        config.auto_index_selection = Some(true);
    }
}

/// Apply smart defaults to IndexConfig
pub fn apply_index_config_defaults(config: &mut IndexConfig) {
    // Default enabled to true
    if config.enabled.is_none() {
        config.enabled = Some(true);
    }

    // Default update_mode to 0 (synchronous)
    if config.update_mode.is_none() {
        config.update_mode = Some(0);
    }

    // Default enable_background_optimization to true
    if config.enable_background_optimization.is_none() {
        config.enable_background_optimization = Some(true);
    }

    // Default build_concurrency to num_cpus
    if config.build_concurrency.is_none() {
        config.build_concurrency = Some(num_cpus::get() as u32);
    }

    // Default memory_limit_mb to 20% of system RAM (or 512MB minimum)
    if config.memory_limit_mb.is_none() {
        let sys = sysinfo::System::new_all();
        let total_mem_mb = (sys.total_memory() / 1024 / 1024) as u32;
        let limit = std::cmp::max(512, total_mem_mb / 5);
        config.memory_limit_mb = Some(limit);
    }

    // Default checkpoint_interval_ms to 60000 (1 minute)
    if config.checkpoint_interval_ms.is_none() {
        config.checkpoint_interval_ms = Some(60000);
    }

    // Default is_primary to false
    if config.is_primary.is_none() {
        config.is_primary = Some(false);
    }

    // Default selectivity_threshold to 0.1
    if config.selectivity_threshold.is_none() {
        config.selectivity_threshold = Some(0.1);
    }

    // Default use_quantization to false
    if config.use_quantization.is_none() {
        config.use_quantization = Some(false);
    }

    // Default queue_representation to empty string
    if config.queue_representation.is_none() {
        config.queue_representation = Some(String::new());
    }
}

/// Apply smart defaults to StorageConfig
pub fn apply_storage_config_defaults(config: &mut StorageConfig) {
    // storage_path defaults to auto-generated based on collection ID
    // (handled at runtime when collection ID is known)

    // Default compression to COMPRESSION_ZSTD
    if config.compression.is_none() {
        config.compression = Some(CompressionAlgorithm::CompressionZstd as i32);
    }

    // Default max_file_size_mb to 128MB
    if config.max_file_size_mb.is_none() {
        config.max_file_size_mb = Some(128);
    }

    // Default enable_caching to true
    if config.enable_caching.is_none() {
        config.enable_caching = Some(true);
    }
}

/// Apply smart defaults to VectorRecord
pub fn apply_vector_record_defaults(record: &mut VectorRecord) {
    // Default timestamp to current time (server-generated)
    if record.timestamp.is_none() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        record.timestamp = Some(now);
    }

    // Default version to 0
    if record.version.is_none() {
        record.version = Some(0);
    }
}

/// Apply smart defaults to QuantizationConfig
pub fn apply_quantization_config_defaults(config: &mut QuantizationConfig) {
    // Default enabled to false
    if config.enabled.is_none() {
        config.enabled = Some(false);
    }

    // Default strategy to SMART_DEFAULTS
    if config.strategy.is_none() {
        config.strategy = Some(quantization_config::Strategy::SmartDefaults as i32);
    }

    // Default enable_progressive_search to true
    if config.enable_progressive_search.is_none() {
        config.enable_progressive_search = Some(true);
    }

    // Default selectivity thresholds
    if config.binary_filter_selectivity.is_none() {
        config.binary_filter_selectivity = Some(0.1);
    }
    if config.int8_ranking_selectivity.is_none() {
        config.int8_ranking_selectivity = Some(0.3);
    }
    if config.pq_ranking_selectivity.is_none() {
        config.pq_ranking_selectivity = Some(0.5);
    }

    // Default training_sample_size to 10000
    if config.training_sample_size.is_none() {
        config.training_sample_size = Some(10000);
    }

    // Default quality_threshold to 0.95
    if config.quality_threshold.is_none() {
        config.quality_threshold = Some(0.95);
    }

    // Additional compatibility defaults
    if config.enable_adaptive_training.is_none() {
        config.enable_adaptive_training = Some(true);
    }
    if config.enable_simd_acceleration.is_none() {
        config.enable_simd_acceleration = Some(true);
    }
    if config.enable_binary.is_none() {
        config.enable_binary = Some(true);
    }
    if config.enable_int8.is_none() {
        config.enable_int8 = Some(true);
    }
    if config.enable_pq.is_none() {
        config.enable_pq = Some(false);
    }

    // PQ defaults
    if config.pq_segments.is_none() {
        config.pq_segments = Some(8);
    }
    if config.pq_bits.is_none() {
        config.pq_bits = Some(8);
    }
    if config.pq_codebooks.is_none() {
        config.pq_codebooks = Some(256);
    }
}

/// Apply smart defaults to HnswConfig
pub fn apply_hnsw_config_defaults(config: &mut HnswConfig) {
    if config.m.is_none() {
        config.m = Some(16);
    }
    if config.ef_construction.is_none() {
        config.ef_construction = Some(200);
    }
    if config.ef_search.is_none() {
        config.ef_search = Some(100);
    }
    if config.max_partition_size.is_none() {
        config.max_partition_size = Some(100000);
    }
    if config.adaptive_parameters.is_none() {
        config.adaptive_parameters = Some(true);
    }
    if config.use_simd.is_none() {
        config.use_simd = Some(true);
    }
    if config.memory_limit_mb.is_none() {
        config.memory_limit_mb = Some(512);
    }
    if config.lazy_loading.is_none() {
        config.lazy_loading = Some(false);
    }
}

/// Apply smart defaults to IvfConfig
pub fn apply_ivf_config_defaults(config: &mut IvfConfig, num_vectors: Option<u32>) {
    // Default n_lists to sqrt(num_vectors) if known
    if config.n_lists.is_none() {
        if let Some(n) = num_vectors {
            config.n_lists = Some((n as f32).sqrt() as u32);
        } else {
            config.n_lists = Some(100); // Default fallback
        }
    }

    if config.n_probe.is_none() {
        config.n_probe = Some(10);
    }
    if config.quantization_bits.is_none() {
        config.quantization_bits = Some(8);
    }
    if config.use_pq.is_none() {
        config.use_pq = Some(false);
    }
    if config.pq_subspaces.is_none() {
        config.pq_subspaces = Some(8);
    }
    if config.train_on_insert.is_none() {
        config.train_on_insert = Some(true);
    }
    if config.min_train_size.is_none() {
        config.min_train_size = Some(1000);
    }
}

/// Apply smart defaults to LshConfig
pub fn apply_lsh_config_defaults(config: &mut LshConfig) {
    if config.n_hash_tables.is_none() {
        config.n_hash_tables = Some(10);
    }
    if config.n_hash_functions.is_none() {
        config.n_hash_functions = Some(4);
    }
    if config.bucket_width.is_none() {
        config.bucket_width = Some(4.0);
    }
    if config.binary_vectors.is_none() {
        config.binary_vectors = Some(false);
    }
    if config.max_candidates.is_none() {
        config.max_candidates = Some(1000);
    }
    if config.projection.is_none() {
        config.projection = Some(0);
    }
}
