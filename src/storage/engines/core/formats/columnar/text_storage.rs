//! TEXT Column Storage Strategies
//!
//! Provides storage strategies for TEXT columns in columnar storage:
//! - **INLINE** (<4KB): Store directly in main Parquet column
//! - **CHUNKED** (4KB-1MB): Split into chunks with per-chunk embeddings for RAG
//! - **SIDECAR** (>1MB): Store in separate files with references
//! - **ADAPTIVE**: Auto-select based on actual content size
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   TEXT Storage Engine                        │
//! ├─────────────────────────────────────────────────────────────┤
//! │  ┌─────────────┐  ┌──────────────┐  ┌───────────────────┐  │
//! │  │   INLINE    │  │   CHUNKED    │  │     SIDECAR       │  │
//! │  │   <4KB      │  │  4KB-1MB     │  │      >1MB         │  │
//! │  │  StringArr  │  │  + Embed     │  │  File + Ref       │  │
//! │  └─────────────┘  └──────────────┘  └───────────────────┘  │
//! ├─────────────────────────────────────────────────────────────┤
//! │  TextColumnWriter │ TextColumnReader │ SidecarManager      │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Use Cases
//!
//! - **INLINE**: Short descriptions, titles, tags
//! - **CHUNKED**: Documents for RAG (embeddings per chunk)
//! - **SIDECAR**: Large documents, logs, raw content

use arrow::array::{Array, ArrayRef, LargeStringArray, StringArray};
use arrow::datatypes::{DataType, Field};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

use crate::core::types::TextStorageStrategy;

/// Thresholds for storage strategy selection
pub const INLINE_THRESHOLD: usize = 4 * 1024; // 4KB
pub const CHUNKED_THRESHOLD: usize = 1024 * 1024; // 1MB
pub const DEFAULT_CHUNK_SIZE: usize = 512; // 512 characters per chunk (for RAG)

/// Errors that can occur during text storage operations
#[derive(Error, Debug)]
pub enum TextStorageError {
    /// Text exceeds maximum allowed size
    #[error("Text exceeds maximum size: {0} bytes (max: {1} bytes)")]
    TextTooLarge(usize, usize),

    /// Sidecar file not found during read
    #[error("Sidecar file not found: {0}")]
    SidecarNotFound(String),

    /// Invalid chunk reference
    #[error("Invalid chunk reference: {0}")]
    InvalidChunkReference(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// IO error during sidecar operations
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Arrow error
    #[error("Arrow error: {0}")]
    ArrowError(String),
}

impl From<arrow::error::ArrowError> for TextStorageError {
    fn from(err: arrow::error::ArrowError) -> Self {
        TextStorageError::ArrowError(err.to_string())
    }
}

/// Configuration for TEXT column storage
#[derive(Debug, Clone)]
pub struct TextStorageConfig {
    /// Storage strategy to use
    pub strategy: TextStorageStrategy,

    /// Maximum size for inline storage (bytes)
    pub inline_threshold: usize,

    /// Maximum size for chunked storage before sidecar (bytes)
    pub chunked_threshold: usize,

    /// Chunk size for chunked storage (characters)
    pub chunk_size: usize,

    /// Enable n-gram bloom filter for CONTAINS queries
    pub enable_ngram_bloom: bool,

    /// N-gram size for bloom filter (default: 3)
    pub ngram_size: usize,

    /// Maximum allowed text size (0 = unlimited)
    pub max_text_size: usize,

    /// Base path for sidecar files
    pub sidecar_base_path: Option<String>,

    /// Compression for sidecar files
    pub sidecar_compression: SidecarCompression,
}

/// Compression options for sidecar files
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarCompression {
    /// No compression
    None,
    /// LZ4 compression (fast)
    Lz4,
    /// Zstd compression (better ratio)
    Zstd,
}

impl Default for SidecarCompression {
    fn default() -> Self {
        Self::Lz4
    }
}

impl Default for TextStorageConfig {
    fn default() -> Self {
        Self {
            strategy: TextStorageStrategy::Adaptive,
            inline_threshold: INLINE_THRESHOLD,
            chunked_threshold: CHUNKED_THRESHOLD,
            chunk_size: DEFAULT_CHUNK_SIZE,
            enable_ngram_bloom: false,
            ngram_size: 3,
            max_text_size: 0, // Unlimited
            sidecar_base_path: None,
            sidecar_compression: SidecarCompression::default(),
        }
    }
}

impl TextStorageConfig {
    /// Create config for small text fields (inline only)
    pub fn for_small_text() -> Self {
        Self {
            strategy: TextStorageStrategy::Inline,
            inline_threshold: INLINE_THRESHOLD,
            ..Default::default()
        }
    }

