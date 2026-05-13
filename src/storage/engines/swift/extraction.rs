//! SWIFT Vector Extractor
//!
//! Implements the VectorExtractor trait for SWIFT engine.
//! Uses UnifiedSwiftReader with StreamAll strategy for efficient extraction.

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tracing::debug;

use crate::index::axis::eventlog::StorageEngineType;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::filesystem::unified_filesystem::UnifiedCachingFilesystem;
use crate::storage::trait_components::extractor::{
    ExtractedVector, ExtractionCapabilities, ExtractionCost, ExtractionError, ExtractionMode,
    ExtractionRequest, ExtractionResult, ExtractionStats, VectorExtractor,
};

use super::unified_reader::{SwiftReadStrategy, SwiftReaderConfig, UnifiedSwiftReader};

/// SWIFT Vector Extractor
///
/// Extracts vectors from SWIFT files using UnifiedSwiftReader with StreamAll strategy.
/// SWIFT's hierarchical superblock structure provides efficient batch extraction.
pub struct SwiftExtractor {
    /// Unified caching filesystem for file access
    filesystem: Arc<UnifiedCachingFilesystem>,
}

impl SwiftExtractor {
    /// Create a new SWIFT extractor
    pub fn new(filesystem: Arc<UnifiedCachingFilesystem>) -> Self {
        Self { filesystem }
    }
}

