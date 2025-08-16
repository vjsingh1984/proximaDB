# SST and VIPER Dual-Mode Architecture Design

## Overview

This document describes the clean, forward-looking design for SST and VIPER storage engines that support both:
1. **Index-driven ID lookups** - When AXIS indexes return top-k IDs
2. **Index-free similarity search** - Direct storage search with progressive quantization

## Design Principles

1. **No Backward Compatibility**: Clean slate design for optimal performance
2. **Dual Mode Operation**: Equally optimized for ID lookup and similarity search
3. **Progressive Refinement**: Multi-level quantization for efficient filtering
4. **Hierarchical Organization**: Efficient data organization for billion-scale vectors
5. **Quantization-First**: Built-in support for multiple quantization levels
6. **Metadata-Aware**: Statistics and indexes at every level for intelligent pruning

## SST Engine Design

### 1. File Structure

```rust
pub struct SstFile {
    // File header containing all metadata
    header: SstHeader,
    
    // Three-tier hierarchy for billion-scale vectors
    superblocks: Vec<SuperBlock>,
    
    // Global indexes for different access patterns
    id_index: IdIndex,              // O(1) ID lookups
    quantized_index: QuantizedIndex, // Fast similarity pre-filtering
    metadata_index: MetadataIndex,   // Attribute filtering
    
    // Memory-mapped regions for zero-copy access
    mmap_regions: Vec<MmapRegion>,
}
```

### 2. Header Structure

```rust
pub struct SstHeader {
    // File identification
    magic: [u8; 8],              // "PROXSST\0"
    version: u32,                // File format version
    file_id: Uuid,               // Unique file identifier
    
    // Collection information
    collection_id: String,
    created_at: i64,
    compaction_level: u8,
    
    // Vector configuration
    dimension: usize,
    distance_metric: DistanceMetric,
    quantization_config: QuantizationConfig,
    
    // Record counts
    total_records: u64,
    deleted_records: u64,
    
    // Layout information
    superblock_count: u32,
    blocks_per_superblock: u32,
    records_per_block: u32,
    
    // Index offsets (from file start)
    superblock_offset: u64,
    id_index_offset: u64,
    quantized_index_offset: u64,
    metadata_index_offset: u64,
    
    // Checksums
    header_checksum: u32,
    file_checksum: u64,
}

pub struct QuantizationConfig {
    // Binary quantization (1 bit per dimension)
    enable_binary: bool,
    binary_threshold: f32,
    
    // INT8 quantization
    enable_int8: bool,
    int8_scale: f32,
    int8_zero_point: i8,
    
    // Product Quantization
    enable_pq: bool,
    pq_segments: u8,        // Number of segments (typically 8-64)
    pq_bits: u8,           // Bits per segment (typically 4-8)
    pq_codebooks: Vec<Codebook>,
    
    // Compression
    compression_algorithm: CompressionAlgorithm,
    compression_level: u8,
}
```

### 3. Hierarchical Block Structure

```rust
pub struct SuperBlock {
    // Metadata
    id: u32,
    size_bytes: u64,
    record_count: u32,
    
    // 1GB superblock = 64 x 16MB blocks
    blocks: Vec<DataBlock>,
    
    // Superblock-level indexes
    id_range: (String, String),     // Min/max IDs in this superblock
    centroid: Vec<f32>,             // Average vector for routing
    quantized_signature: Vec<u8>,   // PQ/binary signature for filtering
    
    // Bloom filter for existence checks
    bloom: BloomFilter,
    bloom_size: u32,
    bloom_hash_count: u8,
    
    // Metadata statistics for pruning
    metadata_stats: HashMap<String, ColumnStats>,
}

pub struct DataBlock {
    // Block metadata
    id: u32,
    offset_in_superblock: u64,
    compressed_size: u32,
    uncompressed_size: u32,
    
    // 16MB block = ~2000 vectors @ 8KB each
    records: Vec<VectorRecord>,
    
    // Block-level quantization for progressive search
    quantized_block: QuantizedBlock,
    
    // Block metadata for filtering
    id_range: (String, String),
    min_timestamp: i64,
    max_timestamp: i64,
    
    // Column statistics for metadata filtering
    metadata_stats: HashMap<String, ColumnStats>,
    
    // Optional compression
    compressed_data: Option<Vec<u8>>,
}

pub struct QuantizedBlock {
    // Multiple quantization levels for progressive search
    
    // Level 1: Binary sketches (1 bit per dimension)
    binary_sketches: Vec<BinarySketch>,
    
    // Level 2: INT8 vectors (8 bits per dimension)
    int8_vectors: Vec<Int8Vector>,
    
    // Level 3: Product Quantization codes (4-8 bits per dimension)
    pq_codes: Vec<PQCode>,
    
    // Precomputed distance tables for PQ (one per query)
    distance_tables: Arc<RwLock<HashMap<QueryId, DistanceTable>>>,
    
    // Block-specific quantization parameters
    local_scale: f32,
    local_offset: f32,
}

pub struct ColumnStats {
    column_name: String,
    data_type: DataType,
    null_count: u32,
    distinct_count: u32,
    min_value: Value,
    max_value: Value,
    
    // For numeric columns
    sum: Option<f64>,
    mean: Option<f64>,
    stddev: Option<f64>,
    
    // For string columns
    total_bytes: Option<u64>,
    max_length: Option<u32>,
    
    // Bloom filter for distinct values
    bloom: Option<BloomFilter>,
}
```

