// RAPTOR Row Group Manager - Hybrid Row Group + Columnar Architecture
// Manages row groups with columnar storage within each group for SIMD optimization

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::core::VectorRecord;
use crate::compute::quantization::storage_engine::StorageQuantizationEngine;
use crate::storage::engines::common::fastlanes_encoding::FastLanesEncoder;
use super::smart_rowgroup_sizing::{SmartRowGroupSizer, OptimalRowGroupSize};
use super::config::RaptorConfig;
use super::common::RowGroupMetadata;

/// Hybrid Row Group containing columnar data for SIMD optimization
#[derive(Debug, Clone)]
pub struct HybridRowGroup {
    /// Row group identifier
    pub id: Uuid,
    /// Number of vectors in this row group
    pub vector_count: usize,
    /// Maximum vectors this row group can hold (from smart sizing)
    pub max_vectors: usize,
    /// Columnar storage within the row group
    pub columnar_data: ColumnarBlock,
    /// HNSW graph for this row group (local graph)
    pub local_hnsw: Option<LocalHnswGraph>,
    /// Row group metadata
    pub metadata: RowGroupMetadata,
}

/// Columnar storage block within a row group (dimension-major format)
#[derive(Debug, Clone)]
pub struct ColumnarBlock {
    /// Vector IDs (for mapping back to original)
    pub vector_ids: Vec<String>,
    /// Transposed vectors - each dimension is a separate array for SIMD
    pub transposed_vectors: TransposedVectors,
    /// FastLanes encoded data for compression
    pub fastlanes_data: Option<FastLanesEncodedData>,
    /// Quantized vectors if quantization is enabled
    pub quantized_data: Option<QuantizedColumnarData>,
    /// Metadata for each vector
    pub metadata_columns: MetadataColumns,
}

/// Dimension-major vector storage for SIMD operations
#[derive(Debug, Clone)]
pub struct TransposedVectors {
    /// Each Vec<f32> represents one dimension across all vectors
    /// dimensions[d] = [v0[d], v1[d], v2[d], ...] for dimension d
    pub dimensions: Vec<Vec<f32>>,
    /// Vector dimension
    pub dimension: usize,
    /// Number of vectors
    pub vector_count: usize,
}

/// FastLanes encoded columnar data
#[derive(Debug, Clone)]
pub struct FastLanesEncodedData {
    /// Encoded dimensions using FastLanes compression
    pub encoded_dimensions: Vec<Vec<u8>>,
    /// Encoding scheme used for each dimension
    pub encoding_schemes: Vec<FastLanesScheme>,
    /// Compression ratio achieved
    pub compression_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FastLanesScheme {
    BitPacked { bits: u8 },
    Delta { base: f32, bits: u8 },
    Dictionary { dict_size: usize },
    RLE,
}

/// Quantized columnar data
#[derive(Debug, Clone)]
pub struct QuantizedColumnarData {
    /// Binary quantized vectors (1 bit per dimension)
    pub binary: Option<Vec<Vec<u8>>>,
    /// INT8 quantized vectors (8 bits per dimension)
    pub int8: Option<Vec<Vec<i8>>>,
    /// PQ4 quantized vectors (4 bits per dimension)
    pub pq4: Option<Vec<Vec<u8>>>,
    /// PQ8 quantized vectors (8 bits per dimension)
    pub pq8: Option<Vec<Vec<u8>>>,
    /// Quantization parameters
    pub quantization_params: QuantizationParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationParams {
    pub scale: f32,
    pub offset: f32,
    pub codebook: Option<Vec<Vec<f32>>>, // For PQ quantization
}

/// Metadata stored in columnar format
#[derive(Debug, Clone)]
pub struct MetadataColumns {
    /// String metadata columns
    pub string_columns: HashMap<String, Vec<Option<String>>>,
    /// Numeric metadata columns
    pub numeric_columns: HashMap<String, Vec<Option<f64>>>,
    /// Boolean metadata columns
    pub boolean_columns: HashMap<String, Vec<Option<bool>>>,
}

/// Local HNSW graph for a single row group
#[derive(Debug, Clone)]
pub struct LocalHnswGraph {
    /// Graph nodes (vector IDs within this row group)
    pub nodes: Vec<GraphNode>,
    /// Entry point for search
    pub entry_point: Option<usize>,
    /// Graph parameters
    pub m: usize, // Max connections per node
    pub ml: f32,  // Level factor
}

#[derive(Debug, Clone)]
pub struct GraphNode {
    pub local_id: usize,        // Index within this row group
    pub global_id: String,      // Original vector ID
    pub level: usize,           // HNSW level
    pub connections: Vec<Vec<usize>>, // Connections per level
}

/// Row Group Manager for RAPTOR engine
pub struct RowGroupManager {
    /// Active row groups
    row_groups: HashMap<Uuid, HybridRowGroup>,
    /// Current row group being written to
    current_row_group: Option<Uuid>,
    /// Smart sizing configuration
    smart_sizer: SmartRowGroupSizer,
    /// Optimal row group size calculated from smart sizer
    optimal_size: OptimalRowGroupSize,
    /// Quantization engine if enabled
    quantization_engine: Option<Arc<StorageQuantizationEngine>>,
    /// FastLanes encoder
    fastlanes_encoder: FastLanesEncoder,
    /// Configuration
    config: RaptorConfig,
}

impl RowGroupManager {
    /// Create new row group manager with smart sizing
    pub fn new(
        config: RaptorConfig,
        smart_sizer: SmartRowGroupSizer,
        quantization_engine: Option<Arc<StorageQuantizationEngine>>,
    ) -> Result<Self> {
        let optimal_size = smart_sizer.calculate_optimal_rowgroup_size()?;
        
        tracing::info!(
            "RAPTOR RowGroupManager initialized: {}",
            optimal_size.rationale
        );
        
        let fastlanes_encoder = FastLanesEncoder::new(
            crate::storage::engines::common::fastlanes_encoding::FastLanesScheme::BitPacked { bits: 16 }
        );
        
        Ok(Self {
            row_groups: HashMap::new(),
            current_row_group: None,
            smart_sizer,
            optimal_size,
            quantization_engine,
            fastlanes_encoder,
            config,
        })
    }
    
