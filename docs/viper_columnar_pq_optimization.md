# VIPER Columnar Storage PQ Optimization Strategy

## Current VIPER Architecture

VIPER uses columnar Parquet storage with:
- **BinaryArray** for compressed vector storage
- **Row groups** of 50,000 vectors
- **Level-based compaction** (level0, level1, etc.)
- **ZSTD compression** at Parquet level
- **Dual column storage**: FP32 + quantized columns

## Optimization Opportunities

### 1. PQ-based Similarity Sorting in Compaction

Similar to SST, VIPER can benefit from sorting vectors by PQ similarity during compaction to improve compression ratios and query performance.

#### Current Compaction Flow:
```
Input Files → MVCC Resolution → Write Output Files
```

#### Optimized Compaction Flow:
```
Input Files → MVCC Resolution → PQ Similarity Sort → Columnar Optimization → Write Output Files
```

### 2. Columnar-Specific Optimizations

VIPER's columnar format provides unique optimization opportunities:

#### A. **Dual Column Strategy with PQ**
```rust
// Current: Single vector column
vector_column: List<Float32>

// Optimized: Dual columns
fp32_column: List<Float32>      // Full precision
pq_column: Binary                // PQ codes
binary_sketch_column: Binary     // Binary sketches for filtering
metadata_bloom: Binary           // Per-row-group bloom filters
```

#### B. **Row Group Organization by Similarity**
- Sort vectors within row groups by PQ similarity
- Create "similarity clusters" within row groups
- Store cluster metadata for efficient pruning

#### C. **Progressive Column Loading**
```
Query → Load binary_sketch_column → Filter row groups
     → Load pq_column → Rank candidates  
     → Load fp32_column → Final reranking
```

## Implementation Plan

### Phase 1: Add Quantization Columns to Schema

```rust
// In viper/schema.rs
pub async fn generate_optimized_schema_with_quantization(
    &self,
    collection_id: &str,
    config: &StorageQuantizationConfig,
) -> Result<Arc<Schema>> {
    let mut fields = vec![
        // Core fields
        Field::new("id", DataType::Utf8, true),
        Field::new("vector", DataType::List(Float32), true),
        
        // Quantization columns
        Field::new("pq_codes", DataType::Binary, true),
        Field::new("binary_sketch", DataType::Binary, true),
        Field::new("cluster_id", DataType::UInt32, true),
        
        // Metadata
        Field::new("metadata", DataType::Utf8, true),
    ];
    
    // Add filterable columns dynamically
    if let Some(filterable) = config.filterable_columns {
        for col in filterable {
            fields.push(create_filterable_field(col));
        }
    }
    
    Ok(Arc::new(Schema::new(fields)))
}
```

### Phase 2: PQ Similarity Sorting in Compaction

```rust
// In viper/compaction.rs
impl CompactionManager {
    async fn compact_with_pq_optimization(
        &self,
        input_files: Vec<String>,
        quantization_manager: &StorageQuantizationEngine,
    ) -> Result<ViperCompactionResult> {
        // Step 1: Read all vectors from input files
        let mut all_records = self.read_all_records(input_files).await?;
        
        // Step 2: MVCC resolution
        let resolved_records = self.resolve_mvcc(all_records).await?;
        
        // Step 3: Generate PQ codes if not present
        let quantized_data = quantization_manager
            .quantize_batch(&resolved_records)
            .await?;
        
        // Step 4: Sort by PQ similarity
        let sorted_records = self.sort_by_pq_similarity(
            resolved_records,
            quantized_data,
        )?;
        
        // Step 5: Write to optimized Parquet with row groups
        let output_files = self.write_optimized_parquet(
            sorted_records,
            quantized_data,
        ).await?;
        
        Ok(ViperCompactionResult {
            output_files,
            // ... other fields
        })
    }
    
    fn sort_by_pq_similarity(
        &self,
        records: Vec<VectorRecord>,
        quantized: Vec<StorageQuantizedData>,
    ) -> Result<Vec<VectorRecord>> {
        // Use hierarchical clustering on PQ codes
        let clusters = self.cluster_by_pq(&quantized)?;
        
        // Sort records by cluster ID
        let mut sorted = Vec::new();
        for cluster in clusters {
            for idx in cluster.indices {
                sorted.push(records[idx].clone());
            }
        }
        
        Ok(sorted)
    }
}
```

### Phase 3: Optimized Parquet Writer

```rust
// In viper/optimized_vector_writer.rs
impl OptimizedVectorWriter {
    pub async fn write_with_quantization(
        &self,
        records: Vec<VectorRecord>,
        quantized: Vec<StorageQuantizedData>,
        writer: &mut ArrowWriter<W>,
    ) -> Result<()> {
        // Create columnar arrays
        let id_array = create_string_array(&records.ids);
        let vector_array = create_float_list_array(&records.vectors);
        let pq_array = create_binary_array(&quantized.pq_codes);
        let sketch_array = create_binary_array(&quantized.sketches);
        
        // Group by similarity for better compression
        let row_groups = self.create_similarity_row_groups(
            &records,
            &quantized,
            self.config.row_group_size,
        )?;
        
        for group in row_groups {
            // Write row group with similarity-sorted vectors
            let batch = RecordBatch::try_new(
                self.schema.clone(),
                vec![
                    Arc::new(id_array.slice(group.start, group.len)),
                    Arc::new(vector_array.slice(group.start, group.len)),
                    Arc::new(pq_array.slice(group.start, group.len)),
                    Arc::new(sketch_array.slice(group.start, group.len)),
                ],
            )?;
            
            writer.write(&batch)?;
        }
        
        Ok(())
    }
}
```

