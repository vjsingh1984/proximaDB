use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write, Seek};

// Import the consolidated RaptorFileMetadata from common
use super::common::{RaptorFileMetadata, SchemaDescriptor, KeyValue};

// Import more types from common
use super::common::{RowPageMetadata, HnswSegmentMetadata, BloomFilterMetadata, FieldDescriptor};

// REMOVED: CompressionCodec - duplicate of config.rs::CompressionCodec
// REMOVED: HnswSegmentMetadata - duplicate of common.rs::HnswSegmentMetadata  
// REMOVED: BloomFilterMetadata - duplicate of common.rs::BloomFilterMetadata

// Re-export from common and config for backward compatibility
pub use super::config::CompressionCodec;
pub use super::common::{HnswSegmentMetadata, BloomFilterMetadata};

/// B-tree index metadata (Artus-style) - UNIQUE to metadata.rs
/// Note: This is obsolete with Matrix Trinity approach but kept for compatibility
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BTreeIndexMetadata {
    pub root_offset: i64,
    pub height: u32,
    pub num_keys: i64,
    pub first_key: Vec<u8>,
    pub last_key: Vec<u8>,
}

impl Default for RaptorFileMetadata {
    fn default() -> Self {
        Self {
            version: 1,
            created_by: "ProximaDB RAPTOR v1.0".to_string(),
            created_at: chrono::Utc::now().timestamp(),
            file_path: String::new(),
            file_size: 0,
            total_rows: 0,
            total_vectors: 0,
            dimension: 0,
            collection_id: String::new(),
            row_groups: Vec::new(),
            num_rowgroups: 0,
            rowgroup_offsets: Vec::new(),
            rowgroup_sizes: Vec::new(),
            rowgroup_vector_counts: Vec::new(),
            schema: SchemaDescriptor {
                vector_dimension: 0,
                metadata_fields: Vec::new(),
                version: 1,
            },
            hnsw_metadata: None,
            global_hnsw_offset: 0,
            global_hnsw_size: 0,
            hnsw_entry_points: Vec::new(),
            hnsw_num_layers: 0,
            global_hnsw_entry: None,
            bloom_filter_metadata: None,
            compression_codec: "zstd".to_string(),
            compression_ratio: 0.0,
            cluster_centroids: Vec::new(),
            cluster_assignments: HashMap::new(),
            custom_metadata: HashMap::new(),
            key_value_metadata: Vec::new(),
            created_by: "ProximaDB RAPTOR v1.0".to_string(),
            footer_offset: 0,
            footer_size: 0,
            last_accessed: 0,
            locality_clusters: Vec::new(),
        }
    }
}

/// Writer and Reader helper methods
impl RaptorFileMetadata {
    /// Write footer to file (Parquet-style)
    pub fn write_footer<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        // Write file header magic (4 bytes) at the beginning if needed
        // This is written once at file creation, not here
        
        // Serialize metadata with bincode
        let metadata_bytes = bincode::serialize(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        
        // Write metadata
        writer.write_all(&metadata_bytes)?;
        
        // Write metadata length (4 bytes)
        writer.write_all(&(metadata_bytes.len() as u32).to_le_bytes())?;
        
        // Write footer magic "RPTR" (4 bytes)
        writer.write_all(&super::RAPTOR_MAGIC)?;
        
        Ok(())
    }
    
    /// Read footer from file (Parquet-style)
    pub fn read_footer<R: std::io::Read + std::io::Seek>(reader: &mut R) -> std::io::Result<Self> {
        use std::io::SeekFrom;
        
        // Seek to end - 8 bytes (length + magic)
        reader.seek(SeekFrom::End(-8))?;
        
        // Read metadata length
        let mut length_bytes = [0u8; 4];
        reader.read_exact(&mut length_bytes)?;
        let metadata_length = u32::from_le_bytes(length_bytes) as i64;
        
        // Read and verify footer magic
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if magic != super::RAPTOR_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid RAPTOR file: bad footer magic"
            ));
        }
        
        // Seek to metadata start
        reader.seek(SeekFrom::End(-8 - metadata_length))?;
        
        // Read metadata bytes
        let mut metadata_bytes = vec![0u8; metadata_length as usize];
        reader.read_exact(&mut metadata_bytes)?;
        
        // Deserialize with bincode
        bincode::deserialize(&metadata_bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
    
    /// Verify file header magic (should be called after opening file)
    pub fn verify_header_magic<R: std::io::Read>(reader: &mut R) -> std::io::Result<()> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if magic != super::RAPTOR_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid RAPTOR file: bad header magic"
            ));
        }
        Ok(())
    }
}