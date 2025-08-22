/// Consolidated RAPTOR compactor that eliminates duplication
/// Replaces: compaction.rs (321 lines) + hnsw_compaction.rs (1,027 lines)
/// Total elimination: ~1,350 lines of duplicated code

use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use anyhow::{Result, Context};
use tracing::{debug, info, warn};
use arrow_array::RecordBatch;

// DIRECT use of unified components - no wrappers
use crate::compute::distance_computation::engine::{UnifiedDistanceCompute, DistanceMetric};
use crate::storage::engines::common::fastlanes_encoding::{FastLanesEncoder, FastLanesScheme};
use crate::storage::persistence::filesystem::zero_copy_filesystem::ZeroCopyFilesystem;
use crate::storage::transaction_coordinator::TransactionCoordinator;
use crate::proto::proximadb::VectorRecord;
use super::common::{RaptorFileMetadata, RowGroup, RowGroupMetadata, SchemaDescriptor};
use super::config::RaptorConfig;
use super::ivf_manager::IvfManager;
use super::consolidated_reader::RaptorReader;

/// Unified compactor handling both standard and HNSW-aware compaction
pub struct RaptorCompactor {
    config: RaptorConfig,
    reader: Arc<RaptorReader>,
    ivf_manager: Option<Arc<IvfManager>>,
    
    // DIRECT references to unified modules
    distance_compute: Arc<UnifiedDistanceCompute>,
    fastlanes_encoder: FastLanesEncoder,
    filesystem: Arc<ZeroCopyFilesystem>,
    transaction_coordinator: Arc<TransactionCoordinator>,
}

impl RaptorCompactor {
    pub fn new(
        config: RaptorConfig,
        reader: Arc<RaptorReader>,
        filesystem: Arc<ZeroCopyFilesystem>,
        transaction_coordinator: Arc<TransactionCoordinator>,
    ) -> Self {
        let fastlanes_scheme = if config.use_fastlanes_encoding {
            FastLanesScheme::BitPacked { bits: 32 }
        } else {
            FastLanesScheme::BitPacked { bits: 32 }
        };
        
        Self {
            config,
            reader,
            ivf_manager: None,
            distance_compute: Arc::new(UnifiedDistanceCompute::default()),
            fastlanes_encoder: FastLanesEncoder::new(fastlanes_scheme),
            filesystem,
            transaction_coordinator,
        }
    }
    
    /// Enable HNSW-aware compaction
    pub fn with_ivf_manager(mut self, ivf_manager: Arc<IvfManager>) -> Self {
        self.ivf_manager = Some(ivf_manager);
        self
    }
    
    /// Unified compaction method handling both scenarios
    pub async fn compact_files(
        &self,
        input_files: Vec<String>,
        output_file: &str,
        collection_id: &str,
    ) -> Result<()> {
        info!("Starting compaction of {} files", input_files.len());
        
        if self.ivf_manager.is_some() {
            // HNSW-aware compaction path
            self.compact_with_ivf_preservation(input_files, output_file, collection_id).await
        } else {
            // Standard K-way merge compaction
            self.compact_standard(input_files, output_file, collection_id).await
        }
    }
    
    /// Standard K-way merge compaction (from compaction.rs)
    async fn compact_standard(
        &self,
        input_files: Vec<String>,
        output_file: &str,
        _collection_id: &str,
    ) -> Result<()> {
        debug!("Performing standard K-way merge compaction");
        
        // Step 1: Read all vectors from input files - DIRECT operations
        let mut all_vectors = Vec::new();
        for file_path in &input_files {
            // DIRECT reader usage - no wrapper
            let batches = self.reader.read_row_groups_selective(&file_path, None).await?;
            
            for batch in batches {
                // DIRECT vector extraction from Arrow RecordBatch
                let vectors = self.extract_vectors_from_batch(&batch)?;
                all_vectors.extend(vectors);
            }
        }
        
        // Step 2: Sort vectors by ID for deterministic output
        all_vectors.sort_by(|a, b| a.id.cmp(&b.id));
        
        // Step 3: Apply MVCC resolution - keep only latest versions
        let deduplicated = self.apply_mvcc_resolution(all_vectors);
        
        // Step 4: Group into row groups (10K vectors each)
        let row_groups = self.create_row_groups(deduplicated, 10000);
        
        // Step 5: Write compacted file - DIRECT filesystem operations
        self.write_compacted_file(output_file, row_groups).await?;
        
        // Step 6: Clean up input files
        for file_path in input_files {
            self.filesystem.delete(&file_path).await?;
        }
        
        Ok(())
    }
    
