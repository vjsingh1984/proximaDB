# ProximaDB Compression & Encoding Design - Release 1.0

## Core Design Principles

### Principle 1: 100% Vector Fidelity (Non-Negotiable)
**Original FP32 vectors MUST maintain perfect accuracy**. Any compression technique for FP32 vectors must be fully reversible with bit-perfect reconstruction. This is an absolute requirement.

### Principle 2: Lossless-Only for Original Vectors
- **FP32 vectors**: ONLY lossless compression (ZSTD) allowed
- **No lossy techniques**: Median normalization, trimmed mean, etc. are FORBIDDEN for FP32
- **Perfect recovery**: Must reconstruct exact original bit-for-bit values
- **User trust**: Users must always get back exactly what they stored

### Principle 3: Quantization as Optional Secondary Index
- **Separate storage**: Quantized vectors stored in addition to, never instead of, originals
- **Lossy allowed here**: Normalization techniques (median, trimmed mean) can apply to quantized copies
- **User choice**: Quantization only when explicitly requested via configuration
- **Clear trade-offs**: Users informed that quantized search trades accuracy for speed

### Principle 4: Storage Engine Determines Quantization Strategy
- **SST (Row)**: NEVER quantize - increases I/O due to row storage model
- **VIPER (Columnar)**: Quantization beneficial - can read only quantized columns
- **But always**: Original FP32 vectors preserved with 100% fidelity

## Executive Summary
Finalized design for granular per-collection compression control with optimal encoding strategies for SST and VIPER storage engines. This document captures all architectural decisions made for Release 1.0, ensuring **100% fidelity for original vectors** while offering optional quantization for performance optimization.

## Visual Architecture

### Compression Resolution Flow
![Compression Resolution](../diagrams/images/compression-resolution.svg)
*[View Mermaid Source](../diagrams/compression-resolution.mmd)*

### VIPER Compression Architecture
![VIPER Architecture](../diagrams/images/viper-compression-architecture.svg)
*[View Mermaid Source](../diagrams/viper-compression-architecture.mmd)*

### SST Vector Compression Techniques
![SST Compression](../diagrams/images/sst-vector-compression.svg)
*[View Mermaid Source](../diagrams/sst-vector-compression.mmd)*

### Mixed Compression Migration
![Migration Flow](../diagrams/images/mixed-compression-migration.svg)
*[View Mermaid Source](../diagrams/mixed-compression-migration.mmd)*

### Compression Decision Tree
![Decision Tree](../diagrams/images/compression-decision-tree.svg)
*[View Mermaid Source](../diagrams/compression-decision-tree.mmd)*

### Final Architecture Decisions
![Final Decisions](../diagrams/images/compression-decisions-final.svg)
*[View Mermaid Source](../diagrams/compression-decisions-final.mmd)*

### SST Adaptive Precision Detail
![Adaptive Precision](../diagrams/images/sst-adaptive-precision-detail.svg)
*[View Mermaid Source](../diagrams/sst-adaptive-precision-detail.mmd)*

### Implementation Timeline
![Timeline](../diagrams/images/implementation-phases.svg)
*[View Mermaid Source](../diagrams/implementation-phases.mmd)*

## Finalized Design Decisions (Release 1.0 - REVISED)

### Engine-Specific Quantization Strategy
![Engine Strategy](../diagrams/images/engine-quantization-strategy.svg)
*[View Mermaid Source](../diagrams/engine-quantization-strategy.mmd)*

### SST Compression Levels
![SST Compression](../diagrams/images/sst-compression-levels.svg)
*[View Mermaid Source](../diagrams/sst-compression-levels.mmd)*

| Component | Decision | Rationale |
|-----------|----------|-----------|
| **Default Compression** | ZSTD-3 | Balanced performance/compression for initial release |
| **SST Quantization** | **NOT RECOMMENDED** | Row storage requires reading entire record - quantization increases I/O |
| **SST Strategy** | FP32 only + ZSTD block compression | Optimal I/O, 100% accuracy, 20-40% compression |
| **VIPER Quantization** | **STRONGLY RECOMMENDED** | Columnar storage allows reading only quantized column |
| **VIPER Strategy** | Write quantized columns (INT8/PQ) | 24x less I/O, 100x faster search |
| **Migration Strategy** | Support mixed compression reading | Allows gradual migration without downtime |
| **Compression Levels** | User configurable (ZSTD 1-9) | Trade-off between speed and compression ratio |
| **Python SDK** | Smart defaults based on engine | VIPER → quantization, SST → FP32 only |

## Design Goals
1. **Granular Control**: Per-collection compression configuration
2. **Mixed Compression**: Support reading mixed compressed/uncompressed files during migration
3. **Optimal Encoding**: Engine-specific encoding for different data types
4. **Zero Downtime**: Change compression without service interruption
5. **Cost Optimization**: Balance storage cost vs CPU overhead

## 1. Proto-Based Configuration

### 1.1 Compression Configuration
```protobuf
// Core compression configuration
enum CompressionAlgorithm {
  COMPRESSION_NONE = 0;      // No compression
  COMPRESSION_ZSTD = 1;      // Zstandard (levels 1-22)
  COMPRESSION_LZ4 = 2;       // LZ4 fast compression (future)
  COMPRESSION_SNAPPY = 3;    // Snappy balanced (future)
}

message CompressionConfig {
  CompressionAlgorithm algorithm = 1;  // Algorithm to use
  optional int32 level = 2;            // Algorithm-specific level
  bool adaptive = 3;                   // Enable adaptive compression
  optional float min_ratio = 4;        // Min compression ratio threshold
}
```

### 1.2 Column-Specific Encoding (VIPER)
```protobuf
// Column encoding hints for Parquet storage
enum ColumnEncoding {
  ENCODING_AUTO = 0;        // Let Parquet decide
  ENCODING_PLAIN = 1;       // No encoding
  ENCODING_DICTIONARY = 2;  // Dictionary for low cardinality
  ENCODING_RLE = 3;         // Run-length encoding
  ENCODING_DELTA = 4;       // Delta encoding
  ENCODING_BITPACKED = 5;   // Bit-packing for integers
}

message FilterableColumnSpec {
  // ... existing fields ...
  optional ColumnEncoding encoding_hint = 6;  // Column-specific encoding
}
```

### 1.3 Storage Optimization Hints
```protobuf
enum AccessPattern {
  ACCESS_PATTERN_UNKNOWN = 0;
  ACCESS_PATTERN_WRITE_HEAVY = 1;  // High write throughput
  ACCESS_PATTERN_READ_HEAVY = 2;   // High read throughput
  ACCESS_PATTERN_BALANCED = 3;     // Mixed read/write
  ACCESS_PATTERN_ARCHIVE = 4;      // Rarely accessed
}

enum DataDensity {
  DENSITY_UNKNOWN = 0;
  DENSITY_DENSE = 1;    // >80% non-zero values
  DENSITY_SPARSE = 2;   // <20% non-zero values
  DENSITY_MIXED = 3;    // Mixed density
}

message StorageOptimizationHints {
  AccessPattern access_pattern = 1;
  DataDensity data_density = 2;
  bool frequent_updates = 3;
  optional int64 expected_size_gb = 4;
  optional float read_write_ratio = 5;
}
```

## 2. Engine-Specific Implementation

### 2.1 VIPER Engine (Columnar/Parquet)

#### Critical Design Decision: Leverage Standard Arrow/Parquet Infrastructure

```
┌─────────────┐     ┌──────────────┐     ┌──────────────┐     ┌─────────────┐
│   FP32      │────▶│  Transform   │────▶│   Standard   │────▶│   Parquet   │
│  Vectors    │     │   Layer      │     │    Arrow     │     │    File     │
└─────────────┘     └──────────────┘     └──────────────┘     └─────────────┘
                           │                     │                     │
                    ┌──────▼──────┐       ┌─────▼─────┐        ┌──────▼──────┐
                    │ Quantization│       │  List<T>  │        │ Compressed  │
                    │   Packing   │       │  Format   │        │   Storage   │
                    │   Sparse→COO│       │  Native   │        │   ZSTD/LZ4  │
                    └─────────────┘       └───────────┘        └─────────────┘
                                                                       │
                    ┌─────────────┐       ┌───────────┐        ┌──────▼──────┐
                    │   FP32      │◀──────│ Transform │◀───────│    Read     │
                    │  Vectors    │       │   Back    │        │   Parquet   │
                    └─────────────┘       └───────────┘        └─────────────┘
```

**Principle**: Use standard Arrow writers/readers with their supported encodings. Custom transformations (quantization, bit-packing) happen BEFORE writing and AFTER reading, not within the Parquet layer.

**Rationale**:
1. Not all Parquet readers/writers support all encodings
2. Maintains compatibility with ecosystem tools
3. Allows us to benefit from Arrow/Parquet optimizations
4. Simplifies debugging and maintenance
5. Data must be converted back to fp32/quantized format for distance calculations anyway

