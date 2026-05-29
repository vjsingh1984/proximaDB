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

//! # Unified WAL Operations
//!
//! Extends the WAL system to handle both vector and graph operations,
//! enabling atomic hybrid transactions across both data types.

use crate::proto::proximadb_v1::{DocumentUpdate, LogEntry, MetricSample};
use crate::storage::document::DocumentRecord;
use crate::storage::memtable::implementations::graph_memtable::GraphOperation;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb_records::ProximaRecord;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Unified WAL operation supporting vector, graph, document, and observability operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnifiedWALOperation {
    /// Vector operation (existing)
    VectorOp(VectorOperation),

    /// Graph operation
    GraphOp(GraphOperation),

    /// Document operation
    DocumentOp(DocumentOperation),

    /// Observability operation (logs, metrics, traces)
    ObservabilityOp(ObservabilityOperation),

    /// Hybrid operation combining vector and graph operations
    HybridOp {
        vector_ops: Vec<VectorOperation>,
        graph_ops: Vec<GraphOperation>,
        /// Ensures atomicity across both operation types
        transaction_id: String,
    },

    /// Batch operation for performance
    BatchOp {
        operations: Vec<UnifiedWALOperation>,
        /// Optional batch identifier
        batch_id: Option<String>,
    },

    /// Time-series operation
    TimeSeriesOp(TimeSeriesOperation),

    /// Checkpoint operation for recovery
    Checkpoint {
        sequence_number: u64,
        timestamp_ms: u64,
        /// Collections/graphs/document-collections/observability namespaces included in checkpoint
        collections: Vec<String>,
        graphs: Vec<String>,
        document_collections: Vec<String>,
        observability_namespaces: Vec<String>,
    },
}

/// Time-series operations for WAL
//
// `InsertRecord` carries a full ProximaRecord; the other variants are
// small. Boxing would force every write-path caller to allocate even
// when buffered in batch Vecs (where the per-variant size is
// amortised). WAL ops are serialised on the durable path — heap
// layout is dominated by the record payload, not the enum tag.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TimeSeriesOperation {
    /// Insert a time-series record into a partition
    InsertRecord {
        collection_id: String,
        /// Timestamp in milliseconds since Unix epoch
        timestamp_ms: i64,
        record: ProximaRecord,
    },
    /// Insert an OHLC bar
    InsertOHLC {
        collection_id: String,
        symbol: String,
        /// Timestamp in milliseconds since Unix epoch
        timestamp_ms: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: i64,
    },
    /// Create a new time partition
    CreatePartition {
        collection_id: String,
        /// Partition start timestamp in milliseconds since Unix epoch
        partition_key_ms: i64,
    },
    /// Drop a time partition
    DropPartition {
        collection_id: String,
        /// Partition start timestamp in milliseconds since Unix epoch
        partition_key_ms: i64,
    },
}

/// Document operations for WAL
//
// `InsertDocument` carries a full DocumentRecord; smaller variants
// describe drops/edits/projections. Boxing every variant would force
// per-call heap allocation on the hot write path. WAL ops are framed
// for durable serialisation — the document payload itself dominates
// the byte budget.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocumentOperation {
    /// Insert or update a document
    InsertDocument {
        collection_id: String,
        document: DocumentRecord,
    },
    /// Insert or update a document as canonical durable record state.
    ///
    /// This is the Phase 2 document rebase WAL shape from
    /// `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc`.
    /// The document facade may still rebuild legacy `DocumentRecord`
    /// projections during recovery, but durable intent is expressed as a
    /// `ProximaRecord`.
    UpsertCanonicalDocumentRecord {
        collection_id: String,
        record: ProximaRecord,
    },
    /// Update a document with patch operations
    UpdateDocument {
        collection_id: String,
        document_id: String,
        updates: Vec<DocumentUpdate>,
        new_version: u64,
    },
    /// Delete a document
    DeleteDocument {
        collection_id: String,
        document_id: String,
    },
    /// Delete a document by canonical record identity.
    DeleteCanonicalDocumentRecord {
        collection_id: String,
        document_id: String,
        record_oid: String,
    },
    /// Batch insert documents
    BatchDocuments {
        collection_id: String,
        documents: Vec<DocumentRecord>,
    },
    /// Create document collection
    CreateCollection {
        collection_id: String,
        config_json: String, // Serialized DocumentCollectionConfig
    },
    /// Delete document collection
    DeleteCollection { collection_id: String },
}

