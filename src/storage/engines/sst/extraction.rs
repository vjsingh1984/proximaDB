//! SST Vector Extractor
//!
//! Implements the VectorExtractor trait for SST engine.
//! Wraps existing UnifiedSstableReader functionality.

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

use super::readers::sst_query_engine::UnifiedSstableReader;

/// SST Vector Extractor
///
/// Extracts vectors from SST files using the UnifiedSstableReader.
/// This is the reference implementation for other engines.
pub struct SstExtractor {
    /// Unified caching filesystem for file access
    filesystem: Arc<UnifiedCachingFilesystem>,
}

impl SstExtractor {
    /// Create a new SST extractor
    pub fn new(filesystem: Arc<UnifiedCachingFilesystem>) -> Self {
        Self { filesystem }
    }
}

#[async_trait]
impl VectorExtractor for SstExtractor {
    async fn extract_vectors(
        &self,
        request: ExtractionRequest,
    ) -> Result<ExtractionResult, ExtractionError> {
        let start = Instant::now();

        if request.file_paths.is_empty() {
            return Ok(ExtractionResult::empty());
        }

        // Convert file:// URLs to local paths if needed
        let file_paths: Vec<String> = request
            .file_paths
            .iter()
            .map(|p| {
                if p.starts_with("file://") {
                    // Strip file:// prefix to get local path
                    p.strip_prefix("file://").unwrap_or(p).to_string()
                } else {
                    p.clone()
                }
            })
            .collect();

        debug!(
            "[SST Extractor] Processing {} files: {:?}",
            file_paths.len(),
            file_paths
        );

        // Filter out non-existent files to handle race conditions gracefully
        // (e.g., temp directories cleaned up before AXIS consumer processes events)
        let mut existing_files = Vec::with_capacity(file_paths.len());
        let mut missing_count = 0;
        for path in &file_paths {
            if std::path::Path::new(path).exists() {
                existing_files.push(path.clone());
            } else {
                missing_count += 1;
                debug!("[SST Extractor] File not found (skipping): {}", path);
            }
        }

        if missing_count > 0 {
            tracing::warn!(
                "[SST Extractor] {} of {} files not found (temp dir cleanup race?)",
                missing_count,
                file_paths.len()
            );
        }

        if existing_files.is_empty() {
            debug!("[SST Extractor] No existing files to process, returning empty result");
            return Ok(ExtractionResult::empty());
        }

        let file_paths = existing_files;

        // Create filesystem factory for this extraction operation
        let filesystem_factory = Arc::new(
            FilesystemFactory::create_default()
                .await
                .map_err(|e| ExtractionError::IoError(e.to_string()))?,
        );

        // Create the SST reader
        let reader = UnifiedSstableReader::new(
            filesystem_factory,
            self.filesystem.clone(),
            "extractor".to_string(),
        );

        // Use the existing read_all_records_for_compaction method
        let records = reader
            .read_all_records_for_compaction(&file_paths)
            .await
            .map_err(|e| ExtractionError::EngineError(format!("SST read error: {}", e)))?;

        debug!(
            "[SST Extractor] Read {} records from {} files",
            records.len(),
            request.file_paths.len()
        );

        // Convert VectorRecord to ExtractedVector
        let mut vectors = Vec::with_capacity(records.len());
        let mut bytes_read = 0usize;

        for record in records {
            // Handle vector filtering based on mode
            let fp32_vector = match request.mode {
                ExtractionMode::Fp32Only | ExtractionMode::Both | ExtractionMode::Auto => {
                    if !record.vector.is_empty() {
                        bytes_read += record.vector.len() * 4; // f32 = 4 bytes
                        Some(record.vector)
                    } else {
                        None
                    }
                }
                ExtractionMode::QuantizedOnly => None,
            };

            // SST doesn't store quantized vectors inline in VectorRecord
            // Quantization is handled at the storage layer
            let quantized = None;

            // Handle metadata - convert HashMap<String, SqlValue> to JSON Value
            let metadata = if !record.metadata.is_empty() {
                // Convert SqlValue map to serde_json::Value
                let json_map: serde_json::Map<String, serde_json::Value> = record
                    .metadata
                    .into_iter()
                    .filter_map(|(k, v)| {
                        // Convert SqlValue to JSON Value
                        sql_value_to_json(&v).map(|jv| (k, jv))
                    })
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
                vectors.push(ExtractedVector {
                    id: record.id,
                    fp32_vector,
                    quantized,
                    metadata,
                });
            }
        }

        // Apply limit and offset if specified
        let total_before_pagination = vectors.len();
        if let Some(offset) = request.offset {
            if offset < vectors.len() {
                vectors = vectors.into_iter().skip(offset).collect();
            } else {
                vectors.clear();
            }
        }
        if let Some(limit) = request.limit {
            vectors.truncate(limit);
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        debug!(
            "[SST Extractor] Extracted {} vectors in {}ms",
            vectors.len(),
            duration_ms
        );

        Ok(ExtractionResult {
            vectors,
            stats: ExtractionStats {
                vectors_extracted: total_before_pagination,
                bytes_read,
                files_processed: request.file_paths.len(),
                duration_ms,
                vectors_skipped: 0,
            },
            continuation_token: None,
        })
    }

    fn extraction_capabilities(&self) -> ExtractionCapabilities {
        ExtractionCapabilities {
            supports_streaming: true,
            supports_incremental: true,
            available_modes: vec![
                ExtractionMode::Fp32Only,
                ExtractionMode::QuantizedOnly,
                ExtractionMode::Both,
                ExtractionMode::Auto,
            ],
            supports_metadata: true,
            optimal_batch_size: 10_000,
        }
    }

    fn estimate_extraction_cost(&self, request: &ExtractionRequest) -> ExtractionCost {
        // Estimate based on file count and typical SST file characteristics
        let files = request.file_paths.len();
        let vectors_per_file = 10_000; // Typical SST file size
        let bytes_per_vector = 768 * 4; // Typical dimension * f32 size

        ExtractionCost {
            estimated_vectors: files * vectors_per_file,
            estimated_bytes: files * vectors_per_file * bytes_per_vector,
            estimated_duration_ms: (files * 50) as u64, // ~50ms per file
            io_cost: 1.0,                               // SST is the baseline
        }
    }

    fn engine_type(&self) -> StorageEngineType {
        StorageEngineType::SST
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
