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

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::memtable::implementations::graph_memtable::{GraphOperation, NodeUpdate, EdgeUpdate};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Unified WAL operation supporting both vector and graph operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnifiedWALOperation {
    /// Vector operation (existing)
    VectorOp(VectorOperation),

    /// Graph operation (new)
    GraphOp(GraphOperation),

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

    /// Checkpoint operation for recovery
    Checkpoint {
        sequence_number: u64,
        timestamp: SystemTime,
        /// Collections/graphs included in checkpoint
        collections: Vec<String>,
        graphs: Vec<String>,
    },
}

/// Vector operations (existing functionality)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorOperation {
    AddVector {
        collection_id: String,
        vector: VectorRecord,
    },
    UpdateVector {
        collection_id: String,
        vector: VectorRecord,
    },
    DeleteVector {
        collection_id: String,
        vector_id: String,
    },
    BatchVectors {
        collection_id: String,
        vectors: Vec<VectorRecord>,
    },
}

/// WAL entry with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedWALEntry {
    /// Unique sequence number
    pub sequence_number: u64,

    /// Operation to apply
    pub operation: UnifiedWALOperation,

    /// Timestamp of the operation
    pub timestamp: SystemTime,

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
        let timestamp = SystemTime::now();
        let checksum = Self::calculate_checksum(&operation);

        Self {
            sequence_number,
            operation,
            timestamp,
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
            UnifiedWALOperation::GraphOp(_) |
            UnifiedWALOperation::HybridOp { .. }
        )
    }

    /// Check if this is a vector operation
    pub fn is_vector_operation(&self) -> bool {
        matches!(
            &self.operation,
            UnifiedWALOperation::VectorOp(_) |
            UnifiedWALOperation::HybridOp { .. }
        )
    }
}

/// WAL writer extension for unified operations
pub struct UnifiedWALWriter {
    /// Base path for WAL files
    base_path: String,

    /// Current sequence number
    sequence_number: std::sync::atomic::AtomicU64,

    /// File handle for current WAL segment
    current_file: Option<std::fs::File>,

    /// Maximum size per WAL segment
    max_segment_size: usize,

    /// Current segment size
    current_segment_size: usize,

    /// Segment counter
    segment_counter: u32,
}

impl UnifiedWALWriter {
    /// Create a new unified WAL writer
    pub fn new(base_path: String) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&base_path)?;

        Ok(Self {
            base_path,
            sequence_number: std::sync::atomic::AtomicU64::new(0),
            current_file: None,
            max_segment_size: 64 * 1024 * 1024, // 64MB segments
            current_segment_size: 0,
            segment_counter: 0,
        })
    }

    /// Append an operation to the WAL
    pub async fn append(&mut self, operation: UnifiedWALOperation) -> anyhow::Result<u64> {
        use std::sync::atomic::Ordering;

        let seq = self.sequence_number.fetch_add(1, Ordering::SeqCst);
        let entry = UnifiedWALEntry::new(seq, operation);

        // Serialize the entry
        let serialized = bincode::serialize(&entry)?;
        let size = serialized.len();

        // Check if we need to rotate the segment
        if self.current_segment_size + size > self.max_segment_size {
            self.rotate_segment().await?;
        }

        // Write to current segment
        if let Some(ref mut file) = self.current_file {
            use std::io::Write;

            // Write size header
            file.write_all(&(size as u32).to_le_bytes())?;

            // Write serialized entry
            file.write_all(&serialized)?;

            // Optionally fsync for durability
            if entry.metadata.as_ref().map(|m| m.requires_fsync).unwrap_or(false) {
                file.sync_all()?;
            }

            self.current_segment_size += size + 4; // Include size header
        } else {
            // Open first segment
            self.open_new_segment()?;
            return Box::pin(self.append(entry.operation)).await;
        }

        Ok(seq)
    }

    /// Rotate to a new WAL segment
    async fn rotate_segment(&mut self) -> anyhow::Result<()> {
        if let Some(mut file) = self.current_file.take() {
            use std::io::Write;
            file.flush()?;
        }

        self.segment_counter += 1;
        self.current_segment_size = 0;
        self.open_new_segment()?;

        Ok(())
    }

    /// Open a new WAL segment file
    fn open_new_segment(&mut self) -> anyhow::Result<()> {
        let filename = format!("{}/wal_{:08}.log", self.base_path, self.segment_counter);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(filename)?;

        self.current_file = Some(file);
        Ok(())
    }

    /// Sync all pending writes
    pub fn sync(&mut self) -> anyhow::Result<()> {
        if let Some(ref mut file) = self.current_file {
            use std::io::Write;
            file.sync_all()?;
        }
        Ok(())
    }
}