/// Observability operations for WAL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ObservabilityOperation {
    /// Write a log entry
    WriteLog { namespace: String, log: LogEntry },
    /// Write a batch of logs
    WriteLogs {
        namespace: String,
        logs: Vec<LogEntry>,
    },
    /// Write a metric sample
    WriteMetric {
        namespace: String,
        metric: MetricSample,
    },
    /// Write a batch of metrics
    WriteMetrics {
        namespace: String,
        metrics: Vec<MetricSample>,
    },
    /// Write a trace span (serialized as JSON for flexibility)
    WriteSpan {
        namespace: String,
        span_json: String,
    },
    /// Create observability namespace
    CreateNamespace {
        namespace: String,
        config_json: String,
    },
    /// Delete observability namespace
    DeleteNamespace { namespace: String },
}

/// Vector operations (existing functionality)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorOperation {
    AddVector {
        collection_id: String,
        record: ProximaRecord,
    },
    UpdateVector {
        collection_id: String,
        record: ProximaRecord,
    },
    DeleteVector {
        collection_id: String,
        vector_id: String,
    },
    BatchVectors {
        collection_id: String,
        records: Vec<ProximaRecord>,
    },
}

/// WAL entry with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedWALEntry {
    /// Unique sequence number
    pub sequence_number: u64,

    /// Operation to apply
    pub operation: UnifiedWALOperation,

    /// Timestamp of the operation (milliseconds since Unix epoch)
    pub timestamp_ms: u64,

    /// CRC32 checksum for integrity
    pub checksum: u32,

    /// Optional metadata
    pub metadata: Option<WALEntryMetadata>,
}

/// Metadata for WAL entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WALEntryMetadata {
    /// Client identifier
    pub client_id: Option<String>,

    /// Request ID for tracing
    pub request_id: Option<String>,

    /// Whether this operation requires fsync
    pub requires_fsync: bool,

    /// Priority for recovery ordering
    pub priority: i32,
}

impl UnifiedWALEntry {
    /// Create a new WAL entry
    pub fn new(sequence_number: u64, operation: UnifiedWALOperation) -> Self {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let checksum = Self::calculate_checksum(&operation);

        Self {
            sequence_number,
            operation,
            timestamp_ms,
            checksum,
            metadata: None,
        }
    }

    /// Calculate CRC32 checksum
    fn calculate_checksum(operation: &UnifiedWALOperation) -> u32 {
        // In production, use a proper CRC32 implementation
        // For now, using a simple hash
        let serialized = bincode::serialize(operation).unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        serialized.hash(&mut hasher);
        hasher.finish() as u32
    }

    /// Verify checksum integrity
    pub fn verify_checksum(&self) -> bool {
        let calculated = Self::calculate_checksum(&self.operation);
        calculated == self.checksum
    }

    /// Check if this is a graph operation
    pub fn is_graph_operation(&self) -> bool {
        matches!(
            &self.operation,
            UnifiedWALOperation::GraphOp(_) | UnifiedWALOperation::HybridOp { .. }
        )
    }

    /// Check if this is a vector operation
    pub fn is_vector_operation(&self) -> bool {
        matches!(
            &self.operation,
            UnifiedWALOperation::VectorOp(_) | UnifiedWALOperation::HybridOp { .. }
        )
    }

    /// Check if this is a document operation
    pub fn is_document_operation(&self) -> bool {
        matches!(&self.operation, UnifiedWALOperation::DocumentOp(_))
    }

    /// Check if this is an observability operation
    pub fn is_observability_operation(&self) -> bool {
        matches!(&self.operation, UnifiedWALOperation::ObservabilityOp(_))
    }

