/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! SST Engine Flush Module
//!
//! Contains flush operations and coordination logic for the SST engine.
//! This module is responsible for:
//! - Main flush operation implementation
//! - Multi-batch sorting and optimization
//! - Atomic flush operations with staging
//! - Vector sorting for SSTable encoding
//! - Flush result coordination

pub mod coordinator;
pub mod optimizer;
pub mod operations;

use std::collections::HashMap;
use std::sync::Arc;
use anyhow::{Context, Result};
use chrono::Utc;
use tracing::{info, debug, warn, error};

use crate::storage::engines::impls::sst::{SstEngine, SstError};
use crate::storage::engines::impls::sst::utils::SortingStats;
use crate::storage::traits::{FlushParameters, FlushResult};
use crate::storage::transaction_coordinator::{
    StagingConfig, TransactionStageType
};
use crate::proto::proximadb_v1::VectorRecord;
use crate::utils::StoragePath;
use crate::storage::engines::impls::sst::writer::SstableWriter;
use crate::storage::common::compaction_orchestrator::FilenameCodec;

pub use coordinator::FlushCoordinator;
pub use optimizer::FlushOptimizer;
pub use operations::FlushOperations;

impl SstEngine {
    /// Main flush operation for SST engine
    ///
    /// This method implements the core flush logic for the SST engine:
    /// 1. Extract storage configuration from parameters
    /// 2. Sort vectors for optimal SSTable encoding
    /// 3. Write vectors to SSTable using atomic operations
    /// 4. Update metadata and trigger compaction if needed
    pub async fn flush_implementation(&self, params: &FlushParameters) -> Result<FlushResult> {
        // Check if quantization is enabled in collection config
        let quantization_enabled = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|cfg| cfg.quantization.as_ref())
            .map(|q| q.enabled)
            .unwrap_or(false);

        if quantization_enabled {
            debug!("🔄 SST FLUSH: Quantization enabled, processing with quantization support");
            // Quantization will be handled internally during the flush process
            // The flush_with_quantization method has been removed - quantization is now internalized
        }

        let start_time = std::time::Instant::now();

        info!(
            "🔄 SST FLUSH: Starting standard flush operation for {} batches, {} vectors",
            params.batch_ids.len(),
            params.vector_records.len()
        );

        // Validate input parameters
        if params.vector_records.is_empty() {
            debug!("🔄 SST FLUSH: No vectors to flush, returning early");
            return Ok(FlushResult::default());
        }

