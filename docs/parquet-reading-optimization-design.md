# Comprehensive Parquet Reading Optimization Design

## Executive Summary

This document proposes an intelligent Parquet reading strategy that optimizes performance based on query characteristics (filters, quantization), storage location (local vs cloud), and data access patterns. The design uses a decision tree approach to select the optimal reading strategy for each scenario.

## Decision Matrix

### Core Optimization Scenarios

| Scenario | Filters | Quantization | Storage | Optimal Strategy |
|----------|---------|-------------|---------|------------------|
| **Full Scan** | None | None | Local | Arrow DirectRead (whole file) |
| **Full Scan** | None | None | Cloud | Download + Arrow (whole file) |
| **Filtered Scan** | Present | None | Local | Metadata + FileSeek + Arrow |
| **Filtered Scan** | Present | None | Cloud | Metadata + HTTP Ranges + Arrow |
| **Quantized Search** | None | Present | Local | Column Seek + Arrow (quantized only) |
| **Quantized Search** | Present | Present | Local | Metadata + Column Seek + Two-Stage |
| **Quantized Search** | None | Present | Cloud | Column Ranges + Arrow (quantized only) |
| **Quantized Search** | Present | Present | Cloud | Metadata + Column Ranges + Two-Stage |

## Architectural Design

### 1. Strategy Selection Engine

```rust
#[derive(Debug, Clone)]
pub enum ParquetReadStrategy {
    // Simple strategies for unfiltered access
    DirectArrowRead,           // Local: Arrow reader on whole file
    DownloadAndArrow,          // Cloud: Download entire file + Arrow
    
    // Metadata-driven strategies for filtered access
    MetadataFilteredLocal {    // Local: File seek based on metadata
        row_groups: Vec<usize>,
        columns: Vec<String>,
    },
    MetadataFilteredCloud {    // Cloud: HTTP ranges based on metadata
        ranges: Vec<std::ops::Range<u64>>,
        columns: Vec<String>,
    },
    
    // Two-stage quantized strategies
    QuantizedTwoStageLocal {   // Local: Column seek + candidate refinement
        quantized_columns: Vec<String>,
        fp32_columns: Vec<String>,
        row_group_filter: Option<Vec<usize>>,
    },
    QuantizedTwoStageCloud {   // Cloud: Column ranges + candidate refinement
        stage1_ranges: Vec<std::ops::Range<u64>>,
        stage2_ranges: Vec<std::ops::Range<u64>>,
        columns: ColumnSet,
    },
}

pub struct StrategySelector {
    filesystem: Arc<FilesystemFactory>,
    metadata_cache: Arc<MetadataCache>,
}

impl StrategySelector {
    pub async fn select_strategy(
        &self,
        query: &VectorQuery,
        file_path: &str,
    ) -> Result<ParquetReadStrategy> {
        let is_cloud = self.is_cloud_storage(file_path);
        let metadata = self.get_or_fetch_metadata(file_path).await?;
        
        // Decision tree logic
        match (query.has_filters(), query.has_quantization(), is_cloud) {
            // No filters, no quantization - simple full read
            (false, false, false) => Ok(ParquetReadStrategy::DirectArrowRead),
            (false, false, true) => Ok(ParquetReadStrategy::DownloadAndArrow),
            
            // Filters present - metadata-driven reading
            (true, false, false) => {
                let row_groups = self.filter_row_groups(&metadata, &query.filters)?;
                Ok(ParquetReadStrategy::MetadataFilteredLocal {
                    row_groups,
                    columns: query.required_columns(),
                })
            }
            (true, false, true) => {
                let ranges = self.calculate_filtered_ranges(&metadata, &query.filters)?;
                Ok(ParquetReadStrategy::MetadataFilteredCloud {
                    ranges,
                    columns: query.required_columns(),
                })
            }
            
            // Quantization - two-stage reading
            (has_filters, true, is_cloud) => {
                self.select_quantized_strategy(query, &metadata, has_filters, is_cloud).await
            }
        }
    }
}
```

### 2. Unified Query Interface