#### Vector Storage Strategy

| Vector Type | Pre-Write Transform | Parquet Storage | Parquet Encoding | Post-Read Transform |
|------------|-------------------|-----------------|------------------|-------------------|
| **FP32 Vectors** | None | List<Float32> | PLAIN or BYTE_STREAM_SPLIT* | None |
| **Quantized INT8** | None | List<Int8> | PLAIN | None |
| **Quantized INT16** | None | List<Int16> | PLAIN | None |
| **Custom Bit-Width (4-6 bit)** | Pack with bytemuck to INT8 | List<Int8> | PLAIN | Unpack with bytemuck |
| **Sparse Vectors** | Convert to COO format | Struct{indices, values} | PLAIN | Reconstruct dense |

*BYTE_STREAM_SPLIT only if supported by Arrow version

#### Implementation Architecture
```rust
// VIPER flush.rs - Layered approach
impl FlushManager {
    async fn flush_vectors(&self, records: &[VectorRecord]) -> Result<()> {
        // Layer 1: Transform vectors based on quantization config
        let transformed_records = self.transform_vectors_for_storage(records)?;
        
        // Layer 2: Build Arrow RecordBatch with standard types
        let batch = self.build_arrow_batch(&transformed_records)?;
        
        // Layer 3: Configure Parquet writer with SUPPORTED encodings only
        let props = self.build_writer_properties()?;
        
        // Layer 4: Write using standard ArrowWriter
        let mut writer = ArrowWriter::try_new(buffer, batch.schema(), Some(props))?;
        writer.write(&batch)?;
        writer.close()?;
    }
    
    fn transform_vectors_for_storage(&self, records: &[VectorRecord]) -> Result<Vec<TransformedRecord>> {
        // Custom transformations BEFORE Parquet
        match self.quantization_config {
            Some(QuantConfig::CustomBitWidth(bits)) if bits < 8 => {
                // Pack multiple values into INT8 using bytemuck
                records.iter().map(|r| {
                    let packed = pack_vector_custom_bits(&r.vector, bits);
                    TransformedRecord {
                        vector_data: VectorData::PackedInt8(packed),
                        ..r.clone()
                    }
                }).collect()
            }
            Some(QuantConfig::Sparse(threshold)) => {
                // Convert to COO format for sparse vectors
                records.iter().map(|r| {
                    let (indices, values) = to_coo_format(&r.vector, threshold);
                    TransformedRecord {
                        vector_data: VectorData::Sparse { indices, values },
                        ..r.clone()
                    }
                }).collect()
            }
            _ => {
                // No transformation - store as-is
                records.iter().map(|r| TransformedRecord {
                    vector_data: VectorData::Dense(r.vector.clone()),
                    ..r.clone()
                }).collect()
            }
        }
    }
    
    fn build_writer_properties(&self) -> Result<WriterProperties> {
        let mut props = WriterProperties::builder()
            .set_compression(self.get_compression()?)
            .set_max_row_group_size(self.config.row_group_size);
        
        // Only use encodings that are ACTUALLY SUPPORTED by our Arrow version
        // Check Arrow documentation for supported encodings per type
        
        // Safe encodings that are widely supported:
        props = props
            .set_dictionary_enabled(true)  // Dictionary encoding for strings
            .set_statistics_enabled(true); // Column statistics
        
        // Conditionally enable advanced encodings if supported
        if self.arrow_supports_byte_stream_split() {
            // Only for FLOAT/DOUBLE columns in newer Arrow versions
            props = props.set_column_encoding(
                ColumnPath::from("vector"),
                Encoding::BYTE_STREAM_SPLIT
            );
        }
        
        Ok(props.build())
    }
}
```

#### Reading Strategy
```rust
impl VectorReader {
    async fn read_vectors(&self, file: &Path) -> Result<Vec<VectorRecord>> {
        // Layer 1: Read Parquet file with standard ArrowReader
        let file = File::open(file)?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)?
            .build()?;
        
        // Layer 2: Process batches
        let mut records = Vec::new();
        for batch in reader {
            let batch = batch?;
            
            // Layer 3: Transform back to VectorRecord format
            let batch_records = self.transform_from_storage(&batch)?;
            records.extend(batch_records);
        }
        
        Ok(records)
    }
    
    fn transform_from_storage(&self, batch: &RecordBatch) -> Result<Vec<VectorRecord>> {
        // Custom transformations AFTER reading from Parquet
        let vector_array = batch.column_by_name("vector")
            .ok_or("Missing vector column")?;
        
        match self.storage_format {
            StorageFormat::PackedCustomBits(bits) => {
                // Unpack custom bit-width vectors
                let packed_array = vector_array.as_any()
                    .downcast_ref::<Int8Array>()
                    .ok_or("Expected Int8Array for packed vectors")?;
                
                (0..batch.num_rows()).map(|i| {
                    let packed = get_packed_vector(packed_array, i);
                    let unpacked = unpack_vector_custom_bits(&packed, bits, self.dimension);
                    // Convert to fp32 for distance calculation
                    let fp32_vector = dequantize_to_fp32(&unpacked, bits);
                    build_vector_record(fp32_vector, batch, i)
                }).collect()
            }
            StorageFormat::Sparse => {
                // Reconstruct dense vectors from COO format
                // ...
            }
            _ => {
                // Standard format - direct conversion
                // ...
            }
        }
    }
}
```

#### Metadata Column Encoding
```rust
// Apply filterable column encoding hints
for column in filterable_columns {
    match column.encoding_hint {
        ENCODING_DICTIONARY => {
            // IDs, categories, low-cardinality strings
            props_builder.set_column_dictionary_enabled(column.name, true)
        }
        ENCODING_RLE => {
            // Boolean flags, repeated values
            props_builder.set_column_encoding(column.name, Encoding::RLE)
        }
        ENCODING_DELTA => {
            // Timestamps, sequential IDs
            props_builder.set_column_encoding(column.name, Encoding::DELTA_BINARY_PACKED)
        }
        _ => {} // Use Parquet auto-detection
    }
}
```

### 2.2 SST Engine (Row-Based/SSTable)

#### SSTable Header v2
```rust
pub struct SstableHeader {
    // ... existing fields ...
    
    // Compression metadata
    pub compression_algorithm: CompressionAlgorithmSst,
    pub compression_level: u8,
    
    // Per-block compression info (for mixed compression)
    pub block_compression_map: Vec<BlockCompressionInfo>,
}

pub struct BlockCompressionInfo {
    pub block_id: u32,
    pub algorithm: CompressionAlgorithmSst,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
}
```

#### Block-Level Compression Architecture
```rust
pub struct DataBlock {
    pub vectors: Vec<VectorRecord>,  // Target: 2000-2500 vectors
    pub block_id: u32,
    pub vector_type: VectorType,     // FP32 only for SST
    pub sort_order: SortOrder,       // ID-based with metadata grouping
}

impl SstableWriter {
    fn write_block(&self, vectors: Vec<VectorRecord>, compression: &CompressionConfig) {
        // Calculate optimal block size based on vector dimension
        let dimension = vectors[0].dimension;
        let vector_size = dimension * 4 + 400; // FP32 + metadata overhead
        let target_vectors = 2000; // Optimal for compression
        
        // Dynamic block sizing based on dimension
        let block_size_mb = match dimension {
            d if d <= 384 => 4,    // 4MB for small vectors
            d if d <= 768 => 8,    // 8MB for medium vectors  
            d if d <= 1536 => 12,  // 12MB for large vectors
            _ => 16,               // 16MB for very large vectors
        };
        
        let vectors_per_block = (block_size_mb * 1_000_000) / vector_size;
        
        for chunk in vectors.chunks(block_size) {
            let block = DataBlock {
                vectors: chunk.to_vec(),
                block_id: self.next_block_id(),
                vector_type: chunk[0].vector_type,
            };
            
            // Serialize entire block
            let block_data = self.serialize_block(&block)?;
            
            // Apply compression to ENTIRE block
            let (compressed_data, compression_info) = match block.vector_type {
                VectorType::Fp32 => {
                    // FP32: ZSTD-3 on entire block
                    let compressed = zstd::compress(&block_data, 3)?;
                    let ratio = block_data.len() as f32 / compressed.len() as f32;
                    println!("Block {} compressed: {}MB → {}MB (ratio: {:.1}x)", 
                        block.block_id,
                        block_data.len() / 1_000_000,
                        compressed.len() / 1_000_000,
                        ratio
                    );
                    (compressed, CompressionInfo::Zstd3)
                }
                VectorType::Int16 => {
                    // INT16: First pack to 12-bit, then ZSTD
                    let packed = self.pack_int16_to_12bit(&block)?;
                    let compressed = zstd::compress(&packed, 3)?;
                    (compressed, CompressionInfo::Adaptive12Bit)
                }
                VectorType::Int8 => {
                    // INT8: First pack to 6-bit, then ZSTD
                    let packed = self.pack_int8_to_6bit(&block)?;
                    let compressed = zstd::compress(&packed, 3)?;
                    (compressed, CompressionInfo::Adaptive6Bit)
                }
            };
            
            self.write_compressed_block(compressed_data, compression_info)?;
        }
    }
}
```

