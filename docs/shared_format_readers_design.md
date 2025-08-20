# Shared Format Readers with Intelligent Memory Mapping

## Key Insight
Instead of duplicating mmap logic in each engine, create **shared format-specific readers** that understand the data layout and can be reused across engines.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Storage Engines                           │
├──────────────────┬──────────────────┬──────────────────────┤
│      SST         │     SWIFT        │    Row-Based Family  │
│                  │                  │                       │
└────────┬─────────┴─────────┬────────┴──────────────────────┘
         │                   │
         ▼                   ▼
┌─────────────────────────────────────────────────────────────┐
│           Shared SST Format Reader                           │
│  • Bloom filter mmap strategy                                │
│  • Index block caching                                       │
│  • Data block streaming                                      │
│  • Shared between SST & SWIFT engines                        │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                    Storage Engines                           │
├──────────────────┬──────────────────┬──────────────────────┤
│     VIPER        │      NOVA        │  Columnar Family     │
│                  │                  │                       │
└────────┬─────────┴─────────┬────────┴──────────────────────┘
         │                   │
         ▼                   ▼
┌─────────────────────────────────────────────────────────────┐
│         Shared Parquet Format Reader                         │
│  • Footer always mmap'd                                      │
│  • Column index smart caching                                │
│  • Row group selective loading                               │
│  • Shared between VIPER & NOVA engines                       │
└─────────────────────────────────────────────────────────────┘
```

## 1. Shared SST Format Reader (SST & SWIFT)

```rust
// src/storage/engines/row_based/shared_sst_reader.rs

pub struct SharedSstFormatReader {
    // Filesystem for I/O
    filesystem: Arc<FilesystemFactory>,
    
    // Memory mapping strategy
    mmap_strategy: SstMmapStrategy,
    
    // Shared cache for hot regions
    bloom_cache: Arc<DashMap<String, Arc<Mmap>>>,     // Always mmap'd
    index_cache: Arc<DashMap<String, Arc<Mmap>>>,     // Usually mmap'd
    
    // Memory pressure monitor
    memory_monitor: Arc<MemoryPressureMonitor>,
    
    // Access pattern tracker
    access_tracker: Arc<AccessPatternTracker>,
}

#[derive(Clone)]
pub struct SstMmapStrategy {
    // Always mmap these regions (critical for performance)
    pub always_mmap: Vec<SstRegion>,
    
    // Conditionally mmap based on memory pressure
    pub conditional_mmap: Vec<(SstRegion, f32)>, // (region, max_pressure_threshold)
    
    // Never mmap these (always stream)
    pub never_mmap: Vec<SstRegion>,
}

#[derive(Clone, Debug)]
pub enum SstRegion {
    BloomFilter,        // First 4KB
    IndexBlock,         // 4KB-64KB typically
    CompressionDict,    // If present
    DataBlocks,         // Large, usually streamed
    Metadata,           // File metadata
}

impl SharedSstFormatReader {
    /// Smart read that handles mmap decisions based on access patterns
    pub async fn read_record(
        &self,
        file_path: &str,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        // Step 1: Check bloom filter (always mmap'd if possible)
        let bloom_data = self.get_bloom_filter(file_path).await?;
        if !self.check_bloom(bloom_data, key) {
            return Ok(None);
        }
        
        // Step 2: Read index block to find data block
        let index_data = self.get_index_block(file_path).await?;
        let block_info = self.find_block_for_key(index_data, key)?;
        
        // Step 3: Read data block (usually NOT mmap'd - too large)
        let data = self.read_data_block(file_path, &block_info).await?;
        
        self.find_in_block(data, key)
    }
    