### Phase 4: Progressive Search with Column Filtering

```rust
// In viper/unified_search_engine.rs
impl ViperUnifiedSearchEngine {
    pub async fn progressive_columnar_search(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<FilterExpression>,
    ) -> Result<Vec<SearchResult>> {
        // Stage 1: Load binary sketch column and filter
        let sketch_column = self.reader.read_column("binary_sketch").await?;
        let candidates = self.filter_by_sketch(query, sketch_column)?;
        
        // Stage 2: Load PQ column for filtered row groups only
        let pq_column = self.reader.read_column_filtered(
            "pq_codes",
            &candidates.row_groups,
        ).await?;
        let ranked = self.rank_by_pq(query, pq_column, candidates)?;
        
        // Stage 3: Load full vectors for top-k candidates
        let vector_column = self.reader.read_column_filtered(
            "vector",
            &ranked.top_k_rows(k * 10),
        ).await?;
        let final_results = self.rerank_full_precision(
            query,
            vector_column,
            k,
        )?;
        
        Ok(final_results)
    }
}
```

## Columnar-Specific Benefits

### 1. **Compression Improvements**
- Similar vectors in same row group compress better
- PQ codes in separate column enable specialized compression
- Binary sketches highly compressible with bit-packing

### 2. **I/O Reduction**
- Load only required columns
- Skip row groups based on bloom filters
- Progressive resolution minimizes data transfer

### 3. **Cache Efficiency**
- Column-wise caching more efficient
- Hot columns (sketches, PQ) stay in memory
- Cold columns (full vectors) loaded on-demand

### 4. **Parallelization**
- Process row groups in parallel
- SIMD operations on columnar data
- GPU-friendly columnar layout

## Performance Projections

### Storage Savings
```
Original: 100GB (1M vectors × 768 dims × 4 bytes)
Optimized:
  - FP32 column: 100GB (unchanged, but better compressed)
  - PQ column: 3.2GB (1M × 32 bytes)
  - Binary sketch: 96MB (1M × 96 bytes)
  - Total: ~103GB (3% overhead)
  - Effective: Better compression due to similarity sorting
```

### Query Performance
```
Traditional VIPER:
  - Load all columns: 100GB I/O
  - Search time: 100ms

Optimized VIPER:
  - Stage 1 (sketch): 96MB I/O, 5ms
  - Stage 2 (PQ): 320MB I/O, 10ms  
  - Stage 3 (vectors): 30KB I/O, 5ms
  - Total: 416MB I/O, 20ms
  - I/O Reduction: 99.6%
  - Speed improvement: 5x
```

## Integration with Common Quantization Module

```rust
// Use the common StorageQuantizationEngine
pub struct ViperQuantizationAdapter {
    base: StorageQuantizationEngine,
    // VIPER-specific additions
    row_group_optimizer: RowGroupOptimizer,
    columnar_layout: ColumnarLayoutManager,
}

impl ViperQuantizationAdapter {
    /// VIPER-specific: Optimize for columnar storage
    pub fn create_columnar_layout(
        &self,
        data: Vec<StorageQuantizedData>,
    ) -> ColumnarLayout {
        ColumnarLayout {
            fp32_column: extract_vectors(&data),
            pq_column: extract_pq_codes(&data),
            sketch_column: extract_sketches(&data),
            metadata: create_column_metadata(&data),
        }
    }
    
    /// VIPER-specific: Row group optimization
    pub fn optimize_row_groups(
        &self,
        layout: &mut ColumnarLayout,
        target_size: usize,
    ) {
        // Group similar vectors in same row group
        let clusters = self.cluster_by_similarity(&layout.pq_column);
        layout.reorder_by_clusters(clusters);
    }
}
```

## Migration Path

### Week 1: Schema Updates
- Add quantization columns to Parquet schema
- Update readers to handle new columns
- Maintain backward compatibility

### Week 2: Compaction Integration
- Add PQ sorting to compaction
- Implement similarity clustering
- Test compression improvements

### Week 3: Search Optimization
- Implement progressive columnar search
- Add column filtering logic
- Benchmark I/O reduction

### Week 4: Production Rollout
- Enable for new collections
- Monitor performance metrics
- Gradual migration of existing data

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| Schema compatibility | Versioned schemas, migration tools |
| Increased storage | Optional columns, compression offsets overhead |
| Compaction overhead | Async processing, incremental rollout |
| Query complexity | Fallback to traditional search |

## Success Metrics

- **Compression Ratio**: 20-30% improvement from similarity sorting
- **I/O Reduction**: 95-99% for filtered queries
- **Query Latency**: 3-5x improvement for k-NN search
- **Storage Overhead**: < 5% for quantization columns
- **Compaction Time**: < 20% increase (offset by better compression)

## Conclusion

VIPER's columnar architecture is ideally suited for PQ-based optimizations:
1. **Column separation** enables progressive loading
2. **Row groups** map naturally to similarity clusters
3. **Parquet compression** benefits from sorted similar vectors
4. **Parallel processing** on columnar data with SIMD/GPU

The combination of columnar storage and PQ quantization can achieve:
- **99%+ I/O reduction** for most queries
- **20-30% better compression** from similarity sorting
- **5x query speedup** from progressive resolution
- **Minimal storage overhead** (< 5%)