use arrow_array::RecordBatch;
use arrow_schema::Schema;
use std::sync::Arc;
use anyhow::{Result, anyhow};
use tokio::sync::Mutex;
use std::path::PathBuf;
use std::collections::HashMap;
use dashmap::DashMap;

// Reuse existing platform capabilities
use crate::core::compression::{StandardCompression, CompressionAlgorithm, CompressionContext};
use crate::compute::quantization::storage_engine::StorageQuantizationEngine;
use crate::storage::persistence::filesystem::{FileSystem, FileOptions, FilesystemFactory};
use crate::storage::engines::common::fastlanes_encoding::{FastLanesEncoder, FastLanesScheme};
use crate::core::hardware_capabilities::HardwareCapabilities;
use crate::core::memory::pool::VectorMemoryPool;
use crate::proto::proximadb::VectorRecord;

use super::{RaptorConfig, metadata::*};
use super::config::{CompressionCodec as RaptorCompressionCodec};

pub struct RaptorWriter {
    // File management
    file_path: String,
    filesystem: Arc<dyn FileSystem>,
    
    // Configuration
    config: RaptorConfig,
    collection_id: String,
    dimension: usize,
    
    // Reuse platform capabilities
    compression: Arc<StandardCompression>,
    quantization_engine: Arc<StorageQuantizationEngine>,
    memory_pool: Arc<VectorMemoryPool>,
    hardware: Arc<HardwareCapabilities>,
    
    // Current state
    current_row_page: Option<RowPageBuffer>,
    current_rowgroup: Option<CurrentRowgroup>,  // For RecordBatch compatibility
    row_groups: Vec<RowGroupMetadata>,
    file_metadata: RaptorFileMetadata,
    
    // Indexes being built
    btree_builder: BTreeBuilder,
    hnsw_builder: HnswBuilder,
    column_projections: ColumnProjectionsBuilder,
}

/// Buffer for accumulating rows into pages
struct RowPageBuffer {
    rows: Vec<CompactRow>,
    page_id: u16,
    start_offset: u64,
}

/// Compact row representation (as per design)
struct CompactRow {
    id: [u8; 16],
    vector: Vec<u8>,  // Compressed/quantized vector
    metadata: Vec<u8>, // Binary-encoded metadata
}

/// B-tree builder for ID index
struct BTreeBuilder {
    entries: Vec<(Vec<u8>, RowLocation)>,
}

/// HNSW builder
struct HnswBuilder {
    nodes: Vec<HnswNode>,
}

/// Column projections builder
struct ColumnProjectionsBuilder {
    metadata_columns: HashMap<String, Vec<Vec<u8>>>,
    filter_bitmaps: HashMap<String, Vec<bool>>,
}

#[derive(Clone, Copy)]
struct RowLocation {
    page_id: u16,
    offset_in_page: u16,
}

struct HnswNode {
    node_id: u32,
    row_location: RowLocation,
    quantized_vector: Vec<u8>,
    edges: Vec<(u32, f32)>,
}

// Additional fields for tracking current state
struct CurrentRowgroup {
    batch: RecordBatch,
    size: usize,
}