    /// Create config for RAG documents (chunked with embeddings)
    pub fn for_rag_documents(chunk_size: usize) -> Self {
        Self {
            strategy: TextStorageStrategy::Chunked,
            chunk_size,
            enable_ngram_bloom: true,
            ..Default::default()
        }
    }

    /// Create config for large documents (sidecar storage)
    pub fn for_large_documents(sidecar_path: String) -> Self {
        Self {
            strategy: TextStorageStrategy::Sidecar,
            sidecar_base_path: Some(sidecar_path),
            sidecar_compression: SidecarCompression::Zstd,
            ..Default::default()
        }
    }
}

/// Determine optimal storage strategy based on content
///
/// # Arguments
/// * `content` - The text content to analyze
/// * `config` - Storage configuration
///
/// # Returns
/// The recommended storage strategy
pub fn determine_storage_strategy(
    content: &str,
    config: &TextStorageConfig,
) -> TextStorageStrategy {
    match config.strategy {
        TextStorageStrategy::Adaptive => {
            let size = content.len();
            if size <= config.inline_threshold {
                TextStorageStrategy::Inline
            } else if size <= config.chunked_threshold {
                TextStorageStrategy::Chunked
            } else {
                TextStorageStrategy::Sidecar
            }
        }
        strategy => strategy, // Use explicit strategy
    }
}

/// Reference to sidecar-stored text
#[derive(Debug, Clone)]
pub struct SidecarRef {
    /// Record ID this sidecar belongs to
    pub record_id: String,

    /// Path to sidecar file
    pub sidecar_path: String,

    /// Offset within sidecar file
    pub offset: u64,

    /// Length of text in bytes
    pub length: u64,

    /// Compression used
    pub compression: SidecarCompression,

    /// Optional checksum for verification
    pub checksum: Option<u64>,
}

impl SidecarRef {
    /// Create a new sidecar reference
    pub fn new(record_id: String, sidecar_path: String, offset: u64, length: u64) -> Self {
        Self {
            record_id,
            sidecar_path,
            offset,
            length,
            compression: SidecarCompression::default(),
            checksum: None,
        }
    }

    /// Set compression type
    pub fn with_compression(mut self, compression: SidecarCompression) -> Self {
        self.compression = compression;
        self
    }

    /// Set checksum
    pub fn with_checksum(mut self, checksum: u64) -> Self {
        self.checksum = Some(checksum);
        self
    }

    /// Serialize to bytes for storage
    pub fn to_bytes(&self) -> Vec<u8> {
        // Simple binary format: path_len(4) + path + offset(8) + length(8) + compression(1)
        let path_bytes = self.sidecar_path.as_bytes();
        let mut bytes = Vec::with_capacity(4 + path_bytes.len() + 17);

        bytes.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        bytes.extend_from_slice(path_bytes);
        bytes.extend_from_slice(&self.offset.to_le_bytes());
        bytes.extend_from_slice(&self.length.to_le_bytes());
        bytes.push(match self.compression {
            SidecarCompression::None => 0,
            SidecarCompression::Lz4 => 1,
            SidecarCompression::Zstd => 2,
        });

        bytes
    }