    /// Add vectors to the current row group (creates new if needed)
    pub async fn add_vectors(&mut self, vectors: Vec<VectorRecord>) -> Result<Vec<Uuid>> {
        let mut row_group_ids = Vec::new();
        let mut remaining_vectors = vectors;
        
        while !remaining_vectors.is_empty() {
            // Get or create current row group
            let row_group_id = self.get_or_create_current_row_group().await?;
            let row_group = self.row_groups.get_mut(&row_group_id)
                .ok_or_else(|| anyhow::anyhow!("Row group not found"))?;
            
            // Calculate how many vectors can fit
            let available_space = row_group.max_vectors - row_group.vector_count;
            let vectors_to_add = remaining_vectors.len().min(available_space);
            
            if vectors_to_add == 0 {
                // Current row group is full, mark it as complete and continue
                self.complete_current_row_group().await?;
                continue;
            }
            
            // Split vectors
            let batch: Vec<VectorRecord> = remaining_vectors.drain(..vectors_to_add).collect();
            
            // Add to row group
            self.add_vectors_to_row_group(row_group_id, batch).await?;
            row_group_ids.push(row_group_id);
            
            // Check if row group is now full
            let row_group = self.row_groups.get(&row_group_id).unwrap();
            if row_group.vector_count >= row_group.max_vectors {
                self.complete_current_row_group().await?;
            }
        }
        
        Ok(row_group_ids)
    }
    
    /// Get or create the current row group for writing
    async fn get_or_create_current_row_group(&mut self) -> Result<Uuid> {
        match self.current_row_group {
            Some(id) => Ok(id),
            None => {
                let new_id = self.create_new_row_group().await?;
                self.current_row_group = Some(new_id);
                Ok(new_id)
            }
        }
    }
    
    /// Create a new row group with optimal sizing
    async fn create_new_row_group(&mut self) -> Result<Uuid> {
        let id = Uuid::new_v4();
        let max_vectors = self.optimal_size.vectors_per_rowgroup;
        
        let row_group = HybridRowGroup {
            id,
            vector_count: 0,
            max_vectors,
            columnar_data: ColumnarBlock {
                vector_ids: Vec::with_capacity(max_vectors),
                transposed_vectors: TransposedVectors {
                    dimensions: Vec::new(),
                    dimension: 0,
                    vector_count: 0,
                },
                fastlanes_data: None,
                quantized_data: None,
                metadata_columns: MetadataColumns {
                    string_columns: HashMap::new(),
                    numeric_columns: HashMap::new(),
                    boolean_columns: HashMap::new(),
                },
            },
            local_hnsw: None,
            metadata: RowGroupMetadata::default(),
        };
        
        self.row_groups.insert(id, row_group);
        
        tracing::debug!(
            "Created new row group {} with capacity {} vectors ({:.1}MB estimated)",
            id,
            max_vectors,
            self.optimal_size.total_rowgroup_bytes as f32 / (1024.0 * 1024.0)
        );
        
        Ok(id)
    }
    