```rust
#[derive(Debug, Clone)]
pub struct VectorQuery {
    pub query_vector: Vec<f32>,
    pub k: usize,
    pub metadata_filters: Option<MetadataFilter>,
    pub quantization_config: Option<QuantizationConfig>,
    pub return_vectors: bool,
    pub distance_metric: Option<DistanceMetric>,
}

impl VectorQuery {
    pub fn has_filters(&self) -> bool {
        self.metadata_filters.is_some()
    }
    
    pub fn has_quantization(&self) -> bool {
        self.quantization_config.is_some()
    }
    
    pub fn required_columns(&self) -> Vec<String> {
        let mut columns = vec!["id".to_string()];
        
        if self.return_vectors {
            columns.push("vector".to_string());
        }
        
        if self.has_filters() {
            columns.push("metadata".to_string());
        }
        
        if let Some(ref quant_config) = self.quantization_config {
            columns.push(quant_config.quantized_column_name());
        }
        
        columns
    }
}
```

### 3. Strategy Implementations

#### 3.1 Direct Arrow Read (Local, No Filters)

```rust
pub struct DirectArrowReader {
    file_cache: Option<Arc<FileCache>>,
}

impl DirectArrowReader {
    pub async fn read_full_file(&self, file_path: &str, query: &VectorQuery) -> Result<QueryResult> {
        // Fastest path: Direct Arrow reader on entire file
        let file = std::fs::File::open(file_path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
        
        // Apply column projection if beneficial
        let builder = if query.required_columns().len() < self.total_columns(&builder)? {
            let projection = self.create_projection_mask(&builder, &query.required_columns())?;
            builder.with_projection(projection)
        } else {
            builder
        };
        
        let reader = builder.build()?;
        let batches: Vec<RecordBatch> = reader.collect::<Result<Vec<_>, _>>()?;
        
        // Process in-memory with Arrow compute
        self.process_batches_with_arrow(batches, query).await
    }
    
    fn process_batches_with_arrow(
        &self,
        batches: Vec<RecordBatch>,
        query: &VectorQuery
    ) -> Result<QueryResult> {
        // Use Arrow compute kernels for filtering and processing
        let filtered_batches = if let Some(ref filter) = query.metadata_filters {
            self.apply_arrow_filters(batches, filter)?
        } else {
            batches
        };
        
        // Convert to vectors and apply vector similarity search
        self.vector_search_on_batches(filtered_batches, query)
    }
}
```

#### 3.2 Metadata-Filtered Local Read (File Seek)

```rust
pub struct MetadataFilteredLocalReader {
    file_seek_cache: Arc<FileSeekCache>,
}

impl MetadataFilteredLocalReader {
    pub async fn read_filtered_local(
        &self,
        file_path: &str,
        row_groups: Vec<usize>,
        columns: Vec<String>,
        query: &VectorQuery,
    ) -> Result<QueryResult> {
        // Step 1: Read metadata to get precise file positions
        let metadata = self.read_parquet_metadata(file_path)?;
        
        // Step 2: Calculate exact file ranges for required data
        let file_ranges = self.calculate_row_group_file_ranges(
            &metadata,
            &row_groups,
            &columns,
        )?;
        
        // Step 3: Use file seeks to read only required data
        let partial_data = self.read_file_ranges_with_seeks(file_path, file_ranges).await?;
        
        // Step 4: Reconstruct Arrow reader from partial data
        let reconstructed_reader = self.create_arrow_reader_from_partial_data(
            partial_data,
            &metadata,
            &row_groups,
        )?;
        
        // Step 5: Process with Arrow compute
        self.process_with_arrow_compute(reconstructed_reader, query).await
    }
    
    async fn read_file_ranges_with_seeks(
        &self,
        file_path: &str,
        ranges: Vec<FileRange>,
    ) -> Result<PartialParquetData> {
        let mut file = tokio::fs::File::open(file_path).await?;
        let mut data_chunks = Vec::new();
        
        // Optimize: Sort ranges by offset to minimize seeks
        let mut sorted_ranges = ranges;
        sorted_ranges.sort_by_key(|r| r.offset);
        
        for range in sorted_ranges {
            file.seek(SeekFrom::Start(range.offset)).await?;
            let mut buffer = vec![0u8; range.length as usize];
            file.read_exact(&mut buffer).await?;
            
            data_chunks.push(DataChunk {
                range,
                data: buffer,
            });
        }
        
        Ok(PartialParquetData { chunks: data_chunks })
    }
}
```

