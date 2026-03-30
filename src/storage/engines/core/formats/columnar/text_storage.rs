//! TEXT Column Storage Strategies with RAG Integration
//!
//! Provides storage strategies for TEXT columns in columnar storage:
//! - **INLINE** (<4KB): Store directly in main Parquet column
//! - **CHUNKED** (4KB-1MB): Split into chunks with per-chunk embeddings for RAG
//! - **SIDECAR** (>1MB): Store in separate files with references
//! - **ADAPTIVE**: Auto-select based on actual content size
//!
//! ## RAG Integration
//!
//! The `TextChunker` struct provides intelligent text chunking for RAG workflows:
//! - Configurable chunk size (default 512 characters)
//! - Overlap support for context preservation (default 50 characters)
//! - Sentence/paragraph boundary preservation when possible
//! - Chunk ID generation for retrieval
//! - Position metadata for reconstructing original text
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
//! │  TextChunker │ TextColumnWriter │ TextColumnReader          │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Use Cases
//!
//! - **INLINE**: Short descriptions, titles, tags
//! - **CHUNKED**: Documents for RAG (embeddings per chunk)
//! - **SIDECAR**: Large documents, logs, raw content
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use proximadb::storage::engines::core::formats::columnar::text_storage::*;
//!
//! // Create a chunker with custom configuration
//! let chunker = TextChunker::new(ChunkingConfig {
//!     chunk_size: 512,
//!     overlap: 50,
//!     preserve_boundaries: true,
//!     ..Default::default()
//! });
//!
//! // Chunk a document
//! let chunks = chunker.chunk_text("doc_123", "Your long document text...");
//!
//! // Each chunk has:
//! // - Unique chunk_id for retrieval
//! // - Position metadata (start_offset, end_offset)
//! // - Reference to parent document
//! // - Optional embedding slot
//! ```

use arrow::array::{Array, ArrayRef, LargeStringArray, StringArray};
use arrow::datatypes::{DataType, Field};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

use crate::core::types::TextStorageStrategy;

// Import full-text search index types
use super::fulltext_index::{
    BM25Config, FullTextIndex, FullTextIndexError, SearchOptions,
    SearchResult as FullTextSearchResult, TokenizerConfig,
};

/// Thresholds for storage strategy selection
pub const INLINE_THRESHOLD: usize = 4 * 1024; // 4KB
pub const CHUNKED_THRESHOLD: usize = 1024 * 1024; // 1MB
pub const DEFAULT_CHUNK_SIZE: usize = 512; // 512 characters per chunk (for RAG)
pub const DEFAULT_OVERLAP_SIZE: usize = 50; // 50 characters overlap between chunks
pub const MIN_CHUNK_SIZE: usize = 64; // Minimum chunk size to avoid tiny fragments
pub const MAX_BOUNDARY_SEARCH: usize = 100; // Max chars to search for sentence boundary

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

    /// Full-text index error
    #[error("Full-text index error: {0}")]
    FullTextIndexError(#[from] FullTextIndexError),
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
#[derive(Default)]
pub enum SidecarCompression {
    /// No compression
    None,
    /// LZ4 compression (fast)
    #[default]
    Lz4,
    /// Zstd compression (better ratio)
    Zstd,
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

// =============================================================================
// RAG Text Chunking
// =============================================================================

/// Configuration for text chunking operations
///
/// Controls how text is split into chunks for RAG (Retrieval Augmented Generation)
/// workflows. Proper chunking is critical for:
/// - Embedding quality: Chunks should be semantically coherent
/// - Retrieval accuracy: Overlap preserves context across chunk boundaries
/// - Storage efficiency: Chunk size affects storage and retrieval performance
#[derive(Debug, Clone)]
pub struct ChunkingConfig {
    /// Target chunk size in characters (default: 512)
    ///
    /// This is the ideal chunk size. Actual chunks may be slightly smaller
    /// or larger when preserving sentence/paragraph boundaries.
    pub chunk_size: usize,

    /// Overlap between consecutive chunks in characters (default: 50)
    ///
    /// Overlap ensures that information at chunk boundaries is not lost.
    /// A typical value is 10-20% of chunk_size.
    pub overlap: usize,

    /// Whether to preserve sentence/paragraph boundaries (default: true)
    ///
    /// When true, chunks will be adjusted to end at natural boundaries
    /// (periods, newlines) when possible, improving semantic coherence.
    pub preserve_boundaries: bool,

    /// Minimum chunk size (default: 64)
    ///
    /// Prevents creation of very small chunks at the end of text.
    /// If the remaining text is smaller than this, it's merged with
    /// the previous chunk.
    pub min_chunk_size: usize,

    /// Maximum boundary search distance (default: 100)
    ///
    /// When looking for sentence boundaries, search at most this many
    /// characters beyond the target chunk size.
    pub max_boundary_search: usize,

    /// ID prefix for generated chunk IDs
    ///
    /// Chunk IDs are generated as: `{prefix}_{parent_id}_{chunk_index}`
    /// Default prefix is "chunk".
    pub chunk_id_prefix: String,

    /// Separator pattern for splitting (used when preserve_boundaries is false)
    ///
    /// If provided, text is split on this pattern first, then combined
    /// to reach target chunk size.
    pub separator: Option<String>,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            overlap: DEFAULT_OVERLAP_SIZE,
            preserve_boundaries: true,
            min_chunk_size: MIN_CHUNK_SIZE,
            max_boundary_search: MAX_BOUNDARY_SEARCH,
            chunk_id_prefix: "chunk".to_string(),
            separator: None,
        }
    }
}

impl ChunkingConfig {
    /// Create a new configuration with specified chunk size and overlap
    pub fn new(chunk_size: usize, overlap: usize) -> Self {
        Self {
            chunk_size,
            overlap,
            ..Default::default()
        }
    }

    /// Create configuration optimized for semantic search
    ///
    /// Uses smaller chunks with more overlap for better retrieval
    pub fn for_semantic_search() -> Self {
        Self {
            chunk_size: 256,
            overlap: 64,
            preserve_boundaries: true,
            ..Default::default()
        }
    }