    /// Get bloom filter with intelligent caching
    async fn get_bloom_filter(&self, file_path: &str) -> Result<Arc<[u8]>> {
        // Check if already mmap'd
        if let Some(mmap) = self.bloom_cache.get(file_path) {
            self.access_tracker.track_hit(file_path, SstRegion::BloomFilter);
            return Ok(Arc::from(&mmap[0..BLOOM_SIZE]));
        }
        
        // Ensure file is in local cache
        let local_path = self.filesystem
            .ensure_cached_range(file_path, 0..BLOOM_SIZE)
            .await?;
        
        // Try to mmap if memory allows
        let memory_pressure = self.memory_monitor.get_pressure();
        
        if memory_pressure < 0.95 { // Almost always mmap bloom filters
            if let Ok(Some(mmap)) = self.filesystem
                .get_regional_mmap(&local_path, 0..BLOOM_SIZE)
                .await 
            {
                self.bloom_cache.insert(file_path.to_string(), mmap.clone());
                return Ok(Arc::from(&mmap[0..BLOOM_SIZE]));
            }
        }
        
        // Fallback: Read directly (very high memory pressure)
        let data = self.filesystem
            .read_range(&local_path, 0, BLOOM_SIZE as u64)
            .await?;
        Ok(Arc::from(data.as_slice()))
    }
    
    /// Shared batch reading optimization (used by both SST and SWIFT)
    pub async fn batch_read_with_mmap_optimization(
        &self,
        file_path: &str,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        // Optimize by reading bloom once for all keys
        let bloom_data = self.get_bloom_filter(file_path).await?;
        
        // Filter keys using bloom
        let mut possible_keys = Vec::new();
        for key in keys {
            if self.check_bloom(&bloom_data, key) {
                possible_keys.push(key);
            }
        }
        
        if possible_keys.is_empty() {
            return Ok(vec![None; keys.len()]);
        }
        
        // Read index once and find all blocks needed
        let index_data = self.get_index_block(file_path).await?;
        let mut blocks_to_read = HashMap::new();
        
        for key in &possible_keys {
            let block_info = self.find_block_for_key(&index_data, key)?;
            blocks_to_read.entry(block_info.offset)
                .or_insert_with(Vec::new)
                .push(key);
        }
        
        // Read blocks in parallel (but don't mmap data blocks)
        let mut results = HashMap::new();
        for (block_offset, keys_in_block) in blocks_to_read {
            let block_data = self.read_data_block(file_path, block_offset).await?;
            for key in keys_in_block {
                if let Some(value) = self.find_in_block(&block_data, key)? {
                    results.insert(key, value);
                }
            }
        }
        
        // Return results in original order
        Ok(keys.iter().map(|k| results.get(k).cloned()).collect())
    }
}
```

## 2. Shared Parquet Format Reader (VIPER & NOVA)

```rust
// src/storage/engines/columnar/shared_parquet_reader.rs

pub struct SharedParquetFormatReader {
    filesystem: Arc<FilesystemFactory>,
    
    // Memory mapping strategy for Parquet
    mmap_strategy: ParquetMmapStrategy,
    
    // Footer is ALWAYS cached and mmap'd when possible
    footer_cache: Arc<DashMap<String, ParquetFooterCache>>,
    
    // Column indexes cached based on access patterns
    column_index_cache: Arc<DashMap<String, Arc<Mmap>>>,
    
    // Row group metadata cache
    row_group_cache: Arc<DashMap<String, Vec<RowGroupMetadata>>>,
    
    memory_monitor: Arc<MemoryPressureMonitor>,
    access_tracker: Arc<AccessPatternTracker>,
}

pub struct ParquetMmapStrategy {
    // Footer strategy (always try to mmap)
    pub footer_max_size: usize,  // Usually 8MB
    
    // Column-specific strategies
    pub column_strategies: HashMap<String, ColumnMmapStrategy>,
    
    // Row group size threshold for mmap
    pub row_group_mmap_threshold: usize, // e.g., 50MB
}

pub enum ColumnMmapStrategy {
    AlwaysMmap,      // Hot columns (e.g., primary key)
    NeverMmap,       // Large blob columns
    Adaptive {       // Based on access patterns
        min_access_count: u32,
        recency_weight: f32,
    },
}

