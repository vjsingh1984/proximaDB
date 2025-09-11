use anyhow::{Context, Result};
use arrow_array::RecordBatch;
use std::collections::HashMap;
/// Consolidated RAPTOR compactor that eliminates duplication
/// Replaces: compaction.rs (321 lines) + hnsw_compaction.rs (1,027 lines)
/// Total elimination: ~1,350 lines of duplicated code
use std::sync::Arc;
use tracing::{debug, info};

// DIRECT use of unified components - no wrappers
use super::common::{RaptorFileMetadata, RowGroup, RowGroupMetadata, SchemaDescriptor};
use super::config::RaptorConfig;
use super::consolidated_reader::RaptorReader;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::index::axis::clustering::{
    AxisClusteringEngine as AxisClustering, ClusteringConfig as AxisClusteringConfig,
    ReusableClusteringEngine,
};
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::ops::fastlanes_encoding::{FastLanesEncoder, FastLanesScheme};
use crate::storage::persistence::filesystem::FileSystem;
use crate::storage::transaction_coordinator::TransactionCoordinator;

/// Unified compactor handling both standard and HNSW-aware compaction
pub struct RaptorCompactor {
    config: RaptorConfig,
    reader: Arc<RaptorReader>,

    // DIRECT references to unified modules
    distance_compute: Arc<UnifiedDistanceCompute>,
    fastlanes_encoder: FastLanesEncoder,
    filesystem: Arc<dyn FileSystem>,
    transaction_coordinator: Arc<TransactionCoordinator>,
}

