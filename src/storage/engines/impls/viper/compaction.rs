// VIPER Compaction - Delegates to Unified Columnar Compaction
//
// This is now a thin wrapper around the unified columnar compaction module

use anyhow::Result;
use std::sync::Arc;
use tracing::info;

use crate::storage::engines::core::formats::columnar::{
    ColumnarCompactionResult, UnifiedColumnarCompaction, VersionContinuityMode,
};
use crate::storage::persistence::filesystem::FilesystemFactory;

/// VIPER compaction service - delegates to unified columnar compaction
///
/// Thin wrapper around the unified columnar compaction module that
/// provides VIPER-specific defaults and behavior.
pub struct ViperCompactionService {
    columnar_compaction: UnifiedColumnarCompaction,
}

impl ViperCompactionService {
    pub fn new(filesystem_factory: Arc<FilesystemFactory>) -> Self {
        Self {
            columnar_compaction: UnifiedColumnarCompaction::new(filesystem_factory)
                .with_version_mode(VersionContinuityMode::Strict), // VIPER uses strict mode by default
        }
    }

    /// Compact Parquet files for VIPER engine
    pub async fn compact_parquet_files(
        &self,
        collection_id: &str,
        input_files: Vec<String>,
        collection_config: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<ColumnarCompactionResult> {
        info!("🗜️ VIPER: Delegating compaction to unified columnar module");

        // Delegate to unified columnar compaction (no metadata collector for VIPER)
        self.columnar_compaction
            .compact_parquet_files(
                collection_id,
                input_files,
                collection_config,
                "VIPER", // Engine name for logging
                None,    // VIPER doesn't need metadata collector
            )
            .await
    }
}