#[async_trait]
impl VectorExtractor for SwiftExtractor {
    async fn extract_vectors(
        &self,
        request: ExtractionRequest,
    ) -> Result<ExtractionResult, ExtractionError> {
        let start = Instant::now();

        if request.file_paths.is_empty() {
            return Ok(ExtractionResult::empty());
        }

        // Filter out non-existent files to handle race conditions gracefully
        // (e.g., temp directories cleaned up before AXIS consumer processes events)
        let mut existing_files = Vec::with_capacity(request.file_paths.len());
        let mut missing_count = 0;
        for path in &request.file_paths {
            // Strip file:// prefix if present
            let local_path = if path.starts_with("file://") {
                path.strip_prefix("file://").unwrap_or(path)
            } else {
                path.as_str()
            };
            if std::path::Path::new(local_path).exists() {
                existing_files.push(path.clone());
            } else {
                missing_count += 1;
                debug!("[SWIFT Extractor] File not found (skipping): {}", path);
            }
        }

        if missing_count > 0 {
            tracing::warn!(
                "[SWIFT Extractor] {} of {} files not found (temp dir cleanup race?)",
                missing_count,
                request.file_paths.len()
            );
        }

        if existing_files.is_empty() {
            debug!("[SWIFT Extractor] No existing files to process, returning empty result");
            return Ok(ExtractionResult::empty());
        }

        // Create filesystem factory for this extraction operation
        let filesystem_factory = Arc::new(
            FilesystemFactory::create_default()
                .await
                .map_err(|e| ExtractionError::IoError(e.to_string()))?,
        );

        let mut all_vectors = Vec::new();
        let mut total_bytes_read = 0u64;
        let mut files_processed = 0usize;

        for file_path in &existing_files {
            let config = SwiftReaderConfig {
                enable_prefetch: true,
                max_concurrent_reads: 4,
                coalesce_threshold_bytes: 64 * 1024, // 64KB
                cache_metadata: false,               // Disable for extraction
                streaming_threshold_mb: 10,          // Stream if file > 10MB
            };

            let reader = UnifiedSwiftReader::new(
                filesystem_factory.clone(),
                file_path.to_string(),
                self.filesystem.clone(),
                "extraction".to_string(),
                config,
            )
            .await
            .map_err(|e| ExtractionError::EngineError(format!("SWIFT reader error: {}", e)))?;

            // Use StreamAll strategy for full extraction
            let read_result = reader
                .read_with_strategy(SwiftReadStrategy::StreamAll)
                .await
                .map_err(|e| ExtractionError::EngineError(format!("SWIFT read error: {}", e)))?;

            total_bytes_read += read_result.bytes_read;
            files_processed += 1;

            debug!(
                "[SWIFT Extractor] Read {} records from {}",
                read_result.records.len(),
                file_path
            );

            // Convert VectorRecord to ExtractedVector
            for record in read_result.records {
                // Handle vector filtering based on mode
                let fp32_vector = match request.mode {
                    ExtractionMode::Fp32Only | ExtractionMode::Both | ExtractionMode::Auto => {
                        if !record.vector.is_empty() {
                            Some(record.vector)
                        } else {
                            None
                        }
                    }
                    ExtractionMode::QuantizedOnly => None,
                };

                // SWIFT doesn't store quantized vectors inline in VectorRecord
                // Quantization is handled at the storage layer via superblock structure
                let quantized = None;

                // Handle metadata - convert HashMap<String, SqlValue> to JSON Value
                let metadata = if !record.metadata.is_empty() {
                    let json_map: serde_json::Map<String, serde_json::Value> = record
                        .metadata
                        .into_iter()
                        .filter_map(|(k, v)| sql_value_to_json(&v).map(|jv| (k, jv)))
                        .collect();
                    if json_map.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(json_map))
                    }
                } else {
                    None
                };

                // Apply ID filter if selective extraction
                let should_include = match &request.vector_ids {
                    Some(ids) => ids.contains(&record.id),
                    None => true,
                };

                if should_include && fp32_vector.is_some() {
                    all_vectors.push(ExtractedVector {
                        id: record.id,
                        fp32_vector,
                        quantized,
                        metadata,
                    });
                }
            }
        }

        // Apply limit and offset if specified
        let total_before_pagination = all_vectors.len();
        if let Some(offset) = request.offset {
            if offset < all_vectors.len() {
                all_vectors = all_vectors.into_iter().skip(offset).collect();
            } else {
                all_vectors.clear();
            }
        }
        if let Some(limit) = request.limit {
            all_vectors.truncate(limit);
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        debug!(
            "[SWIFT Extractor] Extracted {} vectors from {} files in {}ms",
            all_vectors.len(),
            files_processed,
            duration_ms
        );

        Ok(ExtractionResult {
            vectors: all_vectors,
            stats: ExtractionStats {
                vectors_extracted: total_before_pagination,
                bytes_read: total_bytes_read as usize,
                files_processed,
                duration_ms,
                vectors_skipped: 0,
            },
            continuation_token: None,
        })
    }

    fn extraction_capabilities(&self) -> ExtractionCapabilities {
        ExtractionCapabilities {
            supports_streaming: true,
            supports_incremental: false, // SWIFT doesn't have LSN-based incremental
            available_modes: vec![
                ExtractionMode::Fp32Only,
                ExtractionMode::QuantizedOnly,
                ExtractionMode::Both,
                ExtractionMode::Auto,
            ],
            supports_metadata: true,
            optimal_batch_size: 50_000, // SWIFT handles larger batches due to superblock structure
        }
    }

    fn estimate_extraction_cost(&self, request: &ExtractionRequest) -> ExtractionCost {
        // SWIFT is optimized for batch reads due to superblock structure
        let files = request.file_paths.len();
        let vectors_per_file = 50_000; // Larger due to superblock structure
        let bytes_per_vector = 768 * 4; // Typical dimension * f32 size

        ExtractionCost {
            estimated_vectors: files * vectors_per_file,
            estimated_bytes: files * vectors_per_file * bytes_per_vector,
            estimated_duration_ms: (files * 100) as u64, // ~100ms per file (larger files)
            io_cost: 0.8, // SWIFT is more efficient than SST for batch reads
        }
    }

    fn engine_type(&self) -> StorageEngineType {
        StorageEngineType::SWIFT
    }
}

/// Helper function to convert SqlValue to JSON Value
fn sql_value_to_json(value: &crate::proto::proximadb_v1::SqlValue) -> Option<serde_json::Value> {
    use crate::proto::proximadb_v1::sql_value::Value;

    value.value.as_ref().map(|v| match v {
        Value::NullValue(_) => serde_json::Value::Null,
        Value::BoolValue(b) => serde_json::Value::Bool(*b),
        Value::Int64Value(i) => serde_json::Value::Number((*i).into()),
        Value::NumberValue(f) => serde_json::Number::from_f64(*f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::StringValue(s) => serde_json::Value::String(s.clone()),
        Value::BytesValue(b) => {
            // Encode bytes as hex string (simpler, no external dependency)
            let hex: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
            serde_json::Value::String(hex)
        }
        Value::ArrayValue(_) => serde_json::Value::String("[array]".to_string()),
        Value::ObjectValue(_) => serde_json::Value::String("[object]".to_string()),
    })
}

// Unit tests are minimal since actual extraction is tested in integration tests.
// The extractor trait tests in extractor.rs cover the core functionality.