## 3. Compression Resolution Strategy

```
                     ┌──────────────────────┐
                     │  SDK Request         │
                     │  compression_config? │
                     └──────────┬───────────┘
                                │
                     ┌──────────▼───────────┐
                     │  Has Config?         │
                     └──────┬───┬───────────┘
                          Yes│   │No
                             │   │
                   ┌─────────▼─┐ ▼─────────────┐
                   │ Validate & │ Server Engine │
                   │  Adjust    │   Defaults    │
                   └─────────┬─┘ └──────┬──────┘
                             │          │
                             ▼          ▼
                     ┌──────────────────────┐
                     │  Apply Compression   │
                     │     Configuration    │
                     └──────────┬───────────┘
                                │
                     ┌──────────▼───────────┐
                     │  Check Access        │
                     │     Pattern          │
                     └──────────┬───────────┘
                                │
                ┌───────────────┼───────────────┐
                ▼               ▼               ▼
         ┌──────────┐    ┌──────────┐   ┌──────────┐
         │ Archive  │    │  Write   │   │  Read    │
         │ ZSTD-9   │    │  Heavy   │   │  Heavy   │
         │ Max Comp │    │   LZ4    │   │  None    │
         └──────────┘    └──────────┘   └──────────┘
```

### 3.1 Collection Service Resolution
```rust
fn resolve_compression_config(
    requested: Option<&CompressionConfig>,
    server_config: &StorageConfig,
    storage_engine: StorageEngine,
) -> CompressionConfig {
    // Priority order:
    // 1. Explicit SDK request (if valid)
    // 2. Server engine-specific defaults
    // 3. No compression
    
    if let Some(config) = requested {
        return validate_and_adjust(config);
    }
    
    // Engine-specific defaults
    match storage_engine {
        StorageEngine::Viper => {
            if server_config.viper.compression_enabled {
                CompressionConfig {
                    algorithm: COMPRESSION_ZSTD,
                    level: Some(3),
                    adaptive: true,
                    min_ratio: Some(1.5),
                }
            } else {
                CompressionConfig::none()
            }
        }
        StorageEngine::Sst => {
            if server_config.sst.compression_enabled {
                CompressionConfig {
                    algorithm: COMPRESSION_ZSTD,
                    level: Some(3),
                    adaptive: false,
                    min_ratio: Some(1.5),
                }
            } else {
                CompressionConfig::none()
            }
        }
    }
}
```

### 3.2 Automatic Compression Selection
```rust
fn auto_select_compression(
    hints: &StorageOptimizationHints,
    data_density: DataDensity,
) -> CompressionConfig {
    match (hints.access_pattern, data_density) {
        // Archive data: Maximum compression
        (ACCESS_PATTERN_ARCHIVE, _) => CompressionConfig {
            algorithm: COMPRESSION_ZSTD,
            level: Some(9),
            adaptive: true,
            min_ratio: Some(1.2),
        },
        
        // Sparse vectors: High compression
        (_, DENSITY_SPARSE) => CompressionConfig {
            algorithm: COMPRESSION_ZSTD,
            level: Some(6),
            adaptive: true,
            min_ratio: Some(1.5),
        },
        
        // Write-heavy: Fast compression
        (ACCESS_PATTERN_WRITE_HEAVY, _) => CompressionConfig {
            algorithm: COMPRESSION_LZ4,
            level: None,
            adaptive: false,
            min_ratio: Some(2.0),
        },
        
        // Read-heavy hot data: No compression
        (ACCESS_PATTERN_READ_HEAVY, DENSITY_DENSE) => CompressionConfig {
            algorithm: COMPRESSION_NONE,
            level: None,
            adaptive: false,
            min_ratio: None,
        },
        
        // Default: Balanced compression
        _ => CompressionConfig {
            algorithm: COMPRESSION_ZSTD,
            level: Some(3),
            adaptive: true,
            min_ratio: Some(1.5),
        },
    }
}
```

## 4. Mixed Compression Support

### 4.1 Block-Level Decompression
```rust
impl SstableReader {
    async fn read_vectors(&self, range: Range<usize>) -> Result<Vec<VectorRecord>> {
        // Determine which blocks contain the requested range
        let start_block = range.start / self.vectors_per_block;
        let end_block = range.end / self.vectors_per_block;
        
        let mut all_vectors = Vec::new();
        
        for block_id in start_block..=end_block {
            // Read and decompress ENTIRE block at once
            let block_vectors = self.read_and_decompress_block(block_id).await?;
            
            // Extract only the vectors we need from this block
            let block_start = block_id * self.vectors_per_block;
            let block_end = block_start + block_vectors.len();
            
            let local_start = range.start.saturating_sub(block_start);
            let local_end = (range.end.min(block_end) - block_start);
            
            all_vectors.extend_from_slice(&block_vectors[local_start..local_end]);
        }
        
        Ok(all_vectors)
    }
    
    async fn read_and_decompress_block(&self, block_id: u32) -> Result<Vec<VectorRecord>> {
        // Read compressed block
        let compressed_data = self.read_raw_block(block_id).await?;
        let block_info = &self.header.block_compression_map[block_id as usize];
        
        // Single decompression for entire block
        let decompressed = match block_info.compression_type {
            CompressionInfo::Zstd3 => {
                // Decompress entire block at once
                let data = zstd::decompress(&compressed_data)?;
                self.deserialize_fp32_block(&data)?
            }
            CompressionInfo::Adaptive12Bit => {
                // Decompress then unpack
                let data = zstd::decompress(&compressed_data)?;
                self.unpack_int16_from_12bit(&data)?
            }
            CompressionInfo::Adaptive6Bit => {
                // Decompress then unpack
                let data = zstd::decompress(&compressed_data)?;
                self.unpack_int8_from_6bit(&data)?
            }
        };
        
        Ok(decompressed)
    }
}
```

### 4.2 Gradual Migration During Compaction
```rust
impl CompactionManager {
    async fn compact_with_compression_change(
        &self,
        input_files: Vec<SstFile>,
        new_compression: &CompressionConfig,
    ) -> Result<SstFile> {
        let mut output_writer = SstableWriter::new();
        
        for input_file in input_files {
            let reader = SstableReader::open(&input_file)?;
            
            // Read blocks with old compression
            for block_id in reader.block_ids() {
                let data = reader.read_block(block_id).await?;
                let records = deserialize_block(&data)?;
                
                // Write with new compression
                output_writer.write_block(&records, new_compression)?;
            }
        }
        
        output_writer.finalize()
    }
}
```

## 5. Trade-offs Analysis

### 5.1 Compression Algorithm Trade-offs

| Algorithm | Compression Ratio | Speed | CPU Usage | Use Case |
|-----------|------------------|-------|-----------|----------|
| **None** | 1:1 | Fastest | None | Hot data, real-time |
| **LZ4** | 2-3:1 | Very Fast | Low | Write-heavy, warm data |
| **Snappy** | 2-3:1 | Fast | Medium | Balanced workloads |
| **ZSTD-1** | 3-4:1 | Fast | Medium | General purpose |
| **ZSTD-3** | 4-5:1 | Moderate | Medium | Default balanced |
| **ZSTD-6** | 5-7:1 | Slow | High | Read-heavy, cold data |
| **ZSTD-9+** | 7-10:1 | Very Slow | Very High | Archive, rarely accessed |

### 5.2 Vector Encoding Trade-offs

| Vector Type | Storage Size | Query Speed | Accuracy | Best For |
|------------|--------------|-------------|----------|----------|
| **FP32 (No Compression)** | 100% | Fastest | 100% | Hot data, high accuracy |
| **FP32 + BYTE_STREAM_SPLIT** | 70-90% | Fast | 100% | General purpose |
| **INT16 Quantized** | 50% | Fast | 99%+ | Balanced |
| **INT8 Quantized** | 25% | Very Fast | 95-99% | Large scale |
| **6-bit Custom** | 19% | Fast* | 90-95% | Very large scale |
| **4-bit Custom** | 12.5% | Fast* | 85-90% | Extreme scale |

*With SIMD optimizations

### 5.3 Metadata Encoding Trade-offs

