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

//! AXIS Index Serialization
//!
//! Provides efficient serialization and deserialization for all AXIS index types
//! with support for incremental updates and delta management.

use crate::index::axis::{AxisHnswConfig, AxisHnswIndex, UnifiedIvfConfig, UnifiedIvfIndex};
use bincode;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Magic bytes for index format identification
const AXIS_MAGIC: &[u8; 4] = b"AXIS";
const VERSION: u16 = 1;

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn unix_now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

/// Serialization error types
#[derive(Debug, thiserror::Error)]
pub enum SerializationError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),

    #[error("Invalid magic bytes")]
    InvalidMagic,

    #[error("Unsupported version: {0}")]
    UnsupportedVersion(u16),

    #[error("Checksum mismatch")]
    ChecksumMismatch,

    #[error("Unknown index type: {0}")]
    UnknownIndex(String),

    #[error("Serialization not supported for index type: {0}")]
    NotSupported(String),
}

pub type Result<T> = std::result::Result<T, SerializationError>;

/// Index types that can be serialized
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Index {
    Hnsw,
    Ivf,
    Lsh,
    Annoy,
    Pq,
    Flat,
}

/// Metadata for serialized index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    /// Index type
    pub index_type: Index,

    /// Collection ID
    pub collection_id: String,

    /// Number of vectors in index
    pub num_vectors: usize,

    /// Vector dimension
    pub dimension: usize,

    /// Timestamp of serialization
    pub timestamp: u64,

    /// Checksum of the data
    pub checksum: u32,

    /// Is this a delta/incremental update
    pub is_delta: bool,

    /// Base checkpoint ID if this is a delta
    pub base_checkpoint_id: Option<String>,

    /// Custom metadata
    pub custom_metadata: Option<Vec<u8>>,
}

/// Header for serialized index file
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexHeader {
    /// Magic bytes (AXIS)
    pub magic: [u8; 4],

    /// Format version
    pub version: u16,

    /// Metadata
    pub metadata: IndexMetadata,
}

/// Checkpoint for incremental updates
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexCheckpoint {
    /// Unique checkpoint ID
    pub checkpoint_id: String,

    /// Timestamp
    pub timestamp: u64,

    /// Full index data
    pub index_data: Vec<u8>,

    /// Metadata
    pub metadata: IndexMetadata,
}

/// Delta update for incremental changes
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexDelta {
    /// Base checkpoint this delta applies to
    pub base_checkpoint_id: String,

    /// Delta ID
    pub delta_id: String,

    /// Timestamp
    pub timestamp: u64,

    /// Operations in this delta
    pub operations: Vec<DeltaOperation>,
}

/// Operations that can be applied as deltas
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DeltaOperation {
    /// Add new vectors
    AddVectors { vectors: Vec<(String, Vec<f32>)> },

    /// Remove vectors by ID
    RemoveVectors { vector_ids: Vec<String> },

    /// Update existing vectors
    UpdateVectors { updates: Vec<(String, Vec<f32>)> },

    /// Rebuild specific parts
    RebuildPartial { affected_nodes: Vec<usize> },
}

/// Main serialization handler for AXIS indexes
pub struct IndexSerializer;

impl IndexSerializer {
    /// Serialize HNSW index to bytes
    pub fn serialize_hnsw(index: &AxisHnswIndex, collection_id: &str) -> Result<Vec<u8>> {
        info!("Serializing HNSW index for collection {}", collection_id);

        // Create metadata
        let metadata = IndexMetadata {
            index_type: crate::index::axis::storage::serialization::Index::Hnsw,
            collection_id: collection_id.to_string(),
            num_vectors: index.len(),
            dimension: index.dimension(),
            timestamp: unix_now_secs(),
            checksum: 0, // Will be calculated after serialization
            is_delta: false,
            base_checkpoint_id: None,
            custom_metadata: None,
        };

        // Serialize index data
        let index_data = index.serialize_internal()?;

        // Calculate checksum
        let checksum = crate::utils::checksum::crc32_fast(&index_data);

        // Create header with updated checksum
        let mut final_metadata = metadata;
        final_metadata.checksum = checksum;

        let header = IndexHeader {
            magic: *AXIS_MAGIC,
            version: VERSION,
            metadata: final_metadata,
        };

        // Combine header and data
        let mut result = Vec::new();
        let header_bytes = bincode::serialize(&header)?;
        result.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
        result.extend_from_slice(&header_bytes);
        result.extend_from_slice(&index_data);

        info!("Serialized HNSW index: {} bytes", result.len());
        Ok(result)
    }