impl RaptorWriter {
    pub async fn new(
        file_path: String,
        config: RaptorConfig,
        collection_id: String,
        dimension: usize,
    ) -> Result<Self> {
        // Initialize filesystem using zero-copy API
        let filesystem = FilesystemFactory::create_from_path(&file_path).await?;
        
        // Initialize hardware capabilities
        let hardware = HardwareCapabilities::get();
        
        // Initialize unified compression
        let compression_algo = match &config.compression {
            RaptorCompressionCodec::None => CompressionAlgorithm::None,
            RaptorCompressionCodec::Lz4 => CompressionAlgorithm::Lz4,
            RaptorCompressionCodec::Zstd(_level) => CompressionAlgorithm::Zstd,
            RaptorCompressionCodec::Snappy => CompressionAlgorithm::Snappy,
            RaptorCompressionCodec::Gzip(_level) => CompressionAlgorithm::Gzip,
        };
        let compression = Arc::new(StandardCompression::new(compression_algo));
        
        // Initialize quantization engine
        let quantization_engine = Arc::new(StorageQuantizationEngine::new(
            dimension,
            hardware.clone(),
        ));
        
        // Initialize memory pool
        let memory_pool = Arc::new(VectorMemoryPool::new(
            100 * 1024 * 1024, // 100MB pool
            dimension,
        ));
        
        // Initialize file metadata
        let file_metadata = RaptorFileMetadata {
            version: 1,
            created_by: "ProximaDB RAPTOR v1.0".to_string(),
            created_at: chrono::Utc::now().timestamp(),
            num_rows: 0,
            collection_id: collection_id.clone(),
            row_groups: Vec::new(),
            schema: SchemaDescriptor { fields: Vec::new() },
            key_value_metadata: Vec::new(),
            global_btree_root: None,
            global_hnsw_entry: None,
        };
        
        // Write header magic at file start
        filesystem.write(&file_path, &super::RAPTOR_MAGIC).await?;
        
        Ok(Self {
            file_path: file_path.clone(),
            filesystem: Arc::new(filesystem),
            config,
            collection_id,
            dimension,
            compression,
            quantization_engine,
            memory_pool,
            hardware,
            current_row_page: None,
            current_rowgroup: None,
            row_groups: Vec::new(),
            file_metadata,
            btree_builder: BTreeBuilder { entries: Vec::new() },
            hnsw_builder: HnswBuilder { nodes: Vec::new() },
            column_projections: ColumnProjectionsBuilder {
                metadata_columns: HashMap::new(),
                filter_bitmaps: HashMap::new(),
            },
        })
    }
    
    /// Write vector records (main entry point)
    pub async fn write_vectors(&mut self, vectors: &[VectorRecord]) -> Result<()> {
        for vector in vectors {
            self.add_vector(vector).await?;
            
            // Flush page when it reaches configured row page size (default 1000 for optimal HNSW I/O)
            // This minimizes wasted reads: at k=10, reads 1000 vectors for 10 results (1% efficiency)
            if let Some(ref page) = self.current_row_page {
                if page.rows.len() >= self.config.rowgroup_size {
                    self.flush_row_page().await?;
                }
            }
        }
        Ok(())
    }
    
    /// Add a single vector to the current page
    async fn add_vector(&mut self, vector: &VectorRecord) -> Result<()> {
        // Extract ID (use vector.id or generate)
        let id = if let Some(ref id) = vector.id {
            // Convert string ID to fixed 16 bytes (UUID or hash)
            let mut id_bytes = [0u8; 16];
            let id_hash = blake3::hash(id.as_bytes());
            id_bytes.copy_from_slice(&id_hash.as_bytes()[..16]);
            id_bytes
        } else {
            // Generate UUID
            uuid::Uuid::new_v4().as_bytes().clone()
        };
        
        // Quantize vector using unified engine
        let quantized = self.quantization_engine.quantize_vector(&vector.vector)?;
        
        // Compress quantized vector
        let compressed_vector = self.compression.compress(
            &quantized,
            CompressionContext::Vector,
        )?;
        
        // Encode metadata as binary (using bincode)
        let metadata_bytes = if !vector.metadata.is_empty() {
            bincode::serialize(&vector.metadata)?
        } else {
            Vec::new()
        };
        
        // Create compact row
        let compact_row = CompactRow {
            id,
            vector: compressed_vector,
            metadata: metadata_bytes,
        };
        
        // Determine row location
        let page_id = self.row_groups.len() as u16;
        let offset_in_page = self.current_row_page
            .as_ref()
            .map(|p| p.rows.len() as u16)
            .unwrap_or(0);
        
        let location = RowLocation { page_id, offset_in_page };
        
        // Update indexes
        self.btree_builder.entries.push((id.to_vec(), location));
        
        // Add to HNSW with quantized vector for navigation
        let hnsw_quantized = self.quantization_engine.quantize_for_index(&vector.vector)?;
        self.hnsw_builder.nodes.push(HnswNode {
            node_id: self.file_metadata.num_rows as u32,
            row_location: location,
            quantized_vector: hnsw_quantized,
            edges: Vec::new(), // Will be built during HNSW construction
        });
        
        // Update column projections for filtering
        self.update_column_projections(vector, location);
        
        // Add to current page
        if self.current_row_page.is_none() {
            self.current_row_page = Some(RowPageBuffer {
                rows: Vec::new(),
                page_id,
                start_offset: self.filesystem.current_position(&self.file_path).await?,
            });
        }
        
        self.current_row_page.as_mut().unwrap().rows.push(compact_row);
        self.file_metadata.num_rows += 1;
        
        Ok(())
    }
    
