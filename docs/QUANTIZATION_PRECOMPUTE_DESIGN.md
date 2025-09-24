# Quantized Vector Precomputation Design Specification

## Executive Summary

This document specifies the design for precomputing quantized vectors during flush operations across all storage engines, leveraging existing quantization capabilities in ProximaDB.

## 1. Data Structure Changes

### 1.1 Enhanced VectorRecord Structure

```protobuf
// Updated proto definition in proximadb.proto
message VectorRecord {
  string id = 1;
  repeated float vector = 2;
  repeated MetadataItem metadata = 3;
  uint32 timestamp = 4;
  optional uint32 updated_at = 5;
  optional uint32 expires_at = 6;
  optional uint32 version = 7;

  // NEW: Structured quantization data
  optional QuantizedVectors quantized = 8;
  optional SourceContent source = 10;
}

// NEW: Structured quantization storage
message QuantizedVectors {
  optional bytes binary = 1;      // 1 bit per dimension
  optional bytes int8 = 2;        // 8 bits per dimension
  optional bytes pq4 = 3;         // 4-bit product quantization
  optional bytes pq8 = 4;         // 8-bit product quantization
  optional bytes pq16 = 5;        // 16-bit product quantization
  optional bytes pq32 = 6;        // 32-bit product quantization

  // Metadata for reconstruction
  optional QuantizationMetadata metadata = 10;
}

message QuantizationMetadata {
  repeated float scales = 1;      // For INT8 dequantization
  repeated float offsets = 2;     // For INT8 dequantization
  optional string codebook_id = 3; // Reference to PQ codebook
  optional uint32 num_subvectors = 4; // PQ configuration
  optional uint32 bits_per_code = 5;  // PQ configuration
}
```

### 1.2 Rust Implementation Structure

```rust
// In src/proto/proximadb_v1/mod.rs (after proto compilation)
#[derive(Clone, Debug, PartialEq)]
pub struct QuantizedVectors {
    pub binary: Option<Vec<u8>>,
    pub int8: Option<Vec<u8>>,
    pub pq4: Option<Vec<u8>>,
    pub pq8: Option<Vec<u8>>,
    pub pq16: Option<Vec<u8>>,
    pub pq32: Option<Vec<u8>>,
    pub metadata: Option<QuantizationMetadata>,
}

impl QuantizedVectors {
    /// Check if any quantization is present
    pub fn has_any(&self) -> bool {
        self.binary.is_some()
            || self.int8.is_some()
            || self.pq4.is_some()
            || self.pq8.is_some()
            || self.pq16.is_some()
            || self.pq32.is_some()
    }

    /// Get the most efficient quantization for search
    pub fn get_optimal_for_search(&self) -> Option<QuantizationType> {
        // Priority: PQ8 > INT8 > Binary (for search quality vs speed)
        if self.pq8.is_some() {
            Some(QuantizationType::PQ8)
        } else if self.int8.is_some() {
            Some(QuantizationType::Int8)
        } else if self.binary.is_some() {
            Some(QuantizationType::Binary)
        } else {
            None
        }
    }

    /// Estimate memory usage
    pub fn memory_bytes(&self) -> usize {
        self.binary.as_ref().map(|v| v.len()).unwrap_or(0)
            + self.int8.as_ref().map(|v| v.len()).unwrap_or(0)
            + self.pq4.as_ref().map(|v| v.len()).unwrap_or(0)
            + self.pq8.as_ref().map(|v| v.len()).unwrap_or(0)
            + self.pq16.as_ref().map(|v| v.len()).unwrap_or(0)
            + self.pq32.as_ref().map(|v| v.len()).unwrap_or(0)
    }
}
```

## 2. Quantization Precomputation Module

### 2.1 Create New Precomputation Service