    /// Deserialize HNSW index from bytes
    pub fn deserialize_hnsw(
        data: &[u8],
        config: &AxisHnswConfig,
    ) -> Result<(AxisHnswIndex, IndexMetadata)> {
        info!("Deserializing HNSW index");

        // Read header length
        if data.len() < 4 {
            return Err(SerializationError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Data too short",
            )));
        }

        let header_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        // Read header
        if data.len() < 4 + header_len {
            return Err(SerializationError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Header incomplete",
            )));
        }

        let header: IndexHeader = bincode::deserialize(&data[4..4 + header_len])?;

        // Validate magic and version
        if header.magic != *AXIS_MAGIC {
            return Err(SerializationError::InvalidMagic);
        }

        if header.version > VERSION {
            return Err(SerializationError::UnsupportedVersion(header.version));
        }

        // Validate checksum
        let index_data = &data[4 + header_len..];
        let checksum = crate::utils::checksum::crc32_fast(index_data);

        if checksum != header.metadata.checksum {
            return Err(SerializationError::ChecksumMismatch);
        }

        // Deserialize index
        let index = AxisHnswIndex::deserialize_internal(index_data, config)?;

        info!(
            "Deserialized HNSW index with {} vectors",
            header.metadata.num_vectors
        );
        Ok((index, header.metadata))
    }

    /// Serialize IVF index to bytes
    pub fn serialize_ivf(index: &UnifiedIvfIndex, collection_id: &str) -> Result<Vec<u8>> {
        info!("Serializing IVF index for collection {}", collection_id);

        let metadata = IndexMetadata {
            index_type: Index::Ivf,
            collection_id: collection_id.to_string(),
            num_vectors: index.len(),
            dimension: index.dimension(),
            timestamp: unix_now_secs(),
            checksum: 0,
            is_delta: false,
            base_checkpoint_id: None,
            custom_metadata: None,
        };

        // Serialize index data
        let index_data = index.serialize_internal()?;

        // Calculate checksum
        let checksum = crate::utils::checksum::crc32_fast(&index_data);

        // Create header with updated checksum
        let mut final_metadata = metadata;
        final_metadata.checksum = checksum;

        let header = IndexHeader {
            magic: *AXIS_MAGIC,
            version: VERSION,
            metadata: final_metadata,
        };

        // Combine header and data
        let mut result = Vec::new();
        let header_bytes = bincode::serialize(&header)?;
        result.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
        result.extend_from_slice(&header_bytes);
        result.extend_from_slice(&index_data);

        info!("Serialized IVF index: {} bytes", result.len());
        Ok(result)
    }

    /// Deserialize IVF index from bytes
    pub fn deserialize_ivf(
        data: &[u8],
        config: &UnifiedIvfConfig,
    ) -> Result<(UnifiedIvfIndex, IndexMetadata)> {
        info!("Deserializing IVF index");

        // Read header length
        if data.len() < 4 {
            return Err(SerializationError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Data too short",
            )));
        }

        let header_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        // Read header
        if data.len() < 4 + header_len {
            return Err(SerializationError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Header incomplete",
            )));
        }

        let header: IndexHeader = bincode::deserialize(&data[4..4 + header_len])?;

        // Validate magic and version
        if header.magic != *AXIS_MAGIC {
            return Err(SerializationError::InvalidMagic);
        }

        if header.version > VERSION {
            return Err(SerializationError::UnsupportedVersion(header.version));
        }

        // Validate checksum
        let index_data = &data[4 + header_len..];
        let checksum = crate::utils::checksum::crc32_fast(index_data);

        if checksum != header.metadata.checksum {
            return Err(SerializationError::ChecksumMismatch);
        }

        // Deserialize index
        let index = UnifiedIvfIndex::deserialize_internal(index_data, config)?;

        info!(
            "Deserialized IVF index with {} vectors",
            header.metadata.num_vectors
        );
        Ok((index, header.metadata))
    }

    /// Create a checkpoint from current index state
    pub fn create_checkpoint(
        index_type: Index,
        index_data: Vec<u8>,
        collection_id: &str,
    ) -> Result<IndexCheckpoint> {
        let checkpoint_id = format!("chk_{}_{}", collection_id, unix_now_millis());

        let timestamp = unix_now_secs();

        let metadata = IndexMetadata {
            index_type,
            collection_id: collection_id.to_string(),
            num_vectors: 0, // Will be updated by specific index
            dimension: 0,   // Will be updated by specific index
            timestamp,
            checksum: crate::utils::checksum::crc32_fast(&index_data),
            is_delta: false,
            base_checkpoint_id: None,
            custom_metadata: None,
        };

        Ok(IndexCheckpoint {
            checkpoint_id,
            timestamp,
            index_data,
            metadata,
        })
    }

    /// Apply delta operations to an index
    pub fn apply_delta(checkpoint: &IndexCheckpoint, delta: &IndexDelta) -> Result<Vec<u8>> {
        if delta.base_checkpoint_id != checkpoint.checkpoint_id {
            return Err(SerializationError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Delta base does not match checkpoint",
            )));
        }

        info!(
            "Applying {} delta operations to checkpoint {}",
            delta.operations.len(),
            checkpoint.checkpoint_id
        );

        // This would need index-specific implementation
        // For now, return the original data
        warn!("Delta application not yet implemented for production");
        Ok(checkpoint.index_data.clone())
    }

    /// Serialize any supported index type
    pub fn serialize_generic(
        index: &dyn SerializableIndex,
        collection_id: &str,
    ) -> Result<Vec<u8>> {
        index.serialize_to_bytes(collection_id)
    }
}

