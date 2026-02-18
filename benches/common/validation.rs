use proximadb::core::search::results::OptimizedSearchRecord;
use proximadb::proto::proximadb_v1::SqlValue;
/// Validation utilities for benchmarks to ensure correctness
use proximadb::storage::traits::FlushResult as TraitFlushResult;
use tracing::debug;

/// Validate flush operation results
pub fn validate_flush_result(
    result: &TraitFlushResult,
    engine: &str,
    compression: &str,
    expected_vectors: usize,
) -> bool {
    let mut valid = true;

    // Check if flush was successful
    if !result.success {
        eprintln!(
            "    ❌ FAILED: Flush failed for {} with {}",
            engine, compression
        );
        return false;
    }

    // Check vectors written
    if result.entries_flushed.unwrap_or(0) == 0 {
        eprintln!(
            "    ⚠️  WARNING: No vectors written for {} with {} (expected {})",
            engine, compression, expected_vectors
        );
        valid = false;
    } else if result.entries_flushed.unwrap_or(0) != expected_vectors as u64 {
        eprintln!(
            "    ⚠️  WARNING: Written {} vectors, expected {} for {} with {}",
            result.entries_flushed.unwrap_or(0),
            expected_vectors,
            engine,
            compression
        );
        valid = false;
    }

    // Check bytes written
    if result.bytes_written.unwrap_or(0) == 0 {
        eprintln!(
            "    ⚠️  WARNING: No bytes written for {} with {}",
            engine, compression
        );
        valid = false;
    }

    // Log success message with debug tracing
    if valid {
        debug!(
            "Flush validated: {} vectors, {} bytes for {} with {}",
            result.entries_flushed.unwrap_or(0),
            result.bytes_written.unwrap_or(0),
            engine,
            compression
        );
    }

    valid
}

/// Validate search results
pub fn validate_search_results(
    results: &[OptimizedSearchRecord],
    engine: &str,
    compression: &str,
    top_k: usize,
    is_filtered: bool,
) -> bool {
    let mut valid = true;

    // Check if we got any results
    if results.is_empty() {
        eprintln!(
            "    ⚠️  WARNING: {} search returned no results for {} with {}",
            if is_filtered { "Filtered" } else { "Pure" },
            engine,
            compression
        );
        valid = false;
    }

    // Check result count doesn't exceed top_k
    if results.len() > top_k {
        eprintln!(
            "    ❌ ERROR: {} search returned {} results (expected <= {}) for {} with {}",
            if is_filtered { "Filtered" } else { "Pure" },
            results.len(),
            top_k,
            engine,
            compression
        );
        valid = false;
    }

    // For filtered search, validate filter application
    if is_filtered && !results.is_empty() {
        // This is a placeholder - actual implementation would check metadata
        // based on the filter criteria used in the benchmark
        debug!("Filter validation would check metadata here");
    }

    // Check result scores are valid (note: score represents distance, lower = more similar)
    for result in results {
        if result.score < 0.0 {
            eprintln!(
                "    ⚠️  WARNING: Invalid distance {} for vector {} in {} with {}",
                result.score, result.id, engine, compression
            );
            valid = false;
        }
        // Log detailed result info for debugging
        debug!(
            "Vector {}: distance={:.6}, similarity={:.6}",
            result.id,
            result.score,
            result.similarity.unwrap_or(0.0)
        );
    }

    if valid && !results.is_empty() {
        debug!(
            "{} search validated: {} results for {} with {}",
            if is_filtered { "Filtered" } else { "Pure" },
            results.len(),
            engine,
            compression
        );
    }

    valid
}

/// Validate metadata filter was properly applied
pub fn validate_metadata_filter(
    results: &[OptimizedSearchRecord],
    field: &str,
    expected_value: &SqlValue,
    engine: &str,
    compression: &str,
) -> bool {
    let mut valid = true;
    let mut invalid_count = 0;

    for result in results {
        // Check if the result has the expected metadata field and value
        if let Some(actual_value) = result.metadata.get(field) {
            if actual_value != expected_value {
                invalid_count += 1;
            }
        } else {
            // Field not present in result
            invalid_count += 1;
        }
    }

    if invalid_count > 0 {
        eprintln!(
            "    ⚠️  WARNING: {} results don't match filter {}={:?} for {} with {}",
            invalid_count, field, expected_value, engine, compression
        );
        valid = false;
    } else if !results.is_empty() {
        debug!(
            "Filter validated: All {} results match {}={:?} for {} with {}",
            results.len(),
            field,
            expected_value,
            engine,
            compression
        );
    }

    valid
}