### 4. ID Index Structure

```rust
// B+ tree for O(log n) ID lookups
pub struct IdIndex {
    // B+ tree nodes
    root: BPlusNode,
    height: u32,
    node_count: u64,
    
    // Direct pointers to blocks for O(1) access after lookup
    id_to_location: HashMap<String, BlockLocation>,
    
    // Statistics
    total_ids: u64,
    unique_ids: u64,
    
    // Optional: Compressed trie for prefix searches
    prefix_trie: Option<CompressedTrie>,
}

pub enum BPlusNode {
    Internal {
        keys: Vec<String>,
        children: Vec<Box<BPlusNode>>,
        level: u32,
    },
    Leaf {
        entries: Vec<(String, BlockLocation)>,
        next: Option<Box<BPlusNode>>,  // For range scans
    },
}

pub struct BlockLocation {
    superblock_idx: u32,
    block_idx: u32,
    offset_in_block: u32,
    size_bytes: u32,
}

// For very large datasets, use a two-level index
pub struct TwoLevelIdIndex {
    // Top level: Sparse index (every Nth ID)
    sparse_index: BTreeMap<String, BlockRange>,
    
    // Bottom level: Dense index per block range
    dense_indexes: Vec<DenseIdIndex>,
    
    // Configuration
    sparse_factor: u32,  // Sample every N IDs
}

pub struct DenseIdIndex {
    start_id: String,
    end_id: String,
    entries: Vec<(String, u32)>,  // (ID, offset_in_block)
}
```

### 5. Quantized Index Structure

```rust
pub struct QuantizedIndex {
    // Global codebooks for PQ
    codebooks: Vec<Codebook>,
    
    // Hierarchical quantized centroids for routing
    level1_centroids: Vec<BinaryCentroid>,   // 32 centroids
    level2_centroids: Vec<Int8Centroid>,     // 256 centroids
    level3_centroids: Vec<PQCentroid>,       // 1024 centroids
    
    // Inverted index: centroid -> block list
    centroid_to_blocks: HashMap<CentroidId, Vec<BlockId>>,
    
    // Precomputed distances between centroids
    centroid_distances: Vec<Vec<f32>>,
}

pub struct Codebook {
    segment_id: u8,
    dimension: usize,
    centroids: Vec<Vec<f32>>,  // 256 centroids for 8-bit PQ
    
    // Precomputed distance table
    distance_table: Vec<Vec<f32>>,
}
```

### 6. Metadata Index Structure

```rust
pub struct MetadataIndex {
    // Column-specific indexes
    column_indexes: HashMap<String, ColumnIndex>,
    
    // Composite indexes for common query patterns
    composite_indexes: Vec<CompositeIndex>,
    
    // Statistics for query optimization
    table_stats: TableStatistics,
}

pub enum ColumnIndex {
    // For categorical columns
    Inverted {
        value_to_blocks: HashMap<Value, BitSet>,
        cardinality: u32,
    },
    
    // For numeric columns
    BTree {
        tree: BTreeMap<Value, BitSet>,
        min: Value,
        max: Value,
        histogram: Histogram,
    },
    
    // For text columns
    FullText {
        token_to_blocks: HashMap<String, BitSet>,
        total_tokens: u64,
    },
}

pub struct CompositeIndex {
    columns: Vec<String>,
    index_type: CompositeIndexType,
    data: BTreeMap<Vec<Value>, BitSet>,
}
```

## VIPER Engine Design

### 1. File Structure