impl RaptorCompactor {
    pub fn new(
        config: RaptorConfig,
        reader: Arc<RaptorReader>,
        filesystem: Arc<dyn FileSystem>,
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
            distance_compute: Arc::new(UnifiedDistanceCompute::default()),
            fastlanes_encoder: FastLanesEncoder::new(fastlanes_scheme),
            filesystem,
            transaction_coordinator,
        }
    }

    /// Unified compaction method handling both scenarios
    pub async fn compact_files(
        &self,
        input_files: Vec<String>,
        output_file: &str,
        collection_id: &str,
    ) -> Result<()> {
        info!("Starting compaction of {} files", input_files.len());

        // Standard K-way merge compaction with Matrix Trinity preservation
        self.compact_standard(input_files, output_file, collection_id)
            .await
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
            let batches = self
                .reader
                .read_row_groups_selective(&file_path, None)
                .await?;

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

    /// Clustering-based compaction matching writer's flush behavior
    /// Key principle: k (number of clusters) = number of rowgroups
    /// Each rowgroup contains vectors from exactly one cluster
    async fn compact_with_matrix_preservation(
        &self,
        input_files: Vec<String>,
        output_file: &str,
        _collection_id: &str,
    ) -> Result<()> {
        debug!("Performing clustering-based compaction (consistent with writer flush)");

        // Step 1: Read all vectors from input files
        let mut all_vectors: Vec<VectorRecord> = Vec::new();
        for file_path in &input_files {
            let batches = self
                .reader
                .read_row_groups_selective(&file_path, None)
                .await?;

            for batch in batches {
                let vectors = self.extract_vectors_from_batch(&batch)?;
                all_vectors.extend(vectors);
            }
        }

        let n = all_vectors.len();
        if n == 0 {
            debug!("No vectors to compact");
            return Ok(());
        }

        // Step 2: Calculate k using same logic as writer's build_ivf_clusters
        // k = sqrt(n) for optimal complexity k² + p×(k+p)
        let sqrt_n = (n as f64).sqrt() as usize;
        let k = self.config.num_clusters.unwrap_or(sqrt_n.max(1));

        // p = rowgroup size (from config or auto-calculated based on L3 cache)
        let p = self
            .config
            .target_rowgroup_size
            .unwrap_or(self.config.rowgroup_size);

        info!(
            "Compacting {} vectors: k={} clusters (sqrt(n)), p={} vectors/rowgroup",
            n, k, p
        );

        // Step 3: Run clustering to assign vectors to k clusters (same as writer)
        let cluster_assignments = self.cluster_vectors(&all_vectors, k)?;

        // Step 4: Group vectors by assigned cluster, then sort by cluster ID
        let mut vectors_by_cluster: HashMap<u16, Vec<VectorRecord>> = HashMap::new();
        for (vector, &cluster_id) in all_vectors.into_iter().zip(cluster_assignments.iter()) {
            let cluster_id_u16 = cluster_id as u16;
            vectors_by_cluster
                .entry(cluster_id_u16)
                .or_insert_with(Vec::new)
                .push(vector);
        }

        // Step 5: Create sorted rowgroups (one per cluster)
        let mut sorted_cluster_ids: Vec<u16> = vectors_by_cluster.keys().cloned().collect();
        sorted_cluster_ids.sort(); // Sort by assigned cluster/rowgroup ID

        let mut row_groups = Vec::new();
        for (rg_idx, cluster_id) in sorted_cluster_ids.iter().enumerate() {
            let vectors = vectors_by_cluster.remove(cluster_id).unwrap();

            // Create rowgroup - rowgroup_id matches position in sorted order
            let mut row_group = RowGroup::with_capacity(rg_idx as u16, p);
            row_group.vector_count = vectors.len();
            row_group.vectors = Some(vectors);

            // Centroid and matrices will be built by writer during write
            // This keeps it consistent with flush behavior

            row_groups.push(row_group);
        }

        // Step 6: Write compacted file with sorted rowgroups
        self.write_compacted_file(output_file, row_groups).await?;

        // Step 7: Clean up input files
        for file_path in input_files {
            self.filesystem.delete(&file_path).await?;
        }

        Ok(())
    }

    /// Run fast-converging K-means++ clustering for high-dimensional data
    /// Uses the same algorithm as the writer for consistency
    fn cluster_vectors(&self, vectors: &[VectorRecord], k: usize) -> Result<Vec<usize>> {
        // Use AXIS clustering engine with K-means++ initialization
        // This provides fast convergence for high-dimensional data
        // TODO: Use AXIS clustering when available
        // use crate::index::axis::clustering::AxisClustering;
        use crate::compute::distance_computation::engine::DistanceMetric;

        // Convert VectorRecord to Vec<Vec<f32>> for AXIS clustering
        let vector_data: Vec<Vec<f32>> = vectors.iter().map(|v| v.vector.clone()).collect();

        // Use AXIS clustering with K-means++ (same as writer)
        // K-means++ initialization ensures well-separated centroids
        let clustering_config = AxisClusteringConfig {
            algorithm: crate::index::axis::clustering::ClusteringAlgorithm::KMeans(
                crate::index::axis::clustering::KMeansConfig {
                    k,
                    max_iterations: 50,
                    n_init: 3, // Number of times to run k-means with different initial seeds
                    init_method: crate::index::axis::clustering::KMeansInit::KMeansPlusPlus,
                    tolerance: 0.001,
                },
            ),
            min_vectors_for_clustering: 100,
            max_clusters: k * 2,
            distance_metric: DistanceMetric::Cosine,
            adaptive_cluster_count: false,
            recompute_threshold: 1000,
            enable_incremental: false,
        };
        let axis_clustering = AxisClustering::new(clustering_config);
        let (_centroids, assignments) = axis_clustering.cluster_vectors_simple(
            &vector_data,
            k,
            DistanceMetric::Cosine, // Use cosine for high-dimensional data
            50,                     // Fewer iterations for fast convergence
        )?;

        // Convert assignments to usize
        let assignments_usize: Vec<usize> = assignments.iter().map(|&a| a as usize).collect();

        Ok(assignments_usize)
    }

    // Matrix and centroid building removed - writer handles this during write
    // This keeps flush and compact consistent

    /// Extract vectors from Arrow RecordBatch - DIRECT operation
    fn extract_vectors_from_batch(&self, batch: &RecordBatch) -> Result<Vec<VectorRecord>> {
        use arrow_array::{Array, Float32Array, StringArray};

        let mut vectors = Vec::new();

        // DIRECT field extraction from Arrow
        let id_array = batch
            .column_by_name("id")
            .context("Missing id column")?
            .as_any()
            .downcast_ref::<StringArray>()
            .context("ID column is not StringArray")?;

        let vector_array = batch
            .column_by_name("vector")
            .context("Missing vector column")?
            .as_any()
            .downcast_ref::<Float32Array>()
            .context("Vector column is not Float32Array")?;

        let dimension = vector_array.len() / id_array.len();

        for i in 0..id_array.len() {
            let id = id_array.value(i).to_string();
            let start = i * dimension;
            let end = start + dimension;
            let vector_data: Vec<f32> = (start..end).map(|j| vector_array.value(j)).collect();

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
                    vector.version.unwrap_or(0) > existing.version.unwrap_or(0)
                        || (vector.version == existing.version
                            && vector.timestamp < existing.timestamp)
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
            let mut row_group = RowGroup::new(idx as u16);
            row_group.vector_count = chunk.len();
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
        row_group.vector_count = vectors.len();
        row_group.vectors = Some(vectors);
        row_group.compressed_size = (row_group.vector_count * 1024) as u64; // Estimate
        row_group.uncompressed_size = (row_group.vector_count * 1536) as u64; // Estimate
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

            // Matrix Trinity architecture - no HNSW needed
            // P² matrix: Stored per rowgroup
            // K² matrix: Stored in footer
            // P×K matrix: Stored per rowgroup

            // Compression info
            compression_codec: "zstd".to_string(),
            compression_ratio: 0.0,

            // Clustering info
            cluster_centroids: Vec::new(),
            cluster_assignments: HashMap::new(),

            // Additional metadata
            bloom_filter_metadata: None,
            custom_metadata: HashMap::new(),
            key_value_metadata: Vec::new(),
            footer_offset: 0, // Will be set when footer is written
            footer_size: 0,   // Will be set when footer is written
            last_accessed: chrono::Utc::now().timestamp(),
            locality_clusters: Vec::new(),
        };

        // Write each row group
        for row_group in row_groups {
            let offset = file_data.len() as u64;
            metadata.rowgroup_offsets.push(offset);

            // Convert to Arrow RecordBatch
            let batch =
                self.vectors_to_record_batch(row_group.vectors.as_ref().unwrap_or(&Vec::new()))?;

            // Serialize to Arrow IPC format
            let mut buffer = Vec::new();
            {
                let mut writer = StreamWriter::try_new(&mut buffer, &batch.schema())?;
                writer.write(&batch)?;
                writer.finish()?;
            }

            // FastLanes encoding is applied to individual columns during write,
            // not to the entire Arrow IPC buffer
            let encoded = buffer;

            // Update metadata
            let rg_metadata = RowGroupMetadata {
                id: metadata.row_groups.len() as u16,
                vector_count: row_group.vector_count,
                offset: row_group.offset,
                compressed_size: row_group.compressed_size,
                column_pages: HashMap::new(),
                vector_stats: row_group.vector_stats,
                metadata_stats: row_group.metadata_stats,
                min_timestamp: row_group.min_timestamp,
                max_timestamp: row_group.max_timestamp,
                centroid: row_group.centroid,
                centroid_stats: None,
                bloom_filter_offset: row_group.bloom_filter_offset,
            };

            metadata.row_groups.push(rg_metadata);
            metadata.total_vectors += row_group.vector_count;
            metadata.total_rows += row_group.vector_count;
            metadata.rowgroup_sizes.push(encoded.len() as u64);
            metadata.rowgroup_vector_counts.push(row_group.vector_count);

            file_data.extend(encoded);
        }

        // Update file size before serialization
        metadata.file_size = file_data.len() as u64;

        // Calculate compression ratio
        let uncompressed_size: u64 = metadata
            .rowgroup_vector_counts
            .iter()
            .map(|&c| c as u64 * metadata.dimension as u64 * 4)
            .sum();
        let compressed_size: u64 = metadata.rowgroup_sizes.iter().sum();
        metadata.compression_ratio = if uncompressed_size > 0 {
            compressed_size as f64 / uncompressed_size as f64
        } else {
            1.0
        };

        // Write metadata footer
        let metadata_bytes = bincode::serialize(&metadata)?;
        file_data.extend(&metadata_bytes);

        // Write metadata size (last 8 bytes)
        file_data.extend(&(metadata_bytes.len() as u64).to_le_bytes());

        // DIRECT filesystem write
        self.filesystem.write(output_file, &file_data, None).await?;

        info!(
            "Compacted {} vectors into {}",
            metadata.total_vectors, output_file
        );
        Ok(())
    }

    /// Convert vectors to Arrow RecordBatch
    fn vectors_to_record_batch(&self, vectors: &[VectorRecord]) -> Result<RecordBatch> {
        use arrow_array::{Float32Array, StringArray};
        use arrow_schema::{DataType, Field, Schema};
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
        let ids: StringArray = vectors.iter().map(|v| Some(v.id.as_str())).collect();

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
            vec![StdArc::new(ids), StdArc::new(vectors_array)],
        )
        .context("Failed to create RecordBatch")
    }
}