/// Validate file system operations
pub async fn validate_filesystem_write(
    base_path: &str,
    engine: &str,
    compression: &str,
    min_expected_files: usize,
) -> bool {
    use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

    let fs_factory = match FilesystemFactory::create(FilesystemConfig::default()).await {
        Ok(factory) => factory,
        Err(e) => {
            eprintln!("    ❌ ERROR: Failed to create filesystem factory: {:?}", e);
            return false;
        }
    };

    let fs = match fs_factory.get_filesystem(&format!("file://{}", base_path)) {
        Ok(fs) => fs,
        Err(e) => {
            eprintln!("    ❌ ERROR: Failed to get filesystem: {:?}", e);
            return false;
        }
    };

    let entries = match fs.list(base_path).await {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!(
                "    ⚠️  WARNING: Failed to list directory {}: {:?}",
                base_path, e
            );
            return false;
        }
    };

    let file_count = entries.len();

    if file_count == 0 {
        eprintln!(
            "    ❌ ERROR: No files created in {} for {} with {}",
            base_path, engine, compression
        );
        false
    } else if file_count < min_expected_files {
        eprintln!(
            "    ⚠️  WARNING: Only {} files created (expected >= {}) in {} for {} with {}",
            file_count, min_expected_files, base_path, engine, compression
        );
        false
    } else {
        eprintln!(
            "    ✅ Filesystem validated: {} files in {} for {} with {}",
            file_count, base_path, engine, compression
        );
        true
    }
}

/// Calculate compression ratio and savings
pub struct CompressionMetrics {
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub ratio: f64,
    pub savings_percent: f64,
}

pub fn calculate_compression_metrics(
    uncompressed_size: u64,
    compressed_size: u64,
) -> CompressionMetrics {
    let ratio = if uncompressed_size > 0 {
        compressed_size as f64 / uncompressed_size as f64
    } else {
        0.0
    };

    let savings_percent = if uncompressed_size > 0 {
        (1.0 - ratio) * 100.0
    } else {
        0.0
    };

    CompressionMetrics {
        uncompressed_size,
        compressed_size,
        ratio,
        savings_percent,
    }
}

/// Summary statistics for benchmark validation
pub struct BenchmarkValidation {
    pub engine: String,
    pub compression: String,
    pub flush_valid: bool,
    pub search_valid: bool,
    pub filter_valid: bool,
    pub filesystem_valid: bool,
    pub overall_status: BenchmarkStatus,
}

#[derive(Debug, PartialEq)]
pub enum BenchmarkStatus {
    Success,
    PartialSuccess,
    Failed,
}

impl BenchmarkValidation {
    pub fn new(engine: &str, compression: &str) -> Self {
        Self {
            engine: engine.to_string(),
            compression: compression.to_string(),
            flush_valid: false,
            search_valid: false,
            filter_valid: false,
            filesystem_valid: false,
            overall_status: BenchmarkStatus::Failed,
        }
    }

    pub fn evaluate(&mut self) {
        let valid_count = [
            self.flush_valid,
            self.search_valid,
            self.filter_valid,
            self.filesystem_valid,
        ]
        .iter()
        .filter(|&&x| x)
        .count();

        self.overall_status = match valid_count {
            4 => BenchmarkStatus::Success,
            2..=3 => BenchmarkStatus::PartialSuccess,
            _ => BenchmarkStatus::Failed,
        };
    }

    pub fn print_summary(&self) {
        let status_icon = match self.overall_status {
            BenchmarkStatus::Success => "✅",
            BenchmarkStatus::PartialSuccess => "⚠️",
            BenchmarkStatus::Failed => "❌",
        };

        eprintln!(
            "\n{} Benchmark Summary for {} with {}:",
            status_icon, self.engine, self.compression
        );
        eprintln!(
            "  Flush:      {}",
            if self.flush_valid { "✅" } else { "❌" }
        );
        eprintln!(
            "  Search:     {}",
            if self.search_valid { "✅" } else { "❌" }
        );
        eprintln!(
            "  Filter:     {}",
            if self.filter_valid { "✅" } else { "❌" }
        );
        eprintln!(
            "  Filesystem: {}",
            if self.filesystem_valid { "✅" } else { "❌" }
        );
        eprintln!("  Overall:    {:?}", self.overall_status);
    }
}