```rust
pub struct ViperFile {
    // Parquet file with custom metadata
    parquet_path: String,
    parquet_metadata: ParquetMetadata,
    
    // VIPER-specific indexes stored separately
    id_index: ViperIdIndex,
    quantized_columns: QuantizedColumns,
    metadata_index: ParquetMetadataIndex,
    
    // Cache for hot data
    hot_cache: HotDataCache,
    
    // Memory-mapped regions for direct access
    mmap_regions: Vec<MmapRegion>,
}
```

### 2. Parquet Schema with Quantization

```rust
pub struct ViperSchema {
    // Required columns
    id_column: ColumnDescriptor,           // STRING, required, unique
    vector_column: ColumnDescriptor,       // FIXED_LEN_BYTE_ARRAY
    timestamp_column: ColumnDescriptor,    // INT64, required
    
    // Quantized columns (stored separately for columnar operations)
    binary_sketch_column: ColumnDescriptor,  // FIXED_LEN_BYTE_ARRAY
    int8_vector_column: ColumnDescriptor,    // FIXED_LEN_BYTE_ARRAY
    pq_codes_column: ColumnDescriptor,       // FIXED_LEN_BYTE_ARRAY
    
    // Metadata columns (dynamic)
    metadata_columns: HashMap<String, ColumnDescriptor>,
    
    // Statistics columns
    norm_column: ColumnDescriptor,         // FLOAT, vector norm
    quality_score_column: ColumnDescriptor, // FLOAT, optional
}

// Custom Parquet metadata for VIPER
pub struct ViperMetadata {
    // Vector configuration
    dimension: usize,
    distance_metric: DistanceMetric,
    
    // Quantization configuration
    quantization_config: QuantizationConfig,
    codebooks: Vec<SerializedCodebook>,
    
    // Collection metadata
    collection_id: String,
    created_at: i64,
    
    // Statistics
    total_vectors: u64,
    distinct_ids: u64,
    
    // Index locations
    id_index_path: String,
    metadata_index_path: String,
}
```

### 3. ID Index for Columnar Storage

```rust
pub struct ViperIdIndex {
    // Two-level index: row group -> page -> row
    row_group_index: Vec<RowGroupIdRange>,
    
    // Dense index for direct ID lookup (for small datasets < 10M)
    dense_index: Option<BTreeMap<String, RowLocation>>,
    
    // Sparse index for large datasets (sample every 1000 IDs)
    sparse_index: BTreeMap<String, RowGroupHint>,
    
    // Bloom filters per row group
    bloom_filters: Vec<BloomFilter>,
    
    // Statistics
    total_ids: u64,
    row_groups: u32,
    pages_per_row_group: u32,
}

pub struct RowGroupIdRange {
    row_group_idx: usize,
    file_offset: u64,
    compressed_size: u64,
    row_count: u32,
    
    // ID range in this row group
    min_id: String,
    max_id: String,
    
    // Bloom filter for existence check
    bloom_filter: BloomFilter,
    
    // Page-level index within row group
    page_ranges: Vec<PageIdRange>,
    
    // Column chunk locations
    column_chunks: HashMap<String, ColumnChunkMetadata>,
}

pub struct PageIdRange {
    page_idx: usize,
    first_id: String,
    last_id: String,
    row_count: u32,
    offset_in_row_group: u64,
    compressed_size: u32,
}

pub struct RowLocation {
    file_path: String,
    row_group: usize,
    page: usize,
    row_in_page: usize,
}

pub struct RowGroupHint {
    row_group_idx: usize,
    approximate_position: f32,  // 0.0 to 1.0 within row group
}
```

### 4. Quantized Columns Structure

```rust
pub struct QuantizedColumns {
    // Separate Parquet columns for each quantization level
    binary_column: ColumnPath,     // 1-bit sketches
    int8_column: ColumnPath,       // INT8 quantized
    pq_column: ColumnPath,         // PQ codes
    
    // Column metadata
    binary_metadata: BinaryColumnMetadata,
    int8_metadata: Int8ColumnMetadata,
    pq_metadata: PQColumnMetadata,
    
    // Codebooks stored as Parquet metadata
    pq_codebooks: Vec<Codebook>,
    
    // Distance computation accelerators
    simd_enabled: bool,
    gpu_enabled: bool,
}

pub struct BinaryColumnMetadata {
    bits_per_vector: u32,
    hash_functions: Vec<HashFunction>,
    threshold: f32,
}

pub struct Int8ColumnMetadata {
    scale: f32,
    zero_point: i8,
    min_value: i8,
    max_value: i8,
}

pub struct PQColumnMetadata {
    segments: u8,
    bits_per_segment: u8,
    codebook_size: u32,
    training_vectors: u32,
}
```