| Encoding | Best For | Compression | Query Speed | CPU Cost |
|----------|----------|-------------|-------------|----------|
| **Dictionary** | Low cardinality strings (IDs, categories) | High (10:1+) | Very Fast | Low |
| **RLE** | Repeated values (flags, states) | Very High (100:1+) | Very Fast | Very Low |
| **Delta** | Sequential values (timestamps) | High (5:1+) | Fast | Low |
| **Bit-Packed** | Small integers | Moderate (2:1) | Fast | Low |
| **Plain** | Random data | None | Fastest | None |

## 6. Performance Impact

### 6.1 Expected Improvements

**Storage Reduction**:
- Dense vectors: 10-30% with BYTE_STREAM_SPLIT + ZSTD
- Sparse vectors: 60-80% with RLE + ZSTD-9
- Quantized vectors: 75% (INT8) to 87.5% (4-bit)
- Metadata: 50-90% with appropriate encoding

**Query Performance**:
- Hot data (no compression): Baseline
- Warm data (LZ4): -5% to -10% due to decompression
- Cold data (ZSTD-9): -20% to -30% due to decompression
- Quantized (INT8): +20% to +40% due to SIMD and cache efficiency

**Write Performance**:
- No compression: Baseline
- LZ4: -10% to -15%
- ZSTD-3: -20% to -30%
- ZSTD-9: -50% to -70%

### 6.2 Memory Impact

**Page Cache Efficiency**:
- Compressed data stays compressed in page cache
- 2-5x more data fits in same memory
- Decompression happens on-demand per query

**Working Set Size**:
- Quantized vectors: 4x more vectors in memory
- Dictionary encoding: 10x+ reduction for repeated strings
- Overall: 2-3x improvement in data density

## 7. Implementation Phases (Updated for Release 1.0)

### Phase 1: Core Infrastructure (Week 1) - IN PROGRESS
- [x] Proto definitions for compression config
- [x] Collection service compression resolution  
- [x] VIPER ZSTD-3 compression support
- [ ] **SST Adaptive Precision implementation** ← PRIORITY

### Phase 2: Mixed Compression (Week 2)
- [ ] SST block headers v2 with adaptive precision metadata
- [ ] Mixed compression reading support
- [ ] Compaction with compression migration
- [ ] VIPER mixed file support

### Phase 3: Advanced VIPER (Week 3)
- [ ] **4-6 bit packing to INT8 with bytemuck** ← PRIORITY
- [ ] Column-specific encoding hints
- [ ] Sparse vector COO format transformation
- [ ] Pre/post transformation layer

### Phase 4: Python SDK (Week 4)
- [ ] Compression config in create_collection
- [ ] **Smart defaults by dimension/engine** ← PRIORITY
- [ ] Update_collection_compression API
- [ ] Optimization hints support

### Phase 5: Future Optimizations (Post-Release)
- [ ] SST XOR delta encoding (secondary method)
- [ ] SST sparse format optimization
- [ ] Hybrid compression selection
- [ ] Auto-tuning based on workload

## 8. Monitoring & Metrics

### 8.1 Compression Metrics
```rust
struct CompressionMetrics {
    // Per-collection metrics
    compression_ratio: f32,          // Compressed/uncompressed size
    compression_time_ms: u64,        // Time to compress during flush
    decompression_time_ms: u64,      // Average decompression time
    bytes_saved: u64,                // Total bytes saved
    
    // Per-algorithm metrics
    algorithm_usage: HashMap<String, u64>,  // Count by algorithm
    average_ratio_by_algo: HashMap<String, f32>,
    
    // Performance impact
    query_latency_impact_ms: f64,    // Additional latency from decompression
    cpu_overhead_percent: f32,       // CPU usage for compression
}
```

### 8.2 Alerting Thresholds
- Compression ratio < min_ratio: Disable compression
- Decompression time > 100ms: Alert for optimization
- CPU overhead > 20%: Consider lighter compression
- Memory pressure: Switch to more aggressive compression

## 9. Configuration Examples

### 9.1 Real-time Search (Low Latency)
```python
client.create_collection(
    name="realtime_search",
    dimension=768,
    compression=CompressionConfig(
        algorithm=CompressionAlgorithm.NONE,  # No compression
    ),
    optimization_hints=StorageOptimizationHints(
        access_pattern=AccessPattern.READ_HEAVY,
        data_density=DataDensity.DENSE,
    )
)
```

### 9.2 Large Scale Archive
```python
client.create_collection(
    name="historical_data",
    dimension=1536,
    compression=CompressionConfig(
        algorithm=CompressionAlgorithm.ZSTD,
        level=9,  # Maximum compression
        adaptive=True,
        min_ratio=1.2,
    ),
    optimization_hints=StorageOptimizationHints(
        access_pattern=AccessPattern.ARCHIVE,
        data_density=DataDensity.SPARSE,
    )
)
```

### 9.3 Balanced Workload
```python
client.create_collection(
    name="product_embeddings",
    dimension=384,
    compression=CompressionConfig(
        algorithm=CompressionAlgorithm.ZSTD,
        level=3,  # Balanced
        adaptive=True,
        min_ratio=1.5,
    ),
    filterable_columns=[
        FilterableColumnSpec(
            name="category",
            data_type=FilterableDataType.STRING,
            encoding_hint=ColumnEncoding.DICTIONARY,
        ),
        FilterableColumnSpec(
            name="price",
            data_type=FilterableDataType.FLOAT,
            encoding_hint=ColumnEncoding.DELTA,
        ),
    ]
)
```

## 10. Future Enhancements

### 10.1 Adaptive Compression
- Monitor access patterns and adjust compression dynamically
- Hot/cold tier migration based on access frequency
- Predictive compression based on data characteristics

### 10.2 Hardware Acceleration
- Intel QAT for hardware compression
- GPU-accelerated decompression for batch queries
- FPGA compression for specialized workloads

### 10.3 Advanced Quantization
- Learned quantization with per-dimension bit allocation
- Vector-specific quantization based on importance
- Progressive quantization with quality levels

## 11. SST Vector Optimization (Custom Serialization)

### 11.1 PRIMARY METHOD: Adaptive Precision Reduction ⭐

```
┌──────────────────────────────────────────────────────────────────┐
│                        SST Vector Analysis                        │
├────────────────┬─────────────────┬─────────────────┬────────────┤
│   Similarity   │    Sparsity     │   Precision     │   Random   │
│    Analysis    │    Detection    │   Analysis      │   Dense    │
├────────────────┼─────────────────┼─────────────────┼────────────┤
│ Similar: >0.8  │  Sparse: >70%   │  Can Reduce     │  Default   │
│      ↓         │       ↓         │       ↓         │      ↓     │
│  XOR Delta     │   COO/CSR       │   Adaptive      │   ZSTD     │
│  40-60% comp   │  70-90% comp    │  50-75% comp    │  10-30%    │
└────────────────┴─────────────────┴─────────────────┴────────────┘
                               ↓
                    ┌──────────────────────┐
                    │   SST Block V2       │
                    ├──────────────────────┤
                    │ • Magic: SST2        │
                    │ • Compression Type   │
                    │ • Vector Encoding    │
                    │ • Dimensions         │
                    │ • Checksums          │
                    └──────────────────────┘
```

**REVISED DECISION: SST should NOT use quantization - FP32 only with block compression**

#### SST ENGINE STRATEGY [FINAL FOR RELEASE 1.0]

##### Primary Approach: FP32-Only with Block Compression
- **Method**: ZSTD on entire data blocks
- **Block Size**: Dimension-aware (targeting 2000-2500 vectors/block)
  - 384D: 4MB blocks (~2100 vectors)
  - 768D: 8MB blocks (~2350 vectors)
  - 1536D: 12-16MB blocks (~2000 vectors)
- **NO QUANTIZATION**: Avoid dual storage in row format
- **Compression Levels**: User configurable (1-9)
  - ZSTD-1: 15-25% reduction, fastest (real-time)
  - ZSTD-3: 20-40% reduction, balanced (default)
  - ZSTD-6: 35-50% reduction, cold storage
  - ZSTD-9: 40-60% reduction, archive
- **Sparse Bonus**: Additional 20-30% if >70% zeros
- **Accuracy**: 100% preserved always

##### Why NO Quantization in SST:
```
Row Storage Reality:
[ID | FP32 Vector | Quantized Vector | Metadata]
        ↑              ↑
   Must read      Adds I/O overhead
   entire row     Makes it WORSE
```

#### VIPER ENGINE STRATEGY [DUAL STORAGE WITH 100% FIDELITY]

##### Critical Requirement: FP32 Column MUST Preserve Full Fidelity
```rust
// VIPER Column Structure with Guaranteed Fidelity
struct ViperColumns {
    // PRIMARY: Original vectors with 100% fidelity - ALWAYS PRESENT
    fp32_vector_column: Column<Vec<f32>>,  // Lossless ZSTD only, NO modifications
    
    // SECONDARY: Optional quantized for performance
    int8_vector_column: Option<Column<Vec<i8>>>,     // Can use normalization
    pq_codes_column: Option<Column<Vec<u8>>>,        // Can use lossy techniques
    
    // METADATA
    metadata_columns: HashMap<String, Column>,
}
```

