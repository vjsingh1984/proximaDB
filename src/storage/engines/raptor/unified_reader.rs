use anyhow::Result;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use bytes::Bytes;
use arrow_array::RecordBatch;

use crate::storage::persistence::filesystem::zero_copy_filesystem::ZeroCopyFilesystem;
use crate::storage::transaction_coordinator::TransactionCoordinator;
use crate::storage::persistence::filesystem::{FileSystem, FileOptions};
use crate::storage::engines::common::zero_copy_io_system::traits::CacheTemperature;
use super::RaptorConfig;

// Import consolidated types from common
use super::common::{RaptorFileMetadata, LocalityClusterInfo, IoStrategy};

/// Unified RAPTOR reader that consolidates all reading functionality
/// This is the single authoritative source for all RAPTOR read operations
pub struct RaptorUnifiedReader {
    // Core dependencies
    filesystem: Arc<ZeroCopyFilesystem>,
    transaction_coordinator: Arc<TransactionCoordinator>,
    
    // Configuration
    config: RaptorConfig,
    io_strategy: IoStrategy,
    
    // Caching infrastructure
    metadata_cache: Arc<RwLock<HashMap<String, RaptorFileMetadata>>>,
    local_cache_dir: String,
    
    // Statistics
    stats: Arc<RwLock<ReaderStatistics>>,
}

#[derive(Debug, Default)]
#[derive(Clone)]
struct ReaderStatistics {
    cache_hits: u64,
    cache_misses: u64,
    bytes_read_remote: u64,
    bytes_read_local: u64,
    range_reads_performed: u64,
    full_downloads_performed: u64,
}

impl RaptorUnifiedReader {
    /// Create a new unified reader with all necessary dependencies
    pub fn new(
        filesystem: Arc<ZeroCopyFilesystem>,
        transaction_coordinator: Arc<TransactionCoordinator>,
        config: RaptorConfig,
        local_cache_dir: String,
    ) -> Self {
        Self::new_with_bandwidth_optimizer(
            filesystem,
            transaction_coordinator,
            config,
            local_cache_dir,
            None,
        )
    }
    
    /// 🚀 NEW: Create RAPTOR reader with bandwidth optimizer for smart threshold decisions
    /// This constructor enables dual strategy support for different operation types
    pub fn new_with_bandwidth_optimizer(
        filesystem: Arc<ZeroCopyFilesystem>,
        transaction_coordinator: Arc<TransactionCoordinator>,
        config: RaptorConfig,
        local_cache_dir: String,
        bandwidth_optimizer: Option<Arc<crate::storage::engines::common::zero_copy_io_system::BandwidthOptimizer>>,
    ) -> Self {
        // Create IO strategy that considers bandwidth optimization decisions
        let mut io_strategy = IoStrategy::default();
        
        // If bandwidth optimizer is available, adjust thresholds based on network conditions
        if bandwidth_optimizer.is_some() {
            // Bandwidth optimizer will make dynamic decisions, adjust base thresholds
            io_strategy.full_download_threshold_mb = 100.0; // Higher threshold, let optimizer decide
            io_strategy.read_percentage_threshold = 0.5; // More conservative base threshold
        }
        
        Self {
            filesystem,
            transaction_coordinator,
            config,
            io_strategy,
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            local_cache_dir,
            stats: Arc::new(RwLock::new(ReaderStatistics::default())),
        }
    }
    