```rust
// New file: src/compute/quantization/precompute.rs

use crate::compute::quantization::{
    unified::{UnifiedQuantizationEngine, UnifiedQuantizationLevel},
    storage_engine::StorageQuantizationEngine,
    selection::QuantizationSelector,
    global_cache::GlobalQuantizationCache,
};
use crate::proto::proximadb_v1::{VectorRecord, QuantizedVectors, QuantizationMetadata};

/// Service for precomputing quantized vectors during flush
pub struct QuantizationPrecomputeService {
    storage_engine: Arc<StorageQuantizationEngine>,
    fallback_engine: Arc<UnifiedQuantizationEngine>,
    global_cache: Arc<GlobalQuantizationCache>,
}

impl QuantizationPrecomputeService {
    /// Create new precompute service using existing engines
    pub fn new(
        storage_engine: Arc<StorageQuantizationEngine>,
        fallback_engine: Arc<UnifiedQuantizationEngine>,
    ) -> Self {
        Self {
            storage_engine,
            fallback_engine,
            global_cache: GlobalQuantizationCache::global(),
        }
    }

    /// Precompute quantized vectors for a batch during flush
    /// This leverages existing quantization capabilities
    pub async fn precompute_batch(
        &self,
        records: &mut Vec<VectorRecord>,
        params: &FlushParameters,
    ) -> Result<()> {
        // Check if quantization is enabled
        let quantization_config = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|c| c.quantization.as_ref());

        if quantization_config.is_none() || !quantization_config.unwrap().enabled {
            return Ok(()); // No quantization needed
        }

        let config = quantization_config.unwrap();
        let collection_id = params.collection_id.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for quantization"))?;

        // Determine which quantization levels to compute
        let levels = self.determine_quantization_levels(
            &config,
            records.len(),
            params.collection_config.as_ref()
                .and_then(|c| c.stats.as_ref())
                .map(|s| s.vector_count as usize)
                .unwrap_or(records.len()),
        );

        // Get or train codebooks if needed for PQ
        if levels.needs_pq() {
            self.ensure_codebooks(collection_id, &levels, records).await?;
        }

        // Batch quantize using existing engines
        self.quantize_batch_with_levels(records, &levels, collection_id).await?;

        Ok(())
    }

    /// Determine which quantization levels to compute based on configuration
    fn determine_quantization_levels(
        &self,
        config: &QuantizationConfig,
        batch_size: usize,
        total_collection_size: usize,
    ) -> QuantizationLevels {
        let mut levels = QuantizationLevels::default();

        // Always compute Binary and INT8 (fast and small)
        levels.binary = true;
        levels.int8 = true;

        // Compute PQ based on collection size and configuration
        if total_collection_size > 10_000 {
            // Use config or smart defaults
            if let Some(pq_bits) = config.pq_bits {
                match pq_bits {
                    4 => levels.pq4 = true,
                    8 => levels.pq8 = true,
                    16 => levels.pq16 = true,
                    32 => levels.pq32 = true,
                    _ => levels.pq8 = true, // Default
                }
            } else {
                // Smart selection based on collection size
                if total_collection_size > 1_000_000 {
                    levels.pq16 = true; // Higher quality for large collections
                } else {
                    levels.pq8 = true; // Standard PQ
                }
            }
        }

        levels
    }

    /// Ensure codebooks exist for PQ quantization
    async fn ensure_codebooks(
        &self,
        collection_id: &str,
        levels: &QuantizationLevels,
        records: &[VectorRecord],
    ) -> Result<()> {
        // Check each PQ level that needs codebooks
        for (pq_level, enabled) in [
            (UnifiedQuantizationLevel::PQ4, levels.pq4),
            (UnifiedQuantizationLevel::PQ8, levels.pq8),
            (UnifiedQuantizationLevel::PQ16, levels.pq16),
            (UnifiedQuantizationLevel::PQ32, levels.pq32),
        ] {
            if !enabled {
                continue;
            }

            let codebook_id = format!("{}:{}", collection_id, pq_level.to_string());

            // Check if codebook exists in global cache
            if self.global_cache.get_codebook(&codebook_id).await.is_none() {
                // Train new codebook using existing StorageQuantizationEngine
                info!("Training {} codebook for collection {}", pq_level, collection_id);

                let vectors: Vec<Vec<f32>> = records.iter()
                    .map(|r| r.vector.clone())
                    .collect();

                // Use existing training capability
                let codebook = self.storage_engine
                    .train_codebook(&vectors, pq_level)
                    .await?;

                // Store in global cache
                self.global_cache
                    .store_codebook(&codebook_id, &codebook)
                    .await?;
            }
        }

        Ok(())
    }

    /// Quantize batch with specified levels using existing engines
    async fn quantize_batch_with_levels(
        &self,
        records: &mut Vec<VectorRecord>,
        levels: &QuantizationLevels,
        collection_id: &str,
    ) -> Result<()> {
        // Prepare vectors for batch quantization
        let vectors: Vec<&[f32]> = records.iter()
            .map(|r| r.vector.as_slice())
            .collect();

        // Binary quantization (always fast)
        let binary_quantized = if levels.binary {
            Some(self.quantize_binary_batch(&vectors)?)
        } else {
            None
        };

        // INT8 quantization (always fast)
        let int8_quantized = if levels.int8 {
            Some(self.quantize_int8_batch(&vectors)?)
        } else {
            None
        };

        // PQ quantization (uses codebooks)
        let pq_results = self.quantize_pq_batch(
            &vectors,
            levels,
            collection_id
        ).await?;

        // Update records with quantized data
        for (i, record) in records.iter_mut().enumerate() {
            let mut quantized = QuantizedVectors::default();

            if let Some(ref binary) = binary_quantized {
                quantized.binary = Some(binary[i].clone());
            }

            if let Some(ref int8) = int8_quantized {
                quantized.int8 = Some(int8.vectors[i].clone());

                // Add metadata for dequantization
                if quantized.metadata.is_none() {
                    quantized.metadata = Some(QuantizationMetadata::default());
                }
                let metadata = quantized.metadata.as_mut().unwrap();
                metadata.scales = vec![int8.scale];
                metadata.offsets = vec![int8.offset];
            }

            // Add PQ quantized data
            if let Some(ref pq4) = pq_results.pq4 {
                quantized.pq4 = Some(pq4[i].clone());
            }
            if let Some(ref pq8) = pq_results.pq8 {
                quantized.pq8 = Some(pq8[i].clone());
            }
            if let Some(ref pq16) = pq_results.pq16 {
                quantized.pq16 = Some(pq16[i].clone());
            }
            if let Some(ref pq32) = pq_results.pq32 {
                quantized.pq32 = Some(pq32[i].clone());
            }

            // Only set if we have quantized data
            if quantized.has_any() {
                record.quantized = Some(quantized);
            }
        }

        Ok(())
    }

    /// Quantize to binary using existing UnifiedQuantizationEngine
    fn quantize_binary_batch(&self, vectors: &[&[f32]]) -> Result<Vec<Vec<u8>>> {
        vectors.iter()
            .map(|v| self.fallback_engine.quantize_to_binary(v))
            .collect()
    }

    /// Quantize to INT8 using existing capabilities
    fn quantize_int8_batch(&self, vectors: &[&[f32]]) -> Result<Int8QuantizedBatch> {
        let mut results = Vec::new();
        let mut scale = 0f32;
        let mut offset = 0f32;

        for vector in vectors {
            let quantized = self.fallback_engine.quantize_to_int8(vector)?;
            results.push(quantized);
        }

        // For batch, compute common scale/offset
        // (In practice, use per-vector or compute from batch statistics)
        Ok(Int8QuantizedBatch {
            vectors: results,
            scale: 127.0, // Example scale
            offset: 0.0,  // Example offset
        })
    }

    /// Quantize with PQ using existing StorageQuantizationEngine
    async fn quantize_pq_batch(
        &self,
        vectors: &[&[f32]],
        levels: &QuantizationLevels,
        collection_id: &str,
    ) -> Result<PQQuantizedBatch> {
        let mut results = PQQuantizedBatch::default();

        // Use storage engine's batch quantization with cached codebooks
        for (level, enabled, target) in [
            (UnifiedQuantizationLevel::PQ4, levels.pq4, &mut results.pq4),
            (UnifiedQuantizationLevel::PQ8, levels.pq8, &mut results.pq8),
            (UnifiedQuantizationLevel::PQ16, levels.pq16, &mut results.pq16),
            (UnifiedQuantizationLevel::PQ32, levels.pq32, &mut results.pq32),
        ] {
            if !enabled {
                continue;
            }

            // Get codebook from cache
            let codebook_id = format!("{}:{}", collection_id, level.to_string());
            let codebook = self.global_cache
                .get_codebook(&codebook_id)
                .await
                .ok_or_else(|| anyhow::anyhow!("Codebook not found for {}", level))?;

            // Use existing quantization method
            let quantized = self.storage_engine
                .quantize_batch_with_codebook(vectors, &codebook, Some(level))
                .await?;

            *target = Some(quantized);
        }

        Ok(results)
    }
}

#[derive(Default)]
struct QuantizationLevels {
    binary: bool,
    int8: bool,
    pq4: bool,
    pq8: bool,
    pq16: bool,
    pq32: bool,
}

impl QuantizationLevels {
    fn needs_pq(&self) -> bool {
        self.pq4 || self.pq8 || self.pq16 || self.pq32
    }
}

struct Int8QuantizedBatch {
    vectors: Vec<Vec<u8>>,
    scale: f32,
    offset: f32,
}

#[derive(Default)]
struct PQQuantizedBatch {
    pq4: Option<Vec<Vec<u8>>>,
    pq8: Option<Vec<Vec<u8>>>,
    pq16: Option<Vec<Vec<u8>>>,
    pq32: Option<Vec<Vec<u8>>>,
}
```

