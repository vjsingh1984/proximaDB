//! TST Vector Extractor
//!
//! Implements the VectorExtractor trait for TST (Time-Series) engine.
//! Extracts vectors from time-partitioned storage.

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, warn};

use crate::index::axis::eventlog::StorageEngineType;
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
use crate::storage::trait_components::extractor::{
    ExtractedVector, ExtractionCapabilities, ExtractionCost, ExtractionError,
    ExtractionMode, ExtractionRequest, ExtractionResult, ExtractionStats,
    QuantizedVector, VectorExtractor,
};

/// TST Vector Extractor
///
/// Extracts vectors from time-partitioned storage.
/// TST stores vectors chronologically with optional OHLC aggregation.
pub struct TstExtractor {
    /// Unified caching filesystem for file access
    filesystem: Arc<UnifiedCachingFilesystem>,
}

impl TstExtractor {
    /// Create a new TST extractor
    pub fn new(filesystem: Arc<UnifiedCachingFilesystem>) -> Self {
        Self { filesystem }
    }
}

#[async_trait]
impl VectorExtractor for TstExtractor {
    async fn extract_vectors(
        &self,
        request: ExtractionRequest,
    ) -> Result<ExtractionResult, ExtractionError> {
        let start = Instant::now();

        if request.file_paths.is_empty() {
            return Ok(ExtractionResult::empty());
        }

        debug!(
            "[TST Extractor] Processing {} files: {:?}",
            request.file_paths.len(),
            request.file_paths
        );

        // Filter out non-existent files
        let existing_files: Vec<String> = request
            .file_paths
            .iter()
            .filter(|p| std::path::Path::new(p).exists())
            .cloned()
            .collect();

        if existing_files.is_empty() {
            debug!("[TST Extractor] No existing files to process, returning empty result");
            return Ok(ExtractionResult::empty());
        }

        let mut all_vectors = Vec::new();
        let mut total_bytes_read = 0;

        // Process each file
        for file_path in &existing_files {
            match self.extract_from_file(file_path, &request).await {
                Ok(mut vectors) => {
                    total_bytes_read += vectors.stats.bytes_read;
                    all_vectors.append(&mut vectors.vectors);
                }
                Err(e) => {
                    warn!("[TST Extractor] Failed to extract from {}: {}", file_path, e);
                }
            }
        }

        // Apply pagination if requested
        let offset = request.offset.unwrap_or(0);
        let limit = request.limit;
        let paginated_vectors: Vec<ExtractedVector> = if let Some(limit) = limit {
            let end = offset + limit;
            all_vectors
                .into_iter()
                .skip(offset)
                .take(end.saturating_sub(offset))
                .collect()
        } else {
            all_vectors.into_iter().skip(offset).collect()
        };

        let vectors_count = paginated_vectors.len();
        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(ExtractionResult {
            vectors: paginated_vectors,
            stats: ExtractionStats {
                vectors_extracted: vectors_count,
                bytes_read: total_bytes_read,
                files_processed: existing_files.len(),
                duration_ms,
                vectors_skipped: 0,
            },
            continuation_token: None,
        })
    }

    fn extraction_capabilities(&self) -> ExtractionCapabilities {
        ExtractionCapabilities {
            supports_streaming: false,
            supports_incremental: true,
            available_modes: vec![
                ExtractionMode::Fp32Only,
                ExtractionMode::Both,
                ExtractionMode::Auto,
            ],
            supports_metadata: true,
            optimal_batch_size: 10_000,
        }
    }

    fn estimate_extraction_cost(&self, request: &ExtractionRequest) -> ExtractionCost {
        // Estimate based on file count
        let estimated_vectors = request.file_paths.len() * 1000; // Assume 1K vectors per file
        let estimated_bytes = estimated_vectors * 512; // 512 bytes per vector estimate
        let estimated_duration_ms = (request.file_paths.len() * 100) as u64; // 100ms per file

        ExtractionCost {
            estimated_vectors,
            estimated_bytes,
            estimated_duration_ms,
            io_cost: 0.8, // TST is columnar, good I/O efficiency
        }
    }

    fn engine_type(&self) -> StorageEngineType {
        StorageEngineType::TST
    }
}

impl TstExtractor {
    /// Extract vectors from a single file
    async fn extract_from_file(
        &self,
        file_path: &str,
        request: &ExtractionRequest,
    ) -> Result<ExtractionResult, ExtractionError> {
        // TODO: Implement actual file reading
        // For now, return empty result with file size estimate
        let metadata = std::fs::metadata(file_path)
            .map_err(|e| ExtractionError::FileNotFound(format!("{}: {}", file_path, e)))?;

        Ok(ExtractionResult {
            vectors: Vec::new(),
            stats: ExtractionStats {
                vectors_extracted: 0,
                bytes_read: metadata.len() as usize,
                files_processed: 1,
                duration_ms: 0,
                vectors_skipped: 0,
            },
            continuation_token: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tst_extractor_capabilities() {
        let fs = Arc::new(UnifiedCachingFilesystem::new(
            crate::storage::persistence::filesystem::FilesystemConfig::default(),
        ));
        let extractor = TstExtractor::new(fs);

        let caps = extractor.extraction_capabilities();
        assert!(caps.supports_incremental);
        assert!(caps.supports_metadata);
        assert_eq!(caps.optimal_batch_size, 10_000);
    }

    #[test]
    fn test_tst_extractor_cost_estimate() {
        let fs = Arc::new(UnifiedCachingFilesystem::new(
            crate::storage::persistence::filesystem::FilesystemConfig::default(),
        ));
        let extractor = TstExtractor::new(fs);

        let request = ExtractionRequest::full(vec!["file1.tst".to_string(), "file2.tst".to_string()]);
        let cost = extractor.estimate_extraction_cost(&request);

        assert_eq!(cost.estimated_vectors, 2000);
        assert_eq!(cost.estimated_bytes, 1024000);
    }
}