/// Trait for indexes that can be serialized
pub trait SerializableIndex: Send + Sync {
    /// Get the index type
    fn index_type(&self) -> Index;

    /// Serialize to bytes
    fn serialize_to_bytes(&self, collection_id: &str) -> Result<Vec<u8>>;

    /// Get number of vectors
    fn len(&self) -> usize;

    /// Get dimension
    fn dimension(&self) -> usize;

    /// Serialize internal data (without header)
    fn serialize_internal(&self) -> Result<Vec<u8>>;
}

/// Serializable HNSW configuration (mirrors AxisHnswConfig)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableHnswConfig {
    pub m: usize,
    pub ef_construction: usize,
    pub ef: usize,
    pub max_layers: usize,
    pub distance_metric: u8, // 0=L2, 1=Cosine, 2=DotProduct
}

/// Serializable ID mapping data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableIdMapping {
    /// External ID -> Internal ID pairs
    pub external_to_internal: Vec<(String, usize)>,
    /// Next available internal ID
    pub next_id: usize,
}

/// Serializable vector data (ID + raw bytes)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableVector {
    pub id: String,
    pub data: Vec<u8>,
}

/// Serializable collection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableCollectionConfig {
    pub dimension: usize,
    pub is_quantized: bool,
    pub quantization_method: Option<u8>, // 0=INT8, 1=PQ8, 2=PQ4, 3=Binary
}

/// Complete serializable HNSW state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableHnswState {
    /// Version for forward compatibility
    pub version: u32,
    /// HNSW configuration
    pub config: SerializableHnswConfig,
    /// Collection configuration for vector interpretation
    pub collection_config: SerializableCollectionConfig,
    /// ID mappings
    pub id_mapping: SerializableIdMapping,
    /// Graph layers: (layer, node_id) -> connections
    pub layers: Vec<((usize, usize), Vec<usize>)>,
    /// Maximum layer in use
    pub max_layer: usize,
    /// Entry point node ID (if any)
    pub entry_point: Option<usize>,
    /// Vector data
    pub vectors: Vec<SerializableVector>,
    /// Quantized vectors (ID -> quantized bytes)
    pub quantized_vectors: Vec<(String, Vec<u8>)>,
    /// Dimension (for validation)
    pub dimension: usize,
}

impl SerializableHnswState {
    pub const CURRENT_VERSION: u32 = 1;
}

/// Extension trait for HNSW serialization
impl SerializableIndex for AxisHnswIndex {
    fn index_type(&self) -> Index {
        Index::Hnsw
    }

    fn serialize_to_bytes(&self, collection_id: &str) -> Result<Vec<u8>> {
        IndexSerializer::serialize_hnsw(self, collection_id)
    }

    fn len(&self) -> usize {
        // Access the actual vector count via id_mapping
        self.id_mapping_len()
    }

    fn dimension(&self) -> usize {
        // Access dimension from vectors collection config
        self.get_dimension()
    }