### 5. Hot Data Cache

```rust
pub struct HotDataCache {
    // LRU cache for frequently accessed records
    record_cache: moka::Cache<String, VectorRecord>,
    
    // Cache for quantized representations
    quantized_cache: moka::Cache<String, QuantizedVector>,
    
    // Row group cache (entire row groups in memory)
    row_group_cache: moka::Cache<usize, RowGroupData>,
    
    // Statistics
    hit_rate: AtomicF32,
    miss_rate: AtomicF32,
    eviction_count: AtomicU64,
}

pub struct RowGroupData {
    vectors: Vec<Vec<f32>>,
    ids: Vec<String>,
    metadata: Vec<HashMap<String, Value>>,
    quantized: Option<QuantizedRowGroup>,
}
```

## Search Algorithms

### 1. SST ID Lookup Algorithm

```rust
impl SstFile {
    pub async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<VectorRecord>> {
        let mut results = Vec::with_capacity(ids.len());
        
        // Step 1: Batch lookup in ID index
        let mut block_locations: Vec<(String, BlockLocation)> = Vec::new();
        for id in ids {
            if let Some(location) = self.id_index.lookup(id) {
                block_locations.push((id.to_string(), location));
            }
        }
        
        // Step 2: Group by superblock and block for efficient loading
        let mut grouped: HashMap<(u32, u32), Vec<(String, u32)>> = HashMap::new();
        for (id, loc) in block_locations {
            grouped
                .entry((loc.superblock_idx, loc.block_idx))
                .or_default()
                .push((id, loc.offset_in_block));
        }
        
        // Step 3: Load blocks in parallel (max 10 concurrent)
        let semaphore = Arc::new(Semaphore::new(10));
        let mut handles = Vec::new();
        
        for ((sb_idx, b_idx), id_offsets) in grouped {
            let sem = semaphore.clone();
            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await?;
                let block = self.load_block(sb_idx, b_idx).await?;
                
                let mut block_results = Vec::new();
                for (id, offset) in id_offsets {
                    if offset < block.records.len() as u32 {
                        block_results.push(block.records[offset as usize].clone());
                    }
                }
                Ok::<Vec<VectorRecord>, Error>(block_results)
            });
            handles.push(handle);
        }
        
        // Step 4: Collect results
        for handle in handles {
            results.extend(handle.await??);
        }
        
        Ok(results)
    }
}
```

### 2. SST Progressive Similarity Search

```rust
impl SstFile {
    pub async fn search_without_index(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<VectorRecord>> {
        // Phase 1: Binary sketch filtering (1 bit/dim)
        let binary_query = quantize_to_binary(query);
        let mut candidate_blocks = Vec::new();
        
        for superblock in &self.superblocks {
            // Check superblock centroid distance
            let binary_dist = hamming_distance(&binary_query, &superblock.quantized_signature);
            if binary_dist > BINARY_THRESHOLD {
                continue;
            }
            
            // Check individual blocks
            for (block_idx, block) in superblock.blocks.iter().enumerate() {
                // Skip blocks that don't match metadata filter
                if let Some(ref f) = filter {
                    if !f.matches_stats(&block.metadata_stats) {
                        continue;
                    }
                }
                
                // Compute binary distance for block
                let block_binary_dist = compute_block_binary_distance(
                    &binary_query,
                    &block.quantized_block.binary_sketches
                );
                
                if block_binary_dist <= BINARY_THRESHOLD {
                    candidate_blocks.push((superblock.id, block_idx, block_binary_dist));
                }
            }
        }
        
        // Sort by binary distance and keep top candidates
        candidate_blocks.sort_by_key(|&(_, _, dist)| dist);
        candidate_blocks.truncate(top_k * 10);
        
        // Phase 2: INT8 filtering
        let int8_query = quantize_to_int8(query);
        let mut int8_candidates = Vec::new();
        
        for (sb_idx, b_idx, _) in candidate_blocks {
            let block = self.load_block_quantized(sb_idx, b_idx).await?;
            
            for (idx, int8_vec) in block.quantized_block.int8_vectors.iter().enumerate() {
                let dist = compute_int8_distance(&int8_query, int8_vec);
                if dist <= INT8_THRESHOLD {
                    int8_candidates.push((sb_idx, b_idx, idx, dist));
                }
            }
        }
        
        // Sort and keep top candidates
        int8_candidates.sort_by_key(|&(_, _, _, dist)| dist);
        int8_candidates.truncate(top_k * 5);
        
        // Phase 3: PQ distance computation
        let pq_query = quantize_to_pq(query, &self.header.quantization_config);
        let mut pq_candidates = Vec::new();
        
        // Precompute distance tables for PQ
        let distance_tables = compute_distance_tables(&pq_query, &self.quantized_index.codebooks);
        
        for (sb_idx, b_idx, vec_idx, _) in int8_candidates {
            let block = self.load_block_quantized(sb_idx, b_idx).await?;
            let pq_code = &block.quantized_block.pq_codes[vec_idx];
            let dist = compute_pq_distance_with_table(pq_code, &distance_tables);
            pq_candidates.push((sb_idx, b_idx, vec_idx, dist));
        }
        
        // Sort and keep top candidates
        pq_candidates.sort_by_key(|&(_, _, _, dist)| OrderedFloat(dist));
        pq_candidates.truncate(top_k * 2);
        
        // Phase 4: Full precision reranking
        let mut final_results = Vec::new();
        
        for (sb_idx, b_idx, vec_idx, _) in pq_candidates {
            let block = self.load_block(sb_idx, b_idx).await?;
            let record = &block.records[vec_idx];
            
            // Apply metadata filter
            if let Some(ref f) = filter {
                if !f.matches(&record.metadata) {
                    continue;
                }
            }
            
            let distance = compute_distance(query, &record.vector, self.header.distance_metric);
            final_results.push((record.clone(), distance));
        }
        
        // Final sort and return top-k
        final_results.sort_by_key(|(_, dist)| OrderedFloat(*dist));
        final_results.truncate(top_k);
        
        Ok(final_results.into_iter().map(|(r, _)| r).collect())
    }
}
```