## 3. Storage Engine Integration

### 3.1 Modified Flush Implementation for All Engines

```rust
// Template for all storage engines (SST, VIPER, NOVA, SWIFT, RAPTOR, HELIX)

impl UnifiedStorageEngine for XxxEngine {
    async fn do_flush(&self, params: FlushParameters) -> Result<Vec<FileHandle>> {
        // Step 1: Precompute quantized vectors
        let mut records = params.vector_records.clone();

        if self.should_precompute_quantization(&params) {
            let precompute_service = QuantizationPrecomputeService::new(
                self.storage_quantization_engine.clone(),
                self.fallback_quantization_engine.clone(),
            );

            precompute_service.precompute_batch(&mut records, &params).await?;

            debug!(
                "Precomputed quantization for {} vectors (binary: {}, int8: {}, pq: {})",
                records.len(),
                records.iter().any(|r| r.quantized.as_ref().map(|q| q.binary.is_some()).unwrap_or(false)),
                records.iter().any(|r| r.quantized.as_ref().map(|q| q.int8.is_some()).unwrap_or(false)),
                records.iter().any(|r| r.quantized.as_ref().map(|q| q.pq8.is_some()).unwrap_or(false)),
            );
        }

        // Step 2: Engine-specific storage
        match self.engine_type() {
            EngineType::Columnar => self.flush_columnar(records, params).await,
            EngineType::RowBased => self.flush_row_based(records, params).await,
        }
    }

    fn should_precompute_quantization(&self, params: &FlushParameters) -> bool {
        // Use existing QuantizationSelector logic
        QuantizationSelector::should_use_persistent_quantization(params, self.engine_name())
    }
}
```