    /// Add vectors to a specific row group
    async fn add_vectors_to_row_group(&mut self, row_group_id: Uuid, vectors: Vec<VectorRecord>) -> Result<()> {
        let row_group = self.row_groups.get_mut(&row_group_id)
            .ok_or_else(|| anyhow::anyhow!("Row group not found"))?;
        
        // Initialize transposed vectors if first batch
        if row_group.columnar_data.transposed_vectors.dimensions.is_empty() && !vectors.is_empty() {
            let dimension = vectors[0].vector.len();
            row_group.columnar_data.transposed_vectors.dimension = dimension;
            row_group.columnar_data.transposed_vectors.dimensions = 
                vec![Vec::with_capacity(row_group.max_vectors); dimension];
        }
        
        // Add vectors in transposed (columnar) format
        for vector in vectors {
            // Add vector ID
            row_group.columnar_data.vector_ids.push(
                vector.id.clone().unwrap_or_else(|| format!("vec_{}", row_group.vector_count))
            );
            
            // Add vector data (transpose: vector[d] -> dimensions[d].push(value))
            for (dim_idx, value) in vector.vector.iter().enumerate() {
                if dim_idx < row_group.columnar_data.transposed_vectors.dimensions.len() {
                    row_group.columnar_data.transposed_vectors.dimensions[dim_idx].push(*value);
                }
            }
            
            // Add metadata (columnar format)
            if !vector.metadata.is_empty() {
                self.add_metadata_columnar(&mut row_group.columnar_data.metadata_columns, vector.metadata)?;
            } else {
                // Add empty metadata
                self.add_empty_metadata_columnar(&mut row_group.columnar_data.metadata_columns)?;
            }
            
            row_group.vector_count += 1;
            row_group.columnar_data.transposed_vectors.vector_count += 1;
        }
        
        Ok(())
    }
    
    /// Add metadata in columnar format
    fn add_metadata_columnar(
        &self,
        metadata_columns: &mut MetadataColumns,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        // Process each metadata field
        for (key, value) in metadata {
            match value {
                serde_json::Value::String(s) => {
                    metadata_columns.string_columns
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(Some(s));
                }
                serde_json::Value::Number(n) => {
                    metadata_columns.numeric_columns
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(n.as_f64());
                }
                serde_json::Value::Bool(b) => {
                    metadata_columns.boolean_columns
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(Some(b));
                }
                _ => {
                    // Serialize complex types as strings
                    metadata_columns.string_columns
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(Some(value.to_string()));
                }
            }
        }
        
        Ok(())
    }
    
    /// Add empty metadata entries to maintain column alignment
    fn add_empty_metadata_columnar(&self, metadata_columns: &mut MetadataColumns) -> Result<()> {
        // Add None to all existing columns to maintain alignment
        for column in metadata_columns.string_columns.values_mut() {
            column.push(None);
        }
        for column in metadata_columns.numeric_columns.values_mut() {
            column.push(None);
        }
        for column in metadata_columns.boolean_columns.values_mut() {
            column.push(None);
        }
        
        Ok(())
    }
    
    /// Complete the current row group (apply compression, quantization, build HNSW)
    async fn complete_current_row_group(&mut self) -> Result<()> {
        if let Some(row_group_id) = self.current_row_group.take() {
            tracing::info!("Completing row group {} with {} vectors", row_group_id, 
                          self.row_groups.get(&row_group_id).map(|rg| rg.vector_count).unwrap_or(0));
            
            // Apply FastLanes compression
            self.apply_fastlanes_compression(row_group_id).await?;
            
            // Apply quantization if enabled
            if self.quantization_engine.is_some() {
                self.apply_quantization(row_group_id).await?;
            }
            
            // Build local HNSW graph
            self.build_local_hnsw(row_group_id).await?;
        }
        
        Ok(())
    }
    