    /// Deserialize from bytes
    pub fn from_bytes(record_id: String, data: &[u8]) -> Result<Self, TextStorageError> {
        if data.len() < 21 {
            return Err(TextStorageError::InvalidChunkReference(
                "Data too short for sidecar reference".to_string(),
            ));
        }

        let path_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if data.len() < 4 + path_len + 17 {
            return Err(TextStorageError::InvalidChunkReference(
                "Invalid sidecar reference format".to_string(),
            ));
        }

        let sidecar_path = String::from_utf8(data[4..4 + path_len].to_vec()).map_err(|e| {
            TextStorageError::InvalidChunkReference(format!("Invalid path encoding: {}", e))
        })?;

        let offset_start = 4 + path_len;
        let offset = u64::from_le_bytes([
            data[offset_start],
            data[offset_start + 1],
            data[offset_start + 2],
            data[offset_start + 3],
            data[offset_start + 4],
            data[offset_start + 5],
            data[offset_start + 6],
            data[offset_start + 7],
        ]);

        let length_start = offset_start + 8;
        let length = u64::from_le_bytes([
            data[length_start],
            data[length_start + 1],
            data[length_start + 2],
            data[length_start + 3],
            data[length_start + 4],
            data[length_start + 5],
            data[length_start + 6],
            data[length_start + 7],
        ]);

        let compression_byte = data[length_start + 8];
        let compression = match compression_byte {
            0 => SidecarCompression::None,
            1 => SidecarCompression::Lz4,
            2 => SidecarCompression::Zstd,
            _ => SidecarCompression::None,
        };

        Ok(Self {
            record_id,
            sidecar_path,
            offset,
            length,
            compression,
            checksum: None,
        })
    }
}

/// Text chunk for CHUNKED strategy (for RAG with per-chunk embeddings)
#[derive(Debug, Clone)]
pub struct TextChunk {
    /// Unique chunk identifier
    pub chunk_id: String,

    /// Parent record ID
    pub parent_id: String,

    /// Index of this chunk within the parent
    pub chunk_index: u32,

    /// Chunk content
    pub content: String,

    /// Optional embedding for this chunk (for RAG)
    pub embedding: Option<Vec<f32>>,

    /// Start offset in original text
    pub start_offset: usize,

    /// End offset in original text
    pub end_offset: usize,

    /// Optional metadata
    pub metadata: HashMap<String, String>,
}

impl TextChunk {
    /// Create a new text chunk
    pub fn new(chunk_id: String, parent_id: String, chunk_index: u32, content: String) -> Self {
        let end_offset = content.len();
        Self {
            chunk_id,
            parent_id,
            chunk_index,
            content,
            embedding: None,
            start_offset: 0,
            end_offset,
            metadata: HashMap::new(),
        }
    }

    /// Set the embedding for this chunk
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set offsets
    pub fn with_offsets(mut self, start: usize, end: usize) -> Self {
        self.start_offset = start;
        self.end_offset = end;
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }

    /// Get content length
    pub fn len(&self) -> usize {
        self.content.len()
    }

    /// Check if chunk is empty
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

/// TEXT column writer that handles different storage strategies
///
/// Collects text values and routes them to appropriate storage:
/// - Inline: Stored directly in Arrow StringArray
/// - Chunked: Split into chunks with optional embeddings
/// - Sidecar: Stored in external files with references
pub struct TextColumnWriter {
    /// Storage configuration
    config: TextStorageConfig,

    /// Buffer for inline text values
    inline_buffer: Vec<Option<String>>,

    /// References to sidecar-stored texts
    sidecar_refs: Vec<SidecarRef>,

    /// Chunks for chunked storage
    chunk_buffer: Vec<TextChunk>,

    /// Mapping from record ID to storage type
    storage_mapping: HashMap<String, StorageType>,

    /// Current sidecar file offset
    current_sidecar_offset: u64,

    /// Statistics
    stats: TextStorageStats,
}

/// Storage type for a record
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageType {
    Inline,
    Chunked,
    Sidecar,
}

/// Statistics about text storage
#[derive(Debug, Clone, Default)]
pub struct TextStorageStats {
    /// Total records written
    pub total_records: u64,
    /// Records stored inline
    pub inline_count: u64,
    /// Records stored chunked
    pub chunked_count: u64,
    /// Records stored in sidecar
    pub sidecar_count: u64,
    /// Total bytes of inline content
    pub inline_bytes: u64,
    /// Total number of chunks created
    pub total_chunks: u64,
    /// Total bytes in sidecar files
    pub sidecar_bytes: u64,
}

impl TextColumnWriter {
    /// Create a new text column writer
    pub fn new(config: TextStorageConfig) -> Self {
        Self {
            config,
            inline_buffer: Vec::new(),
            sidecar_refs: Vec::new(),
            chunk_buffer: Vec::new(),
            storage_mapping: HashMap::new(),
            current_sidecar_offset: 0,
            stats: TextStorageStats::default(),
        }
    }