    fn serialize_internal(&self) -> Result<Vec<u8>> {
        info!("Starting HNSW serialize_internal");

        // 1. Serialize config
        let config = SerializableHnswConfig {
            m: self.get_config_m(),
            ef_construction: self.get_config_ef_construction(),
            ef: self.get_config_ef(),
            max_layers: self.get_config_max_layers(),
            distance_metric: self.get_config_distance_metric_code(),
        };

        // 2. Get collection config
        let (dimension, is_quantized, quant_method) = self.get_collection_config_details();
        let collection_config = SerializableCollectionConfig {
            dimension,
            is_quantized,
            quantization_method: quant_method,
        };

        // 3. Serialize ID mapping
        let id_mapping = self.serialize_id_mapping();

        // 4. Serialize graph layers
        let layers = self.serialize_layers();

        // 5. Get max layer and entry point
        let max_layer = self.get_max_layer();
        let entry_point = self.get_entry_point();

        // 6. Serialize vectors
        let vectors = self.serialize_vectors();

        // 7. Serialize quantized vectors
        let quantized_vectors = self.serialize_quantized_vectors();

        // Create complete state
        let state = SerializableHnswState {
            version: SerializableHnswState::CURRENT_VERSION,
            config,
            collection_config,
            id_mapping,
            layers,
            max_layer,
            entry_point,
            vectors,
            quantized_vectors,
            dimension,
        };

        // Serialize with bincode
        let bytes = bincode::serialize(&state).map_err(SerializationError::Bincode)?;

        info!(
            "HNSW serialize_internal complete: {} bytes, {} vectors, {} layers",
            bytes.len(),
            state.vectors.len(),
            state.layers.len()
        );

        Ok(bytes)
    }
}

/// Extension trait for HNSW deserialization
impl AxisHnswIndex {
    /// Deserialize HNSW index from bytes
    pub fn deserialize_internal(data: &[u8], config: &AxisHnswConfig) -> Result<Self> {
        info!("Starting HNSW deserialize_internal: {} bytes", data.len());

        // Deserialize the state
        let state: SerializableHnswState =
            bincode::deserialize(data).map_err(SerializationError::Bincode)?;

        // Validate version
        if state.version > SerializableHnswState::CURRENT_VERSION {
            return Err(SerializationError::UnsupportedVersion(state.version as u16));
        }

        // Create new index with deserialized config
        let index = match AxisHnswIndex::new(config.clone(), state.dimension) {
            Ok(idx) => idx,
            Err(e) => {
                return Err(SerializationError::Io(std::io::Error::other(
                    e.to_string(),
                )));
            }
        };

        // Capture counts before moving
        let vector_count = state.id_mapping.external_to_internal.len();

        // Restore the index state using public reconstruction method
        if let Err(e) = index.restore_from_state(
            state.id_mapping,
            state.layers,
            state.max_layer,
            state.entry_point,
            state.vectors,
            state.quantized_vectors,
            state.collection_config,
        ) {
            return Err(SerializationError::Io(std::io::Error::other(
                format!("Failed to restore HNSW state: {}", e),
            )));
        }

        info!(
            "HNSW deserialize_internal complete: {} vectors restored",
            vector_count
        );

        Ok(index)
    }
}

/// Extension trait for IVF serialization
impl SerializableIndex for UnifiedIvfIndex {
    fn index_type(&self) -> Index {
        Index::Ivf
    }

    fn serialize_to_bytes(&self, collection_id: &str) -> Result<Vec<u8>> {
        IndexSerializer::serialize_ivf(self, collection_id)
    }

    fn len(&self) -> usize {
        // This would call the actual IVF index method
        0 // Placeholder
    }

    fn dimension(&self) -> usize {
        // This would call the actual IVF index method
        0 // Placeholder
    }

    fn serialize_internal(&self) -> Result<Vec<u8>> {
        // Serialize IVF-specific data structures
        // This would include:
        // - Centroids
        // - Posting lists
        // - Vector assignments
        // - Training data if needed

        // For now, return placeholder
        Ok(vec![])
    }
}

/// Extension trait for IVF deserialization
impl UnifiedIvfIndex {
    fn deserialize_internal(_data: &[u8], config: &UnifiedIvfConfig) -> Result<Self> {
        // Deserialize IVF-specific data structures
        // This would reconstruct:
        // - Centroids
        // - Posting lists
        // - Vector assignments

        // For now, return placeholder with collection_id
        let collection_id = "default_collection".to_string(); // Placeholder collection ID
        match UnifiedIvfIndex::new(collection_id, config.clone()) {
            Ok(index) => Ok(index),
            Err(e) => Err(SerializationError::Io(std::io::Error::other(
                e.to_string(),
            ))),
        }
    }
}