    /// HNSW-aware compaction preserving graph structure (from hnsw_compaction.rs)
    async fn compact_with_ivf_preservation(
        &self,
        input_files: Vec<String>,
        output_file: &str,
        collection_id: &str,
    ) -> Result<()> {
        debug!("Performing HNSW-aware compaction with graph preservation");
        
        let ivf_manager = self.ivf_manager.as_ref()
            .context("HNSW manager required for graph-aware compaction")?;
        
        // Calculate total vectors for smart HNSW parameter selection
        // Use actual dimension from config for accurate calculation
        let dimension = self.config.dimension;
        let bytes_per_vector = dimension * 4 + 100; // 4 bytes per f32 + metadata overhead
        
        let mut total_vectors = 0usize;
        for file_path in &input_files {
            if let Ok(metadata) = self.filesystem.metadata(file_path).await {
                // More accurate estimation using actual dimension
                let estimated_vectors = metadata.size as usize / bytes_per_vector;
                total_vectors += estimated_vectors;
            }
        }
        
        info!("Compacting {} files with ~{} vectors (dim={}, bytes/vec={})", 
            input_files.len(), total_vectors, dimension, bytes_per_vector);
        
        // Step 1: Load HNSW graph structure
        let graph = ivf_manager.load_graph(collection_id).await?;
        
        // Step 2: Read vectors and maintain graph relationships
        let mut vectors_by_id: HashMap<String, VectorRecord> = HashMap::new();
        for file_path in &input_files {
            let batches = self.reader.read_row_groups_selective(&file_path, None).await?;
            
            for batch in batches {
                let vectors = self.extract_vectors_from_batch(&batch)?;
                for vector in vectors {
                    vectors_by_id.insert(vector.id.clone(), vector);
                }
            }
        }
        
        // Step 3: Create locality-aware row groups based on HNSW connectivity
        let mut row_groups = Vec::new();
        let mut visited = HashSet::new();
        
        // BFS traversal to group connected nodes
        for entry_point in &graph.entry_points {
            if visited.contains(&entry_point.id) {
                continue;
            }
            
            let mut current_group = Vec::new();
            let mut queue = vec![entry_point.id.clone()];
            
            while let Some(node_id) = queue.pop() {
                if visited.contains(&node_id) || current_group.len() >= 1000 {
                    continue;
                }
                
                visited.insert(node_id.clone());
                
                if let Some(vector) = vectors_by_id.get(&node_id) {
                    current_group.push(vector.clone());
                    
                    // Add neighbors to queue for locality grouping
                    if let Some(neighbors) = graph.get_neighbors(&node_id) {
                        queue.extend(neighbors.iter().cloned());
                    }
                }
            }
            
            if !current_group.is_empty() {
                row_groups.push(self.create_row_group_from_vectors(current_group));
            }
        }
        
        // Step 4: Add any remaining vectors not in graph
        let mut remaining = Vec::new();
        for (id, vector) in vectors_by_id {
            if !visited.contains(&id) {
                remaining.push(vector);
                if remaining.len() >= 10000 {
                    row_groups.push(self.create_row_group_from_vectors(remaining.clone()));
                    remaining.clear();
                }
            }
        }
        if !remaining.is_empty() {
            row_groups.push(self.create_row_group_from_vectors(remaining));
        }
        
        // Step 5: Write compacted file with preserved locality
        self.write_compacted_file(output_file, row_groups).await?;
        
        // Step 6: Update HNSW index with new file location
        ivf_manager.update_file_location(collection_id, output_file).await?;
        
        // Step 7: Clean up input files
        for file_path in input_files {
            self.filesystem.delete(&file_path).await?;
        }
        
        Ok(())
    }
    