    /// Write text value using appropriate strategy
    ///
    /// # Arguments
    /// * `record_id` - Unique identifier for the record
    /// * `content` - Text content to store
    ///
    /// # Returns
    /// Ok(()) on success, error if content violates constraints
    pub fn write(&mut self, record_id: &str, content: &str) -> Result<(), TextStorageError> {
        // Check max size constraint
        if self.config.max_text_size > 0 && content.len() > self.config.max_text_size {
            return Err(TextStorageError::TextTooLarge(
                content.len(),
                self.config.max_text_size,
            ));
        }

        let strategy = determine_storage_strategy(content, &self.config);
        self.stats.total_records += 1;

        match strategy {
            TextStorageStrategy::Inline | TextStorageStrategy::Adaptive => {
                self.write_inline(record_id, content);
            }
            TextStorageStrategy::Chunked => {
                self.write_chunked(record_id, content)?;
            }
            TextStorageStrategy::Sidecar => {
                self.write_sidecar(record_id, content)?;
            }
        }

        Ok(())
    }

    /// Write a null value
    pub fn write_null(&mut self, record_id: &str) {
        self.inline_buffer.push(None);
        self.storage_mapping
            .insert(record_id.to_string(), StorageType::Inline);
        self.stats.total_records += 1;
    }

    /// Write content inline
    fn write_inline(&mut self, record_id: &str, content: &str) {
        self.inline_buffer.push(Some(content.to_string()));
        self.storage_mapping
            .insert(record_id.to_string(), StorageType::Inline);
        self.stats.inline_count += 1;
        self.stats.inline_bytes += content.len() as u64;
    }

    /// Write content chunked
    fn write_chunked(&mut self, record_id: &str, content: &str) -> Result<(), TextStorageError> {
        let chunks = self.split_into_chunks(record_id, content);
        let chunk_count = chunks.len();

        self.chunk_buffer.extend(chunks);
        self.storage_mapping
            .insert(record_id.to_string(), StorageType::Chunked);
        self.stats.chunked_count += 1;
        self.stats.total_chunks += chunk_count as u64;

        // Also store a reference in inline buffer for the main record
        self.inline_buffer
            .push(Some(format!("__chunked__:{}", chunk_count)));

        Ok(())
    }

    /// Write content to sidecar
    fn write_sidecar(&mut self, record_id: &str, content: &str) -> Result<(), TextStorageError> {
        let sidecar_path = self.get_sidecar_path(record_id)?;
        let length = content.len() as u64;

        let sidecar_ref = SidecarRef::new(
            record_id.to_string(),
            sidecar_path,
            self.current_sidecar_offset,
            length,
        )
        .with_compression(self.config.sidecar_compression);

        self.sidecar_refs.push(sidecar_ref);
        self.current_sidecar_offset += length;

        self.storage_mapping
            .insert(record_id.to_string(), StorageType::Sidecar);
        self.stats.sidecar_count += 1;
        self.stats.sidecar_bytes += length;

        // Store reference in inline buffer
        self.inline_buffer
            .push(Some(format!("__sidecar__:{}", record_id)));

        Ok(())
    }

    /// Split text into chunks
    fn split_into_chunks(&self, record_id: &str, content: &str) -> Vec<TextChunk> {
        let mut chunks = Vec::new();
        let chars: Vec<char> = content.chars().collect();
        let chunk_size = self.config.chunk_size;

        let mut start = 0;
        let mut chunk_index = 0u32;

        while start < chars.len() {
            let end = (start + chunk_size).min(chars.len());
            let chunk_content: String = chars[start..end].iter().collect();

            let chunk_id = format!("{}_{}", record_id, chunk_index);
            let chunk = TextChunk::new(chunk_id, record_id.to_string(), chunk_index, chunk_content)
                .with_offsets(start, end);

            chunks.push(chunk);
            start = end;
            chunk_index += 1;
        }

        chunks
    }

    /// Get sidecar file path for a record
    fn get_sidecar_path(&self, record_id: &str) -> Result<String, TextStorageError> {
        match &self.config.sidecar_base_path {
            Some(base) => Ok(format!("{}/{}.sidecar", base, record_id)),
            None => Err(TextStorageError::ConfigError(
                "Sidecar base path not configured".to_string(),
            )),
        }
    }