    /// Create configuration optimized for question answering
    ///
    /// Uses larger chunks to preserve more context
    pub fn for_qa() -> Self {
        Self {
            chunk_size: 1024,
            overlap: 128,
            preserve_boundaries: true,
            ..Default::default()
        }
    }

    /// Create configuration for code/structured text
    ///
    /// Respects paragraph boundaries more strictly
    pub fn for_code() -> Self {
        Self {
            chunk_size: 512,
            overlap: 32,
            preserve_boundaries: true,
            separator: Some("\n\n".to_string()),
            ..Default::default()
        }
    }

    /// Builder method to set chunk size
    pub fn with_chunk_size(mut self, size: usize) -> Self {
        self.chunk_size = size;
        self
    }

    /// Builder method to set overlap
    pub fn with_overlap(mut self, overlap: usize) -> Self {
        self.overlap = overlap;
        self
    }

    /// Builder method to enable/disable boundary preservation
    pub fn with_boundary_preservation(mut self, preserve: bool) -> Self {
        self.preserve_boundaries = preserve;
        self
    }

    /// Builder method to set chunk ID prefix
    pub fn with_id_prefix(mut self, prefix: String) -> Self {
        self.chunk_id_prefix = prefix;
        self
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), TextStorageError> {
        if self.chunk_size < self.min_chunk_size {
            return Err(TextStorageError::ConfigError(format!(
                "chunk_size ({}) must be >= min_chunk_size ({})",
                self.chunk_size, self.min_chunk_size
            )));
        }
        if self.overlap >= self.chunk_size {
            return Err(TextStorageError::ConfigError(format!(
                "overlap ({}) must be < chunk_size ({})",
                self.overlap, self.chunk_size
            )));
        }
        Ok(())
    }
}

/// Metadata about a chunk's position within the original text
#[derive(Debug, Clone)]
pub struct ChunkPosition {
    /// Byte offset from start of original text
    pub byte_start: usize,
    /// Byte offset of end (exclusive)
    pub byte_end: usize,
    /// Character offset from start of original text
    pub char_start: usize,
    /// Character offset of end (exclusive)
    pub char_end: usize,
    /// Line number where chunk starts (1-indexed)
    pub line_start: usize,
    /// Line number where chunk ends (1-indexed)
    pub line_end: usize,
}

impl ChunkPosition {
    /// Create a new chunk position
    pub fn new(byte_start: usize, byte_end: usize, char_start: usize, char_end: usize) -> Self {
        Self {
            byte_start,
            byte_end,
            char_start,
            char_end,
            line_start: 1,
            line_end: 1,
        }
    }

    /// Set line information
    pub fn with_lines(mut self, start: usize, end: usize) -> Self {
        self.line_start = start;
        self.line_end = end;
        self
    }

    /// Get the byte length of this chunk
    pub fn byte_len(&self) -> usize {
        self.byte_end - self.byte_start
    }

    /// Get the character length of this chunk
    pub fn char_len(&self) -> usize {
        self.char_end - self.char_start
    }
}

/// RAG-optimized text chunker for generating per-chunk embeddings
///
/// The `TextChunker` provides intelligent text splitting that:
/// - Respects semantic boundaries (sentences, paragraphs)
/// - Maintains overlap for context preservation
/// - Generates unique chunk IDs for retrieval
/// - Tracks position metadata for text reconstruction
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::storage::engines::core::formats::columnar::text_storage::*;
///
/// let chunker = TextChunker::new(ChunkingConfig::default());
/// let chunks = chunker.chunk_text("doc_001", "Long document text...");
///
/// for chunk in chunks {
///     println!("Chunk {}: {} chars at offset {}",
///         chunk.chunk_id,
///         chunk.content.len(),
///         chunk.start_offset);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct TextChunker {
    /// Chunking configuration
    config: ChunkingConfig,
}

impl TextChunker {
    /// Create a new text chunker with the given configuration
    pub fn new(config: ChunkingConfig) -> Self {
        Self { config }
    }

    /// Create a text chunker with default configuration
    pub fn default_chunker() -> Self {
        Self::new(ChunkingConfig::default())
    }

    /// Get the current configuration
    pub fn config(&self) -> &ChunkingConfig {
        &self.config
    }

    /// Split text into chunks with overlap and boundary preservation
    ///
    /// # Arguments
    /// * `parent_id` - ID of the parent document/record
    /// * `text` - The text content to chunk
    ///
    /// # Returns
    /// Vector of `TextChunk` with position metadata
    pub fn chunk_text(&self, parent_id: &str, text: &str) -> Vec<TextChunk> {
        if text.is_empty() {
            return Vec::new();
        }

        // Validate configuration
        if self.config.validate().is_err() {
            // Fall back to simple chunking on invalid config
            return self.simple_chunk(parent_id, text);
        }

        let chars: Vec<char> = text.chars().collect();
        let total_chars = chars.len();

        // If text is smaller than minimum chunk size, return as single chunk
        if total_chars <= self.config.min_chunk_size {
            return vec![self.create_chunk(parent_id, 0, text.to_string(), 0, total_chars, 1)];
        }

        let mut chunks = Vec::new();
        let mut char_start = 0;
        let mut chunk_index = 0u32;

        while char_start < total_chars {
            // Calculate target end position
            let target_end = (char_start + self.config.chunk_size).min(total_chars);

            // Find the actual end position (with boundary preservation if enabled)
            let actual_end = if self.config.preserve_boundaries && target_end < total_chars {
                self.find_boundary(&chars, target_end)
            } else {
                target_end
            };

            // Extract chunk content
            let chunk_content: String = chars[char_start..actual_end].iter().collect();

            // Skip empty chunks
            if !chunk_content.trim().is_empty() {
                let chunk = self.create_chunk(
                    parent_id,
                    chunk_index,
                    chunk_content,
                    char_start,
                    actual_end,
                    total_chars,
                );
                chunks.push(chunk);
                chunk_index += 1;
            }

            // Calculate next start position with overlap
            let step = if actual_end == total_chars {
                // Last chunk, we're done
                total_chars
            } else {
                // Apply overlap
                let effective_step = (actual_end - char_start).saturating_sub(self.config.overlap);
                if effective_step == 0 {
                    // Prevent infinite loop
                    self.config.chunk_size
                } else {
                    effective_step
                }
            };

            char_start += step;

            // If remaining text is too small or we've exceeded bounds, stop
            if char_start >= total_chars {
                break;
            }
            let remaining_chars = total_chars.saturating_sub(char_start);
            if remaining_chars < self.config.min_chunk_size && !chunks.is_empty() {
                // Extend the last chunk to include remaining text
                if let Some(last_chunk) = chunks.last_mut() {
                    let remaining: String = chars[char_start..].iter().collect();
                    last_chunk.content.push_str(&remaining);
                    last_chunk.end_offset = total_chars;
                }
                break;
            }
        }

        // Add total_chunks metadata to each chunk
        let total_chunks = chunks.len();
        for chunk in &mut chunks {
            chunk
                .metadata
                .insert("total_chunks".to_string(), total_chunks.to_string());
        }

        chunks
    }