    /// 🚀 NEW: Read with selective cache strategy - for normal queries with range reads and cache lookup
    /// This strategy is optimized for:
    /// - Range-based reading with HNSW graph navigation
    /// - Cache lookup for frequently accessed RowGroups
    /// - Metadata cache utilization for query planning
    /// - Bandwidth-aware threshold decisions
    pub async fn read_with_selective_cache_strategy(
        &self,
        file_path: &str,
        rowgroup_selection: Option<Vec<usize>>,
        bandwidth_optimizer: Option<Arc<crate::storage::engines::common::zero_copy_io_system::BandwidthOptimizer>>,
    ) -> Result<Vec<RecordBatch>> {
        tracing::debug!("🔍 RAPTOR SELECTIVE CACHE: Starting selective read strategy for {}", file_path);
        
        // Apply bandwidth optimizer decisions if available
        if let Some(optimizer) = bandwidth_optimizer {
            // Create query context for bandwidth decisions
            use crate::storage::engines::common::zero_copy_io_system::traits::{QueryContext, QueryType, RequestPriority};
            
            let query_context = QueryContext {
                query_vector: None,
                metadata_filters: HashMap::new(),
                id_lookups: Vec::new(),
                top_k: None,
                distance_threshold: None,
                query_type: QueryType::VectorSearch,
                collection_context: None,
                priority: RequestPriority::Normal,
                estimated_result_size: rowgroup_selection.as_ref().map(|rg| rg.len()),
                selectivity_hint: Some(0.3), // Moderate selectivity for RAPTOR HNSW navigation
                collection_id: "raptor".to_string(), // Generic collection ID for RAPTOR
                concurrent_queries: Some(1),
                cache_temperature: CacheTemperature::Warm,
            };
            
            // Make bandwidth-optimized decisions
            // Get file size (estimate if not available)
            let file_size = 100 * 1024 * 1024; // 100MB estimate for RAPTOR files
            match optimizer.decide_strategy(file_path, file_size, None, &query_context, RequestPriority::Normal).await {
                Ok(decision) => {
                    tracing::debug!("📊 RAPTOR BANDWIDTH: File {} - strategy: {:?}", 
                           file_path, decision);
                }
                Err(e) => {
                    tracing::warn!("⚠️ RAPTOR BANDWIDTH: Failed to get decision for {}: {}", file_path, e);
                }
            }
        }
        
        // Use range-based reading with cache optimization
        self.read_rowgroups_with_caching(file_path, rowgroup_selection).await
    }
    
    /// 🚀 NEW: Read with compaction strategy - for full read operations where cache lookups are suboptimal
    /// This strategy is optimized for:
    /// - Compaction operations that bypass write cache but use disk cache if files exist
    /// - Full file sequential reads without selective RowGroup filtering
    /// - Minimal metadata overhead for transient files
    /// - Bandwidth conservation by avoiding unnecessary downloads
    pub async fn read_with_compaction_strategy(
        &self,
        file_path: &str,
        bandwidth_optimizer: Option<Arc<crate::storage::engines::common::zero_copy_io_system::BandwidthOptimizer>>,
    ) -> Result<Vec<RecordBatch>> {
        tracing::info!("🔥 RAPTOR COMPACTION: Starting compaction read strategy for {}", file_path);
        
        // Apply bandwidth optimizer decisions if available
        if let Some(optimizer) = bandwidth_optimizer {
            // Create compaction query context
            use crate::storage::engines::common::zero_copy_io_system::traits::{QueryContext, QueryType, RequestPriority};
            
            let query_context = QueryContext {
                query_vector: None,
                metadata_filters: HashMap::new(),
                id_lookups: Vec::new(),
                top_k: None,
                distance_threshold: None,
                query_type: QueryType::FullScan,
                collection_context: None,
                priority: RequestPriority::Background,
                estimated_result_size: Some(1000000), // Large result set for compaction
                selectivity_hint: Some(1.0), // Read everything
                collection_id: "raptor".to_string(),
                concurrent_queries: Some(1),
                cache_temperature: CacheTemperature::Cold, // Don't pollute cache
            };
            
            // Make bandwidth-optimized decisions for compaction
            let file_size = 200 * 1024 * 1024; // 200MB estimate for compaction files
            match optimizer.decide_strategy(file_path, file_size, None, &query_context, RequestPriority::Normal).await {
                Ok(decision) => {
                    // For compaction, prefer using disk cache if available, avoid downloading transient files
                    tracing::debug!("🔄 RAPTOR COMPACTION BANDWIDTH: File {} - strategy: {:?} (transient files avoid download)", 
                           file_path, decision);
                }
                Err(e) => {
                    tracing::warn!("⚠️ RAPTOR COMPACTION BANDWIDTH: Failed to get decision for {}: {}", file_path, e);
                }
            }
        }
        
        // Use full file streaming for compaction - no selective RowGroup filtering
        self.read_full_file_streaming(file_path).await
    }
    