    /// Build Arrow array for inline text
    ///
    /// Returns a StringArray (Utf8) for normal text or LargeStringArray (LargeUtf8)
    /// for very large inline content.
    pub fn build_inline_array(&self) -> ArrayRef {
        // Check if we need LargeString (>2GB total)
        let total_bytes: usize = self
            .inline_buffer
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|s| s.len())
            .sum();

        if total_bytes > i32::MAX as usize {
            // Use LargeStringArray for very large data
            let array: LargeStringArray = self.inline_buffer.iter().collect();
            Arc::new(array)
        } else {
            // Use StringArray for normal data
            let array: StringArray = self.inline_buffer.iter().collect();
            Arc::new(array)
        }
    }

    /// Get chunks for separate storage
    pub fn get_chunks(&self) -> &[TextChunk] {
        &self.chunk_buffer
    }

    /// Get mutable chunks for adding embeddings
    pub fn get_chunks_mut(&mut self) -> &mut [TextChunk] {
        &mut self.chunk_buffer
    }

    /// Get sidecar references
    pub fn get_sidecar_refs(&self) -> &[SidecarRef] {
        &self.sidecar_refs
    }

    /// Get storage statistics
    pub fn stats(&self) -> &TextStorageStats {
        &self.stats
    }

    /// Get storage type for a record
    pub fn get_storage_type(&self, record_id: &str) -> Option<StorageType> {
        self.storage_mapping.get(record_id).copied()
    }

    /// Get Arrow field for this column
    pub fn arrow_field(column_name: &str, use_large_string: bool) -> Field {
        if use_large_string {
            Field::new(column_name, DataType::LargeUtf8, true)
        } else {
            Field::new(column_name, DataType::Utf8, true)
        }
    }

    /// Clear all buffers
    pub fn clear(&mut self) {
        self.inline_buffer.clear();
        self.sidecar_refs.clear();
        self.chunk_buffer.clear();
        self.storage_mapping.clear();
        self.current_sidecar_offset = 0;
    }

    /// Get number of records written
    pub fn len(&self) -> usize {
        self.inline_buffer.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.inline_buffer.is_empty()
    }
}

/// TEXT column reader with lazy loading support
///
/// Reads text from columnar storage, handling different storage strategies
/// and optionally lazy-loading sidecar content.
pub struct TextColumnReader {
    /// Storage configuration
    config: TextStorageConfig,

    /// Cached sidecar content
    sidecar_cache: HashMap<String, String>,

    /// Maximum cache size in bytes
    max_cache_bytes: usize,

    /// Current cache size
    current_cache_bytes: usize,
}

impl TextColumnReader {
    /// Create a new text column reader
    pub fn new(config: TextStorageConfig) -> Self {
        Self {
            config,
            sidecar_cache: HashMap::new(),
            max_cache_bytes: 100 * 1024 * 1024, // 100MB default
            current_cache_bytes: 0,
        }
    }

    /// Set maximum cache size
    pub fn with_max_cache(mut self, max_bytes: usize) -> Self {
        self.max_cache_bytes = max_bytes;
        self
    }

    /// Load text values from Arrow array
    ///
    /// # Arguments
    /// * `array` - Arrow array containing text values
    ///
    /// # Returns
    /// Vector of optional strings
    pub fn load_from_array(
        &self,
        array: &ArrayRef,
    ) -> Result<Vec<Option<String>>, TextStorageError> {
        // Handle both StringArray and LargeStringArray
        if let Some(string_array) = array.as_any().downcast_ref::<StringArray>() {
            Ok((0..string_array.len())
                .map(|i| {
                    if string_array.is_null(i) {
                        None
                    } else {
                        Some(string_array.value(i).to_string())
                    }
                })
                .collect())
        } else if let Some(large_array) = array.as_any().downcast_ref::<LargeStringArray>() {
            Ok((0..large_array.len())
                .map(|i| {
                    if large_array.is_null(i) {
                        None
                    } else {
                        Some(large_array.value(i).to_string())
                    }
                })
                .collect())
        } else {
            Err(TextStorageError::ArrowError(
                "Expected StringArray or LargeStringArray".to_string(),
            ))
        }
    }