impl SharedParquetFormatReader {
    /// Read specific columns with intelligent mmap
    pub async fn read_columns(
        &self,
        file_path: &str,
        columns: &[String],
        row_filter: Option<&FilterExpression>,
    ) -> Result<RecordBatch> {
        // Step 1: Get footer (always try to mmap)
        let footer = self.get_footer_with_mmap(file_path).await?;
        
        // Step 2: Determine which row groups to read
        let row_groups = if let Some(filter) = row_filter {
            self.prune_row_groups(&footer, filter)?
        } else {
            (0..footer.num_row_groups()).collect()
        };
        
        // Step 3: Determine mmap strategy for each column
        let column_strategies = self.determine_column_strategies(
            file_path,
            columns,
            &row_groups,
            &footer,
        ).await?;
        
        // Step 4: Read data with appropriate strategy
        let mut batches = Vec::new();
        
        for rg_idx in row_groups {
            let batch = self.read_row_group_with_strategy(
                file_path,
                rg_idx,
                columns,
                &column_strategies,
                &footer,
            ).await?;
            batches.push(batch);
        }
        
        // Combine batches
        Ok(self.combine_batches(batches))
    }
    
    /// Always try to mmap footer - it's critical for performance
    async fn get_footer_with_mmap(&self, file_path: &str) -> Result<Arc<ParquetFooter>> {
        // Check cache first
        if let Some(cached) = self.footer_cache.get(file_path) {
            self.access_tracker.track_hit(file_path, ParquetRegion::Footer);
            return Ok(cached.footer.clone());
        }
        
        // Calculate footer location
        let file_size = self.filesystem.file_size(file_path).await?;
        let footer_start = file_size.saturating_sub(self.mmap_strategy.footer_max_size as u64);
        let footer_range = footer_start..file_size;
        
        // Ensure footer is in local cache
        let local_path = self.filesystem
            .ensure_cached_range(file_path, footer_range.clone())
            .await?;
        
        // Try to mmap footer (almost always succeeds - small size)
        let footer_data = if self.memory_monitor.get_pressure() < 0.99 {
            if let Ok(Some(mmap)) = self.filesystem
                .get_regional_mmap(&local_path, footer_range.clone())
                .await
            {
                // Footer is mmap'd
                self.column_index_cache.insert(
                    format!("{}_footer", file_path),
                    mmap.clone(),
                );
                mmap
            } else {
                // Read footer into memory
                Arc::new(self.filesystem.read_range(&local_path, footer_range).await?)
            }
        } else {
            // Extreme memory pressure - stream read
            Arc::new(self.filesystem.read_range(&local_path, footer_range).await?)
        };
        
        // Parse footer
        let footer = self.parse_footer(footer_data)?;
        
        // Cache it
        self.footer_cache.insert(
            file_path.to_string(),
            ParquetFooterCache {
                footer: footer.clone(),
                last_access: Instant::now(),
            },
        );
        
        Ok(footer)
    }
    
    /// Smart column reading based on access patterns
    async fn read_row_group_with_strategy(
        &self,
        file_path: &str,
        rg_idx: usize,
        columns: &[String],
        strategies: &HashMap<String, ColumnReadStrategy>,
        footer: &ParquetFooter,
    ) -> Result<RecordBatch> {
        let rg_metadata = &footer.row_groups[rg_idx];
        let local_path = self.ensure_row_group_cached(file_path, rg_idx, rg_metadata).await?;
        
        let mut column_data = HashMap::new();
        
        for column in columns {
            let strategy = strategies.get(column)
                .unwrap_or(&ColumnReadStrategy::Stream);
            
            let data = match strategy {
                ColumnReadStrategy::Mmap => {
                    // Try to mmap this column
                    let column_range = self.get_column_range(rg_metadata, column)?;
                    
                    if let Ok(Some(mmap)) = self.filesystem
                        .get_regional_mmap(&local_path, column_range.clone())
                        .await
                    {
                        // Successfully mmap'd
                        self.decode_column_from_mmap(mmap, column, rg_metadata)?
                    } else {
                        // Fallback to streaming
                        self.stream_column(&local_path, column, rg_metadata).await?
                    }
                }
                
                ColumnReadStrategy::Stream => {
                    // Stream this column (large or cold)
                    self.stream_column(&local_path, column, rg_metadata).await?
                }
                
                ColumnReadStrategy::Cached => {
                    // This column should be in cache
                    if let Some(cached) = self.get_cached_column(file_path, rg_idx, column) {
                        cached
                    } else {
                        // Cache miss - stream and cache
                        let data = self.stream_column(&local_path, column, rg_metadata).await?;
                        self.cache_column(file_path, rg_idx, column, &data);
                        data
                    }
                }
            };
            
            column_data.insert(column.clone(), data);
        }
        
        Ok(self.build_record_batch(column_data))
    }
    
