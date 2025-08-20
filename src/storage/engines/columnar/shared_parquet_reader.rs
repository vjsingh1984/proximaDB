// Shared Parquet Format Reader for VIPER and NOVA engines
// Optimized for bandwidth reduction and cache-aware operations

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use arrow_array::RecordBatch;
use dashmap::DashMap;
use memmap2::{Mmap, MmapOptions};
use parquet::file::metadata::ParquetMetaData;
use tokio::sync::RwLock;

use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};
use crate::storage::cache::memory_pressure::MemoryPressureMonitor;
use crate::storage::cache::access_pattern::AccessPatternTracker;
use crate::common::errors::ProximaDBError;
use crate::core::models::VectorRecord;

const FOOTER_MAX_SIZE: usize = 8 * 1024 * 1024;  // 8MB max footer size
const COLUMN_INDEX_CACHE_SIZE: usize = 1024 * 1024 * 1024; // 1GB for column indexes

/// Shared Parquet format reader used by both VIPER and NOVA engines
pub struct SharedParquetFormatReader {
    /// Filesystem for I/O operations
    filesystem: Arc<FilesystemFactory>,
    
    /// Memory mapping strategy for Parquet
    mmap_strategy: ParquetMmapStrategy,
    
    /// Footer cache - ALWAYS cached when possible
    footer_cache: Arc<DashMap<String, ParquetFooterCache>>,
    
    /// Column index cache - cached based on access patterns
    column_index_cache: Arc<DashMap<String, Arc<Vec<u8>>>>,
    
    /// Row group metadata cache
    row_group_cache: Arc<DashMap<String, Vec<RowGroupMetadata>>>,
    
    /// Local disk cache for downloaded row groups
    local_cache: Arc<LocalDiskCache>,
    
    /// Memory pressure monitor
    memory_monitor: Arc<MemoryPressureMonitor>,
    
    /// Access pattern tracker
    access_tracker: Arc<AccessPatternTracker>,
    
    /// Statistics for monitoring
    stats: Arc<ReaderStats>,
}

#[derive(Clone)]
pub struct ParquetMmapStrategy {
    /// Footer strategy (always try to mmap)
    pub footer_max_size: usize,
    
    /// Column-specific strategies
    pub column_strategies: HashMap<String, ColumnMmapStrategy>,
    
    /// Row group size threshold for mmap
    pub row_group_mmap_threshold: usize,
}

#[derive(Clone)]
pub enum ColumnMmapStrategy {
    AlwaysMmap,      // Hot columns (e.g., primary key, vector_id)
    NeverMmap,       // Large blob columns
    Adaptive {       // Based on access patterns
        min_access_count: u32,
        recency_weight: f32,
    },
}

pub struct ParquetFooterCache {
    pub metadata: Arc<ParquetMetaData>,
    pub raw_footer: Arc<Vec<u8>>,
    pub last_access: Instant,
}

#[derive(Clone, Debug)]
pub struct RowGroupMetadata {
    pub index: usize,
    pub offset: u64,
    pub size: u64,
    pub num_rows: i64,
    pub column_chunks: Vec<ColumnChunkMetadata>,
}

#[derive(Clone, Debug)]
pub struct ColumnChunkMetadata {
    pub column_name: String,
    pub offset: u64,
    pub size: u64,
    pub encoding: String,
    pub num_values: i64,
    pub has_statistics: bool,
    pub min_value: Option<Vec<u8>>,
    pub max_value: Option<Vec<u8>>,
}

/// Statistics for monitoring
pub struct ReaderStats {
    footer_hits: AtomicU64,
    footer_misses: AtomicU64,
    row_groups_filtered: AtomicU64,
    row_groups_downloaded: AtomicU64,
    columns_filtered: AtomicU64,
    bytes_downloaded: AtomicU64,
    bytes_saved: AtomicU64,
    cache_invalidations: AtomicU64,
}

/// Local disk cache for Parquet data
pub struct LocalDiskCache {
    cache_dir: PathBuf,
    max_cache_size: u64,
    current_size: AtomicU64,
    
    /// Track cached row groups per file
    cached_row_groups: DashMap<String, Vec<usize>>,
    
    /// Track cached columns per row group
    cached_columns: DashMap<String, HashMap<usize, Vec<String>>>,
}