    /// Check if this is a time-series operation
    pub fn is_timeseries_operation(&self) -> bool {
        matches!(&self.operation, UnifiedWALOperation::TimeSeriesOp(_))
    }
}

/// WAL writer extension for unified operations
pub struct UnifiedWALWriter {
    /// Base path for WAL files
    base_path: String,

    /// Current sequence number
    sequence_number: std::sync::atomic::AtomicU64,

    /// Filesystem factory for file operations
    filesystem: Arc<FilesystemFactory>,

    /// Current segment path
    current_segment_path: Option<String>,

    /// Current segment data buffer
    current_segment_data: Vec<u8>,

    /// Maximum size per WAL segment
    max_segment_size: usize,

    /// Segment counter
    segment_counter: u32,
}

impl UnifiedWALWriter {
    /// Create a new unified WAL writer
    pub async fn new(base_path: String) -> anyhow::Result<Self> {
        // Create filesystem factory with default config
        let filesystem = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create filesystem: {}", e))?,
        );

        // Ensure base directory exists. If caller passed a URL, don't prefix again.
        let base_url = if base_path.contains("://") {
            base_path.clone()
        } else {
            format!("file://{}", base_path)
        };
        let fs = filesystem.get_filesystem(&base_url)?;
        fs.create_dir_all(&base_url).await?;

        // Discover existing WAL files to resume from max sequence number
        let mut max_seq: u64 = 0;
        let mut segment_count: u64 = 0;
        if let Ok(files) = fs.list(&base_url).await {
            for file_info in &files {
                // WAL filenames: wal_YYYYMMDD_HHMMSS_{min_seq}_{max_seq}_{uuid}.{ext}
                if let Some(name) = file_info.name.split('/').next_back()
                    && name.starts_with("wal_")
                {
                    segment_count += 1;
                    // Extract max sequence from filename (field 3, 0-indexed)
                    let parts: Vec<&str> = name.split('_').collect();
                    if parts.len() >= 4
                        && let Ok(seq) = parts[3].parse::<u64>()
                    {
                        max_seq = max_seq.max(seq);
                    }
                }
            }
        }

        if max_seq > 0 {
            tracing::info!(
                "WAL recovery: found {} segments, resuming from sequence {}",
                segment_count,
                max_seq
            );
        } else {
            tracing::debug!("WAL writer initialized fresh for path: {}", base_path);
        }

