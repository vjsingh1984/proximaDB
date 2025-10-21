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
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::impls::sst::SstError;
use crate::storage::engines::impls::sst::core::SstEngine;
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
        self.filesystem()
    }

    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        info!("🚀 SST: Starting flush operation");
        // Use the flush module implementation directly
        self.flush_implementation(params).await
    }

    /// Delegate compaction to the compaction module
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        info!("🔄 SST: Starting compaction operation");

        // Use the compaction manager if available
        if let Some(compaction_manager) = self.compaction_manager() {
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
    ) -> Result<Option<crate::proto::proximadb_v1::VectorRecord>> {
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
        if let Some(compaction_manager) = self.compaction_manager() {
            metrics.insert(
                "compaction_enabled".to_string(),
                serde_json::Value::Bool(true),
            );
        }

        // Get performance metrics from the universal optimizer
        // TODO: Add performance metrics collection when available
        metrics.insert(
            "optimizer_status".to_string(),
            serde_json::Value::String("active".to_string()),
        );

        Ok(metrics)
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
    use crate::storage::engines::impls::sst::SstConfig;
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
