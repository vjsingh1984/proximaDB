use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write, Seek};

/// RAPTOR file footer metadata (Parquet-style)
/// This is stored at the END of the file like Parquet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorFileMetadata {
    /// File format version
    pub version: i32,
    /// Creator string (e.g., "ProximaDB RAPTOR v1.0")
    pub created_by: String,
    /// Creation timestamp
    pub created_at: i64,
    /// Total number of rows/vectors
    pub num_rows: i64,
    /// Collection ID
    pub collection_id: String,
    /// List of row groups in the file
    pub row_groups: Vec<RowGroupMetadata>,
    /// Schema descriptor
    pub schema: SchemaDescriptor,
    /// Key-value metadata
    pub key_value_metadata: Vec<KeyValue>,
    
    // RAPTOR/Artus extensions
    /// Global B-tree root for ID lookups across row groups
    pub global_btree_root: Option<i64>,
    /// Global HNSW entry point for similarity search
    pub global_hnsw_entry: Option<i32>,
}

/// Row group metadata (stored in footer)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowGroupMetadata {
    pub ordinal: i32,
    pub total_byte_size: i64,
    pub num_rows: i64,
    pub row_pages: Vec<RowPageMetadata>,
    pub column_projections_offset: Option<i64>,
    pub hnsw_segment: Option<HnswSegmentMetadata>,
    pub btree_index: Option<BTreeIndexMetadata>,
    pub bloom_filter: Option<BloomFilterMetadata>,
}

/// Row page metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowPageMetadata {
    pub page_id: u16,
    pub file_offset: i64,
    pub compressed_size: i64,
    pub uncompressed_size: i64,
    pub num_rows: i32,
    pub first_id: Vec<u8>,
    pub last_id: Vec<u8>,
    pub compression: CompressionCodec,
}

/// Schema descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaDescriptor {
    pub fields: Vec<FieldDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDescriptor {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub metadata: Vec<KeyValue>,
    // Vector field extensions
    pub dimension: Option<i32>,
    pub distance_metric: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: Option<String>,
}

/// Compression codec enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionCodec {
    None,
    Lz4,
    Zstd { level: i32 },
    Snappy,
}

/// B-tree index metadata (Artus-style)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BTreeIndexMetadata {
    pub root_offset: i64,
    pub height: u32,
    pub num_keys: i64,
    pub first_key: Vec<u8>,
    pub last_key: Vec<u8>,
}

/// HNSW segment metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswSegmentMetadata {
    pub file_offset: i64,
    pub size_bytes: i64,
    pub num_nodes: i32,
    pub entry_point: i32,
    pub max_level: i32,
    pub ef_construction: i32,
    pub m: i32,
}

/// Bloom filter metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterMetadata {
    pub file_offset: i64,
    pub size_bytes: i64,
    pub num_bits: i64,
    pub num_hashes: i32,
    pub false_positive_rate: f64,
}

impl Default for RaptorFileMetadata {
    fn default() -> Self {
        Self {
            version: 1,
            created_by: "ProximaDB RAPTOR v1.0".to_string(),
            created_at: chrono::Utc::now().timestamp(),
            num_rows: 0,
            collection_id: String::new(),
            row_groups: Vec::new(),
            schema: SchemaDescriptor { fields: Vec::new() },
            key_value_metadata: Vec::new(),
            global_btree_root: None,
            global_hnsw_entry: None,
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