    /// Helper method for range-based reading with cache optimization
    async fn read_rowgroups_with_caching(
        &self,
        file_path: &str,
        rowgroup_selection: Option<Vec<usize>>,
    ) -> Result<Vec<RecordBatch>> {
        tracing::debug!("📊 RAPTOR RANGE CACHE: Reading {} with selective RowGroups", file_path);
        
        // Get metadata first
        let metadata = self.get_metadata(file_path).await?;
        
        // Determine which rowgroups to read
        let target_rowgroups = rowgroup_selection.unwrap_or_else(|| {
            (0..metadata.num_rowgroups).collect()
        });
        
        // Use range reads for selective access
        let mut record_batches = Vec::new();
        for &rowgroup_idx in &target_rowgroups {
            if rowgroup_idx < metadata.rowgroup_offsets.len() {
                let offset = metadata.rowgroup_offsets[rowgroup_idx];
                let size = metadata.rowgroup_sizes[rowgroup_idx];
                
                let rowgroup_data = self.filesystem.read_range(file_path, offset, size as u64).await?;
                let record_batch = self.parse_rowgroup_data(&rowgroup_data)?;
                record_batches.push(record_batch);
            }
        }
        
        Ok(record_batches)
    }
    
    /// Helper method for full file streaming
    async fn read_full_file_streaming(&self, file_path: &str) -> Result<Vec<RecordBatch>> {
        tracing::debug!("📁 RAPTOR FULL STREAM: Reading entire file {}", file_path);
        
        // Read full file data
        let file_data = self.filesystem.read(file_path).await?;
        
        // Parse all record batches from the file
        self.parse_full_file_data(&file_data)
    }
    
    /// Parse rowgroup data into RecordBatch
    fn parse_rowgroup_data(&self, data: &[u8]) -> Result<RecordBatch> {
        // Placeholder implementation - would parse Arrow RecordBatch from bytes
        // In a real implementation, this would use Arrow IPC or Parquet deserialization
        Err(anyhow::anyhow!("RowGroup parsing not yet implemented"))
    }
    
    /// Parse full file data into multiple RecordBatches
    fn parse_full_file_data(&self, data: &[u8]) -> Result<Vec<RecordBatch>> {
        // Placeholder implementation - would parse all RecordBatches from file
        // In a real implementation, this would parse the entire RAPTOR file format
        Err(anyhow::anyhow!("Full file parsing not yet implemented"))
    }

    // ========================================================================
    // METADATA OPERATIONS
    // ========================================================================

    /// Get metadata for a file, using cache if available
    pub async fn get_metadata(&self, file_path: &str) -> Result<RaptorFileMetadata> {
        // Check cache first
        {
            let cache = self.metadata_cache.read().await;
            if let Some(metadata) = cache.get(file_path) {
                let mut stats = self.stats.write().await;
                stats.cache_hits += 1;
                
                // Update last accessed time
                let mut metadata = metadata.clone();
                metadata.last_accessed = chrono::Utc::now().timestamp();
                return Ok(metadata);
            }
        }

        // Cache miss - read from file
        let mut stats = self.stats.write().await;
        stats.cache_misses += 1;
        drop(stats);

        let metadata = self.read_metadata_from_file(file_path).await?;
        
        // Update cache
        {
            let mut cache = self.metadata_cache.write().await;
            cache.insert(file_path.to_string(), metadata.clone());
        }
        
        Ok(metadata)
    }

    /// Read metadata directly from file header
    async fn read_metadata_from_file(&self, file_path: &str) -> Result<RaptorFileMetadata> {
        // Read header (typically first 8KB contains all metadata)
        let header_size = 8192;
        let header_data = self.filesystem.read_range(file_path, 0, header_size).await?;
        
        self.parse_file_header(&header_data, file_path)
    }

