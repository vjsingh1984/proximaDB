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

//! SST Engine Trait Implementations
//!
//! Contains the main trait implementations for the SST engine,
//! delegating to the appropriate modules for actual functionality.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use tracing::{debug, info};

use crate::core::search::results::OptimizedSearchRecord;
use crate::storage::engines::sst::core::SstEngine;
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageEngineStrategy,
    StorageQueryContext, UnifiedStorageEngine,
};

#[async_trait]
impl UnifiedStorageEngine for SstEngine {
    fn engine_name(&self) -> &'static str {
        "sst"
    }

    fn engine_version(&self) -> &'static str {
        crate::version::PROXIMADB_VERSION
    }

    fn strategy(&self) -> StorageEngineStrategy {
        StorageEngineStrategy::Sst
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        self.filesystem().as_ref()
    }

    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        info!("🚀 SST: Starting flush operation");
        // Use the flush module implementation directly
        self.flush_implementation(params).await
    }

    /// SST's LSM bulk-load override (Phase 2F-b).
    ///
    /// Same shape as NOVA's override — SST's `flush_implementation`
    /// already takes `FlushParameters { vector_records, collection_config, .. }`
    /// and writes a single SSTable file via the existing writer, so
    /// the trait method just builds a synthetic params and delegates.
    /// WAL + memtable are bypassed because we never call into the
    /// per-record insert path.
    ///
    /// SST adds quantization on top of NOVA's path (Binary → INT8 →
    /// FP32 progressive search), so bulk-loaded segments inherit the
    /// same hierarchical bloom filters and progressive quantization
    /// the normal flush produces.
    async fn ingest_sorted_segment(
        &self,
        collection_id: &str,
        base_path: &str,
        records: Vec<proximadb_records::ProximaRecord>,
    ) -> Result<crate::storage::traits::SegmentIngestResult> {
        use crate::proto::proximadb_v1::{Collection, StorageAssignment};

        let count = records.len();
        if count == 0 {
            return Ok(crate::storage::traits::SegmentIngestResult {
                collection_id: collection_id.to_string(),
                record_count: 0,
                synthetic_segment_id: "empty".to_string(),
                used_engine_specific_path: true,
            });
        }

        // Minimal synthetic Collection so flush_implementation knows
        // where to write. `config = None` lets dimension fall back to
        // inspecting records[0].embeddings[0].dim inside the flush
        // path (existing fallback chain). `base_location` is the only
        // field flush_implementation actually reads from the
        // assignment for storage URL resolution.
        let collection_config = Some(Collection {
            id: collection_id.to_string(),
            config: None,
            stats: None,
            created_at: 0,
            updated_at: 0,
            storage_assignment: Some(StorageAssignment {
                primary_path: base_path.to_string(),
                backup_paths: vec![],
                engine: 0,
                engine_config: std::collections::HashMap::new(),
                base_location: base_path.to_string(),
                assigned_at: 0,
            }),
        });

        let params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: true,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            vector_records: records,
            trigger_compaction: false,
            batch_ids: vec![],
            collection_config,
            estimated_size: 0,
        };

        let flush_result = self.flush_implementation(&params).await?;
        let synthetic_segment_id = flush_result
            .file_paths
            .first()
            .cloned()
            .unwrap_or_else(|| format!("sst-bulkload-{collection_id}-{count}"));

        Ok(crate::storage::traits::SegmentIngestResult {
            collection_id: collection_id.to_string(),
            record_count: count,
            synthetic_segment_id,
            used_engine_specific_path: true,
        })
    }

    /// Delegate compaction to the compaction module
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        info!("🔄 SST: Starting compaction operation");

        // Use the compaction manager if available
        if let Some(_compaction_manager) = self.compaction_manager() {
            // Trigger compaction through the manager
            // This would be implemented in the compaction module
            Ok(CompactionResult {
                success: true,
                collections_affected: params
                    .collection_id
                    .as_ref()
                    .map(|id| vec![id.clone()])
                    .unwrap_or_default(),
                entries_processed: Some(0),
                entries_removed: Some(0),
                bytes_read: Some(0),
                bytes_written: Some(0),
                input_files: Some(0),
                output_files: Some(0),
                duration_ms: Some(0),
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
            })
        } else {
            Ok(CompactionResult {
                success: false,
                collections_affected: params
                    .collection_id
                    .as_ref()
                    .map(|id| vec![id.clone()])
                    .unwrap_or_default(),
                entries_processed: Some(0),
                entries_removed: Some(0),
                bytes_read: Some(0),
                bytes_written: Some(0),
                input_files: Some(0),
                output_files: Some(0),
                duration_ms: Some(0),
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
            })
        }
    }

    /// Get vector by ID
    async fn vector_by_id(
        &self,
        collection_id: &str,
        _base_path: &str,
        vector_id: &str,
    ) -> Result<Option<proximadb_records::ProximaRecord>> {
        debug!(
            "🔍 SST: Looking up vector {} in collection {}",
            vector_id, collection_id
        );

        // Check if vector exists using bloom filters
        let exists = self.contains_vector(collection_id, vector_id).await?;

        if !exists {
            return Ok(None);
        }

        // In a real implementation, this would read from SST files
        // For now, return None as a placeholder
        Ok(None)
    }

    /// Delegate search to the search module
    async fn search_vectors_unified(
        &self,
        ctx: &StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        info!("🔍 SST: Starting unified search");

        // Use the modular search implementation
        self.search_vectors_unified(ctx).await
    }

    /// Get real collection statistics for cost-based query optimization
    async fn collection_stats(
        &self,
        collection_id: &str,
    ) -> Result<crate::storage::traits::CollectionStats> {
        let storage_url = self.get_collection_storage_url(collection_id).await?;
        let fs = self.filesystem().get_filesystem(&storage_url)?;

        let mut total_bytes: u64 = 0;
        let mut file_count: u64 = 0;

        // Scan SSTable files to estimate vector count and total bytes
        if let Ok(entries) = fs.list(&storage_url).await {
            for entry in &entries {
                if !entry.metadata.is_directory {
                    total_bytes += entry.metadata.size;
                    if entry.url.ends_with(".sst") || entry.url.ends_with(".proximablock") {
                        file_count += 1;
                    }
                }
            }
        }

        // Estimate row count from file sizes:
        // Average vector record ~= 4 bytes/dim * 128 dims + 256 bytes metadata = 768 bytes
        // With compression (~2x), avg ~384 bytes per record on disk
        let avg_record_bytes: u64 = 384;
        let estimated_row_count = if avg_record_bytes > 0 && total_bytes > 0 {
            total_bytes / avg_record_bytes
        } else {
            0
        };

        Ok(crate::storage::traits::CollectionStats {
            row_count: estimated_row_count,
            avg_vector_bytes: if file_count > 0 { 512 } else { 0 },
            engine_strategy: StorageEngineStrategy::Sst,
            has_metadata_index: true, // SST always has bloom filters
            has_hnsw_index: self.axis_manager().is_some(),
            total_bytes,
            dimension: None, // Determined at query time from collection config
            index_type: Some("bloom_filter".to_string()),
        })
    }

    /// Read ALL records of a collection from persisted SST files (Phase 8 F1).
    /// Discovers SST data files at the service-resolved `storage_url` and reads
    /// them via the compaction reader. WAL/memtable records are merged in by the
    /// service layer.
    async fn read_all_records(
        &self,
        collection_id: &str,
        storage_url: Option<&str>,
    ) -> Result<Vec<proximadb_records::ProximaRecord>> {
        let Some(storage_url) = storage_url else {
            return Ok(Vec::new());
        };
        let fs = self.filesystem().get_filesystem(storage_url)?;

        let mut files = Vec::new();
        if let Ok(entries) = fs.list(storage_url).await {
            for entry in &entries {
                if !entry.metadata.is_directory
                    && (entry.url.ends_with(".sst") || entry.url.ends_with(".proximablock"))
                {
                    files.push(entry.url.clone());
                }
            }
        }
        if files.is_empty() {
            return Ok(Vec::new());
        }

        let reader = super::sst_reader::UnifiedSSTReader::for_compaction(
            self.filesystem().clone(),
            collection_id.to_string(),
        )?;
        let mut all = Vec::new();
        for file in &files {
            all.extend(reader.read_batch(file).await?);
        }
        Ok(all)
    }

    /// Collect engine metrics
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        metrics.insert(
            "engine".to_string(),
            serde_json::Value::String("sst".to_string()),
        );

        metrics.insert(
            "version".to_string(),
            serde_json::Value::String(self.engine_version().to_string()),
        );

        // Add SST-specific metrics
        if let Some(_compaction_manager) = self.compaction_manager() {
            metrics.insert(
                "compaction_enabled".to_string(),
                serde_json::Value::Bool(true),
            );
        }

        // Get performance metrics from the universal optimizer
        // Deferred: Add performance metrics collection when available
        metrics.insert(
            "optimizer_status".to_string(),
            serde_json::Value::String("active".to_string()),
        );

        Ok(metrics)
    }

    /// Engine-level RLS predicate (spec §8 — Phase E proof-of-concept).
    ///
    /// Extracts tenant_id from the query context so the scan iterator can
    /// apply row-level isolation without referencing application-layer middleware.
    fn rls_record_filter(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
    ) -> Option<crate::storage::traits::RlsRecordPredicate> {
        // Prefer explicit tenant context (String); fall back to user context (Option<String>)
        let tenant_id = ctx
            .tenant_context
            .as_ref()
            .map(|tc| tc.tenant_id.as_str())
            .or_else(|| {
                ctx.user_context
                    .as_ref()
                    .and_then(|uc| uc.tenant_id.as_deref())
            });

        let principal = ctx.user_context.as_ref().map(|uc| uc.user_id.as_str());

        if tenant_id.is_none() && principal.is_none() {
            return None;
        }

        Some(crate::storage::traits::RlsRecordPredicate {
            required_tenant_id: tenant_id.map(str::to_string),
            required_principal: principal.map(str::to_string),
        })
    }
}