##### Storage Strategy: Separate Columns with Different Techniques
```
Column Storage:
├── fp32_vector_column     ← ALWAYS stored, 100% fidelity, lossless only
│   └── Compression: ZSTD only (NO normalization allowed)
├── int8_vector_column     ← Optional, can use normalization + quantization
│   └── Compression: Trimmed mean → Quantize → ZSTD
├── pq_codes_column        ← Optional, maximum compression
│   └── Compression: Median normalize → PQ4/8 → ZSTD  
└── metadata_columns       ← Standard columnar compression
```

##### Quantized Column Optimization (Secondary Storage Only)
```rust
// Normalization + Quantization for MAXIMUM compression
// Applied ONLY to quantized columns, NEVER to FP32
impl QuantizedColumnCompression {
    fn compress_for_search(&self, original_vectors: &[Vec<f32>]) -> CompressedColumn {
        // Clone for quantization - originals untouched
        let working_copy = original_vectors.to_vec();
        
        // Step 1: Analyze distribution
        let stats = analyze_distribution(&working_copy);
        
        // Step 2: Select best normalization for compression
        let normalized = match stats.distribution_type {
            Distribution::Normal => {
                // Mean normalization: fastest, good for normal data
                mean_normalize(working_copy)  // 2-3x compression boost
            },
            Distribution::Skewed => {
                // Trimmed mean: robust to outliers
                trimmed_mean_normalize(working_copy, 0.05)  // 3-4x boost
            },
            Distribution::HeavyTailed => {
                // Median: most robust but slower
                median_normalize(working_copy)  // 4-5x boost
            }
        };
        
        // Step 3: Quantize normalized vectors
        let quantized = match self.config.quantization_type {
            QuantType::INT8 => int8_quantize(normalized),    // 24x reduction
            QuantType::PQ8 => pq8_quantize(normalized),      // 48x reduction
            QuantType::PQ4 => pq4_quantize(normalized),      // 96x reduction
        };
        
        // Step 4: Apply ZSTD for additional 20-40% reduction
        let compressed = zstd::compress(&quantized, self.config.zstd_level);
        
        CompressedColumn {
            data: compressed,
            compression_chain: vec![
                "Normalization (lossy)",
                "Quantization (lossy)", 
                "ZSTD (lossless)"
            ],
            total_reduction: "60-120x for quantized column",
            original_fidelity: false,  // This is the quantized copy
        }
    }
}
```

- **Benefits**: 
  - FP32 column: 100% fidelity always available
  - INT8 column: 24x less I/O with normalization boosting compression
  - PQ column: 48-96x reduction with aggressive normalization
  - Flexible: Use quantized for search, FP32 for final results

```rust
struct AdaptivePrecisionBlock {
    precision_bits: u8,        // Bits kept (6, 8, 12, or 16)
    scale: f32,                // Scaling factor for normalization
    offset: f32,               // Normalization offset  
    min_value: f32,            // Original min for reconstruction
    max_value: f32,            // Original max for reconstruction
    compressed_data: Vec<u8>,  // Reduced precision packed data
}
```

**Implementation Details**:
```rust
enum VectorCompressionStrategy {
    // FP32 vectors - lossless only
    Fp32Lossless {
        algorithm: CompressionAlgorithm,  // ZSTD-3
    },
    
    // Quantized vectors - can apply adaptive precision
    QuantizedAdaptive {
        source_bits: u8,      // Original quantization (8 or 16)
        target_bits: u8,      // Further reduction (6, 12, etc)
        preserve_range: bool, // Maintain original quantization range
    },
}

impl AdaptivePrecisionCompressor {
    fn compress(vectors: &QuantizedVectors, target_bits: u8) -> AdaptivePrecisionBlock {
        // 1. Analyze range
        let (min, max) = find_range(vectors);
        let range = max - min;
        
        // 2. Calculate scale for target precision
        let max_value = match target_bits {
            6 => 63.0,      // 2^6 - 1
            8 => 255.0,     // 2^8 - 1
            12 => 4095.0,   // 2^12 - 1
            16 => 65535.0,  // 2^16 - 1
            _ => 255.0,
        };
        
        let scale = max_value / range;
        let offset = min;
        
        // 3. Quantize and pack
        let mut compressed = Vec::new();
        for vector in vectors {
            for &value in vector {
                let normalized = (value - offset) * scale;
                let quantized = normalized.round() as u32;
                // Pack based on target_bits
                pack_bits(&mut compressed, quantized, target_bits);
            }
        }
        
        AdaptivePrecisionBlock {
            precision_bits: target_bits,
            scale,
            offset,
            min_value: min,
            max_value: max,
            compressed_data: compressed,
        }
    }
    
    fn decompress(block: &AdaptivePrecisionBlock) -> Vec<Vec<f32>> {
        // Fast decompression with predictable overhead
        let mut vectors = Vec::new();
        let mut bits = BitReader::new(&block.compressed_data);
        
        while !bits.is_empty() {
            let mut vector = Vec::with_capacity(self.dimension);
            for _ in 0..self.dimension {
                let quantized = bits.read(block.precision_bits);
                let normalized = quantized as f32;
                let value = (normalized / block.scale) + block.offset;
                vector.push(value);
            }
            vectors.push(vector);
        }
        
        vectors
    }
}
```

**Performance Characteristics**:
- ✅ Universal applicability (works for ALL vectors)
- ✅ Consistent 50-75% compression ratio
- ✅ Predictable -5% read performance impact
- ✅ Tunable precision (6/8/12/16 bits)
- ✅ Simple implementation, low bug risk

#### FUTURE: XOR-Based Delta Encoding (Phase 5)
*Deferred to future optimization phase*
- Will be added as secondary method for sequential similar vectors
- Compression: 40-60% (less than adaptive precision)
- Limited applicability (only ~30% of vectors benefit)

#### FUTURE: Sparse Vector Optimization (Phase 5)
**Compression Ratio**: 10-30% for >70% sparse vectors
**CPU Cost**: Low write, Very low read
**Best For**: Sparse embeddings

```rust
enum SparseFormat {
    COO { indices: Vec<u32>, values: Vec<f32> },           // Coordinate format
    CSR { indptr: Vec<u32>, indices: Vec<u32>, values: Vec<f32> }, // Compressed sparse row
    RLE { runs: Vec<(u32, f32)> },                        // Run-length encoding
}
```

**Trade-offs**:
- ✅ Massive savings for sparse data
- ✅ Very fast operations on sparse vectors
- ❌ Overhead for dense vectors
- ❌ Format conversion cost

### 11.2 SST Block Format v2

```rust
#[repr(C)]
struct DataBlockHeaderV2 {
    magic: [u8; 4],                  // "SST2"
    version: u16,                    // Format version
    compression_type: CompressionType, // Compression used
    vector_encoding: VectorEncoding,  // Vector encoding method
    
    // Sizes
    uncompressed_size: u32,          // Original size
    compressed_size: u32,            // Compressed size
    vector_count: u32,               // Number of vectors
    vector_dimension: u32,           // Dimension per vector
    
    // Compression metadata
    compression_meta: [u8; 32],      // Algorithm-specific metadata
    
    // Checksums
    header_checksum: u32,            // CRC32 of header
    data_checksum: u32,              // CRC32 of data
}
```

### 11.3 Streaming Decompression Architecture

```rust
trait VectorDecompressor: Send + Sync {
    fn decompress_next(&mut self, input: &[u8]) -> Result<Vec<f32>>;
    fn skip(&mut self, count: usize) -> Result<()>;
    fn reset(&mut self);
}

struct StreamingReader {
    header: DataBlockHeaderV2,
    decompressor: Box<dyn VectorDecompressor>,
    buffer: Vec<u8>,
    position: usize,
}
```

**Benefits**:
- Constant memory usage regardless of block size
- Efficient range queries
- Parallel decompression support
- Zero-copy where possible

## 12. Final Recommendations & Trade-offs (Release 1.0 Decisions)

### 12.1 VIPER Engine - CONFIRMED APPROACH

**DECISION**: Use standard Arrow/Parquet infrastructure with pre/post transformations

| Scenario | Recommendation | Rationale |
|----------|---------------|-----------|
| **FP32 Vectors** | Store as List<Float32> with PLAIN + ZSTD-3 | Maximum compatibility, good compression |
| **INT8/INT16 Quantized** | Store as List<Int8/Int16> with PLAIN + ZSTD-3 | Native Parquet support, fast |
| **Custom Bit-Width (4-6 bit)** | **Pack to INT8 with bytemuck** ✓ | Maximum compression, standard storage |
| **Sparse Vectors** | Transform to struct{indices, values} | Leverages Parquet's columnar format |
| **Metadata Columns** | Use dictionary encoding for strings | Built-in Parquet optimization |