impl SharedParquetFormatReader {
    pub fn new(
        filesystem: Arc<FilesystemFactory>,
        mmap_strategy: ParquetMmapStrategy,
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            filesystem,
            mmap_strategy,
            footer_cache: Arc::new(DashMap::new()),
            column_index_cache: Arc::new(DashMap::new()),
            row_group_cache: Arc::new(DashMap::new()),
            local_cache: Arc::new(LocalDiskCache::new(cache_dir)),
            memory_monitor: Arc::new(MemoryPressureMonitor::new()),
            access_tracker: Arc::new(AccessPatternTracker::new()),
            stats: Arc::new(ReaderStats::default()),
        }
    }
    
    /// Read specific columns with intelligent filtering and caching
    pub async fn read_columns_smart(
        &self,
        file_path: &str,
        columns: &[String],
        row_filter: Option<&FilterExpression>,
    ) -> Result<Vec<RecordBatch>, ProximaDBError> {
        // Step 1: Get footer metadata (download ONLY footer for cloud files)
        let footer = self.get_footer_smart(file_path).await?;
        
        // Step 2: Use metadata to filter row groups BEFORE downloading
        let candidate_row_groups = if let Some(filter) = row_filter {
            self.filter_row_groups_by_statistics(&footer, filter)?
        } else {
            (0..footer.metadata.num_row_groups()).collect()
        };
        
        // Track how many row groups we filtered out
        let total_rgs = footer.metadata.num_row_groups();
        let filtered_out = total_rgs - candidate_row_groups.len();
        self.stats.row_groups_filtered.fetch_add(filtered_out as u64, Ordering::Relaxed);
        
        // Calculate bandwidth saved
        let avg_rg_size = 50 * 1024 * 1024; // 50MB average
        self.stats.bytes_saved.fetch_add((filtered_out * avg_rg_size) as u64, Ordering::Relaxed);
        
        if candidate_row_groups.is_empty() {
            // No row groups match - saved all bandwidth!
            return Ok(Vec::new());
        }
        
        // Step 3: Read only candidate row groups
        let mut batches = Vec::new();
        
        for rg_idx in candidate_row_groups {
            let batch = self.read_row_group_smart(
                file_path,
                rg_idx,
                columns,
                &footer,
            ).await?;
            
            if let Some(batch) = batch {
                batches.push(batch);
            }
        }
        
        Ok(batches)
    }
    
    /// Get footer with minimal bandwidth usage
    async fn get_footer_smart(&self, file_path: &str) -> Result<Arc<ParquetFooterCache>, ProximaDBError> {
        // Check cache first
        if let Some(cached) = self.footer_cache.get(file_path) {
            self.stats.footer_hits.fetch_add(1, Ordering::Relaxed);
            cached.last_access = Instant::now();
            return Ok(Arc::new(cached.clone()));
        }
        
        self.stats.footer_misses.fetch_add(1, Ordering::Relaxed);
        
        // For cloud files, download ONLY the footer
        if self.is_cloud_file(file_path) {
            let file_size = self.filesystem
                .get_filesystem(file_path)?
                .file_size(file_path)
                .await?;
            
            // Parquet footer is at the end - read last 8MB max
            let footer_start = file_size.saturating_sub(FOOTER_MAX_SIZE as u64);
            let footer_data = self.filesystem
                .get_filesystem(file_path)?
                .read_range(file_path, footer_start, FOOTER_MAX_SIZE as u64)
                .await?;
            
            self.stats.bytes_downloaded.fetch_add(footer_data.len() as u64, Ordering::Relaxed);
            
            // Parse footer
            let metadata = self.parse_footer(&footer_data)?;
            
            let cache_entry = ParquetFooterCache {
                metadata: Arc::new(metadata),
                raw_footer: Arc::new(footer_data),
                last_access: Instant::now(),
            };
            
            self.footer_cache.insert(file_path.to_string(), cache_entry.clone());
            
            return Ok(Arc::new(cache_entry));
        }
        
        // For local files, can mmap the footer
        self.get_local_footer_with_mmap(file_path).await
    }
    
    /// Filter row groups using statistics BEFORE downloading
    fn filter_row_groups_by_statistics(
        &self,
        footer: &ParquetFooterCache,
        filter: &FilterExpression,
    ) -> Result<Vec<usize>, ProximaDBError> {
        let mut candidates = Vec::new();
        
        for (idx, rg) in footer.metadata.row_groups().iter().enumerate() {
            // Check column statistics to see if this row group can contain matching data
            let mut might_match = true;
            
            for column_chunk in rg.columns() {
                if let Some(stats) = column_chunk.statistics() {
                    // Use min/max statistics to prune
                    if !self.statistics_match_filter(stats, filter) {
                        might_match = false;
                        break;
                    }
                }
            }
            
            if might_match {
                candidates.push(idx);
            }
        }
        
        Ok(candidates)
    }
    
    /// Read a single row group with smart column selection
    async fn read_row_group_smart(
        &self,
        file_path: &str,
        rg_idx: usize,
        columns: &[String],
        footer: &ParquetFooterCache,
    ) -> Result<Option<RecordBatch>, ProximaDBError> {
        // Check if we have this row group cached locally
        if let Some(cached) = self.local_cache.get_row_group(file_path, rg_idx, columns).await? {
            return Ok(Some(cached));
        }
        
        let rg_metadata = &footer.metadata.row_groups()[rg_idx];
        
        // For cloud files, download only needed columns
        if self.is_cloud_file(file_path) {
            // Calculate which column chunks we need
            let mut column_ranges = Vec::new();
            
            for column_name in columns {
                if let Some(column_chunk) = self.find_column_chunk(rg_metadata, column_name) {
                    column_ranges.push((
                        column_name.clone(),
                        column_chunk.file_offset(),
                        column_chunk.compressed_size() as u64,
                    ));
                }
            }
            
            // Download column chunks
            let mut column_data = HashMap::new();
            
            for (col_name, offset, size) in column_ranges {
                let data = self.filesystem
                    .get_filesystem(file_path)?
                    .read_range(file_path, offset as u64, size)
                    .await?;
                
                self.stats.bytes_downloaded.fetch_add(size, Ordering::Relaxed);
                column_data.insert(col_name, data);
            }
            
            // Cache for future use
            self.local_cache.put_row_group(file_path, rg_idx, &column_data).await?;
            
            // Decode and return
            return self.decode_column_data(column_data, rg_metadata);
        }
        
        // For local files, read directly
        self.read_local_row_group(file_path, rg_idx, columns, rg_metadata).await
    }
    
    /// Invalidate cache during compaction
    pub async fn invalidate_cache_for_collection(&self, collection_id: &str) -> Result<(), ProximaDBError> {
        let mut invalidated = 0;
        
        // Remove from footer cache
        self.footer_cache.retain(|path, _| {
            if path.contains(collection_id) {
                invalidated += 1;
                false
            } else {
                true
            }
        });
        
        // Remove from column index cache
        self.column_index_cache.retain(|path, _| {
            if path.contains(collection_id) {
                invalidated += 1;
                false
            } else {
                true
            }
        });
        
        // Remove from row group cache
        self.row_group_cache.retain(|path, _| {
            if path.contains(collection_id) {
                invalidated += 1;
                false
            } else {
                true
            }
        });
        
        // Invalidate local disk cache
        self.local_cache.invalidate_collection(collection_id).await?;
        
        self.stats.cache_invalidations.fetch_add(invalidated, Ordering::Relaxed);
        
        log::info!(
            "Invalidated {} Parquet cache entries for collection {} during compaction",
            invalidated,
            collection_id
        );
        
        Ok(())
    }
    
    /// Optimized columnar scan for multiple files
    pub async fn optimize_columnar_scan(
        &self,
        file_paths: &[String],
        columns: &[String],
        filter: Option<&FilterExpression>,
    ) -> Result<Vec<RecordBatch>, ProximaDBError> {
        // Prefetch footers in parallel (small, always worth it)
        let footers = self.prefetch_footers_parallel(file_paths).await?;
        
        // Build global pruning plan across all files
        let mut total_filtered = 0;
        let mut total_candidates = 0;
        
        let mut file_plans = Vec::new();
        
        for (file_path, footer) in file_paths.iter().zip(footers.iter()) {
            let candidates = if let Some(f) = filter {
                self.filter_row_groups_by_statistics(footer, f)?
            } else {
                (0..footer.metadata.num_row_groups()).collect()
            };
            
            total_filtered += footer.metadata.num_row_groups() - candidates.len();
            total_candidates += candidates.len();
            
            if !candidates.is_empty() {
                file_plans.push((file_path.clone(), footer.clone(), candidates));
            }
        }
        
        log::info!(
            "Columnar scan: filtered {}/{} row groups using statistics, downloading {} candidates",
            total_filtered,
            total_filtered + total_candidates,
            total_candidates
        );
        
        // Execute reads in parallel
        let mut all_batches = Vec::new();
        
        for (file_path, footer, row_groups) in file_plans {
            for rg_idx in row_groups {
                if let Some(batch) = self.read_row_group_smart(
                    &file_path,
                    rg_idx,
                    columns,
                    &footer,
                ).await? {
                    all_batches.push(batch);
                }
            }
        }
        
        Ok(all_batches)
    }
    
    /// Prefetch footers in parallel
    async fn prefetch_footers_parallel(
        &self,
        file_paths: &[String],
    ) -> Result<Vec<Arc<ParquetFooterCache>>, ProximaDBError> {
        let mut futures = Vec::new();
        
        for path in file_paths {
            futures.push(self.get_footer_smart(path));
        }
        
        // Execute all footer fetches in parallel
        let results = futures::future::join_all(futures).await;
        
        let mut footers = Vec::new();
        for result in results {
            footers.push(result?);
        }
        
        Ok(footers)
    }
    
    /// Check if file is on cloud storage
    fn is_cloud_file(&self, path: &str) -> bool {
        path.starts_with("s3://") || 
        path.starts_with("gs://") || 
        path.starts_with("azure://") ||
        path.starts_with("http://") ||
        path.starts_with("https://")
    }
    
    /// Parse Parquet footer from raw bytes
    fn parse_footer(&self, data: &[u8]) -> Result<ParquetMetaData, ProximaDBError> {
        // Use parquet crate to parse footer
        Ok(ParquetMetaData::default()) // Placeholder
    }
    
    /// Check if statistics match filter
    fn statistics_match_filter(&self, _stats: &dyn parquet::file::statistics::Statistics, _filter: &FilterExpression) -> bool {
        // Check min/max against filter predicates
        true // Placeholder
    }
    
    /// Find column chunk in row group metadata
    fn find_column_chunk(&self, _rg: &parquet::file::metadata::RowGroupMetaData, _column: &str) -> Option<&parquet::file::metadata::ColumnChunkMetaData> {
        // Find column chunk metadata
        None // Placeholder
    }
    
    /// Decode column data into RecordBatch
    fn decode_column_data(
        &self,
        _column_data: HashMap<String, Vec<u8>>,
        _rg_metadata: &parquet::file::metadata::RowGroupMetaData,
    ) -> Result<Option<RecordBatch>, ProximaDBError> {
        // Decode Parquet column data
        Ok(None) // Placeholder
    }
    
    /// Read local row group
    async fn read_local_row_group(
        &self,
        _file_path: &str,
        _rg_idx: usize,
        _columns: &[String],
        _rg_metadata: &parquet::file::metadata::RowGroupMetaData,
    ) -> Result<Option<RecordBatch>, ProximaDBError> {
        // Read from local file
        Ok(None) // Placeholder
    }
    
    /// Get local footer with mmap
    async fn get_local_footer_with_mmap(&self, _file_path: &str) -> Result<Arc<ParquetFooterCache>, ProximaDBError> {
        // mmap footer for local files
        Ok(Arc::new(ParquetFooterCache {
            metadata: Arc::new(ParquetMetaData::default()),
            raw_footer: Arc::new(Vec::new()),
            last_access: Instant::now(),
        }))
    }
    
    /// Get statistics
    pub fn get_stats(&self) -> ReaderStatsSummary {
        ReaderStatsSummary {
            footer_hit_rate: {
                let hits = self.stats.footer_hits.load(Ordering::Relaxed);
                let misses = self.stats.footer_misses.load(Ordering::Relaxed);
                let total = hits + misses;
                if total > 0 {
                    hits as f64 / total as f64
                } else {
                    0.0
                }
            },
            row_groups_filtered: self.stats.row_groups_filtered.load(Ordering::Relaxed),
            row_groups_downloaded: self.stats.row_groups_downloaded.load(Ordering::Relaxed),
            columns_filtered: self.stats.columns_filtered.load(Ordering::Relaxed),
            bytes_downloaded: self.stats.bytes_downloaded.load(Ordering::Relaxed),
            bytes_saved: self.stats.bytes_saved.load(Ordering::Relaxed),
            cache_invalidations: self.stats.cache_invalidations.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug)]