#### 3.3 Quantized Two-Stage Reader

```rust
pub struct QuantizedTwoStageReader {
    distance_compute: Arc<UnifiedDistanceCompute>,
    quantization_engine: Arc<UnifiedQuantizationEngine>,
}

impl QuantizedTwoStageReader {
    pub async fn execute_two_stage_search(
        &self,
        strategy: QuantizedStrategy,
        query: &VectorQuery,
    ) -> Result<QueryResult> {
        match strategy {
            QuantizedStrategy::Local { quantized_columns, fp32_columns, row_groups } => {
                self.two_stage_local(query, quantized_columns, fp32_columns, row_groups).await
            }
            QuantizedStrategy::Cloud { stage1_ranges, stage2_ranges, columns } => {
                self.two_stage_cloud(query, stage1_ranges, stage2_ranges, columns).await
            }
        }
    }
    
    async fn two_stage_local(
        &self,
        query: &VectorQuery,
        quantized_columns: Vec<String>,
        fp32_columns: Vec<String>,
        row_groups: Option<Vec<usize>>,
    ) -> Result<QueryResult> {
        // Stage 1: Read only quantized columns using file seeks
        let quantized_data = self.read_columns_with_seeks(
            &query.file_path,
            &quantized_columns,
            row_groups.as_ref(),
        ).await?;
        
        // Stage 1: Fast quantized search
        let candidates = self.search_quantized_data(
            quantized_data,
            &query.query_vector,
            query.k * 3, // candidate multiplier
        ).await?;
        
        // Stage 2: Read FP32 vectors for candidates only
        let candidate_positions = self.get_candidate_file_positions(&candidates)?;
        let fp32_data = self.read_specific_vectors_with_seeks(
            &query.file_path,
            &fp32_columns,
            candidate_positions,
        ).await?;
        
        // Stage 2: Exact distance calculation
        self.refine_candidates_with_fp32(candidates, fp32_data, query).await
    }
    
    async fn read_specific_vectors_with_seeks(
        &self,
        file_path: &str,
        columns: &[String],
        positions: Vec<VectorPosition>,
    ) -> Result<Vec<VectorRecord>> {
        // This is the key optimization: read only specific vector positions
        // instead of entire row groups
        
        let mut file = tokio::fs::File::open(file_path).await?;
        let mut vectors = Vec::new();
        
        // Group positions by row group for efficient seeking
        let grouped_positions = self.group_positions_by_row_group(positions);
        
        for (row_group_idx, positions_in_group) in grouped_positions {
            let row_group_data = self.read_row_group_selective(
                &mut file,
                row_group_idx,
                columns,
                positions_in_group,
            ).await?;
            
            vectors.extend(row_group_data);
        }
        
        Ok(vectors)
    }
}
```

## Decision Tree Implementation

### Complete Strategy Selection Logic