**Key Trade-offs**:
- ✅ **Compatibility**: Works with all Parquet tools
- ✅ **Maintenance**: Leverages Arrow/Parquet improvements
- ✅ **Debugging**: Standard format, easy to inspect
- ❌ **Optimization Limits**: Cannot use custom encodings
- ❌ **Transformation Overhead**: Pre/post processing cost

### 12.2 SST Engine - REVISED APPROACH

**DECISION**: FP32-only with configurable ZSTD compression levels

| Configuration | Compression Level | Storage Reduction | Performance | Use Case |
|--------------|------------------|-------------------|-------------|----------|
| **Real-time** | ZSTD-1 | 15-25% | Write: 9.5K/s, Read: 50K/s | Hot data |
| **Balanced** | ZSTD-3 (default) | 20-40% | Write: 9K/s, Read: 49K/s | General |
| **Storage-optimized** | ZSTD-6 | 35-50% | Write: 7K/s, Read: 47K/s | Cold data |
| **Archive** | ZSTD-9 | 40-60% | Write: 5K/s, Read: 45K/s | Rarely accessed |
| **Sparse vectors** | Any + sparse | +20-30% bonus | Same | >70% zeros |

**Why NO Quantization in SST**:
- Row storage requires reading entire record
- Adding quantized field INCREASES I/O (not decreases)
- Quantization benefits only work with columnar storage

**Key Trade-offs**:
- ✅ **Maximum Compression**: Custom techniques for 50-70% reduction
- ✅ **Streaming Reads**: Optimized for sequential access
- ✅ **Selective Access**: Bloom filters prevent unnecessary decompression
- ❌ **Complexity**: Custom serialization to maintain
- ❌ **Write Performance**: 20-30% slower due to analysis

### 12.3 Decision Matrix

| Factor | VIPER Approach | SST Approach | Winner |
|--------|---------------|--------------|--------|
| **Compatibility** | Standard Parquet | Custom Format | VIPER |
| **Compression Ratio** | 30-50% | 50-70% | SST |
| **Write Performance** | Fast | 20-30% slower | VIPER |
| **Read Performance** | Standard | 5-10% faster | SST |
| **Maintenance** | Low | High | VIPER |
| **Flexibility** | Limited | High | SST |
| **Ecosystem Tools** | Full support | None | VIPER |
| **Memory Usage** | Standard | +240KB/block | VIPER |

### 12.4 Release 1.0 Configuration

```yaml
# ProximaDB Release 1.0 Compression Configuration
compression_strategy:
  defaults:
    algorithm: "zstd"
    level: 3  # Balanced for initial release
    mixed_compression_support: true  # Gradual migration
    
  viper:
    approach: "standard_with_transforms"
    compression:
      algorithm: "zstd"
      level: 3
    transforms:
      - pack_4_6_bit_to_int8    # Bytemuck packing
      - sparse_to_coo            # Before write
      - coo_to_dense            # After read
      - unpack_from_int8        # After read
      
  sst:
    approach: "type_aware_compression"
    compression:
      fp32_vectors:
        method: "zstd_3_lossless"
        expected_reduction: "10-30%"
        accuracy: "100%"
      int16_vectors:
        method: "adaptive_12bit"
        expected_reduction: "62.5%"
        accuracy: "preserved_within_quantization"
      int8_vectors:
        method: "adaptive_6bit"
        expected_reduction: "81%"
        accuracy: "preserved_within_quantization"
    expected_performance:
      write_throughput: "9K vec/s (-10%)"  # Better than original estimate
      read_throughput: "49K vec/s (-2%)"   # Better than original estimate
      storage_reduction: "10-81%"          # Depends on vector type
      
  python_sdk:
    smart_defaults:
      enabled: true
      rules:
        - dimension: [0, 512]
          engine: "sst"
          compression: "adaptive_8bit"
        - dimension: [513, 1024]
          engine: "viper"
          compression: "zstd_3"
        - dimension: [1025, 2048]
          engine: "viper"
          compression: "zstd_6"
        - sparse_threshold: 0.7
          compression: "coo_format"
```

### 12.5 Implementation Priority (Release 1.0)

1. **Phase 1 (Current)**: 
   - ✅ VIPER ZSTD-3 compression
   - 🔄 **SST Adaptive Precision** ← IN PROGRESS
   
2. **Phase 2 (Week 2)**: 
   - SST block headers v2 with adaptive precision metadata
   - Mixed compression reading support
   
3. **Phase 3 (Week 3)**: 
   - **VIPER 4-6 bit packing with bytemuck**
   - Pre/post transformation pipeline
   
4. **Phase 4 (Week 4)**: 
   - **Python SDK smart defaults**
   - Collection compression API
   
5. **Phase 5 (Post-Release)**: 
   - SST XOR delta (deferred)
   - Sparse format optimization (deferred)
   - Auto-tuning (deferred)

### 12.6 Risk Mitigation

| Risk | Mitigation Strategy |
|------|-------------------|
| **Compatibility Break** | Version headers, backward compatibility layer |
| **Performance Regression** | Adaptive strategy with fallback to simple compression |
| **Memory Overflow** | Bounded buffers, streaming architecture |
| **Accuracy Loss** | Configurable precision thresholds, validation tests |
| **Maintenance Burden** | Comprehensive tests, clear documentation |

## 13. SST Block Size Optimization

### 13.1 Vector Size Calculations
```
Per Vector Storage Requirements:
- Vector data: dimension × 4 bytes (FP32)
- Vector ID: ~50 bytes average
- Metadata: ~200-400 bytes average
- Total overhead: ~20-25% of vector size

Examples:
- 384D: 1.5KB vector + 0.4KB overhead = ~2KB/vector
- 768D: 3KB vector + 0.4KB overhead = ~3.5KB/vector
- 1536D: 6KB vector + 0.4KB overhead = ~6.5KB/vector
```

### 13.2 SST Storage Implementation
**Note**: SST uses sort-merge algorithms during flush/compaction, NOT BTreeMap (which is only for in-memory operations)

1. **Flush**: Memtable → Sorted disk blocks
2. **Compaction**: Multi-way merge sort of SSTable files
3. **Sorting**: ID-based primary, metadata similarity secondary
4. **Compression**: Applied to entire sorted blocks

### 13.3 Optimal Block Sizes by Dimension

| Dimension | Vector + Overhead | Block Size | Vectors/Block | Use Case |
|-----------|------------------|------------|---------------|----------|
| 128D | 0.9KB | 2MB | ~2,200 | Small embeddings |
| 384D | 1.9KB | 4MB | ~2,100 | text-embedding-3-small |
| 768D | 3.4KB | 8MB | ~2,350 | BERT-base |
| 1024D | 4.4KB | 8-12MB | ~2,000 | Large LMs |
| 1536D | 6.4KB | 12-16MB | ~2,000 | GPT-3 ada |
| 2048D | 8.4KB | 16MB | ~1,900 | Very large |
| 3072D | 12.4KB | 24-32MB | ~2,000 | Specialized |

**Target**: 2000-2500 vectors per block for optimal ZSTD compression

### 13.4 Dynamic Block Sizing Formula
```rust
fn calculate_optimal_block_size(dimension: usize) -> usize {
    let vector_size = dimension * 4 + 400; // FP32 + overhead
    let target_vectors = 2000;
    let block_size = vector_size * target_vectors;
    
    // Round to nearest MB, min 2MB, max 32MB
    let block_mb = (block_size / 1_000_000).max(2).min(32);
    block_mb * 1_000_000
}
```

### 13.5 Environment-Specific Tuning

| Environment | Block Size | Rationale |
|------------|------------|-----------|
| **Cloud (S3/GCS)** | 16-32MB | Reduce API calls |
| **Local NVMe** | 4-8MB | Cache optimization |
| **Memory-Constrained** | 1-2MB | Trade compression for memory |
| **Sparse Vectors** | 2x normal | Capture sparsity patterns |

## 14. Final Decision Summary

### Critical Insight
**Storage architecture determines quantization strategy:**

#### SST (Row Storage)
```
❌ NEVER use quantization - it INCREASES I/O:
[ID | FP32 Vector | Quantized Vector | Metadata] = 7.7KB per row
         6KB     +      1.5KB      +    200B
                  ↑
            Makes it WORSE, not better
```

**Strategy**:
- FP32 vectors ONLY
- Dynamic block sizing (4-16MB based on dimension)
- User-configurable ZSTD levels (1-9)
- Sort-merge algorithms for disk operations
- Metadata grouping for +10-15% compression