        Ok(Self {
            base_path,
            sequence_number: std::sync::atomic::AtomicU64::new(max_seq),
            filesystem,
            current_segment_path: None,
            current_segment_data: Vec::new(),
            max_segment_size: 64 * 1024 * 1024, // 64MB segments
            segment_counter: segment_count as u32,
        })
    }

    /// Append an operation to the WAL
    pub async fn append(&mut self, operation: UnifiedWALOperation) -> anyhow::Result<u64> {
        use std::sync::atomic::Ordering;

        // fetch_add returns the old value before incrementing
        // So first call returns 0, then increments to 1
        let seq = self.sequence_number.fetch_add(1, Ordering::SeqCst);
        let entry = UnifiedWALEntry::new(seq, operation);

        // Serialize the entry
        let serialized = bincode::serialize(&entry)?;
        let size = serialized.len();

        tracing::debug!(
            "WAL append: seq={}, size={} bytes, checksum={}",
            seq,
            size,
            entry.checksum
        );

        // Check if we need to rotate the segment
        if self.current_segment_data.len() + size + 4 > self.max_segment_size {
            self.rotate_segment().await?;
        }

        // If no current segment, create one
        if self.current_segment_path.is_none() {
            self.open_new_segment().await?;
        }

        // Append to buffer
        // Write size header (4 bytes)
        self.current_segment_data
            .extend_from_slice(&(size as u32).to_le_bytes());
        // Write serialized entry
        self.current_segment_data.extend_from_slice(&serialized);

        // If requires immediate fsync, flush to disk
        if entry.metadata.as_ref().is_some_and(|m| m.requires_fsync) {
            self.flush_current_segment().await?;
        }

        Ok(seq)
    }

    /// Flush any buffered WAL entries to disk
    /// This should be called before shutdown to ensure durability
    pub async fn flush(&mut self) -> anyhow::Result<()> {
        self.flush_current_segment().await
    }

    /// Rotate to a new WAL segment
    async fn rotate_segment(&mut self) -> anyhow::Result<()> {
        // Flush current segment if exists
        if self.current_segment_path.is_some() {
            self.flush_current_segment().await?;
        }

        self.segment_counter += 1;
        self.current_segment_data.clear();
        self.open_new_segment().await?;

        Ok(())
    }

    /// Flush current segment to disk
    async fn flush_current_segment(&mut self) -> anyhow::Result<()> {
        if let Some(ref path) = self.current_segment_path
            && !self.current_segment_data.is_empty()
        {
            let url = format!("file://{}", path);
            let fs = self.filesystem.get_filesystem(&url)?;

            // Read existing data if file exists
            let mut full_data = if fs.exists(&url).await? {
                fs.read(&url).await?
            } else {
                Vec::new()
            };

            // Append new data
            full_data.extend_from_slice(&self.current_segment_data);

            // Write back atomically
            fs.write(&url, &full_data, None).await?;
            fs.sync_file(&url).await?;

            // Clear buffer after successful write
            self.current_segment_data.clear();
        }
        Ok(())
    }

    /// Open a new WAL segment file
    async fn open_new_segment(&mut self) -> anyhow::Result<()> {
        let filename = format!("{}/wal_{:08}.log", self.base_path, self.segment_counter);
        self.current_segment_path = Some(filename.clone());
        self.current_segment_data.clear();

        // Ensure the file exists
        let url = format!("file://{}", filename);
        let fs = self.filesystem.get_filesystem(&url)?;
        if !fs.exists(&url).await? {
            // Create empty file
            fs.write(&url, &[], None).await?;
        }

        Ok(())
    }

    /// Sync all pending writes
    pub async fn sync(&mut self) -> anyhow::Result<()> {
        // Flush any pending data to disk
        self.flush_current_segment().await?;
        Ok(())
    }
}

/// WAL reader for recovery
pub struct UnifiedWALReader {
    base_path: String,
    filesystem: Arc<FilesystemFactory>,
}

impl UnifiedWALReader {
    /// Create a new WAL reader
    pub async fn new(base_path: String) -> anyhow::Result<Self> {
        // Create filesystem factory with default config
        let filesystem = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create filesystem: {}", e))?,
        );