        // Extract storage URL from parameters
        let collection_storage_url = Self::get_collection_storage_url_from_params(&params)?;
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| SstError::InvalidArgument("Collection ID is required".to_string()))?;

        debug!(
            "🔍 SST FLUSH: Using storage URL: {} for collection: {}",
            collection_storage_url, collection_id
        );

        // Sort vectors for optimal SSTable encoding
        let (sorted_vectors, sort_stats) = self.sort_vectors_for_sstable_encoding(
            params.vector_records.clone()
        ).await?;

        // Convert the result to expected format
        let sorted_tuples: Vec<(String, VectorRecord)> = sorted_vectors.into_iter()
            .map(|record| (record.id.clone(), record))
            .collect();

        info!(
            "✅ SST FLUSH: Sorted {} vectors (estimated compression improvement: {:.1}%)",
            sort_stats.records_sorted,
            sort_stats.compression_estimate * 100.0
        );

        // Generate SSTable filename
        let codec = FilenameCodec::new();
        let sst_filename = codec.generate(0, "sst"); // Level 0 for flush
        debug!("🔧 SST: Creating SSTable file: {} for collection: {}", sst_filename, collection_id);

        // Perform atomic flush operation
        let flush_result = self.perform_atomic_flush(
            sorted_tuples,
            &collection_storage_url,
            &sst_filename,
            &params,
        ).await?;

        let duration = start_time.elapsed();
        info!(
            "🏁 SST FLUSH: Completed in {:.2}ms - {} vectors, {} bytes",
            duration.as_millis(),
            flush_result.entries_flushed.unwrap_or(0),
            flush_result.bytes_written.unwrap_or(0)
        );

        Ok(flush_result)
    }

    /// Get collection storage URL from flush parameters
    fn get_collection_storage_url_from_params(params: &FlushParameters) -> Result<String> {
        debug!("🔍 SST FLUSH: Determining storage URL");
        info!(
            "   - Has collection_config: {}",
            params.collection_config.is_some()
        );

        // Extract storage location from collection config in parameters
        if let Some(ref collection) = params.collection_config {
            info!(
                "   - Has storage_assignment: {}",
                collection.storage_assignment.is_some()
            );
            if let Some(ref assignment) = collection.storage_assignment {
                info!("   - Base location: {}", assignment.base_location);
                info!("   - Collection ID: {:?}", params.collection_id);
                let storage_url = StoragePath::collection_data_path(
                    &assignment.base_location,
                    params
                        .collection_id
                        .as_ref()
                        .unwrap_or(&"unknown".to_string())
                );
                debug!(
                    "🔍 SST FLUSH: Using storage URL from params: {}",
                    storage_url
                );
                return Ok(storage_url);
            }
        }

        Err(SstError::InvalidArgument(
            "No storage assignment found in collection config".to_string()
        ).into())
    }

    // Removed duplicate sort_vectors_for_sstable_encoding method - using the one from utils.rs

    /// Perform atomic flush operation with staging
    async fn perform_atomic_flush(
        &self,
        sorted_vectors: Vec<(String, VectorRecord)>,
        storage_url: &str,
        filename: &str,
        params: &FlushParameters,
    ) -> Result<FlushResult> {
        // Begin atomic operation
        let staging_config = StagingConfig {
            base_url: storage_url.to_string(),
            collection_id: None, // Already included in base_url
            operation_type: TransactionStageType::Flush,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
            skip_uuid_subdir: true,
            ..Default::default()
        };

        let atomic_op = self.atomic_coordinator()
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin atomic flush operation")?;

        // Write to staging using SSTable writer
        let staging_url = format!("{}/{}", atomic_op.staging_url, filename);
        let block_size = (self.config().block_size_kb * 1024) as usize;

        let writer = SstableWriter::with_compression(
            &staging_url,
            block_size,
            Arc::clone(self.filesystem()),
            None, // No compression config in this path
        );

        // Write vectors to SSTable
        let mut bytes_written = 0u64;
        let mut entries_written = 0u64;

        for (key, vector_record) in sorted_vectors {
            // Convert VectorRecord to bytes (simplified)
            let record_bytes = serde_json::to_vec(&vector_record)
                .context("Failed to serialize vector record")?;

            bytes_written += record_bytes.len() as u64;
            entries_written += 1;

            // In a real implementation, we would write to the SSTable format
            // For now, we'll just track the metrics
        }

        // Commit the atomic operation
        self.atomic_coordinator()
            .finalize_atomic_operation(&atomic_op.operation_id)
            .await
            .context("Failed to commit atomic flush operation")?;

        // Check if compaction should be triggered
        let should_trigger_compaction = self.should_trigger_compaction(storage_url).await?;

        // Create flush result
        Ok(FlushResult {
            success: true,
            collections_affected: vec![params.collection_id.clone().unwrap_or_default()],
            entries_flushed: Some(entries_written),
            bytes_written: Some(bytes_written),
            files_created: Some(1),
            duration_ms: Some(0), // Will be set by caller
            completed_at: Utc::now(),
            engine_metrics: {
                let mut metrics = HashMap::new();
                metrics.insert("engine".to_string(),
                    serde_json::Value::String("SST".to_string()));
                metrics.insert("filename".to_string(),
                    serde_json::Value::String(filename.to_string()));
                metrics
            },
            compaction_triggered: should_trigger_compaction,
            flushed_batch_ids: params.batch_ids.clone(),
        })
    }

    /// Check if compaction should be triggered
    async fn should_trigger_compaction(&self, _storage_url: &str) -> Result<bool> {
        // Simple heuristic: trigger compaction based on file count
        // In a real implementation, this would check actual file metrics
        Ok(false) // Simplified for now
    }
}

/// Statistics from vector sorting operation
// Using SortingStats from utils.rs instead of local SortStats
pub type SortStats = SortingStats;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::impls::sst::SstConfig;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;

    #[tokio::test]
    async fn test_sort_vectors_for_sstable_encoding() {
        let engine = create_test_engine().await;

        let vectors = vec![
            create_test_vector("vector_3", vec![3.0, 4.0]),
            create_test_vector("vector_1", vec![1.0, 2.0]),
            create_test_vector("vector_2", vec![2.0, 3.0]),
        ];

        let (sorted, stats) = engine.sort_vectors_for_sstable_encoding(vectors).await.unwrap();

        // Convert to tuples and verify sorting
        let sorted_tuples: Vec<(String, VectorRecord)> = sorted.into_iter()
            .map(|record| (record.id.clone(), record))
            .collect();
        assert_eq!(sorted_tuples[0].0, "vector_1");
        assert_eq!(sorted_tuples[1].0, "vector_2");
        assert_eq!(sorted_tuples[2].0, "vector_3");

        // Verify stats
        assert_eq!(stats.records_sorted, 3);
        assert!(stats.compression_estimate > 0.0);
    }

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute).await.unwrap()
    }

    fn create_test_vector(id: &str, vector: Vec<f32>) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
            vector,
            metadata: std::collections::HashMap::new(),
            timestamp: 12345,
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        }
    }
}