#### VIPER (Columnar Storage)
```
✅ ALWAYS use quantization - it REDUCES I/O by 24x:
├── fp32_column: [...]      ← Skip during search
├── int8_column: [...]      ← Read ONLY this
└── metadata: [...]         ← Skip if not filtering
```

**Strategy**:
- Store quantized vectors in separate columns
- Product Quantization for maximum compression
- Read only needed columns
- Optional FP32 for precision reranking

### Optimal Two-Stage Architecture
```yaml
# Stage 1: VIPER for fast candidate selection
viper_collection:
  vectors: int8_quantized      # 24x less I/O
  search: top_1000_candidates   # 100x faster
  
# Stage 2: SST for precision reranking
sst_collection:  
  vectors: fp32_only           # 100% accuracy
  compression: zstd_3          # 20-40% reduction
  block_size: dimension_aware  # 4-16MB
  search: rerank_top_10        # Final results
```

## Conclusion

This unified design provides:

1. **SST**: FP32-only with dynamic block sizing (20-60% compression)
2. **VIPER**: Quantization-first approach (24x I/O reduction)
3. **Block Optimization**: 2000-2500 vectors per block target
4. **Flexibility**: Per-collection compression configuration
5. **Two-Stage Search**: VIPER candidates → SST precision

The key insight is that row storage (SST) should NEVER use quantization while columnar storage (VIPER) should ALWAYS use quantization. Block sizes should be dynamically adjusted based on vector dimensions to maintain optimal compression ratios.

## 15. Enhancement Synergies

### 15.1 VIPER Vector Compression Enhancement Alignment

The proposed VIPER vector compression enhancement (`viper_vector_compression_enhancement.adoc`) offers strong synergies with our compression design:

#### Critical Clarification: Normalization ONLY for Quantized Copies

**IMPORTANT**: Normalization techniques (median, trimmed mean, etc.) apply ONLY to quantized vector copies, NEVER to original FP32 vectors.

```rust
// VIPER stores both original and quantized vectors
struct ViperStorage {
    // Original vectors - ALWAYS preserved with 100% fidelity
    fp32_vectors: Vec<Vec<f32>>,     // Lossless ZSTD only
    
    // Quantized copies - can use normalization
    quantized_vectors: Option<QuantizedVectors>,  // Lossy techniques allowed
}
```

#### Complementary Techniques (For Quantized Copies Only)

**1. Median Normalization + PQ Quantization**
```rust
// Applied ONLY to create quantized secondary index
// Original FP32 vectors remain untouched
impl ViperQuantization {
    fn create_quantized_index(&self, original_vectors: &[Vec<f32>]) -> QuantizedIndex {
        // Step 1: Clone vectors for quantization (originals preserved)
        let vectors_copy = original_vectors.to_vec();
        
        // Step 2: Apply normalization to COPY only
        let normalized = median_normalize(vectors_copy);
        
        // Step 3: Quantize the normalized copy
        let quantized = pq8_quantize(normalized);
        
        // Original vectors unchanged, quantized index created
        QuantizedIndex {
            quantized_data: quantized,
            normalization_params: params,
            original_vectors_preserved: true,  // Always true
        }
    }
}
```

**2. Trimmed Mean for Robustness (Quantized Only)**
- Enhancement proposes trimmed mean (5% trim) for outlier handling
- Applied ONLY when creating quantized representations
- Original FP32 vectors always stored without modification
- Benefits: Better compression of quantized index without touching originals

**3. Adaptive Central Tendency Selection (Quantized Only)**
- Enhancement's adaptive method selection based on data distribution
- Used ONLY for quantized vector generation
- Original vectors always use lossless ZSTD compression only

#### Implementation Synergy

```rust
// Unified VIPER compression pipeline
impl ViperCompressionPipeline {
    pub fn compress_vectors(&self, vectors: &[Vec<f32>]) -> CompressedBatch {
        // 1. Adaptive normalization from enhancement
        let (normalized, method) = self.adaptive_normalize(vectors);
        
        // 2. Our PQ quantization design
        let quantized = match self.config.quantization {
            QuantizationType::PQ8 => self.pq8_quantize(&normalized),
            QuantizationType::PQ4 => self.pq4_quantize(&normalized),
        };
        
        // 3. Apply ZSTD compression
        let compressed = zstd::compress(&quantized, self.config.zstd_level);
        
        CompressedBatch {
            data: compressed,
            normalization_params: method.params(),
            quantization_type: self.config.quantization,
        }
    }
}
```

#### Recommended Combined Approach

1. **Adopt Trimmed Mean Normalization**: More robust than simple mean, faster than full median
2. **Layer Normalization + Quantization**: Normalize first, then quantize for optimal compression
3. **Sidecar Metadata**: Enhancement's `.vmeta` files align with our metadata design
4. **Adaptive Selection**: Use enhancement's distribution analysis for automatic optimization

### 15.2 Refactoring Design Patterns Enhancement Alignment

The refactoring enhancement (`refactoring_design_patterns.adoc`) provides architectural patterns that strengthen our compression implementation:

#### Configuration System Integration

**Enhanced Configuration Builder (Priority 1)**
```rust
// Apply to compression configuration
pub struct CompressionConfigBuilder {
    environment: Environment,
    storage_engine: StorageEngineType,
    compression_profiles: HashMap<String, CompressionProfile>,
}

impl CompressionConfigBuilder {
    pub fn with_sst_compression(level: u8) -> Self
    pub fn with_viper_quantization(type: QuantizationType) -> Self
    pub fn validate_and_build(self) -> Result<ValidatedCompressionConfig>
}
```

Benefits:
- Type-safe compression configuration
- Environment-specific optimization profiles
- Compile-time validation of compression settings

#### Error Handling for Compression

**Unified Error System (Priority 2)**
```rust
#[derive(Error, Debug)]
pub enum CompressionError {
    #[error("Compression failed: {operation} - {context}")]
    CompressionFailed {
        operation: String,
        context: String,
        code: CompressionErrorCode,
        fallback_available: bool,
    },
    
    #[error("Decompression error: corrupted data in block {block_id}")]
    DecompressionFailed {
        block_id: String,
        recovery_strategy: RecoveryStrategy,
    },
}
```

Benefits:
- Clear error handling for compression failures
- Automatic fallback to uncompressed storage
- Better debugging of compression issues

#### Observability for Compression

**Compression Metrics (Priority 3)**
```rust
#[instrument(
    name = "compression.compress_block",
    fields(engine, method, input_size, output_size),
    metrics = ["compression_ratio", "compression_time_ms"]
)]
pub async fn compress_block(&self, block: DataBlock) -> Result<CompressedBlock> {
    // Automatic metrics collection for:
    // - Compression ratios per engine
    // - Performance impact tracking
    // - Adaptive threshold tuning
}
```

Benefits:
- Real-time compression effectiveness monitoring
- Performance regression detection
- Data-driven compression level tuning

#### Plugin Architecture for Compression

**Compression Plugin Interface (Priority 4)**
```rust
pub trait CompressionPlugin: ProximaPlugin {
    fn algorithm_name(&self) -> &str;
    fn compress(&self, data: &[u8], level: u8) -> Result<Vec<u8>>;
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>>;
    fn estimate_ratio(&self, sample: &[u8]) -> f32;
}

// Allows third-party compression algorithms
// Examples: LZ4, Snappy, Brotli plugins
```

Benefits:
- Extensible compression algorithm support
- A/B testing different compression methods
- Community-contributed optimizations

### 15.3 Combined Implementation Strategy

#### Phase 1: Foundation (Weeks 1-2)
1. Implement SST block compression with ZSTD
2. Add basic compression configuration
3. Create compression metrics collection

#### Phase 2: VIPER Enhancement (Weeks 3-4)
1. Integrate trimmed mean normalization from enhancement
2. Implement PQ quantization columns
3. Add adaptive method selection
4. Create `.vmeta` sidecar files

#### Phase 3: Architecture Integration (Weeks 5-6)
1. Apply configuration builder pattern
2. Implement comprehensive error handling
3. Add observability framework
4. Create compression plugin interface

#### Phase 4: Optimization (Weeks 7-8)
1. Performance baseline establishment
2. Compression ratio optimization
3. Query performance tuning
4. Documentation and testing

### 15.4 Expected Combined Benefits

**Compression Effectiveness**
- SST: 30-40% reduction (ZSTD alone)
- VIPER: 85-95% reduction (normalization + PQ8 + ZSTD)
- Overall: 70-80% storage reduction

**Performance Impact**
- Write: 5-10% overhead (amortized through batching)
- Read: <5% overhead (offset by reduced I/O)
- Query: 10-15% overhead for decompression (cached results)

**Operational Benefits**
- Automatic compression tuning through observability
- Clear error handling and recovery
- Extensible through plugin architecture
- Environment-specific optimization profiles

## 16. Comprehensive Trade-off Analysis

### 16.1 SST Engine Trade-offs