### 3.2 Pure Columnar Engine Storage (VIPER, NOVA)

VIPER and NOVA use Apache Parquet/Arrow columnar format:

```rust
impl ViperEngine {
    async fn flush_columnar(
        &self,
        records: Vec<VectorRecord>,
        params: FlushParameters,
    ) -> Result<Vec<FileHandle>> {
        // Convert to Arrow RecordBatch with separate columns
        let mut columns: Vec<(String, Arc<dyn Array>)> = vec![];

        // Standard columns
        columns.push(("id", build_string_array(&records.iter().map(|r| &r.id))));
        columns.push(("vector", build_float32_array(&records.iter().map(|r| &r.vector))));
        columns.push(("metadata", build_struct_array(&records.iter().map(|r| &r.metadata))));

        // Add quantized columns if present (separate Parquet columns)
        if records.iter().any(|r| r.quantized.as_ref().map(|q| q.binary.is_some()).unwrap_or(false)) {
            columns.push(("vector_binary", build_binary_array(&records.iter().map(|r|
                r.quantized.as_ref().and_then(|q| q.binary.as_ref())
            ))));
        }

        if records.iter().any(|r| r.quantized.as_ref().map(|q| q.int8.is_some()).unwrap_or(false)) {
            columns.push(("vector_int8", build_int8_array(&records.iter().map(|r|
                r.quantized.as_ref().and_then(|q| q.int8.as_ref())
            ))));
        }

        if records.iter().any(|r| r.quantized.as_ref().map(|q| q.pq8.is_some()).unwrap_or(false)) {
            columns.push(("vector_pq8", build_binary_array(&records.iter().map(|r|
                r.quantized.as_ref().and_then(|q| q.pq8.as_ref())
            ))));
        }

        // Write Parquet file with projection-friendly columns
        let batch = RecordBatch::try_new(schema, columns)?;
        self.write_parquet(batch, params).await
    }
}
```

### 3.3 FastLanesDataBlock Engine Storage (SST, SWIFT, HELIX)

These engines use FastLanesDataBlock which can be configured for row or columnar encoding:

```rust
impl SstEngine {
    async fn flush_with_fastlanes(
        &self,
        records: Vec<VectorRecord>,
        params: FlushParameters,
    ) -> Result<Vec<FileHandle>> {
        // SST uses FastLanesDataBlock with row-based encoding
        let mut blocks = Vec::new();

        // Process records in blocks
        for chunk in records.chunks(self.config.block_size) {
            let mut block = FastLanesDataBlock::new(
                chunk.to_vec(),
                BlockCompressionConfig::row_based() // Row-based configuration
            );

            // Quantized vectors are stored inline in VectorRecord within the block
            block.encode_with_scheme(FastLanesScheme::BitPacked { bits: 16 })?;
            blocks.push(block);
        }

        self.write_blocks(blocks, params).await
    }
}

impl HelixEngine {
    async fn flush_with_fastlanes(
        &self,
        records: Vec<VectorRecord>,
        params: FlushParameters,
    ) -> Result<Vec<FileHandle>> {
        // HELIX uses FastLanesDataBlock with columnar encoding + Hilbert ordering
        let ordered_records = self.hilbert_order(records)?;

        let mut block = FastLanesDataBlock::new(
            ordered_records,
            BlockCompressionConfig::columnar() // Columnar configuration
        );

        // Apply columnar encoding for better compression
        block.encode_with_scheme(FastLanesScheme::Delta {
            block_size: 128
        })?;

        self.write_block(block, params).await
    }
}
```

### 3.4 RAPTOR Engine Storage (Direct FastLanes Encoding)

RAPTOR uses FastLanes encoding directly without DataBlock wrapper:

```rust
impl RaptorEngine {
    async fn flush_with_fastlanes(
        &self,
        records: Vec<VectorRecord>,
        params: FlushParameters,
    ) -> Result<Vec<FileHandle>> {
        // Cluster into rowgroups
        let rowgroups = self.cluster_into_rowgroups(records)?;

        for rowgroup in rowgroups {
            let mut columnar_block = ColumnarBlock::default();

            // Extract quantized vectors if present
            let quantized_data = if rowgroup.records.iter()
                .any(|r| r.quantized.is_some()) {

                let mut quantized = QuantizedColumnarData::default();

                // Binary quantization column with FastLanes
                if let Some(binary_vectors) = extract_binary_quantization(&rowgroup.records) {
                    quantized.binary = Some(
                        FastLanesEncoder::encode(binary_vectors, FastLanesScheme::BitPacked { bits: 1 })?
                    );
                }

                // INT8 quantization column with FastLanes
                if let Some(int8_vectors) = extract_int8_quantization(&rowgroup.records) {
                    quantized.int8 = Some(
                        FastLanesEncoder::encode(int8_vectors, FastLanesScheme::FrameOfReference {
                            reference: 0,
                            bits: 8
                        })?
                    );
                }

                // PQ codes column with FastLanes
                if let Some(pq_codes) = extract_pq_codes(&rowgroup.records) {
                    quantized.pq_codes = Some(
                        FastLanesEncoder::encode(pq_codes, FastLanesScheme::BitPacked { bits: 8 })?
                    );
                }

                Some(quantized)
            } else {
                None
            };

            rowgroup.quantized_data = quantized_data;

            // Also encode main vectors with FastLanes
            columnar_block.vectors = FastLanesEncoder::encode(
                extract_vectors(&rowgroup.records),
                FastLanesScheme::Delta { block_size: 256 }
            )?;

            rowgroup.columnar_data = Some(columnar_block);
        }

        self.write_rowgroups(rowgroups, params).await
    }
}
```

## 4. Search Path Optimization

### 4.1 Progressive Search with Precomputed Quantization

```rust
impl ProgressiveSearch for XxxEngine {
    async fn search(&self, query: &SearchRequest) -> Result<Vec<SearchResult>> {
        // Stage 1: Scan using binary quantization (fastest)
        let candidates = if self.has_quantized_binary() {
            self.scan_binary_quantized(query, 10 * query.top_k).await?
        } else {
            vec![]
        };

        // Stage 2: Refine with INT8 (fast)
        let refined = if !candidates.is_empty() && self.has_quantized_int8() {
            self.refine_with_int8(query, candidates, 5 * query.top_k).await?
        } else {
            candidates
        };

        // Stage 3: Refine with PQ (balanced)
        let pq_refined = if !refined.is_empty() && self.has_quantized_pq() {
            self.refine_with_pq(query, refined, 2 * query.top_k).await?
        } else {
            refined
        };

        // Stage 4: Final ranking with FP32 (accurate)
        self.final_ranking_fp32(query, pq_refined, query.top_k).await
    }
}
```