```rust
pub struct OptimizedParquetReader {
    strategy_selector: StrategySelector,
    direct_reader: DirectArrowReader,
    filtered_local_reader: MetadataFilteredLocalReader,
    filtered_cloud_reader: MetadataFilteredCloudReader,
    quantized_reader: QuantizedTwoStageReader,
}

impl OptimizedParquetReader {
    pub async fn execute_query(&self, query: VectorQuery) -> Result<QueryResult> {
        // Step 1: Select optimal strategy
        let strategy = self.strategy_selector.select_strategy(&query, &query.file_path).await?;
        
        tracing::info!("Selected strategy: {:?} for query: {:?}", strategy, query);
        
        // Step 2: Execute strategy
        match strategy {
            ParquetReadStrategy::DirectArrowRead => {
                tracing::debug!("💽 Direct Arrow read for local file");
                self.direct_reader.read_full_file(&query.file_path, &query).await
            }
            
            ParquetReadStrategy::DownloadAndArrow => {
                tracing::debug!("☁️ Download and Arrow read for cloud file");
                self.download_and_read_with_arrow(&query).await
            }
            
            ParquetReadStrategy::MetadataFilteredLocal { row_groups, columns } => {
                tracing::debug!("🔍 Metadata-filtered local read with file seeks");
                self.filtered_local_reader.read_filtered_local(
                    &query.file_path,
                    row_groups,
                    columns,
                    &query,
                ).await
            }
            
            ParquetReadStrategy::MetadataFilteredCloud { ranges, columns } => {
                tracing::debug!("☁️ Metadata-filtered cloud read with HTTP ranges");
                self.filtered_cloud_reader.read_filtered_cloud(
                    &query.file_path,
                    ranges,
                    columns,
                    &query,
                ).await
            }
            
            ParquetReadStrategy::QuantizedTwoStageLocal { quantized_columns, fp32_columns, row_group_filter } => {
                tracing::debug!("⚡ Two-stage quantized search (local)");
                self.quantized_reader.execute_two_stage_search(
                    QuantizedStrategy::Local {
                        quantized_columns,
                        fp32_columns,
                        row_groups: row_group_filter,
                    },
                    &query,
                ).await
            }
            
            ParquetReadStrategy::QuantizedTwoStageCloud { stage1_ranges, stage2_ranges, columns } => {
                tracing::debug!("☁️ Two-stage quantized search (cloud)");
                self.quantized_reader.execute_two_stage_search(
                    QuantizedStrategy::Cloud {
                        stage1_ranges,
                        stage2_ranges,
                        columns,
                    },
                    &query,
                ).await
            }
        }
    }
}
```

## Performance Characteristics

### Expected Performance Benefits

| Scenario | Strategy | Local Performance | Cloud Performance |
|----------|----------|-------------------|-------------------|
| **No filters, no quantization** | Full file read | ⭐⭐⭐⭐⭐ (Arrow optimized) | ⭐⭐⭐ (Single download) |
| **Filters, no quantization** | Metadata-driven | ⭐⭐⭐⭐ (50-90% less I/O) | ⭐⭐⭐⭐⭐ (90-99% less transfer) |
| **No filters, quantization** | Column-only | ⭐⭐⭐⭐ (Column seeks) | ⭐⭐⭐⭐ (Column ranges) |
| **Filters + quantization** | Two-stage filtered | ⭐⭐⭐⭐⭐ (Optimal I/O) | ⭐⭐⭐⭐⭐ (Minimal transfer) |

### Memory Usage Optimization

```rust
pub struct MemoryOptimizedReader {
    memory_limit: usize,
    streaming_threshold: usize,
}

impl MemoryOptimizedReader {
    fn should_use_streaming(&self, estimated_data_size: usize) -> bool {
        estimated_data_size > self.streaming_threshold
    }
    
    async fn execute_with_memory_awareness(
        &self,
        strategy: ParquetReadStrategy,
        query: &VectorQuery,
    ) -> Result<QueryResult> {
        let estimated_size = self.estimate_data_size(&strategy, query)?;
        
        if self.should_use_streaming(estimated_size) {
            // Use streaming approach for large datasets
            self.execute_streaming_strategy(strategy, query).await
        } else {
            // Use in-memory approach for smaller datasets
            self.execute_memory_strategy(strategy, query).await
        }
    }
}
```

## Configuration and Tuning

### Adaptive Configuration

```toml
[parquet_reader.optimization]
# Thresholds for strategy selection
full_read_size_threshold = "100MB"      # Below this, always read full file
streaming_memory_threshold = "512MB"    # Above this, use streaming
seek_vs_read_threshold = 0.3            # If seeking >30% of file, read full file

# Local file optimizations
enable_file_seek_optimization = true
seek_buffer_size = "64KB"               # Buffer size for seek operations
max_concurrent_seeks = 4                # Parallel seeks for non-sequential access

# Cloud storage optimizations  
enable_metadata_caching = true
metadata_cache_ttl = "1h"               # Cache Parquet metadata
range_request_chunk_size = "1MB"        # Chunk size for range requests
max_concurrent_ranges = 8               # Parallel range requests

# Two-stage search optimizations
candidate_multiplier = 3.0              # Stage 1 candidate expansion
quantized_column_cache = true           # Cache quantized column data
```