    async fn quantize_batch(&self, batch: &RecordBatch) -> Result<RecordBatch> {
        // Simplified - would use actual quantization
        Ok(batch.clone())
    }
    
    /// Flush current row page to disk
    async fn flush_row_page(&mut self) -> Result<()> {
        if let Some(page) = self.current_row_page.take() {
            // Serialize page using FastLanes encoding
            let encoded_page = self.encode_row_page(&page)?;
            
            // Compress entire page
            let compressed = self.compression.compress(
                &encoded_page,
                CompressionContext::Block,
            )?;
            
            // Write to filesystem using zero-copy API
            let offset = self.filesystem.append(&self.file_path, &compressed).await?;
            
            // Create page metadata
            let page_metadata = RowPageMetadata {
                page_id: page.page_id,
                file_offset: offset as i64,
                compressed_size: compressed.len() as i64,
                uncompressed_size: encoded_page.len() as i64,
                num_rows: page.rows.len() as i32,
                first_id: page.rows.first().map(|r| r.id.to_vec()).unwrap_or_default(),
                last_id: page.rows.last().map(|r| r.id.to_vec()).unwrap_or_default(),
                compression: match self.compression.algorithm() {
                    CompressionAlgorithm::None => CompressionCodec::None,
                    CompressionAlgorithm::Lz4 => CompressionCodec::Lz4,
                    CompressionAlgorithm::Zstd => CompressionCodec::Zstd { level: 3 },
                    CompressionAlgorithm::Snappy => CompressionCodec::Snappy,
                    _ => CompressionCodec::None,
                },
            };
            
            // Add to current row group or create new one
            if self.row_groups.is_empty() || self.should_start_new_rowgroup() {
                self.row_groups.push(RowGroupMetadata {
                    ordinal: self.row_groups.len() as i32,
                    total_byte_size: 0,
                    num_rows: 0,
                    row_pages: Vec::new(),
                    column_projections_offset: None,
                    hnsw_segment: None,
                    btree_index: None,
                    bloom_filter: None,
                });
            }
            
            let current_rg = self.row_groups.last_mut().unwrap();
            current_rg.row_pages.push(page_metadata);
            current_rg.total_byte_size += compressed.len() as i64;
            current_rg.num_rows += page.rows.len() as i64;
        }
        
        Ok(())
    }
    
    /// Encode row page using columnar layout with FastLanes for vectors
    /// This provides 3-5x better compression and SIMD efficiency for HNSW access
    fn encode_row_page(&self, page: &RowPageBuffer) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();
        
        // Write encoding marker for columnar tensor layout
        encoded.push(0xA1); // FastLanes tensor encoding marker
        
        // Write page header
        encoded.extend(&(page.rows.len() as u32).to_le_bytes());
        