## 5. Implementation Instructions for CLAUDE.md

Add the following to CLAUDE.md:

```markdown
## Quantized Vector Precomputation Implementation

### Overview
All storage engines MUST precompute quantized vectors during flush operations to enable fast searches. This leverages existing quantization capabilities in `compute::quantization::*`.

### Key Requirements

1. **Always Precompute During Flush**:
   - Binary and INT8: Always compute (fast, <5% overhead)
   - PQ: Compute for collections >10K vectors
   - Use `QuantizationPrecomputeService` from `compute::quantization::precompute`

2. **Use Existing Quantization Code**:
   - Binary: `UnifiedQuantizationEngine::quantize_to_binary()`
   - INT8: `UnifiedQuantizationEngine::quantize_to_int8()`
   - PQ: `StorageQuantizationEngine::quantize_batch_with_level()`
   - Codebooks: Store in `GlobalQuantizationCache`

3. **Storage Patterns**:
   - Row-based (SST, RAPTOR): Store in `VectorRecord.quantized` field
   - Columnar (VIPER, NOVA): Store as separate columns (`vector_binary`, `vector_int8`, `vector_pq8`)
   - Block-based (SWIFT): Store per-block sketches
   - Zone-based (HELIX): Store with zone maps

4. **Progressive Search**:
   - Stage 1: Binary scan (10x candidates)
   - Stage 2: INT8 refinement (5x candidates)
   - Stage 3: PQ refinement (2x candidates)
   - Stage 4: FP32 final ranking (exact top-k)

### Implementation Checklist

When implementing quantization precomputation:

- [ ] Add `QuantizationPrecomputeService` to engine's flush path
- [ ] Modify `VectorRecord` to include `QuantizedVectors` field
- [ ] For columnar engines: Add quantized columns to schema
- [ ] For row-based engines: Include quantized in serialization
- [ ] Update search to use precomputed quantization
- [ ] Add metrics for quantization overhead
- [ ] Test with collections of varying sizes

### Performance Targets

- Flush overhead: <10% for Binary+INT8, <20% with PQ
- Search speedup: 5-10x for first-stage filtering
- Storage overhead: <5% for Binary+INT8, <20% with PQ
- Memory savings: 30-50% when using quantized vectors in cache

### Testing Requirements

All engines must pass these tests:
- Precomputation during flush for 1K, 10K, 100K, 1M vectors
- Progressive search using precomputed quantization
- Correct reconstruction from quantized vectors
- Performance benchmarks showing speedup

### Common Pitfalls to Avoid

1. Do NOT quantize at search time (too slow)
2. Do NOT store only quantized without original (need FP32 for final ranking)
3. Do NOT recompute codebooks on every flush (use GlobalQuantizationCache)
4. Do NOT use same quantization for all collection sizes (use smart selection)
```

## 6. Migration Plan

### Phase 1: Infrastructure (Week 1)
1. Add `QuantizedVectors` to proto definition
2. Create `QuantizationPrecomputeService`
3. Update `VectorRecord` handling

### Phase 2: Engine Integration (Week 2-3)
1. SST: Add precomputation to flush
2. VIPER: Add quantized columns
3. NOVA: Add quantized columns
4. SWIFT: Add block sketches
5. RAPTOR: Update row format
6. HELIX: Add zone-aware quantization

### Phase 3: Search Optimization (Week 4)
1. Implement progressive search
2. Add quantization-aware query planning
3. Performance tuning

### Phase 4: Testing & Benchmarking (Week 5)
1. Unit tests for precomputation
2. Integration tests across engines
3. Performance benchmarks
4. Production validation

## 7. Expected Outcomes

### Performance Improvements
- **Search latency**: 5-10x faster for large collections
- **Memory usage**: 30-50% reduction with quantized cache
- **Throughput**: 3-5x higher QPS

### Storage Impact
- **Flush time**: +10% for Binary+INT8, +20% with PQ
- **Storage size**: +5% for Binary+INT8, +20% with PQ
- **Compaction**: Minimal impact (preserves quantization)

### Quality Metrics
- **Recall@10**: >95% with INT8, >99% with PQ8
- **Recall@100**: >99% with progressive refinement
```