/// Additional trait implementations for SST engine optimization
#[async_trait]
impl crate::storage::engines::core::ops::UniversallyOptimized for SstEngine {
    fn universal_optimizer(
        &self,
    ) -> &crate::storage::engines::core::ops::UniversalPerformanceOptimizer {
        self.universal_optimizer()
    }

    async fn setup_engine_optimizations(&self) -> Result<()> {
        info!("🔧 SST: Setting up engine-specific optimizations");

        // Enable prefetching for SSTable files based on access patterns
        self.universal_optimizer().prefetch_data(&[]).await?;

        // Setup SSTable-specific cache eviction if needed
        self.universal_optimizer().evict_cache_if_needed().await?;

        info!("✅ SST: Engine-specific optimizations setup complete");
        Ok(())
    }

    async fn collect_performance_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        // SST-specific metrics
        metrics.insert(
            "engine_type".to_string(),
            serde_json::Value::String("SST".to_string()),
        );

        metrics.insert(
            "optimization_strategy".to_string(),
            serde_json::Value::String(format!("{:?}", self.universal_optimizer().get_strategy())),
        );

        // Universal optimizer configuration
        let config = self.universal_optimizer().get_config();
        metrics.insert(
            "cache_size_mb".to_string(),
            serde_json::Value::Number(serde_json::Number::from(config.cache_size_mb)),
        );
        metrics.insert(
            "parallel_operations".to_string(),
            serde_json::Value::Number(serde_json::Number::from(config.parallel_operations)),
        );
        metrics.insert(
            "enable_prefetching".to_string(),
            serde_json::Value::Bool(config.enable_prefetching),
        );

        Ok(metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::storage::engines::sst::SstConfig;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_engine_name() {
        let engine = create_test_engine().await;
        assert_eq!(engine.engine_name(), "sst");
    }

    #[tokio::test]
    async fn test_engine_strategy() {
        let engine = create_test_engine().await;
        assert!(matches!(engine.strategy(), StorageEngineStrategy::Sst));
    }

    #[tokio::test]
    async fn test_collect_metrics() {
        let engine = create_test_engine().await;
        let metrics = engine.collect_engine_metrics().await.unwrap();

        assert_eq!(
            metrics["engine"],
            serde_json::Value::String("sst".to_string())
        );
        assert!(metrics.contains_key("version"));
    }

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute)
            .await
            .unwrap()
    }
}