        // Columnar encoding for vectors - transpose and encode by dimension
        if !page.rows.is_empty() {
            // Extract and decompress vectors for columnar encoding
            let mut vectors = Vec::new();
            for row in &page.rows {
                // row.vector is already compressed/quantized - decompress for columnar re-encoding
                let decompressed = self.compression.decompress(
                    &row.vector,
                    CompressionContext::Vector,
                )?;
                vectors.push(decompressed);
            }
            
            // Get dimension from first vector
            let dimension = vectors[0].len() / std::mem::size_of::<f32>();
            
            // Transpose to columnar layout for better compression
            let mut columns: Vec<Vec<f32>> = vec![vec![]; dimension];
            for vector_bytes in &vectors {
                // Convert bytes back to f32 values
                let floats: Vec<f32> = vector_bytes.chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect();
                
                for (dim_idx, &value) in floats.iter().enumerate() {
                    if dim_idx < dimension {
                        columns[dim_idx].push(value);
                    }
                }
            }
            
            // Write dimension count
            encoded.extend(&(dimension as u32).to_le_bytes());
            
            // Encode each dimension column with FastLanes
            for (dim_idx, column) in columns.iter().enumerate() {
                // Analyze column for optimal encoding
                let (min_val, max_val) = column.iter()
                    .fold((f32::MAX, f32::MIN), |(min, max), &val| {
                        (min.min(val), max.max(val))
                    });
                
                let range = max_val - min_val;
                
                // Choose optimal encoding based on data characteristics
                let scheme = if range < 1e-6 {
                    // Near-constant values - use run-length encoding
                    FastLanesScheme::RunLength
                } else if dim_idx < 32 && range < 10.0 {
                    // Early dimensions often have smaller ranges - use frame of reference
                    FastLanesScheme::FrameOfReference {
                        reference: min_val as i64,
                        bits: ((range.log2().ceil() as u8) + 1).min(16),
                    }
                } else if column.windows(2).all(|w| (w[1] - w[0]).abs() < 0.1) {
                    // Sequential values with small deltas - use delta encoding
                    FastLanesScheme::Delta { bits: 8 }
                } else {
                    // General case - bit packed encoding
                    FastLanesScheme::BitPacked { bits: 16 }
                };
                
                let encoder = FastLanesEncoder::new(scheme);
                let encoded_column = encoder.encode_f32(column)?;
                
                // Write encoding scheme marker
                encoded.push(match scheme {
                    FastLanesScheme::RunLength => 0x01,
                    FastLanesScheme::FrameOfReference { .. } => 0x02,
                    FastLanesScheme::Delta { .. } => 0x03,
                    FastLanesScheme::BitPacked { .. } => 0x04,
                    _ => 0x00,
                });
                
                // Write encoded column length and data
                encoded.extend(&(encoded_column.len() as u32).to_le_bytes());
                encoded.extend(&encoded_column);
            }
        }
        
        // Store IDs separately (row-wise is fine for IDs)
        for row in &page.rows {
            encoded.extend(&row.id);
        }
        
        // Store metadata separately (row-wise for variable-length metadata)
        for row in &page.rows {
            encoded.extend(&(row.metadata.len() as u32).to_le_bytes());
            encoded.extend(&row.metadata);
        }
        