    /// Shared optimization for columnar engines
    pub async fn optimize_columnar_scan(
        &self,
        file_paths: &[String],
        columns: &[String],
        filter: Option<&FilterExpression>,
    ) -> Result<Vec<RecordBatch>> {
        // This method is shared by both VIPER and NOVA
        // It implements sophisticated optimizations that benefit both engines
        
        // 1. Analyze access pattern across files
        let pattern = self.analyze_multi_file_pattern(file_paths, columns);
        
        // 2. Prefetch footers in parallel
        let footers = self.prefetch_footers_parallel(file_paths).await?;
        
        // 3. Build global pruning strategy
        let pruning_plan = self.build_pruning_plan(&footers, filter)?;
        
        // 4. Execute reads with optimal strategy
        let mut results = Vec::new();
        
        for (file_path, file_plan) in pruning_plan {
            let batch = self.execute_file_plan(
                &file_path,
                &file_plan,
                columns,
                &pattern,
            ).await?;
            results.push(batch);
        }
        
        Ok(results)
    }
}

#[derive(Clone)]
enum ColumnReadStrategy {
    Mmap,    // Memory map this column
    Stream,  // Stream from disk
    Cached,  // Should be in cache
}
```

## 3. Integration with Engines

### SST Engine Integration
```rust
impl SstEngine {
    pub fn new(config: SstConfig) -> Self {
        // Create shared reader with SST-specific configuration
        let shared_reader = SharedSstFormatReader::new(
            SstMmapStrategy {
                always_mmap: vec![SstRegion::BloomFilter, SstRegion::IndexBlock],
                conditional_mmap: vec![
                    (SstRegion::CompressionDict, 0.8),
                    (SstRegion::Metadata, 0.9),
                ],
                never_mmap: vec![SstRegion::DataBlocks], // Too large
            }
        );
        
        Self {
            shared_reader: Arc::new(shared_reader),
            // ... other fields
        }
    }
    
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        // Delegate to shared reader
        self.shared_reader.read_record(&self.current_file, key).await
    }
}
```

### SWIFT Engine Integration
```rust
impl SwiftEngine {
    pub fn new(config: SwiftConfig) -> Self {
        // SWIFT can use the same shared reader with different strategy
        let shared_reader = SharedSstFormatReader::new(
            SstMmapStrategy {
                always_mmap: vec![SstRegion::BloomFilter], // Less aggressive
                conditional_mmap: vec![
                    (SstRegion::IndexBlock, 0.7),
                    (SstRegion::CompressionDict, 0.8),
                ],
                never_mmap: vec![SstRegion::DataBlocks],
            }
        );
        
        Self {
            shared_reader: Arc::new(shared_reader),
            // ... swift-specific fields
        }
    }
    