    /// Apply FastLanes compression to a row group
    async fn apply_fastlanes_compression(&mut self, row_group_id: Uuid) -> Result<()> {
        let row_group = self.row_groups.get_mut(&row_group_id)
            .ok_or_else(|| anyhow::anyhow!("Row group not found"))?;
        
        let mut encoded_dimensions = Vec::new();
        let mut encoding_schemes = Vec::new();
        
        // Compress each dimension separately using FastLanes
        for dimension_data in &row_group.columnar_data.transposed_vectors.dimensions {
            if !dimension_data.is_empty() {
                let encoded = self.fastlanes_encoder.encode_f32(dimension_data)?;
                encoded_dimensions.push(encoded);
                encoding_schemes.push(FastLanesScheme::BitPacked { bits: 16 }); // Default scheme
            }
        }
        
        // Calculate compression ratio
        let original_size = row_group.columnar_data.transposed_vectors.dimensions.len() * 
                           row_group.vector_count * 4; // 4 bytes per f32
        let compressed_size: usize = encoded_dimensions.iter().map(|d| d.len()).sum();
        let compression_ratio = original_size as f32 / compressed_size.max(1) as f32;
        
        row_group.columnar_data.fastlanes_data = Some(FastLanesEncodedData {
            encoded_dimensions,
            encoding_schemes,
            compression_ratio,
        });
        
        tracing::debug!("FastLanes compression: {:.2}x ratio for row group {}", 
                       compression_ratio, row_group_id);
        
        Ok(())
    }
    
    /// Apply quantization to a row group
    async fn apply_quantization(&mut self, row_group_id: Uuid) -> Result<()> {
        if let Some(ref quantization_engine) = self.quantization_engine {
            let row_group = self.row_groups.get_mut(&row_group_id)
                .ok_or_else(|| anyhow::anyhow!("Row group not found"))?;
            
            // Reconstruct vectors for quantization (transpose back)
            let mut vectors = Vec::new();
            for i in 0..row_group.vector_count {
                let mut vector = Vec::with_capacity(row_group.columnar_data.transposed_vectors.dimension);
                for dim_data in &row_group.columnar_data.transposed_vectors.dimensions {
                    if i < dim_data.len() {
                        vector.push(dim_data[i]);
                    }
                }
                vectors.push(vector);
            }
            
            // Apply quantization
            let quantized = quantization_engine.quantize_batch(&vectors, None).await?;
            
            // Store quantized data in columnar format
            // Note: quantized is Vec<StorageQuantizedData>, need to extract data appropriately
            // For now, create empty structure as the exact mapping needs clarification
            row_group.columnar_data.quantized_data = Some(QuantizedColumnarData {
                binary: Vec::new(), // Would extract from quantized[*].filter
                int8: Vec::new(),   // Would extract from quantized[*].fast
                pq4: Vec::new(),    // Would extract from quantized[*].primary if PQ4
                pq8: Vec::new(),    // Would extract from quantized[*].primary if PQ8
                quantization_params: QuantizationParams {
                    scale: 1.0,
                    offset: 0.0,
                    codebook: Vec::new(), // Would extract from quantization metadata
                },
            });
            
            tracing::debug!("Applied quantization to row group {}", row_group_id);
        }
        
        Ok(())
    }
    
    /// Build local HNSW graph for a row group
    async fn build_local_hnsw(&mut self, row_group_id: Uuid) -> Result<()> {
        // Placeholder for HNSW construction
        // This would integrate with the existing HNSW implementation
        tracing::debug!("Building local HNSW for row group {}", row_group_id);
        Ok(())
    }
    
    /// Get row group by ID
    pub fn get_row_group(&self, id: &Uuid) -> Option<&HybridRowGroup> {
        self.row_groups.get(id)
    }
    
    /// Get all row group IDs
    pub fn get_row_group_ids(&self) -> Vec<Uuid> {
        self.row_groups.keys().cloned().collect()
    }
    
    /// Get smart sizing configuration
    pub fn get_smart_sizing_info(&self) -> &OptimalRowGroupSize {
        &self.optimal_size
    }
    
    /// Update smart sizing (recalculate optimal row group size)
    pub fn update_smart_sizing(&mut self, new_sizer: SmartRowGroupSizer) -> Result<()> {
        self.smart_sizer = new_sizer;
        self.optimal_size = self.smart_sizer.calculate_optimal_rowgroup_size()?;
        
        tracing::info!(
            "Updated RAPTOR smart sizing: {}",
            self.optimal_size.rationale
        );
        
        Ok(())
    }
}