### 3. VIPER ID Lookup Algorithm

```rust
impl ViperFile {
    pub async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<VectorRecord>> {
        // Step 1: Check hot cache
        let mut results = Vec::new();
        let mut cache_misses = Vec::new();
        
        for id in ids {
            if let Some(record) = self.hot_cache.record_cache.get(id) {
                results.push(record.clone());
            } else {
                cache_misses.push(id);
            }
        }
        
        if cache_misses.is_empty() {
            return Ok(results);
        }
        
        // Step 2: Use bloom filters to find row groups
        let mut row_groups_to_read: HashMap<usize, Vec<String>> = HashMap::new();
        
        for id in cache_misses {
            // Check sparse index first for approximate location
            if let Some(hint) = self.id_index.find_approximate_location(id) {
                let rg_idx = hint.row_group_idx;
                if self.id_index.bloom_filters[rg_idx].might_contain(id) {
                    row_groups_to_read.entry(rg_idx).or_default().push(id.to_string());
                }
            } else {
                // Fallback: check all row groups with bloom filters
                for (rg_idx, bloom) in self.id_index.bloom_filters.iter().enumerate() {
                    if bloom.might_contain(id) {
                        row_groups_to_read.entry(rg_idx).or_default().push(id.to_string());
                    }
                }
            }
        }
        
        // Step 3: Read row groups efficiently
        for (rg_idx, ids_to_find) in row_groups_to_read {
            // First, read just the ID column
            let id_column = self.read_column(rg_idx, "id").await?;
            
            // Find positions of matching IDs
            let mut positions = Vec::new();
            for (pos, stored_id) in id_column.iter().enumerate() {
                if ids_to_find.contains(stored_id) {
                    positions.push(pos);
                }
            }
            
            if positions.is_empty() {
                continue;
            }
            
            // Read full records at specific positions
            let records = self.read_rows_at_positions(rg_idx, &positions).await?;
            
            // Update cache and results
            for record in records {
                self.hot_cache.record_cache.insert(record.id.clone(), record.clone());
                results.push(record);
            }
        }
        
        Ok(results)
    }
    
    async fn read_rows_at_positions(
        &self,
        row_group_idx: usize,
        positions: &[usize],
    ) -> Result<Vec<VectorRecord>> {
        let reader = ParquetReader::open(&self.parquet_path)?;
        let row_group = reader.get_row_group(row_group_idx)?;
        
        // Read only necessary columns
        let id_array = row_group.column("id").read_positions(positions)?;
        let vector_array = row_group.column("vector").read_positions(positions)?;
        let timestamp_array = row_group.column("timestamp").read_positions(positions)?;
        
        // Read metadata columns if present
        let metadata_columns = self.get_metadata_columns();
        let mut metadata_arrays = HashMap::new();
        for col_name in metadata_columns {
            if let Ok(array) = row_group.column(&col_name).read_positions(positions) {
                metadata_arrays.insert(col_name, array);
            }
        }
        
        // Construct VectorRecords
        let mut records = Vec::new();
        for i in 0..positions.len() {
            let mut metadata = HashMap::new();
            for (col_name, array) in &metadata_arrays {
                metadata.insert(col_name.clone(), array.get(i)?);
            }
            
            records.push(VectorRecord {
                id: id_array.get(i)?,
                vector: vector_array.get(i)?,
                timestamp: timestamp_array.get(i)?,
                metadata,
            });
        }
        
        Ok(records)
    }
}
```