    async fn batch_get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>> {
        // Use shared batch optimization
        self.shared_reader.batch_read_with_mmap_optimization(
            &self.current_file,
            keys
        ).await
    }
}
```

### VIPER Engine Integration
```rust
impl ViperEngine {
    pub fn new(config: ViperConfig) -> Self {
        let shared_reader = SharedParquetFormatReader::new(
            ParquetMmapStrategy {
                footer_max_size: 8 * 1024 * 1024, // 8MB footer
                column_strategies: config.hot_columns.iter().map(|col| {
                    (col.clone(), ColumnMmapStrategy::AlwaysMmap)
                }).collect(),
                row_group_mmap_threshold: 50 * 1024 * 1024, // 50MB
            }
        );
        
        Self {
            shared_reader: Arc::new(shared_reader),
            // ... viper-specific fields
        }
    }
    
    async fn scan_with_filter(&self, filter: &FilterExpression) -> Result<Vec<VectorRecord>> {
        // Use shared columnar optimization
        let batches = self.shared_reader.optimize_columnar_scan(
            &self.parquet_files,
            &self.projection_columns,
            Some(filter),
        ).await?;
        
        // Convert to VectorRecords
        self.batches_to_vectors(batches)
    }
}
```

### NOVA Engine Integration
```rust
impl NovaEngine {
    pub fn new(config: NovaConfig) -> Self {
        // NOVA might have different hot columns than VIPER
        let shared_reader = SharedParquetFormatReader::new(
            ParquetMmapStrategy {
                footer_max_size: 10 * 1024 * 1024, // Larger footer cache
                column_strategies: HashMap::new(), // Adaptive for all
                row_group_mmap_threshold: 100 * 1024 * 1024, // More aggressive
            }
        );
        
        Self {
            shared_reader: Arc::new(shared_reader),
            // ... nova-specific optimizations
        }
    }
}
```

## 4. Benefits of This Architecture

### Code Reuse
- **SST & SWIFT**: Share 90% of SST reading logic
- **VIPER & NOVA**: Share 85% of Parquet reading logic
- **Memory Management**: Unified strategies across format families

### Performance Optimizations (Shared Across Engines)
```rust
// SST Family Optimizations (SST & SWIFT benefit)
- Bloom filter always in memory (4KB per file)
- Index blocks cached with LRU (60KB per file)
- Batch reading with single bloom check
- Parallel block reading for range scans

// Parquet Family Optimizations (VIPER & NOVA benefit)
- Footer always mmap'd when possible
- Column statistics cached
- Row group pruning
- Predicate pushdown
- Selective column reading
```

### Memory Efficiency
```rust
// Shared memory pools across engines
- Bloom filters: Max 100MB shared across SST/SWIFT
- Parquet footers: Max 500MB shared across VIPER/NOVA
- Column indexes: LRU with 1GB limit
- Automatic eviction under pressure
```

## 5. Configuration

```toml
[format_readers.sst]
# Shared between SST and SWIFT engines
bloom_cache_size = "100MB"
index_cache_size = "500MB"
always_mmap_bloom = true
always_mmap_index = true
data_block_mmap_threshold = "never"  # Too large

[format_readers.parquet]
# Shared between VIPER and NOVA engines
footer_cache_size = "500MB"
column_index_cache_size = "1GB"
always_mmap_footer = true
hot_columns = ["id", "timestamp", "embedding_id"]
row_group_mmap_threshold = "50MB"

[format_readers.memory]
# Shared memory management
total_mmap_limit = "10GB"
pressure_threshold = 0.8
critical_threshold = 0.95
eviction_policy = "lru_with_frequency"
```

## Summary

By creating **shared format readers** with intelligent memory mapping:

1. **Effort Reduction**: Write complex mmap logic once per format, not per engine
2. **Performance Gains**: Optimizations benefit multiple engines
3. **Memory Efficiency**: Shared caches and coordinated eviction
4. **Maintainability**: Format-specific logic in one place
5. **Flexibility**: Each engine can still customize strategies

The key insight is that **format determines access patterns**, not engines:
- SST format → Bloom + Index hot, Data cold
- Parquet format → Footer + Column indexes hot, Data varies

This approach gives us the best of both worlds:
- **Complexity where it matters**: In format readers that understand data layout
- **Simplicity where needed**: In engines that focus on their unique features
- **Reuse across engines**: Shared optimizations benefit multiple engines