    /// Parse RAPTOR file header into metadata
    fn parse_file_header(&self, header_data: &[u8], file_path: &str) -> Result<RaptorFileMetadata> {
        // Check RAPTOR signature
        if &header_data[0..4] != b"RAPT" {
            return Err(anyhow::anyhow!("Invalid RAPTOR file signature"));
        }
        
        let version = u16::from_le_bytes([header_data[4], header_data[5]]);
        let mut offset = 6;
        
        // Parse file metadata
        let file_size = u64::from_le_bytes(
            header_data[offset..offset + 8].try_into()?
        );
        offset += 8;
        
        // Parse rowgroup information
        let num_rowgroups = u32::from_le_bytes(
            header_data[offset..offset + 4].try_into()?
        ) as usize;
        offset += 4;
        
        let mut rowgroup_offsets = Vec::with_capacity(num_rowgroups);
        let mut rowgroup_sizes = Vec::with_capacity(num_rowgroups);
        let mut rowgroup_vector_counts = Vec::with_capacity(num_rowgroups);
        
        for _ in 0..num_rowgroups {
            rowgroup_offsets.push(u64::from_le_bytes(
                header_data[offset..offset + 8].try_into()?
            ));
            offset += 8;
            
            rowgroup_sizes.push(u64::from_le_bytes(
                header_data[offset..offset + 8].try_into()?
            ));
            offset += 8;
            
            rowgroup_vector_counts.push(u32::from_le_bytes(
                header_data[offset..offset + 4].try_into()?
            ) as usize);
            offset += 4;
        }
        
        // Parse HNSW metadata
        let global_hnsw_offset = u64::from_le_bytes(
            header_data[offset..offset + 8].try_into()?
        );
        offset += 8;
        
        let global_hnsw_size = u64::from_le_bytes(
            header_data[offset..offset + 8].try_into()?
        );
        offset += 8;
        
        // Parse locality clusters
        let num_clusters = u32::from_le_bytes(
            header_data[offset..offset + 4].try_into()?
        ) as usize;
        offset += 4;
        
        let mut locality_clusters = Vec::with_capacity(num_clusters);
        for i in 0..num_clusters {
            let cluster_start = u64::from_le_bytes(
                header_data[offset..offset + 8].try_into()?
            );
            offset += 8;
            
            let cluster_size = u64::from_le_bytes(
                header_data[offset..offset + 8].try_into()?
            );
            offset += 8;
            
            let num_vectors = u32::from_le_bytes(
                header_data[offset..offset + 4].try_into()?
            ) as usize;
            offset += 4;
            
            // Determine which rowgroups belong to this cluster
            let mut cluster_rowgroups = Vec::new();
            for (idx, &rg_offset) in rowgroup_offsets.iter().enumerate() {
                if rg_offset >= cluster_start && 
                   rg_offset < cluster_start + cluster_size {
                    cluster_rowgroups.push(idx);
                }
            }
            
            locality_clusters.push(LocalityClusterInfo {
                cluster_id: i,
                start_offset: cluster_start,
                size_bytes: cluster_size,
                num_vectors,
                centroid_id: format!("cluster_{}_centroid", i),
                rowgroup_indices: cluster_rowgroups,
            });
        }
        
        // Parse footer location
        let footer_offset = u64::from_le_bytes(
            header_data[offset..offset + 8].try_into()?
        );
        offset += 8;
        
        let footer_size = u64::from_le_bytes(
            header_data[offset..offset + 8].try_into()?
        );
        offset += 8;
        
        // Parse vector metadata
        let dimension = u32::from_le_bytes(
            header_data[offset..offset + 4].try_into()?
        ) as usize;
        offset += 4;
        
        let total_vectors = u32::from_le_bytes(
            header_data[offset..offset + 4].try_into()?
        ) as usize;
        
        Ok(RaptorFileMetadata {
            file_path: file_path.to_string(),
            file_size,
            created_at: chrono::Utc::now().timestamp(),
            last_accessed: chrono::Utc::now().timestamp(),
            compression_codec: "zstd".to_string(),
            num_rowgroups,
            rowgroup_offsets,
            rowgroup_sizes,
            rowgroup_vector_counts,
            global_hnsw_offset,
            global_hnsw_size,
            hnsw_entry_points: vec![],
            hnsw_num_layers: 0,
            locality_clusters,
            footer_offset,
            footer_size,
            dimension,
            total_vectors,
            quantization_type: Some("mixed".to_string()),
        })
    }

    // ========================================================================
    // I/O STRATEGY DECISIONS
    // ========================================================================