        Ok(Self {
            base_path,
            filesystem,
        })
    }

    /// Read all WAL entries from a segment
    pub async fn read_segment(&self, segment_number: u32) -> anyhow::Result<Vec<UnifiedWALEntry>> {
        let filename = format!("{}/wal_{:08}.log", self.base_path, segment_number);
        let url = format!("file://{}", filename);
        let fs = self.filesystem.get_filesystem(&url)?;

        if !fs.exists(&url).await? {
            return Ok(Vec::new());
        }

        let data = fs.read(&url).await?;
        tracing::debug!(
            "Reading WAL segment {}: {} total bytes",
            segment_number,
            data.len()
        );
        let mut entries = Vec::new();
        let mut cursor = 0;

        while cursor < data.len() {
            // Read size header
            if cursor + 4 > data.len() {
                tracing::debug!(
                    "End of WAL segment at cursor {}, {} bytes remaining",
                    cursor,
                    data.len() - cursor
                );
                break;
            }

            let size = u32::from_le_bytes([
                data[cursor],
                data[cursor + 1],
                data[cursor + 2],
                data[cursor + 3],
            ]) as usize;

            cursor += 4;

            // Sanity check on size
            if size == 0 || size > 10 * 1024 * 1024 {
                // 10MB max per entry
                tracing::warn!(
                    "Invalid WAL entry size {} at cursor {}, skipping rest of segment",
                    size,
                    cursor - 4
                );
                break;
            }

            // Read entry
            if cursor + size > data.len() {
                tracing::warn!(
                    "Truncated WAL entry at cursor {}: need {} bytes, have {}",
                    cursor,
                    size,
                    data.len() - cursor
                );
                break;
            }

            let entry_data = &data[cursor..cursor + size];
            match bincode::deserialize::<UnifiedWALEntry>(entry_data) {
                Ok(entry) => {
                    if entry.verify_checksum() {
                        entries.push(entry);
                    } else {
                        tracing::warn!("Checksum mismatch for entry {}", entries.len());
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to deserialize WAL entry at cursor {}, size {}: {:?}",
                        cursor,
                        size,
                        e
                    );
                    // Log first few bytes for debugging
                    let preview = &entry_data[..entry_data.len().min(32)];
                    tracing::debug!(
                        "Entry data preview (first {} bytes): {:?}",
                        preview.len(),
                        preview
                    );
                    // Continue trying to read more entries
                }
            }

            cursor += size;
        }

        Ok(entries)
    }

    /// Read all WAL entries for recovery
    pub async fn read_all(&self) -> anyhow::Result<Vec<UnifiedWALEntry>> {
        let mut all_entries = Vec::new();
        let mut segment = 0;

        tracing::debug!("WAL reader starting to read from path: {}", self.base_path);

        loop {
            let entries = self.read_segment(segment).await?;
            tracing::debug!("Read segment {}: {} entries", segment, entries.len());
            if entries.is_empty() {
                break;
            }
            all_entries.extend(entries);
            segment += 1;
        }

        tracing::debug!("WAL reader total entries read: {}", all_entries.len());
        Ok(all_entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Node;
    use proximadb_records::EmbeddingCell;

    #[tokio::test]
    async fn test_unified_wal_operations() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap().to_string();

        // Create writer
        let mut writer = UnifiedWALWriter::new(path.clone()).await.unwrap();

        // Test graph operation
        let graph_op = UnifiedWALOperation::GraphOp(GraphOperation::CreateNode {
            graph_id: "test_graph".to_string(),
            node: Node {
                id: "node1".to_string(),
                labels: vec!["TestNode".to_string()],
                properties: Default::default(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            },
        });

        let seq1 = writer.append(graph_op.clone()).await.unwrap();
        assert_eq!(seq1, 0);

        // Test vector operation
        let vector_op = UnifiedWALOperation::VectorOp(VectorOperation::AddVector {
            collection_id: "test_collection".to_string(),
            record: test_record("vec1", vec![0.1, 0.2, 0.3]),
        });

        let seq2 = writer.append(vector_op).await.unwrap();
        assert_eq!(seq2, 1);

        // Sync to disk
        writer.sync().await.unwrap();

        // Read back
        let reader = UnifiedWALReader::new(path).await.unwrap();
        let entries = reader.read_all().await.unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_graph_operation());
        assert!(entries[1].is_vector_operation());
    }

    #[test]
    fn test_hybrid_operation() {
        let hybrid_op = UnifiedWALOperation::HybridOp {
            vector_ops: vec![VectorOperation::AddVector {
                collection_id: "coll1".to_string(),
                record: test_record("vec1", vec![0.1, 0.2]),
            }],
            graph_ops: vec![GraphOperation::CreateNode {
                graph_id: "graph1".to_string(),
                node: Node {
                    id: "node1".to_string(),
                    labels: vec!["Label".to_string()],
                    properties: Default::default(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                },
            }],
            transaction_id: "tx123".to_string(),
        };

        let entry = UnifiedWALEntry::new(0, hybrid_op);
        assert!(entry.is_graph_operation());
        assert!(entry.is_vector_operation());
        assert!(entry.verify_checksum());
    }

    fn test_record(id: &str, vector: Vec<f32>) -> ProximaRecord {
        ProximaRecord {
            oid: id.to_string(),
            embeddings: vec![EmbeddingCell {
                model_id: "test".to_string(),
                modality: "dense_vector".to_string(),
                dim: vector.len() as u32,
                values: proximadb_records::EmbeddingValues::Fp32(vector),
                ..Default::default()
            }],
            created_at_ns: 0,
            ..Default::default()
        }
    }
}