    /// Extract vectors from Arrow RecordBatch - DIRECT operation
    fn extract_vectors_from_batch(&self, batch: &RecordBatch) -> Result<Vec<VectorRecord>> {
        use arrow_array::{StringArray, Float32Array, Array};
        
        let mut vectors = Vec::new();
        
        // DIRECT field extraction from Arrow
        let id_array = batch.column_by_name("id")
            .context("Missing id column")?
            .as_any()
            .downcast_ref::<StringArray>()
            .context("ID column is not StringArray")?;
        
        let vector_array = batch.column_by_name("vector")
            .context("Missing vector column")?
            .as_any()
            .downcast_ref::<Float32Array>()
            .context("Vector column is not Float32Array")?;
        
        let dimension = vector_array.len() / id_array.len();
        
        for i in 0..id_array.len() {
            let id = id_array.value(i).to_string();
            let start = i * dimension;
            let end = start + dimension;
            let vector_data: Vec<f32> = (start..end)
                .map(|j| vector_array.value(j))
                .collect();
            
            vectors.push(VectorRecord {
                id,
                vector: vector_data,
                metadata: Vec::new(), // Would extract metadata if present
                version: None,
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                source: None,
                quantized_vector: None, // No quantized data in this extraction
            });
        }
        
        Ok(vectors)
    }
    
    /// Apply MVCC resolution - keep only latest versions
    fn apply_mvcc_resolution(&self, mut vectors: Vec<VectorRecord>) -> Vec<VectorRecord> {
        let mut latest_by_id: HashMap<String, VectorRecord> = HashMap::new();
        
        for vector in vectors.drain(..) {
            let should_keep = latest_by_id
                .get(&vector.id)
                .map(|existing| {
                    // Keep if newer version or same version with earlier timestamp
                    vector.version.unwrap_or(0) > existing.version.unwrap_or(0) ||
                    (vector.version == existing.version && vector.timestamp < existing.timestamp)
                })
                .unwrap_or(true);
            
            if should_keep {
                latest_by_id.insert(vector.id.clone(), vector);
            }
        }
        
        latest_by_id.into_values().collect()
    }
    
    /// Create row groups from vectors
    fn create_row_groups(&self, vectors: Vec<VectorRecord>, group_size: usize) -> Vec<RowGroup> {
        let mut row_groups = Vec::new();
        let mut current_offset = 0u64;
        
        for (idx, chunk) in vectors.chunks(group_size).enumerate() {
            let mut row_group = RowGroup::new(idx as u32);
            row_group.row_count = chunk.len();
            row_group.offset = current_offset;
            row_group.vectors = Some(chunk.to_vec());
            
            // Calculate size (would be actual compressed size)
            row_group.compressed_size = (chunk.len() * 1024) as u64; // Estimate
            row_group.uncompressed_size = (chunk.len() * 1536) as u64; // Estimate
            
            current_offset += row_group.compressed_size;
            row_groups.push(row_group);
        }
        
        row_groups
    }
    
    /// Create row group from vector list
    fn create_row_group_from_vectors(&self, vectors: Vec<VectorRecord>) -> RowGroup {
        let mut row_group = RowGroup::new(0);
        row_group.row_count = vectors.len();
        row_group.vectors = Some(vectors);
        row_group.compressed_size = (row_group.row_count * 1024) as u64; // Estimate
        row_group.uncompressed_size = (row_group.row_count * 1536) as u64; // Estimate
        row_group
    }
    