| Aspect | Choice | Trade-off | Rationale |
|--------|--------|-----------|-----------|
| **Quantization** | Never use | Accuracy vs Size | Row storage requires reading entire record anyway |
| **Block Size** | 8MB default | Memory vs Compression | Optimal for 768D vectors (~2350 vectors/block) |
| **ZSTD Level** | 3-6 default | Speed vs Ratio | Balanced for write-heavy workloads |
| **Metadata Sorting** | Always enabled | CPU vs Compression | +10-15% compression worth sorting cost |

**Decision Matrix:**
```yaml
sst_profiles:
  write_optimized:
    zstd_level: 1      # Fastest writes
    block_size: 4MB    # Lower memory usage
    trade_off: "30% less compression for 50% faster writes"
    
  balanced:
    zstd_level: 3      # Default
    block_size: 8MB    # Optimal
    trade_off: "Best overall performance"
    
  storage_optimized:
    zstd_level: 9      # Maximum compression
    block_size: 16MB   # Better compression
    trade_off: "40% better compression, 2x slower writes"
```

### 16.2 VIPER Engine Trade-offs

| Aspect | Choice | Trade-off | Rationale |
|--------|--------|-----------|-----------|
| **FP32 Column** | Always Present | Storage vs Fidelity | 100% fidelity non-negotiable |
| **FP32 Compression** | ZSTD Only | Limited compression | Lossless requirement |
| **Quantized Normalization** | Adaptive | Speed vs Compression | Applied to copies only |
| **Quantization** | PQ8 default | Accuracy vs Size | Secondary index for speed |
| **Dual Storage** | FP32 + Quantized | 2x storage | Flexibility worth the cost |

**Decision Matrix:**
```yaml
viper_profiles:
  fidelity_first:  # DEFAULT
    fp32_column: 
      compression: zstd_3       # Lossless only
      normalization: none       # FORBIDDEN for FP32
    quantized_column:
      normalization: adaptive   # Can use any technique
      quantization: pq8        # Good balance
    trade_off: "100% fidelity + fast search, 2x storage"
    
  storage_optimized:
    fp32_column:
      compression: zstd_9       # Maximum lossless
      normalization: none       # Still forbidden
    quantized_column:
      normalization: median     # Best compression
      quantization: pq4        # Aggressive
    trade_off: "100% fidelity maintained, 95% search accuracy"
    
  speed_optimized:
    fp32_column:
      compression: zstd_1       # Fast compression
      cache: true              # Keep in memory
    quantized_column:
      normalization: mean       # Fastest
      quantization: int8       # Simple
    trade_off: "100% fidelity + fastest search, more memory"
```

**Compression Gains with Normalization on Quantized Columns:**
| Quantization Type | Without Normalization | With Normalization | Additional Gain |
|-------------------|----------------------|-------------------|-----------------|
| INT8 | 4x reduction | 8-12x reduction | 2-3x |
| PQ8 | 8x reduction | 24-32x reduction | 3-4x |
| PQ4 | 16x reduction | 64-80x reduction | 4-5x |

**Key Insight**: Normalization techniques provide 2-5x additional compression when applied to quantized columns, making the dual-storage approach more viable.

### 16.3 Combined Strategy Trade-offs

#### Memory vs Compression
```
Small Blocks (2-4MB):
  ✅ Lower memory footprint
  ✅ Faster random access
  ❌ 10-20% worse compression
  
Large Blocks (8-16MB):
  ✅ 40-60% better compression
  ✅ Fewer metadata entries
  ❌ Higher memory requirements
  
Decision: Dynamic sizing based on available memory
```

#### Accuracy vs Speed
```
Full Precision Path:
  ✅ 100% accuracy guaranteed
  ❌ 24x more I/O required
  Use: Final reranking only
  
Quantized Path:
  ✅ 24x less I/O
  ✅ 100x faster search
  ❌ 1-5% accuracy loss
  Use: Candidate selection
  
Decision: Two-stage architecture maximizes both
```

#### Complexity vs Features
```
Simple Compression:
  ✅ Easy to implement
  ✅ Predictable behavior
  ❌ 30-40% compression only
  
Advanced Pipeline:
  ✅ 70-95% compression
  ✅ Adaptive optimization
  ❌ Complex implementation
  ❌ More failure modes
  
Decision: Phased rollout with fallbacks
```

### 16.4 Workload-Specific Recommendations

#### High-Throughput Ingestion
```yaml
configuration:
  sst:
    zstd_level: 1
    block_size: 4MB
    async_compression: true
  viper:
    normalization: mean
    quantization: pq8
    batch_size: 10000
expected:
  throughput: 100K vectors/sec
  compression: 60%
  accuracy: 99%
```

#### Storage-Constrained Deployment
```yaml
configuration:
  sst:
    zstd_level: 9
    block_size: 16MB
  viper:
    normalization: median
    quantization: pq4
    aggressive_pruning: true
expected:
  compression: 90-95%
  throughput: 10K vectors/sec
  accuracy: 97%
```

#### Latency-Critical Search
```yaml
configuration:
  sst:
    zstd_level: 3
    block_size: 8MB
    cache_decompressed: true
  viper:
    normalization: trimmed_mean
    quantization: pq8
    prefetch_columns: true
expected:
  p99_latency: <10ms
  compression: 70%
  accuracy: 99%
```

### 16.5 Risk Mitigation Strategies

| Risk | Mitigation | Fallback |
|------|------------|----------|
| **Compression Failure** | Validate before commit | Store uncompressed |
| **Corruption** | CRC32 checksums | Recovery from WAL |
| **Memory Pressure** | Dynamic block sizing | Smaller blocks |
| **CPU Overload** | Async compression | Lower compression level |
| **Accuracy Loss** | Monitor recall metrics | Disable quantization |

### 16.6 Decision Framework

```rust
fn select_compression_strategy(
    workload: &WorkloadProfile,
    resources: &SystemResources,
    requirements: &Requirements,
) -> CompressionStrategy {
    match (workload.write_rate, resources.memory, requirements.accuracy) {
        (High, _, _) => Strategy::FastWrites,
        (_, Low, _) => Strategy::SmallBlocks,
        (_, _, Critical) => Strategy::NoQuantization,
        (Low, High, Normal) => Strategy::MaxCompression,
        _ => Strategy::Balanced,
    }
}
```

## 17. Final Consolidated Design

### Architecture Overview

![Combined Compression Strategy](./diagrams/images/combined-compression-strategy.svg)

### Key Decisions (With 100% Fidelity Guarantee)

1. **100% Vector Fidelity**: Original FP32 vectors ALWAYS preserved bit-perfectly
2. **SST Never Quantizes**: Row storage makes quantization counterproductive
3. **VIPER Dual Storage**: Original FP32 (lossless) + Optional quantized (lossy) columns
4. **Normalization for Quantized Only**: Trimmed mean/median apply ONLY to quantized copies
5. **Dynamic Block Sizing**: Adapts to vector dimensions and memory
6. **Two-Stage Search**: VIPER quantized candidates → SST/VIPER FP32 precision
7. **Configuration Profiles**: Pre-defined for common workloads
8. **Observability First**: Metrics drive optimization decisions
9. **Plugin Architecture**: Extensible for future algorithms

### Storage Guarantees

```rust
// Every storage engine MUST implement this contract
trait VectorStorageGuarantee {
    /// Original vectors MUST be retrievable with 100% fidelity
    fn retrieve_original(&self, id: &str) -> Vec<f32>;
    
    /// Quantized vectors are optional secondary indices
    fn retrieve_quantized(&self, id: &str) -> Option<QuantizedVector>;
    
    /// Compression info must indicate if lossy techniques were used
    fn compression_info(&self) -> CompressionInfo {
        CompressionInfo {
            original_compression: "ZSTD (lossless)",
            quantized_compression: "PQ8 + Normalization (lossy)",
            fidelity_guarantee: true,  // Always true for originals
        }
    }
}
```

### Implementation Roadmap

**Phase 1 (Weeks 1-2): Foundation**
- SST block compression with ZSTD
- Basic configuration system
- Initial metrics collection

**Phase 2 (Weeks 3-4): VIPER Enhancement**
- Trimmed mean normalization integration
- PQ8/PQ4 quantization columns
- Sidecar metadata files

**Phase 3 (Weeks 5-6): Architecture**
- Configuration builder pattern
- Comprehensive error handling
- Observability framework
- Plugin interfaces

**Phase 4 (Weeks 7-8): Optimization**
- Performance baselines
- Adaptive tuning
- Documentation
- Testing suite

### Success Metrics

- **Storage Reduction**: 70-80% overall
- **Query Performance**: <10ms p99 latency
- **Accuracy**: 99%+ recall@10
- **Throughput**: 50K+ vectors/sec ingestion
- **Operational**: Zero compression-related incidents