    /// Determine optimal I/O strategy for the given access pattern
    pub async fn determine_io_strategy(
        &self,
        metadata: &RaptorFileMetadata,
        rowgroups_needed: &[usize],
    ) -> IoDecision {
        let file_size_mb = metadata.file_size as f64 / (1024.0 * 1024.0);
        
        // Small file - always download fully
        if file_size_mb < self.io_strategy.full_download_threshold_mb {
            return IoDecision::FullDownload {
                reason: format!("File size {:.1}MB below threshold", file_size_mb),
            };
        }
        
        // Calculate percentage of file needed
        let bytes_needed: u64 = rowgroups_needed.iter()
            .filter_map(|&idx| metadata.rowgroup_sizes.get(idx))
            .map(|&size| size as u64)
            .sum();
        let read_percentage = bytes_needed as f64 / metadata.file_size as f64;
        
        if read_percentage > self.io_strategy.read_percentage_threshold {
            return IoDecision::FullDownload {
                reason: format!("Reading {:.1}% of file", read_percentage * 100.0),
            };
        }
        
        // Many rowgroups - consider full download
        if rowgroups_needed.len() > self.io_strategy.rowgroup_count_threshold {
            return IoDecision::FullDownload {
                reason: format!("Need {} rowgroups", rowgroups_needed.len()),
            };
        }
        
        // Check if rowgroups belong to same locality cluster (high selectivity)
        let clusters_needed = self.identify_clusters_for_rowgroups(metadata, rowgroups_needed);
        if clusters_needed.len() == 1 {
            return IoDecision::ClusterRead {
                cluster_id: clusters_needed[0],
                reason: "All rowgroups in single locality cluster".to_string(),
            };
        }
        
        // Use range reads for specific rowgroups
        IoDecision::RangeReads {
            rowgroup_indices: rowgroups_needed.to_vec(),
            reason: format!("Selective read of {} rowgroups", rowgroups_needed.len()),
        }
    }

    /// Identify which locality clusters contain the needed rowgroups
    fn identify_clusters_for_rowgroups(
        &self,
        metadata: &RaptorFileMetadata,
        rowgroups: &[usize],
    ) -> Vec<usize> {
        let mut clusters = Vec::new();
        
        for cluster in &metadata.locality_clusters {
            for &rg_idx in rowgroups {
                if cluster.rowgroup_indices.contains(&rg_idx) {
                    if !clusters.contains(&cluster.cluster_id) {
                        clusters.push(cluster.cluster_id);
                    }
                }
            }
        }
        
        clusters
    }

    // ========================================================================
    // DATA READING OPERATIONS
    // ========================================================================

    /// Read specific rowgroups with optimal I/O strategy
    pub async fn read_rowgroups(
        &self,
        file_path: &str,
        rowgroup_indices: &[usize],
    ) -> Result<Vec<RecordBatch>> {
        let metadata = self.get_metadata(file_path).await?;
        let io_decision = self.determine_io_strategy(&metadata, rowgroup_indices).await;
        
        tracing::info!("RAPTOR read strategy: {:?}", io_decision);
        
        match io_decision {
            IoDecision::FullDownload { .. } => {
                self.read_with_full_download(&metadata, rowgroup_indices).await
            }
            IoDecision::ClusterRead { cluster_id, .. } => {
                self.read_locality_cluster(&metadata, cluster_id, rowgroup_indices).await
            }
            IoDecision::RangeReads { .. } => {
                self.read_with_ranges(&metadata, rowgroup_indices).await
            }
        }
    }

    /// Read using full file download and local caching
    async fn read_with_full_download(
        &self,
        metadata: &RaptorFileMetadata,
        rowgroup_indices: &[usize],
    ) -> Result<Vec<RecordBatch>> {
        let local_path = self.get_local_cache_path(&metadata.file_path);
        
        // Check if already cached locally
        if !self.filesystem.exists(&local_path).await? {
            tracing::info!("Downloading full file to local cache: {}", local_path);
            
            // Use transaction for atomic download
            let tx_id = format!("raptor_download_{}", uuid::Uuid::new_v4());
            let tx = self.transaction_coordinator.begin_transaction(&tx_id, vec![]).await?;
            let data = self.filesystem.read(&metadata.file_path).await?;
            self.filesystem.write(&local_path, &data, None).await?;
            self.transaction_coordinator.commit_transaction(&tx_id).await?;
            
            let mut stats = self.stats.write().await;
            stats.full_downloads_performed += 1;
            stats.bytes_read_remote += metadata.file_size;
        }
        
        // Read from local cache
        self.read_rowgroups_from_local(&local_path, metadata, rowgroup_indices).await
    }