### 4. VIPER Columnar Similarity Search

```rust
impl ViperFile {
    pub async fn search_without_index(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<VectorRecord>> {
        let num_row_groups = self.parquet_metadata.num_row_groups();
        let mut all_candidates = BinaryHeap::new();
        
        // Process each row group in parallel
        let semaphore = Arc::new(Semaphore::new(4)); // Max 4 concurrent row groups
        let mut handles = Vec::new();
        
        for rg_idx in 0..num_row_groups {
            let sem = semaphore.clone();
            let query = query.to_vec();
            let filter = filter.clone();
            
            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await?;
                self.search_row_group(rg_idx, &query, top_k * 2, filter).await
            });
            handles.push(handle);
        }
        
        // Collect results from all row groups
        for handle in handles {
            let candidates = handle.await??;
            for candidate in candidates {
                all_candidates.push(candidate);
                if all_candidates.len() > top_k * 10 {
                    all_candidates.pop();  // Remove worst
                }
            }
        }
        
        // Extract top-k candidates
        let mut final_candidates = Vec::new();
        while let Some(candidate) = all_candidates.pop() {
            final_candidates.push(candidate);
            if final_candidates.len() >= top_k * 2 {
                break;
            }
        }
        
        // Load full records for final candidates
        let mut results = Vec::new();
        for (_, rg_idx, row_idx) in final_candidates {
            let record = self.read_single_row(rg_idx, row_idx).await?;
            results.push(record);
            if results.len() >= top_k {
                break;
            }
        }
        
        Ok(results)
    }
    
    async fn search_row_group(
        &self,
        rg_idx: usize,
        query: &[f32],
        candidates_to_return: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<(f32, usize, usize)>> {
        // Phase 1: Binary filtering (columnar operation)
        let binary_column = self.read_column(rg_idx, "binary_sketch").await?;
        let binary_query = quantize_to_binary(query);
        
        // SIMD-accelerated Hamming distance computation
        let binary_distances = if self.quantized_columns.simd_enabled {
            compute_hamming_batch_simd(&binary_query, &binary_column)
        } else {
            compute_hamming_batch(&binary_query, &binary_column)
        };
        
        // Select top 20% based on binary distance
        let percentile_20 = select_percentile(&binary_distances, 0.2);
        let mut binary_candidates = Vec::new();
        for (idx, dist) in binary_distances.iter().enumerate() {
            if *dist <= percentile_20 {
                binary_candidates.push(idx);
            }
        }
        
        if binary_candidates.is_empty() {
            return Ok(Vec::new());
        }
        
        // Phase 2: Apply metadata filter if provided
        let filtered_candidates = if let Some(f) = filter {
            let mut filtered = Vec::new();
            
            // Read metadata columns for candidates
            for col_name in f.get_columns() {
                let col_data = self.read_column_subset(rg_idx, &col_name, &binary_candidates).await?;
                for (idx, value) in col_data.iter().enumerate() {
                    if f.matches_value(&col_name, value) {
                        filtered.push(binary_candidates[idx]);
                    }
                }
            }
            
            filtered
        } else {
            binary_candidates
        };
        
        if filtered_candidates.is_empty() {
            return Ok(Vec::new());
        }
        
        // Phase 3: PQ refinement
        let pq_column = self.read_column_subset(rg_idx, "pq_codes", &filtered_candidates).await?;
        let pq_query = quantize_to_pq(query, &self.quantized_columns.pq_metadata);
        
        // Precompute distance table
        let distance_table = compute_distance_table(&pq_query, &self.quantized_columns.pq_codebooks);
        
        // Compute PQ distances
        let mut pq_results = Vec::new();
        for (idx, pq_code) in pq_column.iter().enumerate() {
            let dist = lookup_pq_distance(pq_code, &distance_table);
            pq_results.push((dist, rg_idx, filtered_candidates[idx]));
        }
        
        // Sort and return top candidates
        pq_results.sort_by_key(|(dist, _, _)| OrderedFloat(*dist));
        pq_results.truncate(candidates_to_return);
        
        Ok(pq_results)
    }
}
```