    /// Load text values, optionally lazy-loading sidecars
    ///
    /// # Arguments
    /// * `inline_values` - Values from the inline column
    /// * `sidecar_refs` - Sidecar references for large texts
    /// * `include_sidecar` - Whether to load sidecar content
    ///
    /// # Returns
    /// Resolved text values with sidecars loaded if requested
    pub async fn load(
        &mut self,
        inline_values: &[Option<String>],
        sidecar_refs: &[SidecarRef],
        include_sidecar: bool,
    ) -> Result<Vec<Option<String>>, TextStorageError> {
        let mut result = inline_values.to_vec();

        if include_sidecar {
            // Build lookup for sidecar refs by record_id
            let sidecar_map: HashMap<&str, &SidecarRef> = sidecar_refs
                .iter()
                .map(|r| (r.record_id.as_str(), r))
                .collect();

            // Replace sidecar references with actual content
            for value in result.iter_mut() {
                if let Some(val) = value.as_ref() {
                    if val.starts_with("__sidecar__:") {
                        let record_id = &val[12..];
                        if let Some(sidecar_ref) = sidecar_map.get(record_id) {
                            let content = self.load_sidecar(sidecar_ref).await?;
                            *value = Some(content);
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Load sidecar content
    async fn load_sidecar(&mut self, sidecar_ref: &SidecarRef) -> Result<String, TextStorageError> {
        // Check cache first
        if let Some(cached) = self.sidecar_cache.get(&sidecar_ref.record_id) {
            return Ok(cached.clone());
        }

        // Load from file
        let content = self.read_sidecar_file(sidecar_ref).await?;

        // Cache if within limits
        if self.current_cache_bytes + content.len() <= self.max_cache_bytes {
            self.current_cache_bytes += content.len();
            self.sidecar_cache
                .insert(sidecar_ref.record_id.clone(), content.clone());
        }

        Ok(content)
    }

    /// Read sidecar file content
    async fn read_sidecar_file(
        &self,
        sidecar_ref: &SidecarRef,
    ) -> Result<String, TextStorageError> {
        use tokio::fs::File;
        use tokio::io::{AsyncReadExt, AsyncSeekExt};

        let mut file = File::open(&sidecar_ref.sidecar_path)
            .await
            .map_err(|_| TextStorageError::SidecarNotFound(sidecar_ref.sidecar_path.clone()))?;

        file.seek(std::io::SeekFrom::Start(sidecar_ref.offset))
            .await?;

        let mut buffer = vec![0u8; sidecar_ref.length as usize];
        file.read_exact(&mut buffer).await?;

        // Decompress if needed
        let content = match sidecar_ref.compression {
            SidecarCompression::None => buffer,
            SidecarCompression::Lz4 => {
                // LZ4 decompression would go here
                // For now, just return raw data
                buffer
            }
            SidecarCompression::Zstd => {
                // Zstd decompression would go here
                // For now, just return raw data
                buffer
            }
        };

        String::from_utf8(content)
            .map_err(|e| TextStorageError::SerializationError(format!("Invalid UTF-8: {}", e)))
    }

    /// Clear the sidecar cache
    pub fn clear_cache(&mut self) {
        self.sidecar_cache.clear();
        self.current_cache_bytes = 0;
    }

    /// Get current cache size in bytes
    pub fn cache_size(&self) -> usize {
        self.current_cache_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TextStorageConfig::default();
        assert_eq!(config.inline_threshold, INLINE_THRESHOLD);
        assert_eq!(config.chunked_threshold, CHUNKED_THRESHOLD);
        assert_eq!(config.strategy, TextStorageStrategy::Adaptive);
    }

    #[test]
    fn test_determine_strategy() {
        let config = TextStorageConfig::default();

        // Short text -> Inline
        let short_text = "Hello, world!";
        assert_eq!(
            determine_storage_strategy(short_text, &config),
            TextStorageStrategy::Inline
        );

        // Medium text -> Chunked
        let medium_text = "x".repeat(5000);
        assert_eq!(
            determine_storage_strategy(&medium_text, &config),
            TextStorageStrategy::Chunked
        );

        // Large text -> Sidecar
        let large_text = "x".repeat(2_000_000);
        assert_eq!(
            determine_storage_strategy(&large_text, &config),
            TextStorageStrategy::Sidecar
        );
    }

    #[test]
    fn test_text_chunk() {
        let chunk = TextChunk::new(
            "chunk_0".to_string(),
            "record_1".to_string(),
            0,
            "Hello".to_string(),
        )
        .with_offsets(0, 5)
        .with_embedding(vec![0.1, 0.2, 0.3]);

        assert_eq!(chunk.chunk_id, "chunk_0");
        assert_eq!(chunk.parent_id, "record_1");
        assert_eq!(chunk.chunk_index, 0);
        assert_eq!(chunk.content, "Hello");
        assert!(chunk.embedding.is_some());
        assert_eq!(chunk.start_offset, 0);
        assert_eq!(chunk.end_offset, 5);
    }

    #[test]
    fn test_sidecar_ref_serialization() {
        let sidecar_ref = SidecarRef::new(
            "record_1".to_string(),
            "/path/to/sidecar".to_string(),
            100,
            500,
        )
        .with_compression(SidecarCompression::Zstd);

        let bytes = sidecar_ref.to_bytes();
        let restored = SidecarRef::from_bytes("record_1".to_string(), &bytes).unwrap();

        assert_eq!(restored.sidecar_path, "/path/to/sidecar");
        assert_eq!(restored.offset, 100);
        assert_eq!(restored.length, 500);
        assert_eq!(restored.compression, SidecarCompression::Zstd);
    }

    #[test]
    fn test_writer_inline() {
        let config = TextStorageConfig::for_small_text();
        let mut writer = TextColumnWriter::new(config);

        writer.write("rec_1", "Hello").unwrap();
        writer.write("rec_2", "World").unwrap();
        writer.write_null("rec_3");

        assert_eq!(writer.len(), 3);
        assert_eq!(writer.stats().inline_count, 2);
        assert_eq!(writer.stats().total_records, 3);
    }

    #[test]
    fn test_writer_chunking() {
        let mut config = TextStorageConfig::default();
        config.strategy = TextStorageStrategy::Chunked;
        config.chunk_size = 10; // Small chunks for testing

        let mut writer = TextColumnWriter::new(config);

        writer
            .write("rec_1", "This is a longer text that will be chunked")
            .unwrap();

        assert!(!writer.get_chunks().is_empty());
        assert!(writer.get_chunks().len() > 1); // Should have multiple chunks
    }

    #[test]
    fn test_writer_max_size() {
        let mut config = TextStorageConfig::default();
        config.max_text_size = 100;

        let mut writer = TextColumnWriter::new(config);

        let result = writer.write("rec_1", &"x".repeat(200));
        assert!(result.is_err());

        if let Err(TextStorageError::TextTooLarge(size, max)) = result {
            assert_eq!(size, 200);
            assert_eq!(max, 100);
        }
    }

    #[test]
    fn test_storage_type_tracking() {
        let config = TextStorageConfig::for_small_text();
        let mut writer = TextColumnWriter::new(config);

        writer.write("rec_1", "Hello").unwrap();

        assert_eq!(writer.get_storage_type("rec_1"), Some(StorageType::Inline));
        assert_eq!(writer.get_storage_type("unknown"), None);
    }

    #[test]
    fn test_build_arrow_array() {
        let config = TextStorageConfig::default();
        let mut writer = TextColumnWriter::new(config);

        writer.write("rec_1", "Hello").unwrap();
        writer.write("rec_2", "World").unwrap();
        writer.write_null("rec_3");

        let array = writer.build_inline_array();
        assert_eq!(array.len(), 3);
    }

    #[test]
    fn test_reader_load_from_array() {
        let config = TextStorageConfig::default();
        let reader = TextColumnReader::new(config);

        let string_array: StringArray = vec![Some("Hello"), Some("World"), None].into();
        let array_ref: ArrayRef = Arc::new(string_array);

        let values = reader.load_from_array(&array_ref).unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values[0], Some("Hello".to_string()));
        assert_eq!(values[1], Some("World".to_string()));
        assert_eq!(values[2], None);
    }

    #[test]
    fn test_config_presets() {
        let rag_config = TextStorageConfig::for_rag_documents(256);
        assert_eq!(rag_config.strategy, TextStorageStrategy::Chunked);
        assert_eq!(rag_config.chunk_size, 256);
        assert!(rag_config.enable_ngram_bloom);

        let large_config = TextStorageConfig::for_large_documents("/sidecars".to_string());
        assert_eq!(large_config.strategy, TextStorageStrategy::Sidecar);
        assert_eq!(
            large_config.sidecar_base_path,
            Some("/sidecars".to_string())
        );
        assert_eq!(large_config.sidecar_compression, SidecarCompression::Zstd);
    }
}