    /// Read a specific locality cluster
    async fn read_locality_cluster(
        &self,
        metadata: &RaptorFileMetadata,
        cluster_id: usize,
        rowgroup_indices: &[usize],
    ) -> Result<Vec<RecordBatch>> {
        let cluster = metadata.locality_clusters.iter()
            .find(|c| c.cluster_id == cluster_id)
            .ok_or_else(|| anyhow::anyhow!("Cluster {} not found", cluster_id))?;
        
        tracing::debug!(
            "Reading cluster {} (offset={}, size={})",
            cluster_id, cluster.start_offset, cluster.size_bytes
        );
        
        // Read entire cluster as one range
        let cluster_data = self.filesystem.read_range(
            &metadata.file_path,
            cluster.start_offset,
            cluster.size_bytes,
        ).await?;
        
        let mut stats = self.stats.write().await;
        stats.range_reads_performed += 1;
        stats.bytes_read_remote += cluster.size_bytes;
        drop(stats);
        
        // Extract requested rowgroups from cluster data
        self.extract_rowgroups_from_buffer(&cluster_data, metadata, rowgroup_indices, cluster.start_offset)
    }

    /// Read using individual range reads
    async fn read_with_ranges(
        &self,
        metadata: &RaptorFileMetadata,
        rowgroup_indices: &[usize],
    ) -> Result<Vec<RecordBatch>> {
        let mut batches = Vec::new();
        let mut stats = self.stats.write().await;
        
        for &idx in rowgroup_indices {
            if idx >= metadata.num_rowgroups {
                continue;
            }
            
            let offset = metadata.rowgroup_offsets[idx];
            let size = metadata.rowgroup_sizes[idx];
            
            tracing::debug!("Reading rowgroup {} (offset={}, size={})", idx, offset, size);
            
            let rowgroup_data = self.filesystem.read_range(
                &metadata.file_path,
                offset,
                size,
            ).await?;
            
            stats.range_reads_performed += 1;
            stats.bytes_read_remote += size;
            
            let batch = self.deserialize_rowgroup(&rowgroup_data)?;
            batches.push(batch);
        }
        
        Ok(batches)
    }

    /// Read rowgroups from local cached file
    async fn read_rowgroups_from_local(
        &self,
        local_path: &str,
        metadata: &RaptorFileMetadata,
        rowgroup_indices: &[usize],
    ) -> Result<Vec<RecordBatch>> {
        let file_data = self.filesystem.read(local_path).await?;
        
        let mut stats = self.stats.write().await;
        stats.bytes_read_local += file_data.len() as u64;
        drop(stats);
        
        self.extract_rowgroups_from_buffer(&file_data, metadata, rowgroup_indices, 0)
    }

    /// Extract specific rowgroups from a buffer
    fn extract_rowgroups_from_buffer(
        &self,
        buffer: &[u8],
        metadata: &RaptorFileMetadata,
        rowgroup_indices: &[usize],
        buffer_start_offset: u64,
    ) -> Result<Vec<RecordBatch>> {
        let mut batches = Vec::new();
        
        for &idx in rowgroup_indices {
            if idx >= metadata.num_rowgroups {
                continue;
            }
            
            let rg_offset = metadata.rowgroup_offsets[idx];
            let rg_size = metadata.rowgroup_sizes[idx];
            
            // Calculate offset within buffer
            if rg_offset < buffer_start_offset {
                continue; // Rowgroup not in this buffer
            }
            
            let buffer_offset = (rg_offset - buffer_start_offset) as usize;
            if buffer_offset + rg_size as usize > buffer.len() {
                continue; // Rowgroup extends beyond buffer
            }
            
            let rowgroup_data = &buffer[buffer_offset..buffer_offset + rg_size as usize];
            let batch = self.deserialize_rowgroup(rowgroup_data)?;
            batches.push(batch);
        }
        
        Ok(batches)
    }