## Performance Optimizations

### 1. Memory Management

```rust
pub struct MemoryManager {
    // Memory budget
    max_memory_bytes: usize,
    current_usage: AtomicUsize,
    
    // Memory pools
    block_pool: ObjectPool<DataBlock>,
    vector_pool: ObjectPool<Vec<f32>>,
    
    // Mmap regions
    mmap_regions: RwLock<Vec<MmapRegion>>,
}

impl MemoryManager {
    pub fn allocate_block(&self) -> Result<PooledObject<DataBlock>> {
        let size = std::mem::size_of::<DataBlock>();
        if self.current_usage.fetch_add(size, Ordering::Relaxed) + size > self.max_memory_bytes {
            self.evict_cold_data()?;
        }
        Ok(self.block_pool.get())
    }
    
    pub fn mmap_file_region(&self, path: &Path, offset: u64, len: usize) -> Result<MmapRegion> {
        let file = File::open(path)?;
        let mmap = unsafe {
            MmapOptions::new()
                .offset(offset)
                .len(len)
                .map(&file)?
        };
        
        let region = MmapRegion {
            mmap: Arc::new(mmap),
            path: path.to_path_buf(),
            offset,
            len,
        };
        
        self.mmap_regions.write().unwrap().push(region.clone());
        Ok(region)
    }
}
```

### 2. SIMD Acceleration

```rust
#[cfg(target_arch = "x86_64")]
pub mod simd {
    use std::arch::x86_64::*;
    
    pub unsafe fn hamming_distance_avx2(a: &[u8], b: &[u8]) -> u32 {
        let chunks = a.len() / 32;
        let mut total = 0u32;
        
        for i in 0..chunks {
            let offset = i * 32;
            let va = _mm256_loadu_si256(a[offset..].as_ptr() as *const __m256i);
            let vb = _mm256_loadu_si256(b[offset..].as_ptr() as *const __m256i);
            let vxor = _mm256_xor_si256(va, vb);
            let popcount = _mm256_popcnt_epi8(vxor);
            let sum = _mm256_sad_epu8(popcount, _mm256_setzero_si256());
            total += _mm256_extract_epi32(sum, 0) as u32;
            total += _mm256_extract_epi32(sum, 4) as u32;
        }
        
        // Handle remainder
        for i in (chunks * 32)..a.len() {
            total += (a[i] ^ b[i]).count_ones();
        }
        
        total
    }
    
    pub unsafe fn dot_product_avx512(a: &[f32], b: &[f32]) -> f32 {
        let chunks = a.len() / 16;
        let mut sum = _mm512_setzero_ps();
        
        for i in 0..chunks {
            let offset = i * 16;
            let va = _mm512_loadu_ps(&a[offset]);
            let vb = _mm512_loadu_ps(&b[offset]);
            sum = _mm512_fmadd_ps(va, vb, sum);
        }
        
        let mut result = _mm512_reduce_add_ps(sum);
        
        // Handle remainder
        for i in (chunks * 16)..a.len() {
            result += a[i] * b[i];
        }
        
        result
    }
}
```

### 3. GPU Acceleration (Optional)

```rust
#[cfg(feature = "cuda")]
pub mod gpu {
    use cudarc::driver::*;
    
    pub struct GpuAccelerator {
        device: Arc<CudaDevice>,
        pq_kernel: CudaFunction,
        distance_kernel: CudaFunction,
    }
    
    impl GpuAccelerator {
        pub fn compute_pq_distances_batch(
            &self,
            queries: &[Vec<f32>],
            codes: &[Vec<u8>],
            codebooks: &[Codebook],
        ) -> Result<Vec<Vec<f32>>> {
            // Transfer data to GPU
            let d_queries = self.device.htod_copy(queries)?;
            let d_codes = self.device.htod_copy(codes)?;
            let d_codebooks = self.device.htod_copy(codebooks)?;
            
            // Allocate output
            let d_distances = self.device.alloc_zeros::<f32>(queries.len() * codes.len())?;
            
            // Launch kernel
            let block_size = 256;
            let grid_size = (codes.len() + block_size - 1) / block_size;
            
            self.pq_kernel.launch(
                &[d_queries, d_codes, d_codebooks, d_distances],
                grid_size,
                block_size,
            )?;
            
            // Copy results back
            let distances = self.device.dtoh_copy(&d_distances)?;
            
            // Reshape into 2D array
            Ok(distances.chunks(codes.len()).map(|c| c.to_vec()).collect())
        }
    }
}
```

