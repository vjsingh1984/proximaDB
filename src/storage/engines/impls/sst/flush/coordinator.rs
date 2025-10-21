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

//! Flush Coordinator
//!
//! Coordinates flush operations across multiple collections and manages
//! the overall flush workflow for the SST engine.

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use crate::storage::engines::impls::sst::SstEngine;
use crate::storage::traits::{FlushParameters, FlushResult};

/// Flush coordinator for managing complex flush operations
pub struct FlushCoordinator {
    engine: Arc<SstEngine>,
}

impl FlushCoordinator {
    /// Create a new flush coordinator
    pub fn new(engine: Arc<SstEngine>) -> Self {
        Self { engine }
    }

    /// Coordinate a flush operation
    pub async fn coordinate_flush(&self, params: FlushParameters) -> Result<FlushResult> {
        debug!("🔄 FlushCoordinator: Starting coordinated flush");

        // Pre-flush validation
        self.validate_flush_parameters(&params)?;

        // Execute the flush
        let result = self.engine.flush_implementation(&params).await?;

        // Post-flush operations
        self.post_flush_operations(&result).await?;

        info!("✅ FlushCoordinator: Flush coordination completed successfully");
        Ok(result)
    }

    /// Validate flush parameters
    fn validate_flush_parameters(&self, params: &FlushParameters) -> Result<()> {
        if params.vector_records.is_empty() {
            return Err(anyhow::anyhow!("No vectors provided for flush"));
        }

        if params.collection_id.is_none() {
            return Err(anyhow::anyhow!("Collection ID is required"));
        }

        if params.collection_config.is_none() {
            return Err(anyhow::anyhow!("Collection configuration is required"));
        }

        debug!("✅ FlushCoordinator: Parameters validation passed");
        Ok(())
    }

    /// Post-flush operations
    async fn post_flush_operations(&self, result: &FlushResult) -> Result<()> {
        debug!("🔧 FlushCoordinator: Executing post-flush operations");

        // Log flush statistics
        if let (Some(entries), Some(bytes)) = (result.entries_flushed, result.bytes_written) {
            info!("📊 Flush Stats: {} entries, {} bytes", entries, bytes);
        }

        // Trigger compaction if needed
        if result.compaction_triggered {
            info!("🔄 Triggering background compaction");
            // Compaction would be triggered here
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::proto::proximadb_v1::VectorRecord;
    use crate::storage::engines::impls::sst::SstConfig;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_flush_coordinator_validation() {
        let engine = create_test_engine().await;
        let coordinator = FlushCoordinator::new(Arc::new(engine));

        // Test with empty vectors - should fail
        let params = FlushParameters {
            vector_records: vec![],
            batch_ids: vec![],
            collection_id: Some("test".to_string()),
            collection_config: None,
            force: false,
            synchronous: true,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            estimated_size: 0,
        };

        assert!(coordinator.validate_flush_parameters(&params).is_err());
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