    /// Write compacted file - DIRECT filesystem operations
    async fn write_compacted_file(
        &self,
        output_file: &str,
        row_groups: Vec<RowGroup>,
    ) -> Result<()> {
        use arrow_ipc::writer::StreamWriter;
        
        let mut file_data = Vec::new();
        let mut metadata = RaptorFileMetadata {
            // Core file metadata
            version: 1,
            created_at: chrono::Utc::now().timestamp(),
            created_by: "RaptorCompactor".to_string(),
            file_path: output_file.to_string(),
            file_size: 0, // Will be updated after writing
            
            // Row and vector counts
            total_rows: 0,
            total_vectors: 0,
            dimension: self.config.dimension,
            
            // Collection info
            collection_id: String::new(), // Could be passed as parameter
            
            // Row groups
            row_groups: Vec::new(),
            num_rowgroups: row_groups.len(),
            rowgroup_offsets: Vec::new(),
            rowgroup_sizes: Vec::new(),
            rowgroup_vector_counts: Vec::new(),
            
            // Schema
            schema: SchemaDescriptor {
                vector_dimension: self.config.dimension,
                metadata_fields: Vec::new(),
                version: 1,
            },
            
            // HNSW metadata (if applicable)
            hnsw_metadata: None,
            global_hnsw_offset: 0,
            global_hnsw_size: 0,
            hnsw_entry_points: Vec::new(),
            
            // Compression info
            compression_codec: "zstd".to_string(),
            compression_ratio: 0.0,
            
            // Clustering info
            cluster_centroids: Vec::new(),
            cluster_assignments: Vec::new(),
        };
        
        // Write each row group
        for row_group in row_groups {
            let offset = file_data.len() as u64;
            metadata.rowgroup_offsets.push(offset);
            
            // Convert to Arrow RecordBatch
            let batch = self.vectors_to_record_batch(
                row_group.vectors.as_ref().unwrap_or(&Vec::new())
            )?;
            
            // Serialize to Arrow IPC format
            let mut buffer = Vec::new();
            {
                let mut writer = StreamWriter::try_new(&mut buffer, &batch.schema())?;
                writer.write(&batch)?;
                writer.finish()?;
            }
            
            // Apply FastLanes encoding if enabled
            let encoded = if self.config.use_fastlanes_encoding {
                self.fastlanes_encoder.encode_bytes(&buffer)?
            } else {
                buffer
            };
            
            // Update metadata
            let rg_metadata = RowGroupMetadata {
                id: metadata.row_groups.len() as u32,
                offset,
                compressed_size: encoded.len() as u64,
                uncompressed_size: buffer.len() as u64,
                row_count: row_group.row_count,
                vector_stats: row_group.vector_stats,
                metadata_stats: row_group.metadata_stats,
                bloom_filter_offset: None,
                hnsw_segment_offset: None,
                compression_codec: "zstd".to_string(),
                min_timestamp: row_group.min_timestamp,
                max_timestamp: row_group.max_timestamp,
                centroid: row_group.centroid,
            };
            
            metadata.row_groups.push(rg_metadata);
            metadata.total_vectors += row_group.row_count;
            metadata.total_rows += row_group.row_count;
            metadata.rowgroup_sizes.push(encoded.len() as u64);
            metadata.rowgroup_vector_counts.push(row_group.row_count);
            
            file_data.extend(encoded);
        }
        
        // Update file size before serialization
        metadata.file_size = file_data.len() as u64;
        
        // Calculate compression ratio
        let uncompressed_size: u64 = metadata.rowgroup_vector_counts.iter().map(|&c| c as u64 * metadata.dimension as u64 * 4).sum();
        let compressed_size: u64 = metadata.rowgroup_sizes.iter().sum();
        metadata.compression_ratio = if uncompressed_size > 0 {
            compressed_size as f32 / uncompressed_size as f32
        } else {
            1.0
        };
        
        // Write metadata footer
        let metadata_bytes = bincode::serialize(&metadata)?;
        file_data.extend(&metadata_bytes);
        
        // Write metadata size (last 8 bytes)
        file_data.extend(&(metadata_bytes.len() as u64).to_le_bytes());
        
        // DIRECT filesystem write
        self.filesystem.write(output_file, file_data).await?;
        
        info!("Compacted {} vectors into {}", metadata.total_vectors, output_file);
        Ok(())
    }
    
    /// Convert vectors to Arrow RecordBatch
    fn vectors_to_record_batch(&self, vectors: &[VectorRecord]) -> Result<RecordBatch> {
        use arrow_array::{StringArray, Float32Array};
        use arrow_schema::{Schema, Field, DataType};
        use std::sync::Arc as StdArc;
        
        if vectors.is_empty() {
            // Return empty batch with correct schema
            let schema = Schema::new(vec![
                Field::new("id", DataType::Utf8, false),
                Field::new("vector", DataType::Float32, false),
            ]);
            return Ok(RecordBatch::new_empty(StdArc::new(schema)));
        }
        
        // Build ID array
        let ids: StringArray = vectors.iter()
            .map(|v| Some(v.id.as_str()))
            .collect();
        
        // Build vector array (flattened)
        let mut vector_data: Vec<f32> = Vec::new();
        for vector in vectors {
            vector_data.extend(&vector.vector);
        }
        let vectors_array = Float32Array::from(vector_data);
        
        // Create schema
        let schema = Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("vector", DataType::Float32, false),
        ]);
        
        // Create RecordBatch
        RecordBatch::try_new(
            StdArc::new(schema),
            vec![
                StdArc::new(ids),
                StdArc::new(vectors_array),
            ],
        ).context("Failed to create RecordBatch")
    }
}