## Testing Strategy

### 1. Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_id_index_lookup() {
        let mut index = IdIndex::new();
        
        // Insert test data
        for i in 0..1000 {
            let id = format!("id_{:04}", i);
            let location = BlockLocation {
                superblock_idx: i / 100,
                block_idx: (i % 100) / 10,
                offset_in_block: (i % 10) as u32,
                size_bytes: 1024,
            };
            index.insert(id, location);
        }
        
        // Test lookups
        let loc = index.lookup("id_0500").unwrap();
        assert_eq!(loc.superblock_idx, 5);
        assert_eq!(loc.block_idx, 0);
        assert_eq!(loc.offset_in_block, 0);
        
        // Test non-existent
        assert!(index.lookup("id_9999").is_none());
    }
    
    #[test]
    fn test_progressive_search() {
        let sst = create_test_sst_file();
        let query = vec![1.0; 128];
        
        // Test binary filtering
        let binary_candidates = sst.filter_by_binary(&query, 100);
        assert!(binary_candidates.len() <= 100);
        
        // Test INT8 filtering
        let int8_candidates = sst.filter_by_int8(&query, &binary_candidates, 50);
        assert!(int8_candidates.len() <= 50);
        
        // Test PQ filtering
        let pq_candidates = sst.filter_by_pq(&query, &int8_candidates, 10);
        assert!(pq_candidates.len() <= 10);
    }
}
```

### 2. Integration Tests

```rust
#[tokio::test]
async fn test_sst_id_lookup_integration() {
    let sst = SstFile::open("test_data/test.sst").await.unwrap();
    
    let ids = vec![
        "id_001".to_string(),
        "id_100".to_string(),
        "id_999".to_string(),
    ];
    
    let records = sst.get_by_ids(&ids).await.unwrap();
    assert_eq!(records.len(), 3);
    
    for (record, expected_id) in records.iter().zip(ids.iter()) {
        assert_eq!(record.id, *expected_id);
    }
}

#[tokio::test]
async fn test_viper_columnar_search() {
    let viper = ViperFile::open("test_data/test.parquet").await.unwrap();
    
    let query = vec![0.5; 128];
    let filter = MetadataFilter::new()
        .add_condition("category", "electronics")
        .add_range("price", 100.0, 500.0);
    
    let results = viper.search_without_index(&query, 10, Some(filter)).await.unwrap();
    
    assert!(results.len() <= 10);
    for record in results {
        assert_eq!(record.metadata["category"], "electronics");
        let price: f64 = record.metadata["price"].parse().unwrap();
        assert!(price >= 100.0 && price <= 500.0);
    }
}
```

### 3. Benchmark Tests

```rust
#[bench]
fn bench_id_lookup_sst(b: &mut Bencher) {
    let sst = create_large_sst_file(1_000_000);
    let ids: Vec<String> = (0..100).map(|i| format!("id_{:07}", i * 10000)).collect();
    
    b.iter(|| {
        let _ = black_box(sst.get_by_ids(&ids));
    });
}

#[bench]
fn bench_progressive_search(b: &mut Bencher) {
    let sst = create_large_sst_file(1_000_000);
    let query = vec![0.5; 768];
    
    b.iter(|| {
        let _ = black_box(sst.search_without_index(&query, 10, None));
    });
}
```

## Migration Plan

### Phase 1: Core Infrastructure (Week 1-2)
- Implement new header structures
- Create ID index infrastructure
- Add quantization support

### Phase 2: SST Implementation (Week 3-4)
- Implement hierarchical block structure
- Add progressive search
- Integrate ID index

### Phase 3: VIPER Implementation (Week 5-6)
- Extend Parquet schema
- Implement columnar operations
- Add ID index for row groups

### Phase 4: Testing & Optimization (Week 7-8)
- Comprehensive testing
- Performance benchmarking
- SIMD/GPU optimization

### Phase 5: Integration (Week 9-10)
- Integrate with AXIS
- Update VectorOperationsService
- Production deployment

## Performance Targets

### ID Lookup Performance
- SST: < 1ms for single ID, < 10ms for 100 IDs
- VIPER: < 2ms for single ID, < 20ms for 100 IDs

### Similarity Search Performance (1M vectors)
- SST: < 50ms for top-10 without index
- VIPER: < 100ms for top-10 without index

### Space Efficiency
- 75-90% compression with quantization
- ID index overhead < 2%
- Metadata index overhead < 5%

### Scalability
- Support 1B+ vectors per collection
- Linear scaling with data size
- Efficient memory usage < 10GB for 1B vectors