/// Manager for delta updates
pub struct DeltaManager {
    /// Base checkpoint
    base_checkpoint: Option<IndexCheckpoint>,

    /// Accumulated deltas since checkpoint
    deltas: Vec<IndexDelta>,

    /// Maximum deltas before forcing new checkpoint
    max_deltas_before_checkpoint: usize,
}

impl DeltaManager {
    pub fn new(max_deltas: usize) -> Self {
        Self {
            base_checkpoint: None,
            deltas: Vec::new(),
            max_deltas_before_checkpoint: max_deltas,
        }
    }

    /// Add a delta operation
    pub fn add_delta(&mut self, operation: DeltaOperation) -> Option<IndexCheckpoint> {
        if let Some(ref checkpoint) = self.base_checkpoint {
            let delta = IndexDelta {
                base_checkpoint_id: checkpoint.checkpoint_id.clone(),
                delta_id: format!("delta_{}", unix_now_millis()),
                timestamp: unix_now_secs(),
                operations: vec![operation],
            };

            self.deltas.push(delta);

            // Check if we need a new checkpoint
            if self.deltas.len() >= self.max_deltas_before_checkpoint {
                debug!("Delta threshold reached, signaling for new checkpoint");
                // Return signal that new checkpoint is needed
                return Some(checkpoint.clone());
            }
        }

        None
    }

    /// Set new base checkpoint
    pub fn set_checkpoint(&mut self, checkpoint: IndexCheckpoint) {
        info!("Setting new base checkpoint: {}", checkpoint.checkpoint_id);
        self.base_checkpoint = Some(checkpoint);
        self.deltas.clear();
    }

    /// Get current deltas
    pub fn get_deltas(&self) -> &[IndexDelta] {
        &self.deltas
    }

    /// Apply all deltas to get current state
    pub fn reconstruct_current_state(&self) -> Result<Option<Vec<u8>>> {
        if let Some(ref checkpoint) = self.base_checkpoint {
            let mut current_data = checkpoint.index_data.clone();

            for delta in &self.deltas {
                current_data = IndexSerializer::apply_delta(checkpoint, delta)?;
            }

            Ok(Some(current_data))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::index::axis::*;
    use serde::{Deserialize, Serialize};

    /// Index type enum for testing
    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum Index {
        Hnsw,
        Ivf,
        Flat,
    }

    #[test]
    fn test_metadata_serialization() {
        let metadata = IndexMetadata {
            index_type: crate::index::axis::storage::serialization::Index::Hnsw,
            collection_id: "test_collection".to_string(),
            num_vectors: 1000,
            dimension: 128,
            timestamp: 1234567890,
            checksum: 0xDEADBEEF,
            is_delta: false,
            base_checkpoint_id: None,
            custom_metadata: None,
        };

        let serialized = bincode::serialize(&metadata).unwrap();
        let deserialized: IndexMetadata = bincode::deserialize(&serialized).unwrap();

        assert_eq!(metadata.index_type, deserialized.index_type);
        assert_eq!(metadata.collection_id, deserialized.collection_id);
        assert_eq!(metadata.num_vectors, deserialized.num_vectors);
        assert_eq!(metadata.checksum, deserialized.checksum);
    }

    #[test]
    fn test_delta_manager() {
        let mut manager = DeltaManager::new(5);

        let checkpoint = IndexCheckpoint {
            checkpoint_id: "test_checkpoint".to_string(),
            timestamp: 1234567890,
            index_data: vec![1, 2, 3, 4, 5],
            metadata: IndexMetadata {
                index_type: crate::index::axis::storage::serialization::Index::Hnsw,
                collection_id: "test".to_string(),
                num_vectors: 100,
                dimension: 128,
                timestamp: 1234567890,
                checksum: 12345,
                is_delta: false,
                base_checkpoint_id: None,
                custom_metadata: None,
            },
        };

        manager.set_checkpoint(checkpoint);

        // Add some deltas
        for i in 0..3 {
            let op = DeltaOperation::AddVectors {
                vectors: vec![(format!("vec_{}", i), vec![0.1; 128])],
            };
            assert!(manager.add_delta(op).is_none());
        }

        assert_eq!(manager.get_deltas().len(), 3);
    }
}