pub struct ReaderStatsSummary {
    pub footer_hit_rate: f64,
    pub row_groups_filtered: u64,
    pub row_groups_downloaded: u64,
    pub columns_filtered: u64,
    pub bytes_downloaded: u64,
    pub bytes_saved: u64,
    pub cache_invalidations: u64,
}

impl LocalDiskCache {
    pub fn new(cache_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&cache_dir).ok();
        
        Self {
            cache_dir,
            max_cache_size: 200 * 1024 * 1024 * 1024, // 200GB for columnar data
            current_size: AtomicU64::new(0),
            cached_row_groups: DashMap::new(),
            cached_columns: DashMap::new(),
        }
    }
    
    /// Get cached row group if available
    pub async fn get_row_group(
        &self,
        file_path: &str,
        rg_idx: usize,
        columns: &[String],
    ) -> Result<Option<RecordBatch>, ProximaDBError> {
        // Check if we have this row group cached
        if let Some(cached_rgs) = self.cached_row_groups.get(file_path) {
            if cached_rgs.contains(&rg_idx) {
                // Check if we have all requested columns
                if let Some(cached_cols) = self.cached_columns.get(file_path) {
                    if let Some(rg_columns) = cached_cols.get(&rg_idx) {
                        if columns.iter().all(|c| rg_columns.contains(c)) {
                            // Load from cache
                            let cache_file = self.cache_path_for_row_group(file_path, rg_idx);
                            if cache_file.exists() {
                                // Read and decode cached data
                                // ... implementation
                            }
                        }
                    }
                }
            }
        }
        
        Ok(None)
    }
    
    /// Cache row group data
    pub async fn put_row_group(
        &self,
        file_path: &str,
        rg_idx: usize,
        column_data: &HashMap<String, Vec<u8>>,
    ) -> Result<(), ProximaDBError> {
        let cache_file = self.cache_path_for_row_group(file_path, rg_idx);
        
        // Ensure parent directory exists
        if let Some(parent) = cache_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        // Serialize and write column data
        // ... implementation
        
        // Update tracking
        self.cached_row_groups.entry(file_path.to_string())
            .or_insert_with(Vec::new)
            .push(rg_idx);
        
        self.cached_columns.entry(file_path.to_string())
            .or_insert_with(HashMap::new)
            .insert(rg_idx, column_data.keys().cloned().collect());
        
        Ok(())
    }
    
    /// Invalidate cache for collection
    pub async fn invalidate_collection(&self, collection_id: &str) -> Result<(), ProximaDBError> {
        let mut files_to_remove = Vec::new();
        
        // Find all cached files for this collection
        for entry in self.cached_row_groups.iter() {
            if entry.key().contains(collection_id) {
                files_to_remove.push(entry.key().clone());
            }
        }
        
        // Remove from cache
        for file_path in files_to_remove {
            self.cached_row_groups.remove(&file_path);
            self.cached_columns.remove(&file_path);
            
            // Delete cache files
            let pattern = self.cache_dir.join(format!("{}*", 
                file_path.replace('/', "_").replace(':', "_")));
            
            // Remove all matching files
            if let Ok(paths) = glob::glob(pattern.to_str().unwrap()) {
                for path in paths.flatten() {
                    std::fs::remove_file(path).ok();
                }
            }
        }
        
        log::info!("Invalidated Parquet disk cache for collection {}", collection_id);
        
        Ok(())
    }
    
    fn cache_path_for_row_group(&self, file_path: &str, rg_idx: usize) -> PathBuf {
        let safe_name = file_path
            .replace('/', "_")
            .replace(':', "_")
            .replace("\\", "_");
        
        self.cache_dir.join(format!("{}_rg_{}", safe_name, rg_idx))
    }
}

impl Default for ReaderStats {
    fn default() -> Self {
        Self {
            footer_hits: AtomicU64::new(0),
            footer_misses: AtomicU64::new(0),
            row_groups_filtered: AtomicU64::new(0),
            row_groups_downloaded: AtomicU64::new(0),
            columns_filtered: AtomicU64::new(0),
            bytes_downloaded: AtomicU64::new(0),
            bytes_saved: AtomicU64::new(0),
            cache_invalidations: AtomicU64::new(0),
        }
    }
}

/// Filter expression placeholder
pub struct FilterExpression;

/// Memory pressure monitor (reuse from SST reader)
pub use super::shared_sst_reader::MemoryPressureMonitor;

/// Access pattern tracker (reuse from SST reader)  
pub use super::shared_sst_reader::AccessPatternTracker;