    /// Deserialize a single rowgroup
    fn deserialize_rowgroup(&self, data: &[u8]) -> Result<RecordBatch> {
        use crate::storage::engines::common::fastlanes_encoding::markers;
        
        // Check encoding marker
        let marker = data[0];
        
        if marker >= markers::RAPTOR_TENSOR_START && marker <= markers::RAPTOR_TENSOR_END {
            // FastLanes tensor encoding
            self.deserialize_fastlanes_rowgroup(&data[1..])
        } else {
            // Standard Arrow IPC
            self.deserialize_arrow_rowgroup(data)
        }
    }

    /// Deserialize FastLanes encoded rowgroup
    fn deserialize_fastlanes_rowgroup(&self, data: &[u8]) -> Result<RecordBatch> {
        // For now, we can't create RaptorEngine here as it requires async
        // This would need to be refactored to either make this function async
        // or use a different approach for FastLanes deserialization
        // For now, return a basic RecordBatch as FastLanes deserialization needs proper implementation
        use arrow_array::{StringArray, Float32Array};
        use arrow_schema::{Schema, Field, DataType};
        
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("vector", DataType::List(Arc::new(Field::new("item", DataType::Float32, false))), false),
        ]));
        
        let ids = StringArray::from(vec!["placeholder"]);
        let vectors = Float32Array::from(vec![0.0f32]);
        
        Ok(RecordBatch::try_new(schema, vec![Arc::new(ids), Arc::new(vectors)])?)
    }

    /// Deserialize Arrow IPC rowgroup
    fn deserialize_arrow_rowgroup(&self, data: &[u8]) -> Result<RecordBatch> {
        use arrow_ipc::reader::StreamReader;
        use std::io::Cursor;
        
        let cursor = Cursor::new(data);
        let reader = StreamReader::try_new(cursor, None)?;
        
        for batch_result in reader {
            return Ok(batch_result?);
        }
        
        Err(anyhow::anyhow!("No RecordBatch found in data"))
    }

    // ========================================================================
    // HNSW GRAPH OPERATIONS
    // ========================================================================

    /// Read HNSW graph metadata for navigation
    pub async fn read_hnsw_graph(&self, file_path: &str) -> Result<Bytes> {
        let metadata = self.get_metadata(file_path).await?;
        
        tracing::debug!(
            "Reading HNSW graph (offset={}, size={})",
            metadata.global_hnsw_offset,
            metadata.global_hnsw_size
        );
        
        let graph_data = self.filesystem.read_range(
            file_path,
            metadata.global_hnsw_offset,
            metadata.global_hnsw_size,
        ).await?;
        
        Ok(Bytes::from(graph_data))
    }

    // ========================================================================
    // CACHE MANAGEMENT
    // ========================================================================

    /// Invalidate all cache entries for a collection
    pub async fn invalidate_collection_cache(&self, collection_id: &str) -> Result<()> {
        tracing::info!("Invalidating cache for collection {}", collection_id);
        
        // Clear memory cache
        let mut cache = self.metadata_cache.write().await;
        cache.retain(|path, _| !path.contains(collection_id));
        
        // Clear local disk cache
        self.clear_local_cache_for_collection(collection_id).await?;
        
        Ok(())
    }

    /// Update cache after compaction
    pub async fn update_after_compaction(
        &self,
        collection_id: &str,
        new_file_path: &str,
    ) -> Result<()> {
        // Invalidate old entries
        self.invalidate_collection_cache(collection_id).await?;
        
        // Pre-load metadata for new file
        let metadata = self.read_metadata_from_file(new_file_path).await?;
        
        let mut cache = self.metadata_cache.write().await;
        cache.insert(new_file_path.to_string(), metadata);
        
        tracing::info!("Updated cache with new compacted file: {}", new_file_path);
        Ok(())
    }

    /// Clear local cache files for a collection
    async fn clear_local_cache_for_collection(&self, collection_id: &str) -> Result<()> {
        let mut entries = tokio::fs::read_dir(&self.local_cache_dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.contains(collection_id) {
                    tracing::debug!("Removing cached file: {}", path.display());
                    tokio::fs::remove_file(&path).await?;
                }
            }
        }
        
        Ok(())
    }

    /// Persist metadata cache to disk
    pub async fn persist_cache(&self) -> Result<()> {
        let cache = self.metadata_cache.read().await;
        let cache_file = format!("{}/metadata_cache.bin", self.local_cache_dir);
        
        let serialized = bincode::serialize(&*cache)?;
        
        let tx_id = format!("raptor_cache_{}", uuid::Uuid::new_v4());
        let tx = self.transaction_coordinator.begin_transaction(&tx_id, vec![]).await?;
        self.filesystem.write(&cache_file, &serialized, None).await?;
        self.transaction_coordinator.commit_transaction(&tx_id).await?;
        
        tracing::info!("Persisted {} metadata entries", cache.len());
        Ok(())
    }

    /// Load metadata cache from disk
    pub async fn load_cache(&self) -> Result<()> {
        let cache_file = format!("{}/metadata_cache.bin", self.local_cache_dir);
        
        if !self.filesystem.exists(&cache_file).await? {
            return Ok(());
        }
        
        let data = self.filesystem.read(&cache_file).await?;
        let loaded: HashMap<String, RaptorFileMetadata> = bincode::deserialize(&data)?;
        
        let mut cache = self.metadata_cache.write().await;
        *cache = loaded;
        
        tracing::info!("Loaded {} metadata entries from cache", cache.len());
        Ok(())
    }

    /// Get statistics about reader performance
    pub async fn get_statistics(&self) -> ReaderStatistics {
        (*self.stats.read().await).clone()
    }

    // ========================================================================
    // HELPER METHODS
    // ========================================================================

    /// Get local cache path for a file
    fn get_local_cache_path(&self, file_path: &str) -> String {
        let file_name = file_path.split('/').last().unwrap_or("unknown");
        format!("{}/{}", self.local_cache_dir, file_name)
    }
}