/// WAL reader for recovery
pub struct UnifiedWALReader {
    base_path: String,
}

impl UnifiedWALReader {
    /// Create a new WAL reader
    pub fn new(base_path: String) -> Self {
        Self { base_path }
    }

    /// Read all WAL entries from a segment
    pub fn read_segment(&self, segment_number: u32) -> anyhow::Result<Vec<UnifiedWALEntry>> {
        let filename = format!("{}/wal_{:08}.log", self.base_path, segment_number);

        if !std::path::Path::new(&filename).exists() {
            return Ok(Vec::new());
        }

        let data = std::fs::read(&filename)?;
        let mut entries = Vec::new();
        let mut cursor = 0;

        while cursor < data.len() {
            // Read size header
            if cursor + 4 > data.len() {
                break;
            }

            let size = u32::from_le_bytes([
                data[cursor],
                data[cursor + 1],
                data[cursor + 2],
                data[cursor + 3],
            ]) as usize;

            cursor += 4;

            // Read entry
            if cursor + size > data.len() {
                tracing::warn!("Truncated WAL entry at position {}", cursor);
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
                    tracing::warn!("Failed to deserialize WAL entry: {}", e);
                }
            }

            cursor += size;
        }

        Ok(entries)
    }

    /// Read all WAL entries for recovery
    pub fn read_all(&self) -> anyhow::Result<Vec<UnifiedWALEntry>> {
        let mut all_entries = Vec::new();
        let mut segment = 0;

        loop {
            let entries = self.read_segment(segment)?;
            if entries.is_empty() {
                break;
            }
            all_entries.extend(entries);
            segment += 1;
        }

        Ok(all_entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Node, Edge};

    #[tokio::test]
    async fn test_unified_wal_operations() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap().to_string();

        // Create writer
        let mut writer = UnifiedWALWriter::new(path.clone()).unwrap();

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
            vector: VectorRecord {
                id: "vec1".to_string(),
                vector: vec![0.1, 0.2, 0.3],
                metadata: Default::default(),
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                quantized_vector: vec![],
                source: None,
            },
        });

        let seq2 = writer.append(vector_op).await.unwrap();
        assert_eq!(seq2, 1);

        // Sync to disk
        writer.sync().unwrap();

        // Read back
        let reader = UnifiedWALReader::new(path);
        let entries = reader.read_all().unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_graph_operation());
        assert!(entries[1].is_vector_operation());
    }

    #[test]
    fn test_hybrid_operation() {
        let hybrid_op = UnifiedWALOperation::HybridOp {
            vector_ops: vec![
                VectorOperation::AddVector {
                    collection_id: "coll1".to_string(),
                    vector: VectorRecord {
                        id: "vec1".to_string(),
                        vector: vec![0.1, 0.2],
                        metadata: Default::default(),
                        timestamp: 0,
                        updated_at: None,
                        expires_at: None,
                        version: None,
                        quantized_vector: vec![],
                        source: None,
                    },
                },
            ],
            graph_ops: vec![
                GraphOperation::CreateNode {
                    graph_id: "graph1".to_string(),
                    node: Node {
                        id: "node1".to_string(),
                        labels: vec!["Label".to_string()],
                        properties: Default::default(),
                        embedding: None,
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    },
                },
            ],
            transaction_id: "tx123".to_string(),
        };

        let entry = UnifiedWALEntry::new(0, hybrid_op);
        assert!(entry.is_graph_operation());
        assert!(entry.is_vector_operation());
        assert!(entry.verify_checksum());
    }
}