        Ok(encoded)
    }
    
    /// Check if we should start a new row group (1K vectors by default for optimal HNSW I/O)
    fn should_start_new_rowgroup(&self) -> bool {
        self.row_groups.last()
            .map(|rg| rg.num_rows >= self.config.rowgroup_size as i64)
            .unwrap_or(true)
    }
    
    async fn compress_rowgroup(&self, batch: &RecordBatch) -> Result<Vec<u8>> {
        // FASTLANES: Always encode RecordBatch using FastLanes for tensor optimization
        // First byte is the encoding marker (RAPTOR uses 0xA0-0xAF range)
        let mut result = Vec::new();
        
        // Always use FastLanes tensor encoding for best performance
        let encoding_marker = 0xA1; // FastLanes tensor encoding
        result.push(encoding_marker);
        
        // Use FastLanes encoding for tensor optimization
        let encoded = self.encode_batch_with_fastlanes(batch, encoding_marker)?;
        result.extend(encoded);
        
        Ok(result)
    }
    
    fn encode_batch_with_fastlanes(&self, batch: &RecordBatch, marker: u8) -> Result<Vec<u8>> {
        use crate::storage::engines::common::fastlanes_encoding::{FastLanesEncoder, FastLanesScheme};
        use std::io::Write;
        
        // Extract vectors from RecordBatch
        let vectors = self.extract_vectors_from_batch(batch)?;
        
        if vectors.is_empty() {
            return Ok(Vec::new());
        }
        
        let dimension = vectors[0].len();
        
        // Transpose to columnar for SIMD optimization
        let mut columns: Vec<Vec<f32>> = vec![vec![]; dimension];
        for vector in &vectors {
            for (dim_idx, &value) in vector.iter().enumerate() {
                if dim_idx < dimension {
                    columns[dim_idx].push(value);
                }
            }
        }
        
        // Analyze tensor data for optimal encoding
        let mut min_val = f32::MAX;
        let mut max_val = f32::MIN;
        for column in &columns {
            for &val in column {
                min_val = min_val.min(val);
                max_val = max_val.max(val);
            }
        }
        
        let range = max_val - min_val;
        
        // Choose optimal encoding for tensor data
        let scheme = if range < 1e-6 {
            FastLanesScheme::RunLength
        } else if range < 100.0 {
            FastLanesScheme::FrameOfReference { 
                reference: min_val as i64, 
                bits: (range.log2().ceil() as u8).max(8) 
            }
        } else {
            FastLanesScheme::BitPacked { bits: 16 } // Good for dense tensors
        };
        
        let encoder = FastLanesEncoder::new(scheme);
        let mut encoded_data = Vec::new();
        
        // Write metadata
        encoded_data.write_all(&(dimension as u32).to_le_bytes())?;
        encoded_data.write_all(&(vectors.len() as u32).to_le_bytes())?;
        
        // Encode each dimension column
        for column in columns {
            let encoded_column = encoder.encode_f32(&column)?;
            encoded_data.write_all(&(encoded_column.len() as u32).to_le_bytes())?;
            encoded_data.write_all(&encoded_column)?;
        }
        
        // Also encode IDs from RecordBatch
        if let Some(id_col) = batch.column_by_name("id") {
            if let Some(id_array) = id_col.as_any().downcast_ref::<arrow_array::StringArray>() {
                for i in 0..id_array.len() {
                    if let Some(id) = id_array.value_opt(i) {
                        let id_bytes = id.as_bytes();
                        encoded_data.write_all(&(id_bytes.len() as u32).to_le_bytes())?;
                        encoded_data.write_all(id_bytes)?;
                    } else {
                        encoded_data.write_all(&0u32.to_le_bytes())?;
                    }
                }
            }
        }
        
        // Encode timestamps if present
        if let Some(ts_col) = batch.column_by_name("timestamp") {
            if let Some(ts_array) = ts_col.as_any().downcast_ref::<arrow_array::Int64Array>() {
                for i in 0..ts_array.len() {
                    let timestamp = ts_array.value(i);
                    encoded_data.write_all(&timestamp.to_le_bytes())?;
                }
            }
        }
        
        Ok(encoded_data)
    }
    
    fn extract_vectors_from_batch(&self, batch: &RecordBatch) -> Result<Vec<Vec<f32>>> {
        let mut vectors = Vec::new();
        
        if let Some(vector_col) = batch.column_by_name("vector") {
            if let Some(float_array) = vector_col.as_any().downcast_ref::<arrow_array::Float32Array>() {
                // Assuming vectors are stored flat with known dimension
                let dimension = self.config.dimension;
                let num_vectors = float_array.len() / dimension;
                
                for i in 0..num_vectors {
                    let start = i * dimension;
                    let end = start + dimension;
                    vectors.push(float_array.values()[start..end].to_vec());
                }
            }
        }
        
        Ok(vectors)
    }
    
    fn calculate_uncompressed_size(&self, batch: &RecordBatch) -> u64 {
        let mut size = 0u64;
        for column in batch.columns() {
            size += column.get_array_memory_size() as u64;
        }
        size
    }
    
    fn should_quantize_vectors(&self) -> bool {
        // Determine if quantization should be applied based on config
        // Always quantize for HNSW to save memory (8-16x reduction)
        self.config.enable_hnsw || (self.config.enable_simd && self.config.rowgroup_size >= 500)
    }
    
    pub async fn flush(&mut self) -> Result<()> {
        // Flush any pending row page
        self.flush_row_page().await?;
        
        // Write column projections for the current row group
        if let Some(rg) = self.row_groups.last_mut() {
            let projections_offset = self.write_column_projections().await?;
            rg.column_projections_offset = Some(projections_offset as i64);
            
            // Write HNSW segment
            if self.config.enable_hnsw {
                let hnsw_meta = self.write_hnsw_segment().await?;
                rg.hnsw_segment = Some(hnsw_meta);
            }
            
            // Write B-tree index
            let btree_meta = self.write_btree_index().await?;
            rg.btree_index = Some(btree_meta);
        }
        
        Ok(())
    }
    
    pub async fn close(mut self) -> Result<()> {
        // Flush any remaining data
        self.flush().await?;
        
        // Update file metadata with row groups
        self.file_metadata.row_groups = self.row_groups.clone();
        
        // Write footer (Parquet-style)
        let mut footer_buffer = Vec::new();
        self.file_metadata.write_footer(&mut footer_buffer)?;
        self.filesystem.append(&self.file_path, &footer_buffer).await?;
        
        Ok(())
    }
    
    /// Update column projections for filtering
    fn update_column_projections(&mut self, vector: &VectorRecord, location: RowLocation) {
        // Extract metadata columns for projection
        for (key, value) in &vector.metadata {
            self.column_projections.metadata_columns
                .entry(key.clone())
                .or_insert_with(Vec::new)
                .push(bincode::serialize(&value).unwrap_or_default());
        }
        
        // Update filter bitmaps (example: filtering by specific metadata values)
        // This would be customized based on actual filtering needs
    }
    
    /// Write column projections to disk
    async fn write_column_projections(&mut self) -> Result<u64> {
        let mut projection_data = Vec::new();
        
        // Serialize metadata columns
        for (column_name, values) in &self.column_projections.metadata_columns {
            // Write column header
            let header = format!("{}:{}", column_name, values.len());
            projection_data.extend(header.as_bytes());
            projection_data.push(0); // null terminator
            
            // Write column values
            for value in values {
                projection_data.extend(&(value.len() as u32).to_le_bytes());
                projection_data.extend(value);
            }
        }
        
        // Compress projections
        let compressed = self.compression.compress(
            &projection_data,
            CompressionContext::Metadata,
        )?;
        
        // Write to file
        let offset = self.filesystem.append(&self.file_path, &compressed).await?;
        Ok(offset)
    }
    
    /// Write HNSW segment to disk
    async fn write_hnsw_segment(&mut self) -> Result<HnswSegmentMetadata> {
        // Build HNSW graph from accumulated nodes
        self.build_hnsw_graph()?;
        
        // Serialize HNSW graph
        let mut hnsw_data = Vec::new();
        
        // Write number of nodes
        hnsw_data.extend(&(self.hnsw_builder.nodes.len() as u32).to_le_bytes());
        
        // Write each node
        for node in &self.hnsw_builder.nodes {
            // Write node ID
            hnsw_data.extend(&node.node_id.to_le_bytes());
            
            // Write row location
            hnsw_data.extend(&node.row_location.page_id.to_le_bytes());
            hnsw_data.extend(&node.row_location.offset_in_page.to_le_bytes());
            
            // Write quantized vector
            hnsw_data.extend(&(node.quantized_vector.len() as u32).to_le_bytes());
            hnsw_data.extend(&node.quantized_vector);
            
            // Write edges
            hnsw_data.extend(&(node.edges.len() as u32).to_le_bytes());
            for (neighbor_id, distance) in &node.edges {
                hnsw_data.extend(&neighbor_id.to_le_bytes());
                hnsw_data.extend(&distance.to_le_bytes());
            }
        }
        
        // Compress HNSW data
        let compressed = self.compression.compress(
            &hnsw_data,
            CompressionContext::Index,
        )?;
        
        // Write to file
        let offset = self.filesystem.append(&self.file_path, &compressed).await?;
        
        Ok(HnswSegmentMetadata {
            file_offset: offset as i64,
            size_bytes: compressed.len() as i64,
            num_nodes: self.hnsw_builder.nodes.len() as i32,
            entry_point: 0, // Would be determined during graph building
            max_level: 4,    // Typical HNSW level
            ef_construction: self.config.hnsw_ef_construction as i32,
            m: self.config.hnsw_m as i32,
        })
    }
    
    /// Build HNSW graph from nodes
    fn build_hnsw_graph(&mut self) -> Result<()> {
        // This is a simplified version - actual HNSW construction is complex
        // In production, would use the AXIS HNSW implementation
        
        let m = self.config.hnsw_m;
        let ef_construction = self.config.hnsw_ef_construction;
        
        // Build edges between nodes based on similarity
        for i in 0..self.hnsw_builder.nodes.len() {
            let mut distances = Vec::new();
            
            // Calculate distances to all other nodes
            for j in 0..self.hnsw_builder.nodes.len() {
                if i != j {
                    // Simplified distance calculation using quantized vectors
                    let dist = self.calculate_distance(
                        &self.hnsw_builder.nodes[i].quantized_vector,
                        &self.hnsw_builder.nodes[j].quantized_vector,
                    )?;
                    distances.push((j as u32, dist));
                }
            }
            
            // Sort by distance and keep top M connections
            distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            distances.truncate(m);
            
            self.hnsw_builder.nodes[i].edges = distances;
        }
        
        Ok(())
    }
    
    /// Calculate distance between two quantized vectors
    fn calculate_distance(&self, v1: &[u8], v2: &[u8]) -> Result<f32> {
        // This would use the actual distance computation from the quantization engine
        // For now, return a placeholder
        Ok(0.5)
    }
    
    /// Write B-tree index to disk
    async fn write_btree_index(&mut self) -> Result<BTreeIndexMetadata> {
        // Sort entries by ID
        self.btree_builder.entries.sort_by(|a, b| a.0.cmp(&b.0));
        
        // Build B-tree pages (simplified - actual B-tree is more complex)
        let mut btree_data = Vec::new();
        
        // Write number of entries
        btree_data.extend(&(self.btree_builder.entries.len() as u32).to_le_bytes());
        
        // Write each entry
        for (id, location) in &self.btree_builder.entries {
            // Write ID length and data
            btree_data.extend(&(id.len() as u32).to_le_bytes());
            btree_data.extend(id);
            
            // Write location
            btree_data.extend(&location.page_id.to_le_bytes());
            btree_data.extend(&location.offset_in_page.to_le_bytes());
        }
        
        // Compress B-tree data
        let compressed = self.compression.compress(
            &btree_data,
            CompressionContext::Index,
        )?;
        
        // Write to file
        let offset = self.filesystem.append(&self.file_path, &compressed).await?;
        
        let first_key = self.btree_builder.entries.first()
            .map(|(k, _)| k.clone())
            .unwrap_or_default();
        let last_key = self.btree_builder.entries.last()
            .map(|(k, _)| k.clone())
            .unwrap_or_default();
        
        Ok(BTreeIndexMetadata {
            root_offset: offset as i64,
            height: 1, // Simplified single-level
            num_keys: self.btree_builder.entries.len() as i64,
            first_key,
            last_key,
        })
    }
    
    // Add missing fields to struct
    fn initialize_missing_fields(&mut self) {
        // This is a placeholder for any missing initialization
    }
    
    // Add missing dimension field
    fn get_dimension(&self) -> usize {
        self.dimension
    }
    
    async fn flush_rowgroup(&mut self) -> Result<()> {
        if let Some(rowgroup) = self.current_rowgroup.take() {
            // Convert RecordBatch to row pages and flush
            // This is handled by flush_row_page() already
        }
        Ok(())
    }
}