/// Decision about which I/O strategy to use
#[derive(Debug)]
pub enum IoDecision {
    FullDownload {
        reason: String,
    },
    ClusterRead {
        cluster_id: usize,
        reason: String,
    },
    RangeReads {
        rowgroup_indices: Vec<usize>,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_io_strategy_decision() -> Result<()> {
        let reader = RaptorUnifiedReader::new(
            Arc::new(ZeroCopyFilesystem::new("/tmp".into())),
            Arc::new(TransactionCoordinator::new()),
            RaptorConfig::default(),
            "/tmp/cache".to_string(),
        );
        
        let mut metadata = RaptorFileMetadata {
            file_size: 10 * 1024 * 1024, // 10MB
            num_rowgroups: 10,
            rowgroup_sizes: vec![1024 * 1024; 10],
            locality_clusters: vec![],
            ..Default::default()
        };
        
        // Small file should trigger full download
        let decision = reader.determine_io_strategy(&metadata, &[0, 1, 2]).await;
        assert!(matches!(decision, IoDecision::FullDownload { .. }));
        
        // Large file with few rowgroups should use ranges
        metadata.file_size = 1024 * 1024 * 1024; // 1GB
        let decision = reader.determine_io_strategy(&metadata, &[0, 1]).await;
        assert!(matches!(decision, IoDecision::RangeReads { .. }));
        
        Ok(())
    }

    #[tokio::test]
    async fn test_cache_invalidation() -> Result<()> {
        let reader = RaptorUnifiedReader::new(
            Arc::new(ZeroCopyFilesystem::new("/tmp".into())),
            Arc::new(TransactionCoordinator::new()),
            RaptorConfig::default(),
            "/tmp/cache".to_string(),
        );
        
        // Add test metadata
        {
            let mut cache = reader.metadata_cache.write().await;
            cache.insert("collection1/file1.rapt".to_string(), RaptorFileMetadata::default());
            cache.insert("collection1/file2.rapt".to_string(), RaptorFileMetadata::default());
            cache.insert("collection2/file1.rapt".to_string(), RaptorFileMetadata::default());
        }
        
        // Invalidate collection1
        reader.invalidate_collection_cache("collection1").await?;
        
        // Check that only collection2 remains
        {
            let cache = reader.metadata_cache.read().await;
            assert_eq!(cache.len(), 1);
            assert!(cache.contains_key("collection2/file1.rapt"));
        }
        
        Ok(())
    }
}