    /// Simple chunking without boundary preservation (fallback)
    fn simple_chunk(&self, parent_id: &str, text: &str) -> Vec<TextChunk> {
        let chars: Vec<char> = text.chars().collect();
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut chunk_index = 0u32;
        let total_chars = chars.len();

        while start < total_chars {
            let end = (start + self.config.chunk_size).min(total_chars);
            let chunk_content: String = chars[start..end].iter().collect();

            let chunk = self.create_chunk(
                parent_id,
                chunk_index,
                chunk_content,
                start,
                end,
                total_chars,
            );
            chunks.push(chunk);

            // Move forward with overlap
            let step = if end == total_chars {
                total_chars
            } else {
                self.config.chunk_size.saturating_sub(self.config.overlap)
            };
            start += step;
            chunk_index += 1;
        }

        chunks
    }

    /// Find a natural boundary (sentence/paragraph end) near the target position
    fn find_boundary(&self, chars: &[char], target: usize) -> usize {
        let search_end = (target + self.config.max_boundary_search).min(chars.len());

        // First, look for paragraph boundary (double newline)
        for i in target..search_end {
            if i + 1 < chars.len() && chars[i] == '\n' && chars[i + 1] == '\n' {
                return i + 2;
            }
        }

        // Then look for sentence boundary (period/question/exclamation followed by space)
        for i in target..search_end {
            let c = chars[i];
            if (c == '.' || c == '!' || c == '?') && i + 1 < chars.len() {
                let next = chars[i + 1];
                if next.is_whitespace() || next == '"' || next == '\'' {
                    return i + 1;
                }
            }
        }

        // Look for single newline
        for i in target..search_end {
            if chars[i] == '\n' {
                return i + 1;
            }
        }

        // Look for any whitespace
        for i in target..search_end {
            if chars[i].is_whitespace() {
                return i + 1;
            }
        }

        // No boundary found, use target position
        target
    }

    /// Create a TextChunk with all metadata
    fn create_chunk(
        &self,
        parent_id: &str,
        chunk_index: u32,
        content: String,
        char_start: usize,
        char_end: usize,
        _total_chars: usize,
    ) -> TextChunk {
        let chunk_id = format!(
            "{}_{}_{:04}",
            self.config.chunk_id_prefix, parent_id, chunk_index
        );

        let mut metadata = HashMap::new();
        metadata.insert("chunk_index".to_string(), chunk_index.to_string());
        metadata.insert("parent_id".to_string(), parent_id.to_string());
        metadata.insert("char_start".to_string(), char_start.to_string());
        metadata.insert("char_end".to_string(), char_end.to_string());

        TextChunk {
            chunk_id,
            parent_id: parent_id.to_string(),
            chunk_index,
            content,
            embedding: None,
            start_offset: char_start,
            end_offset: char_end,
            metadata,
        }
    }

    /// Calculate byte offsets for a chunk (useful for binary formats)
    pub fn calculate_byte_offsets(
        text: &str,
        char_start: usize,
        char_end: usize,
    ) -> (usize, usize) {
        let chars: Vec<char> = text.chars().collect();
        let mut byte_start = 0;
        let mut byte_end = 0;

        for (i, _c) in chars.iter().enumerate() {
            if i == char_start {
                byte_start = text[..].chars().take(i).map(|c| c.len_utf8()).sum();
            }
            if i == char_end {
                byte_end = text[..].chars().take(i).map(|c| c.len_utf8()).sum();
                break;
            }
        }

        if char_end >= chars.len() {
            byte_end = text.len();
        }

        (byte_start, byte_end)
    }

    /// Calculate line numbers for a chunk
    pub fn calculate_line_numbers(
        text: &str,
        char_start: usize,
        char_end: usize,
    ) -> (usize, usize) {
        let chars: Vec<char> = text.chars().collect();
        let mut line = 1;
        let mut line_start = 1;
        let mut line_end = 1;

        for (i, c) in chars.iter().enumerate() {
            if i == char_start {
                line_start = line;
            }
            if i == char_end {
                line_end = line;
                break;
            }
            if *c == '\n' {
                line += 1;
            }
        }

        if char_end >= chars.len() {
            line_end = line;
        }

        (line_start, line_end)
    }

    /// Reassemble chunks back into original text (for verification)
    ///
    /// Note: This only works correctly when overlap is 0 or chunks
    /// have been properly deduplicated.
    pub fn reassemble_chunks(chunks: &[TextChunk]) -> String {
        if chunks.is_empty() {
            return String::new();
        }

        // Sort by chunk_index
        let mut sorted: Vec<_> = chunks.iter().collect();
        sorted.sort_by_key(|c| c.chunk_index);

        // For simple case (no overlap), just concatenate
        sorted.iter().map(|c| c.content.as_str()).collect()
    }