## Usage Examples

### Example 1: Simple Vector Search (No Filters)

```rust
let query = VectorQuery {
    file_path: "s3://vectors/embeddings.parquet".to_string(),
    query_vector: vec![0.1; 768],
    k: 10,
    metadata_filters: None,          // No filters
    quantization_config: None,       // No quantization
    return_vectors: true,
    distance_metric: Some(DistanceMetric::Cosine),
};

// Strategy: DownloadAndArrow - download entire file and use Arrow
let results = reader.execute_query(query).await?;
```

### Example 2: Filtered Search (Metadata Filters)

```rust
let query = VectorQuery {
    file_path: "file:///data/vectors.parquet".to_string(),
    query_vector: vec![0.1; 768],
    k: 10,
    metadata_filters: Some(MetadataFilter {
        category: FilterValue::Equals("tech".to_string()),
        timestamp: FilterValue::Range(start_time..end_time),
    }),
    quantization_config: None,
    return_vectors: false,           // Only IDs needed
    distance_metric: Some(DistanceMetric::Cosine),
};

// Strategy: MetadataFilteredLocal - file seeks based on row group statistics
let results = reader.execute_query(query).await?;
```

### Example 3: Quantized Two-Stage Search

```rust
let query = VectorQuery {
    file_path: "gcs://ml-vectors/large-dataset.parquet".to_string(),
    query_vector: vec![0.1; 768],
    k: 50,
    metadata_filters: Some(MetadataFilter {
        domain: FilterValue::In(vec!["science", "technology"]),
    }),
    quantization_config: Some(QuantizationConfig {
        method: QuantizationMethod::PQ8,
        quantized_column: "vector_pq8".to_string(),
    }),
    return_vectors: true,
    distance_metric: Some(DistanceMetric::Euclidean),
};

// Strategy: QuantizedTwoStageCloud - HTTP ranges for quantized + FP32 data
let results = reader.execute_query(query).await?;
```

## Implementation Roadmap

### Phase 1: Core Strategy Framework (Week 1)
- [ ] Implement `StrategySelector` with decision tree logic
- [ ] Create `VectorQuery` unified interface
- [ ] Implement `DirectArrowReader` for simple cases

### Phase 2: Metadata-Driven Reading (Week 2)
- [ ] Implement `MetadataFilteredLocalReader` with file seeks
- [ ] Implement `MetadataFilteredCloudReader` with HTTP ranges
- [ ] Add metadata caching layer

### Phase 3: Two-Stage Quantized Search (Week 3)
- [ ] Implement `QuantizedTwoStageReader`
- [ ] Add candidate-specific vector reading
- [ ] Optimize memory usage for large datasets

### Phase 4: Performance Tuning (Week 4)
- [ ] Add comprehensive benchmarking
- [ ] Implement adaptive configuration
- [ ] Add performance monitoring and metrics

## Conclusion

This comprehensive design provides optimal Parquet reading strategies for all combinations of:
- **Query characteristics**: Filters, quantization, column requirements
- **Storage types**: Local files, cloud storage (S3, GCS, Azure)
- **Data access patterns**: Full scans, filtered scans, two-stage searches

The key innovations are:

1. **Intelligent Strategy Selection**: Automatic optimization based on query and storage characteristics
2. **File Seek Optimization**: Use file seeks instead of full reads for local filtered queries  
3. **Two-Stage Precision**: Read only quantized data first, then specific FP32 vectors for candidates
4. **Memory Awareness**: Streaming vs in-memory processing based on data size
5. **Cloud Optimization**: HTTP range requests with metadata-driven precision

This design achieves the best of both worlds: maximum performance for simple cases and sophisticated optimization for complex scenarios.