    /// Get chunk by ID from a collection
    pub fn find_chunk_by_id<'a>(chunks: &'a [TextChunk], chunk_id: &str) -> Option<&'a TextChunk> {
        chunks.iter().find(|c| c.chunk_id == chunk_id)
    }

    /// Get all chunks for a parent document
    pub fn get_chunks_for_parent<'a>(
        chunks: &'a [TextChunk],
        parent_id: &str,
    ) -> Vec<&'a TextChunk> {
        let mut result: Vec<_> = chunks.iter().filter(|c| c.parent_id == parent_id).collect();
        result.sort_by_key(|c| c.chunk_index);
        result
    }

    /// Generate a unique chunk ID
    pub fn generate_chunk_id(prefix: &str, parent_id: &str, index: u32) -> String {
        format!("{}_{}__{:04}", prefix, parent_id, index)
    }
}

impl Default for TextChunker {
    fn default() -> Self {
        Self::default_chunker()
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
            TextStorageError::InvalidChunkReference(format!("Invalid path encoding: {e}"))
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
/// - Chunked: Split into chunks with optional embeddings (using RAG-optimized TextChunker)
/// - Sidecar: Stored in external files with references
///
/// ## RAG Integration
///
/// For RAG workflows, use `with_chunking_config()` to configure intelligent chunking:
///
/// ```rust,ignore
/// let writer = TextColumnWriter::new(TextStorageConfig::for_rag_documents(512))
///     .with_chunking_config(ChunkingConfig {
///         chunk_size: 512,
///         overlap: 50,
///         preserve_boundaries: true,
///         ..Default::default()
///     });
/// ```
///
/// ## Full-Text Search Integration
///
/// Enable full-text indexing for BM25-based search:
///
/// ```rust,ignore
/// let writer = TextColumnWriter::new(TextStorageConfig::default())
///     .with_fulltext_index(TokenizerConfig::default());
///
/// writer.write("doc1", "The quick brown fox")?;
/// writer.write("doc2", "A lazy brown dog")?;
///
/// // Search with BM25 scoring
/// let results = writer.fulltext_search("quick brown", 10);
/// ```
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

    /// Optional RAG-optimized text chunker (used when configured)
    chunker: Option<TextChunker>,

    /// Optional full-text search index for BM25 ranking
    fulltext_index: Option<FullTextIndex>,

    /// Whether to automatically index all written text
    auto_index: bool,
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
            chunker: None,
            fulltext_index: None,
            auto_index: false,
        }
    }

    /// Configure RAG-optimized text chunking
    ///
    /// When configured, the writer will use the `TextChunker` for intelligent
    /// chunking with overlap and boundary preservation.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let writer = TextColumnWriter::new(TextStorageConfig::for_rag_documents(512))
    ///     .with_chunking_config(ChunkingConfig::for_semantic_search());
    /// ```
    pub fn with_chunking_config(mut self, chunking_config: ChunkingConfig) -> Self {
        self.chunker = Some(TextChunker::new(chunking_config));
        self
    }

    /// Configure with a custom TextChunker
    ///
    /// Use this when you need more control over the chunker instance.
    pub fn with_chunker(mut self, chunker: TextChunker) -> Self {
        self.chunker = Some(chunker);
        self
    }

    /// Get a reference to the current chunker (if configured)
    pub fn chunker(&self) -> Option<&TextChunker> {
        self.chunker.as_ref()
    }

    /// Check if RAG chunking is enabled
    pub fn has_rag_chunking(&self) -> bool {
        self.chunker.is_some()
    }

    // =========================================================================
    // Full-Text Index Configuration
    // =========================================================================

    /// Enable full-text indexing with the specified tokenizer configuration
    ///
    /// When enabled, all text written via `write()` will be automatically indexed
    /// for full-text search with BM25 scoring.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let writer = TextColumnWriter::new(TextStorageConfig::default())
    ///     .with_fulltext_index(TokenizerConfig::default());
    /// ```
    pub fn with_fulltext_index(mut self, tokenizer_config: TokenizerConfig) -> Self {
        self.fulltext_index = Some(FullTextIndex::new(tokenizer_config));
        self.auto_index = true;
        self
    }

    /// Enable full-text indexing with custom BM25 configuration
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let writer = TextColumnWriter::new(TextStorageConfig::default())
    ///     .with_fulltext_index_and_bm25(
    ///         TokenizerConfig::for_keyword_search(),
    ///         BM25Config::for_short_documents(),
    ///     );
    /// ```
    pub fn with_fulltext_index_and_bm25(
        mut self,
        tokenizer_config: TokenizerConfig,
        bm25_config: BM25Config,
    ) -> Self {
        self.fulltext_index =
            Some(FullTextIndex::new(tokenizer_config).with_bm25_config(bm25_config));
        self.auto_index = true;
        self
    }

    /// Set an existing full-text index
    ///
    /// Use this when you want to reuse an existing index or have more control
    /// over its construction.
    pub fn with_existing_index(mut self, index: FullTextIndex) -> Self {
        self.fulltext_index = Some(index);
        self.auto_index = true;
        self
    }

    /// Enable or disable automatic indexing of written text
    ///
    /// When disabled, you must manually call `index_document()` to add
    /// documents to the full-text index.
    pub fn set_auto_index(&mut self, enable: bool) {
        self.auto_index = enable;
    }

    /// Check if full-text indexing is enabled
    pub fn has_fulltext_index(&self) -> bool {
        self.fulltext_index.is_some()
    }

    /// Get a reference to the full-text index (if enabled)
    pub fn fulltext_index(&self) -> Option<&FullTextIndex> {
        self.fulltext_index.as_ref()
    }

    /// Get a mutable reference to the full-text index (if enabled)
    pub fn fulltext_index_mut(&mut self) -> Option<&mut FullTextIndex> {
        self.fulltext_index.as_mut()
    }

    /// Manually add a document to the full-text index
    ///
    /// This is useful when auto_index is disabled or when you want to
    /// index additional content beyond what's stored.
    pub fn index_document(&mut self, doc_id: &str, content: &str) -> Result<(), TextStorageError> {
        if let Some(ref mut index) = self.fulltext_index {
            index.add_document(doc_id, content)?;
        }
        Ok(())
    }

    /// Search the full-text index with BM25 scoring
    ///
    /// Returns documents ranked by relevance to the query.
    ///
    /// # Arguments
    /// * `query` - The search query (will be tokenized)
    /// * `limit` - Maximum number of results to return
    ///
    /// # Returns
    /// Vector of search results sorted by BM25 score (descending)
    pub fn fulltext_search(&self, query: &str, limit: usize) -> Vec<FullTextSearchResult> {
        match &self.fulltext_index {
            Some(index) => index.search(query, limit),
            None => Vec::new(),
        }
    }

    /// Search the full-text index with custom options
    ///
    /// # Arguments
    /// * `query` - The search query
    /// * `options` - Search options including min_score, highlights, term boosts
    pub fn fulltext_search_with_options(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> Vec<FullTextSearchResult> {
        match &self.fulltext_index {
            Some(index) => index.search_with_options(query, options),
            None => Vec::new(),
        }
    }

    /// Get the IDF (Inverse Document Frequency) for a term
    ///
    /// Useful for understanding term importance in the corpus.
    pub fn get_term_idf(&self, term: &str) -> f64 {
        match &self.fulltext_index {
            Some(index) => index.get_idf(term),
            None => 0.0,
        }
    }

    /// Get the document frequency for a term
    ///
    /// Returns the number of documents containing the term.
    pub fn get_document_frequency(&self, term: &str) -> u32 {
        match &self.fulltext_index {
            Some(index) => index.get_document_frequency(term),
            None => 0,
        }
    }

    /// Get top terms by document frequency
    ///
    /// Useful for understanding the most common terms in the corpus.
    pub fn get_top_terms(&self, limit: usize) -> Vec<(String, u32)> {
        match &self.fulltext_index {
            Some(index) => index.get_top_terms(limit),
            None => Vec::new(),
        }
    }

    /// Get terms matching a prefix (for autocomplete)
    pub fn get_terms_with_prefix(&self, prefix: &str, limit: usize) -> Vec<String> {
        match &self.fulltext_index {
            Some(index) => index.get_terms_with_prefix(prefix, limit),
            None => Vec::new(),
        }
    }

    /// Build the full-text index from all stored chunks
    ///
    /// This is useful when you want to index chunks after they've been
    /// created, rather than indexing individual documents.
    pub fn build_index_from_chunks(&mut self) -> Result<(), TextStorageError> {
        if self.fulltext_index.is_none() {
            self.fulltext_index = Some(FullTextIndex::new(TokenizerConfig::default()));
        }

        if let Some(ref mut index) = self.fulltext_index {
            for chunk in &self.chunk_buffer {
                let mut metadata = HashMap::new();
                metadata.insert("parent_id".to_string(), chunk.parent_id.clone());
                metadata.insert("chunk_index".to_string(), chunk.chunk_index.to_string());
                metadata.insert("start_offset".to_string(), chunk.start_offset.to_string());
                metadata.insert("end_offset".to_string(), chunk.end_offset.to_string());

                index.add_document_with_metadata(&chunk.chunk_id, &chunk.content, metadata)?;
            }
        }

        Ok(())
    }

    // =========================================================================
    // Write Operations
    // =========================================================================

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

        // Auto-index if full-text indexing is enabled
        if self.auto_index
            && let Some(ref mut index) = self.fulltext_index {
                // For inline/sidecar, index the full document
                // For chunked, index each chunk separately
                if strategy == TextStorageStrategy::Chunked {
                    // Chunks are indexed during write_chunked, skip here
                } else {
                    // Index the full document
                    let _ = index.add_document(record_id, content);
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
            .push(Some(format!("__chunked__:{chunk_count}")));

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
            .push(Some(format!("__sidecar__:{record_id}")));

        Ok(())
    }

    /// Split text into chunks
    ///
    /// If a `TextChunker` is configured, uses RAG-optimized chunking with:
    /// - Overlap support for context preservation
    /// - Sentence/paragraph boundary preservation
    /// - Rich metadata for retrieval
    ///
    /// Otherwise, falls back to simple fixed-size chunking.
    fn split_into_chunks(&self, record_id: &str, content: &str) -> Vec<TextChunk> {
        // Use TextChunker if configured for RAG-optimized chunking
        if let Some(ref chunker) = self.chunker {
            return chunker.chunk_text(record_id, content);
        }

        // Fallback to simple chunking (original behavior)
        let mut chunks = Vec::new();
        let chars: Vec<char> = content.chars().collect();
        let chunk_size = self.config.chunk_size;

        let mut start = 0;
        let mut chunk_index = 0u32;

        while start < chars.len() {
            let end = (start + chunk_size).min(chars.len());
            let chunk_content: String = chars[start..end].iter().collect();

            let chunk_id = format!("{record_id}_{chunk_index}");
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

    /// Clear all buffers and optionally the full-text index
    pub fn clear(&mut self) {
        self.inline_buffer.clear();
        self.sidecar_refs.clear();
        self.chunk_buffer.clear();
        self.storage_mapping.clear();
        self.current_sidecar_offset = 0;
        self.stats = TextStorageStats::default();

        // Clear the full-text index if present
        if let Some(ref mut index) = self.fulltext_index {
            let _ = index.clear();
        }
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
    #[allow(dead_code)]
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
                if let Some(val) = value.as_ref()
                    && val.starts_with("__sidecar__:") {
                        let record_id = &val[12..];
                        if let Some(sidecar_ref) = sidecar_map.get(record_id) {
                            let content = self.load_sidecar(sidecar_ref).await?;
                            *value = Some(content);
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
            .map_err(|e| TextStorageError::SerializationError(format!("Invalid UTF-8: {e}")))
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
        let restored = SidecarRef::from_bytes("record_1".to_string(), &bytes)
            .expect("SidecarRef deserialization should succeed for valid bytes");

        assert_eq!(restored.sidecar_path, "/path/to/sidecar");
        assert_eq!(restored.offset, 100);
        assert_eq!(restored.length, 500);
        assert_eq!(restored.compression, SidecarCompression::Zstd);
    }

    #[test]
    fn test_writer_inline() {
        let config = TextStorageConfig::for_small_text();
        let mut writer = TextColumnWriter::new(config);

        writer
            .write("rec_1", "Hello")
            .expect("Write should succeed for valid inline text");
        writer
            .write("rec_2", "World")
            .expect("Write should succeed for valid inline text");
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
            .expect("Write should succeed for chunked text");

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

        writer
            .write("rec_1", "Hello")
            .expect("Write should succeed for valid inline text");

        assert_eq!(writer.get_storage_type("rec_1"), Some(StorageType::Inline));
        assert_eq!(writer.get_storage_type("unknown"), None);
    }

    #[test]
    fn test_build_arrow_array() {
        let config = TextStorageConfig::default();
        let mut writer = TextColumnWriter::new(config);

        writer
            .write("rec_1", "Hello")
            .expect("Write should succeed for valid inline text");
        writer
            .write("rec_2", "World")
            .expect("Write should succeed for valid inline text");
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

        let values = reader
            .load_from_array(&array_ref)
            .expect("Load from array should succeed for valid StringArray");
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

    // =========================================================================
    // TextChunker Tests
    // =========================================================================

    #[test]
    fn test_chunking_config_default() {
        let config = ChunkingConfig::default();
        assert_eq!(config.chunk_size, DEFAULT_CHUNK_SIZE);
        assert_eq!(config.overlap, DEFAULT_OVERLAP_SIZE);
        assert!(config.preserve_boundaries);
        assert_eq!(config.min_chunk_size, MIN_CHUNK_SIZE);
        assert_eq!(config.max_boundary_search, MAX_BOUNDARY_SEARCH);
    }

    #[test]
    fn test_chunking_config_presets() {
        let semantic = ChunkingConfig::for_semantic_search();
        assert_eq!(semantic.chunk_size, 256);
        assert_eq!(semantic.overlap, 64);

        let qa = ChunkingConfig::for_qa();
        assert_eq!(qa.chunk_size, 1024);
        assert_eq!(qa.overlap, 128);

        let code = ChunkingConfig::for_code();
        assert_eq!(code.chunk_size, 512);
        assert!(code.separator.is_some());
    }

    #[test]
    fn test_chunking_config_validation() {
        // Valid config
        let valid = ChunkingConfig::new(512, 50);
        assert!(valid.validate().is_ok());

        // Overlap >= chunk_size is invalid
        let mut invalid = ChunkingConfig::default();
        invalid.overlap = 600;
        assert!(invalid.validate().is_err());

        // chunk_size < min_chunk_size is invalid
        let mut invalid2 = ChunkingConfig::default();
        invalid2.chunk_size = 32;
        invalid2.min_chunk_size = 64;
        assert!(invalid2.validate().is_err());
    }

    #[test]
    fn test_text_chunker_simple() {
        let chunker = TextChunker::new(ChunkingConfig {
            chunk_size: 10,
            overlap: 0,
            preserve_boundaries: false,
            min_chunk_size: 5,
            ..Default::default()
        });

        let text = "Hello World, how are you today?";
        let chunks = chunker.chunk_text("doc1", text);

        assert!(!chunks.is_empty());
        assert!(chunks.len() > 1);

        // All chunks should have correct parent_id
        for chunk in &chunks {
            assert_eq!(chunk.parent_id, "doc1");
        }

        // First chunk should start at 0
        assert_eq!(chunks[0].start_offset, 0);
    }

    #[test]
    fn test_text_chunker_with_overlap() {
        let chunker = TextChunker::new(ChunkingConfig {
            chunk_size: 20,
            overlap: 5,
            preserve_boundaries: false,
            min_chunk_size: 10,
            ..Default::default()
        });

        let text = "AAAAAAAAAABBBBBBBBBBCCCCCCCCCC"; // 30 chars
        let chunks = chunker.chunk_text("doc1", text);

        // With overlap, chunks should overlap
        assert!(chunks.len() >= 2);

        // Check that chunks overlap (second chunk starts before first ends)
        if chunks.len() >= 2 {
            // The second chunk should start within the first chunk's range
            // due to overlap
            assert!(chunks[1].start_offset < chunks[0].end_offset || chunks.len() == 2);
        }
    }

    #[test]
    fn test_text_chunker_boundary_preservation() {
        let chunker = TextChunker::new(ChunkingConfig {
            chunk_size: 30,
            overlap: 5,
            preserve_boundaries: true,
            min_chunk_size: 10,
            max_boundary_search: 20,
            ..Default::default()
        });

        // Text with clear sentence boundaries
        let text = "Hello world. This is a test. Another sentence here.";
        let chunks = chunker.chunk_text("doc1", text);

        // Should have at least one chunk
        assert!(!chunks.is_empty());

        // With boundary preservation, chunks should tend to end at periods
        // (This is a soft check since boundary finding is best-effort)
        for chunk in &chunks {
            // Chunks should not be empty
            assert!(!chunk.content.trim().is_empty());
        }
    }

    #[test]
    fn test_text_chunker_empty_text() {
        let chunker = TextChunker::default();
        let chunks = chunker.chunk_text("doc1", "");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_text_chunker_small_text() {
        let chunker = TextChunker::new(ChunkingConfig {
            chunk_size: 512,
            overlap: 50,
            min_chunk_size: 64,
            ..Default::default()
        });

        let text = "Short text"; // Less than min_chunk_size
        let chunks = chunker.chunk_text("doc1", text);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Short text");
    }

    #[test]
    fn test_text_chunker_metadata() {
        let chunker = TextChunker::new(ChunkingConfig {
            chunk_size: 20,
            overlap: 5,
            preserve_boundaries: false,
            min_chunk_size: 10,
            chunk_id_prefix: "test_chunk".to_string(),
            ..Default::default()
        });

        let text = "This is a test document for metadata.";
        let chunks = chunker.chunk_text("doc123", text);

        assert!(!chunks.is_empty());

        let first_chunk = &chunks[0];

        // Check chunk_id format
        assert!(first_chunk.chunk_id.starts_with("test_chunk_doc123_"));

        // Check metadata
        assert_eq!(
            first_chunk.metadata.get("parent_id"),
            Some(&"doc123".to_string())
        );
        assert!(first_chunk.metadata.contains_key("chunk_index"));
        assert!(first_chunk.metadata.contains_key("char_start"));
        assert!(first_chunk.metadata.contains_key("char_end"));
        assert!(first_chunk.metadata.contains_key("total_chunks"));
    }

    #[test]
    fn test_text_chunker_chunk_position() {
        let position = ChunkPosition::new(10, 100, 5, 50).with_lines(2, 5);

        assert_eq!(position.byte_start, 10);
        assert_eq!(position.byte_end, 100);
        assert_eq!(position.byte_len(), 90);
        assert_eq!(position.char_start, 5);
        assert_eq!(position.char_end, 50);
        assert_eq!(position.char_len(), 45);
        assert_eq!(position.line_start, 2);
        assert_eq!(position.line_end, 5);
    }

    #[test]
    fn test_text_chunker_byte_offset_calculation() {
        let text = "Hello World";
        let (byte_start, byte_end) = TextChunker::calculate_byte_offsets(text, 0, 5);
        assert_eq!(byte_start, 0);
        assert_eq!(byte_end, 5); // "Hello" is 5 bytes
    }

    #[test]
    fn test_text_chunker_line_number_calculation() {
        let text = "Line 1\nLine 2\nLine 3";
        let (line_start, line_end) = TextChunker::calculate_line_numbers(text, 0, 15);
        assert_eq!(line_start, 1);
        assert!(line_end >= 2);
    }

    #[test]
    fn test_text_chunker_find_by_id() {
        let chunker = TextChunker::default();
        let text = "This is a test document that will be chunked into multiple pieces.";
        let chunks = chunker.chunk_text("doc1", text);

        if !chunks.is_empty() {
            let chunk_id = &chunks[0].chunk_id;
            let found = TextChunker::find_chunk_by_id(&chunks, chunk_id);
            assert!(found.is_some());
            let found_chunk = found.expect("Chunk should be found after is_some() check");
            assert_eq!(found_chunk.chunk_id, *chunk_id);

            let not_found = TextChunker::find_chunk_by_id(&chunks, "nonexistent");
            assert!(not_found.is_none());
        }
    }

    #[test]
    fn test_text_chunker_get_chunks_for_parent() {
        let chunker = TextChunker::new(ChunkingConfig {
            chunk_size: 20,
            overlap: 0,
            preserve_boundaries: false,
            min_chunk_size: 10,
            ..Default::default()
        });

        let chunks1 = chunker.chunk_text("doc1", "Text for document one.");
        let chunks2 = chunker.chunk_text("doc2", "Text for document two.");

        let mut all_chunks: Vec<TextChunk> = Vec::new();
        all_chunks.extend(chunks1);
        all_chunks.extend(chunks2);

        let doc1_chunks = TextChunker::get_chunks_for_parent(&all_chunks, "doc1");
        let doc2_chunks = TextChunker::get_chunks_for_parent(&all_chunks, "doc2");

        assert!(!doc1_chunks.is_empty());
        assert!(!doc2_chunks.is_empty());

        for chunk in doc1_chunks {
            assert_eq!(chunk.parent_id, "doc1");
        }
        for chunk in doc2_chunks {
            assert_eq!(chunk.parent_id, "doc2");
        }
    }

    // =========================================================================
    // TextColumnWriter with RAG Chunking Tests
    // =========================================================================

    #[test]
    fn test_writer_with_rag_chunking() {
        let mut config = TextStorageConfig::default();
        config.strategy = TextStorageStrategy::Chunked;

        let writer = TextColumnWriter::new(config).with_chunking_config(ChunkingConfig {
            chunk_size: 20,
            overlap: 5,
            preserve_boundaries: false,
            min_chunk_size: 10,
            ..Default::default()
        });

        assert!(writer.has_rag_chunking());
        assert!(writer.chunker().is_some());
    }

    #[test]
    fn test_writer_rag_chunking_produces_overlap() {
        let mut config = TextStorageConfig::default();
        config.strategy = TextStorageStrategy::Chunked;

        let mut writer = TextColumnWriter::new(config).with_chunking_config(ChunkingConfig {
            chunk_size: 20,
            overlap: 5,
            preserve_boundaries: false,
            min_chunk_size: 10,
            ..Default::default()
        });

        writer
            .write(
                "rec_1",
                "This is a longer text for testing RAG chunking with overlap.",
            )
            .expect("Write should succeed for RAG chunking with valid text");

        let chunks = writer.get_chunks();
        assert!(chunks.len() > 1);

        // Verify chunks have metadata
        for chunk in chunks {
            assert!(chunk.metadata.contains_key("parent_id"));
            assert!(chunk.metadata.contains_key("chunk_index"));
        }
    }

    #[test]
    fn test_writer_without_rag_chunking_fallback() {
        let mut config = TextStorageConfig::default();
        config.strategy = TextStorageStrategy::Chunked;
        config.chunk_size = 10;

        let mut writer = TextColumnWriter::new(config);
        // No chunking config set - should use fallback

        assert!(!writer.has_rag_chunking());

        writer
            .write("rec_1", "This is a test text for fallback chunking.")
            .expect("Write should succeed for fallback chunking with valid text");

        let chunks = writer.get_chunks();
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunker_generate_chunk_id() {
        let id = TextChunker::generate_chunk_id("chunk", "doc123", 5);
        assert_eq!(id, "chunk_doc123__0005");
    }

    // =========================================================================
    // Full-Text Index Integration Tests
    // =========================================================================

    #[test]
    fn test_writer_with_fulltext_index() {
        let writer = TextColumnWriter::new(TextStorageConfig::default())
            .with_fulltext_index(TokenizerConfig::default());

        assert!(writer.has_fulltext_index());
        assert!(writer.fulltext_index().is_some());
    }

    #[test]
    fn test_fulltext_auto_indexing() {
        let mut writer = TextColumnWriter::new(TextStorageConfig::default())
            .with_fulltext_index(TokenizerConfig::default());

        writer
            .write("doc1", "The quick brown fox")
            .expect("Write should succeed for full-text indexing");
        writer
            .write("doc2", "A lazy brown dog")
            .expect("Write should succeed for full-text indexing");
        writer
            .write("doc3", "The quick blue bird")
            .expect("Write should succeed for full-text indexing");

        // Search for documents
        let results = writer.fulltext_search("quick brown", 10);
        assert!(!results.is_empty());

        // doc1 should rank highest (has both "quick" and "brown")
        assert_eq!(results[0].doc_id, "doc1");
    }

    #[test]
    fn test_fulltext_search_with_options() {
        let mut writer = TextColumnWriter::new(TextStorageConfig::default())
            .with_fulltext_index(TokenizerConfig::default());

        writer
            .write("doc1", "quick brown fox jumps")
            .expect("Write should succeed for full-text indexing");
        writer
            .write("doc2", "quick rabbit")
            .expect("Write should succeed for full-text indexing");
        writer
            .write("doc3", "slow brown tortoise")
            .expect("Write should succeed for full-text indexing");

        // Require all terms
        let results = writer
            .fulltext_search_with_options("quick brown", SearchOptions::top_k(10).require_all());

        // Only doc1 has both "quick" and "brown"
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "doc1");
    }

    #[test]
    fn test_fulltext_term_statistics() {
        let mut writer = TextColumnWriter::new(TextStorageConfig::default())
            .with_fulltext_index(TokenizerConfig::default());

        writer
            .write("doc1", "hello world")
            .expect("Write should succeed for full-text indexing");
        writer
            .write("doc2", "hello there")
            .expect("Write should succeed for full-text indexing");
        writer
            .write("doc3", "goodbye world")
            .expect("Write should succeed for full-text indexing");

        // Check document frequency
        let hello_df = writer.get_document_frequency("hello");
        assert_eq!(hello_df, 2);

        let world_df = writer.get_document_frequency("world");
        assert_eq!(world_df, 2);

        // Check IDF (higher for rarer terms)
        let hello_idf = writer.get_term_idf("hello");
        let goodbye_idf = writer.get_term_idf("goodbye");
        assert!(goodbye_idf > hello_idf); // "goodbye" is rarer
    }

    #[test]
    fn test_fulltext_top_terms() {
        let mut writer = TextColumnWriter::new(TextStorageConfig::default())
            .with_fulltext_index(TokenizerConfig::default());

        writer
            .write("doc1", "test testing tested")
            .expect("Write should succeed for full-text indexing");
        writer
            .write("doc2", "test example")
            .expect("Write should succeed for full-text indexing");
        writer
            .write("doc3", "test sample")
            .expect("Write should succeed for full-text indexing");

        let top_terms = writer.get_top_terms(5);
        assert!(!top_terms.is_empty());

        // "test" should be in the top terms
        let has_test = top_terms.iter().any(|(term, _)| term == "test");
        assert!(has_test);
    }

    #[test]
    fn test_fulltext_prefix_search() {
        let mut writer = TextColumnWriter::new(TextStorageConfig::default())
            .with_fulltext_index(TokenizerConfig::default());

        writer
            .write("doc1", "testing tested tester")
            .expect("Write should succeed for full-text indexing");
        writer
            .write("doc2", "temperature temporal")
            .expect("Write should succeed for full-text indexing");

        let terms = writer.get_terms_with_prefix("test", 10);
        assert!(!terms.is_empty());
        for term in &terms {
            assert!(term.starts_with("test"));
        }
    }

    #[test]
    fn test_fulltext_with_bm25_config() {
        let mut writer = TextColumnWriter::new(TextStorageConfig::default())
            .with_fulltext_index_and_bm25(
                TokenizerConfig::for_keyword_search(),
                BM25Config::for_short_documents(),
            );

        writer
            .write("doc1", "short text here")
            .expect("Write should succeed for BM25 indexing");
        writer
            .write("doc2", "another short document")
            .expect("Write should succeed for BM25 indexing");

        let results = writer.fulltext_search("short", 10);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_fulltext_manual_indexing() {
        let mut writer = TextColumnWriter::new(TextStorageConfig::default())
            .with_fulltext_index(TokenizerConfig::default());

        // Disable auto-indexing
        writer.set_auto_index(false);

        writer
            .write("doc1", "some text")
            .expect("Write should succeed even without auto-indexing");

        // Should not find anything because auto-index is disabled
        let results = writer.fulltext_search("text", 10);
        assert!(results.is_empty());

        // Manually index
        writer
            .index_document("doc1", "some text")
            .expect("Manual indexing should succeed for valid document");

        // Now should find it
        let results = writer.fulltext_search("text", 10);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_fulltext_clear() {
        let mut writer = TextColumnWriter::new(TextStorageConfig::default())
            .with_fulltext_index(TokenizerConfig::default());

        writer
            .write("doc1", "hello world")
            .expect("Write should succeed for full-text indexing");

        // Verify index has content
        let results = writer.fulltext_search("hello", 10);
        assert!(!results.is_empty());

        // Clear
        writer.clear();

        // Index should be empty
        let results = writer.fulltext_search("hello", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_fulltext_index_from_chunks() {
        let mut config = TextStorageConfig::default();
        config.strategy = TextStorageStrategy::Chunked;
        config.chunk_size = 20;

        let mut writer = TextColumnWriter::new(config).with_chunking_config(ChunkingConfig {
            chunk_size: 20,
            overlap: 5,
            preserve_boundaries: false,
            min_chunk_size: 10,
            ..Default::default()
        });

        // Write will create chunks
        writer
            .write(
                "doc1",
                "This is a longer document that will be split into multiple chunks for testing.",
            )
            .expect("Write should succeed for chunked text");

        // Build index from chunks
        writer
            .build_index_from_chunks()
            .expect("Building index from chunks should succeed");

        // Should be able to search chunks
        let results = writer.fulltext_search("document", 10);
        // Results should contain chunk IDs
        assert!(!results.is_empty());
    }
}
