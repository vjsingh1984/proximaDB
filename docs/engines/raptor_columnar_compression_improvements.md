# RAPTOR Columnar Compression Improvements

## Current Issues

### 1. Monolithic Rowgroup Compression
Currently, the entire rowgroup is compressed as a single block after columnar encoding:
```rust
// All fields compressed together
let compressed = self.compression.compress(&encoded_page, ...)
```

This prevents:
- Selective field reading
- Optimal compression per data type
- Lazy loading of expensive fields

### 2. Missing Column Page Architecture

Current `RowGroupMetadata` structure lacks column page offsets:
```rust
pub struct RowGroupMetadata {
    // Only has monolithic offsets
    pub offset: u64,
    pub compressed_size: u64,
    // Missing: per-column page offsets
}
```

## Proposed Improvements

### 1. Column Page Structure
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnPageMetadata {
    pub column_type: ColumnType,
    pub offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub compression: CompressionAlgorithm,
    pub encoding: FastLanesScheme,
    pub null_count: u32,
    pub min_value: Option<Vec<u8>>,
    pub max_value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnType {
    VectorsFp32,
    VectorsQuantized,
    Ids,
    Metadata(String), // Key name
    SourceContent,
    P2Matrix,
    PxKMatrix,
}
```

### 2. Enhanced RowGroupMetadata
```rust
pub struct RowGroupMetadata {
    pub id: u16,
    pub row_count: usize,
    
    // Column pages with individual compression
    pub column_pages: HashMap<ColumnType, ColumnPageMetadata>,
    
    // Quick lookups
    pub vector_page_offset: u64,
    pub id_page_offset: u64,
    pub metadata_pages: HashMap<String, u64>, // Key -> offset
    pub source_content_offset: Option<u64>,
    
    // Matrix data (already separate)
    pub p2_matrix_offset: Option<u64>,
    pub pxk_matrix_offset: Option<u64>,
    
    // Statistics for pruning
    pub bloom_filter_offset: Option<u64>,
    pub vector_stats: VectorStats,
    pub metadata_stats: HashMap<String, ColumnStats>,
}
```

### 3. Per-Column Compression Strategy

```rust
impl RaptorWriter {
    fn flush_row_page_columnar(&mut self) -> Result<()> {
        let page = self.current_row_page.take().unwrap();
        let mut column_pages = HashMap::new();
        
        // === 1. Compress ID column separately ===
        let id_data = self.encode_id_column(&page)?;
        let id_compressed = self.compression.compress(
            &id_data,
            CompressionAlgorithm::Zstd,  // Good for moderate entropy
            9,  // Higher compression for IDs
            CompressionContext::ColumnPage,
        )?;
        let id_offset = self.filesystem.append(&self.file_path, &id_compressed).await?;
        column_pages.insert(ColumnType::Ids, ColumnPageMetadata {
            column_type: ColumnType::Ids,
            offset: id_offset,
            compressed_size: id_compressed.len() as u64,
            compression: CompressionAlgorithm::Zstd,
            ..Default::default()
        });
        
        // === 2. Compress vectors with specialized compression ===
        let vector_data = self.encode_vector_column(&page)?;
        let vector_compressed = self.compression.compress(
            &vector_data,
            CompressionAlgorithm::Lz4,  // Fast decompression for hot path
            3,  // Lower compression, faster access
            CompressionContext::VectorColumn,
        )?;
        let vector_offset = self.filesystem.append(&self.file_path, &vector_compressed).await?;
        
        // === 3. Compress metadata columns individually ===
        let metadata_columns = self.group_metadata_by_key(&page)?;
        for (key, values) in metadata_columns {
            let meta_data = self.encode_metadata_column(&key, &values)?;
            
            // Choose compression based on cardinality
            let algorithm = if self.is_low_cardinality(&values) {
                CompressionAlgorithm::Zstd  // Better for dictionary-encoded
            } else {
                CompressionAlgorithm::Snappy  // Faster for high cardinality
            };
            
            let meta_compressed = self.compression.compress(
                &meta_data,
                algorithm,
                6,
                CompressionContext::MetadataColumn,
            )?;
            let meta_offset = self.filesystem.append(&self.file_path, &meta_compressed).await?;
            column_pages.insert(
                ColumnType::Metadata(key.clone()),
                ColumnPageMetadata { 
                    offset: meta_offset,
                    compressed_size: meta_compressed.len() as u64,
                    ..Default::default()
                }
            );
        }
        
        // === 4. Compress source content with maximum compression ===
        if self.has_source_content(&page) {
            let source_data = self.encode_source_content(&page)?;
            let source_compressed = self.compression.compress(
                &source_data,
                CompressionAlgorithm::Zstd,  // Best ratio for text
                19,  // Maximum compression
                CompressionContext::SourceContent,
            )?;
            let source_offset = self.filesystem.append(&self.file_path, &source_compressed).await?;
            column_pages.insert(ColumnType::SourceContent, ColumnPageMetadata {
                offset: source_offset,
                compressed_size: source_compressed.len() as u64,
                ..Default::default()
            });
        }
        
        // === 5. P² matrix (already separate) ===
        let p2_matrix = self.build_p2_matrix(&page_vectors)?;
        let p2_compressed = self.compression.compress(
            &bincode::serialize(&p2_matrix)?,
            CompressionAlgorithm::Lz4,  // Fast access for navigation
            6,
            CompressionContext::MatrixData,
        )?;
        
        // Update rowgroup metadata with column pages
        self.row_groups.last_mut().unwrap().column_pages = column_pages;
        
        Ok(())
    }
}
```

### 4. Selective Column Reading

```rust
impl RaptorReader {
    /// Read only specific columns from a rowgroup
    pub async fn read_columns(
        &self,
        file_path: &str,
        rg_id: u16,
        columns: &[ColumnType],
    ) -> Result<PartialRowGroup> {
        let metadata = self.read_metadata(file_path).await?;
        let rg_metadata = &metadata.row_groups[rg_id as usize];
        
        let mut partial = PartialRowGroup::new();
        
        for column_type in columns {
            if let Some(page_meta) = rg_metadata.column_pages.get(column_type) {
                // Read only this column page
                let compressed = self.filesystem.read_range(
                    file_path,
                    page_meta.offset,
                    page_meta.compressed_size,
                ).await?;
                
                // Decompress with appropriate algorithm
                let decompressed = self.compression.decompress(
                    &compressed,
                    page_meta.compression,
                )?;
                
                // Decode based on column type
                match column_type {
                    ColumnType::VectorsFp32 => {
                        partial.vectors = Some(self.decode_vector_column(&decompressed)?);
                    }
                    ColumnType::Ids => {
                        partial.ids = Some(self.decode_id_column(&decompressed)?);
                    }
                    ColumnType::Metadata(key) => {
                        partial.metadata.insert(
                            key.clone(),
                            self.decode_metadata_column(&decompressed)?
                        );
                    }
                    _ => {}
                }
            }
        }
        
        Ok(partial)
    }
    
    /// Example: Search without loading metadata or source content
    pub async fn search_vectors_only(
        &self,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<SearchResult>> {
        // Only load vectors and IDs, skip metadata/source
        let partial = self.read_columns(
            file_path,
            rg_id,
            &[ColumnType::VectorsFp32, ColumnType::Ids],
        ).await?;
        
        // Search using only loaded columns
        self.search_in_partial(&partial, query, k)
    }
}
```

## Benefits

### 1. **Optimal Compression Ratios**
- Vectors: LZ4 (fast decompression, 2-3x)
- IDs: ZSTD-9 (moderate compression, 5-8x)
- Metadata: Dictionary + ZSTD (10-20x for low cardinality)
- Source content: ZSTD-19 (maximum compression, 15-30x)
- P² matrix: FastLanes + LZ4 (10x with fast access)

### 2. **Selective Reading**
- Search: Read only vectors + IDs (skip metadata/source)
- Metadata filtering: Read only specific metadata columns
- RAG retrieval: Load source content only for final results

### 3. **Memory Efficiency**
- Load 100KB of vectors instead of 1MB rowgroup
- Lazy load source content (potentially MBs) only when needed
- Cache hot columns independently

### 4. **Performance Impact**
- **Search latency**: -30-40% (smaller reads)
- **Memory usage**: -50-70% (selective loading)
- **Storage size**: -20-30% (better compression)
- **Cache efficiency**: +100% (column-level caching)

## Implementation Plan

### Phase 1: Column Page Infrastructure (2 days)
- [ ] Add `ColumnPageMetadata` structure
- [ ] Update `RowGroupMetadata` with column pages
- [ ] Implement column page writer

### Phase 2: Per-Column Compression (3 days)
- [ ] Implement `flush_row_page_columnar()`
- [ ] Add compression strategy selection per column type
- [ ] Update encoding for each column type

### Phase 3: Selective Reading (2 days)
- [ ] Implement `read_columns()` in reader
- [ ] Add `PartialRowGroup` structure
- [ ] Update search to use selective loading

### Phase 4: Testing & Optimization (2 days)
- [ ] Benchmark compression ratios per column
- [ ] Test selective reading performance
- [ ] Validate lazy loading benefits

## Estimated Impact

For a typical 1024-vector rowgroup:
- **Current**: 1MB compressed block (all fields)
- **Proposed**:
  - Vectors: 400KB (LZ4)
  - IDs: 20KB (ZSTD-9)
  - Metadata: 50KB (Dictionary + ZSTD)
  - Source: 200KB (ZSTD-19, loaded lazily)
  - P² matrix: 50KB (FastLanes + LZ4)

**Search operation**: Load only 420KB (vectors + IDs) vs 1MB
**Memory saving**: 58%
**Latency improvement**: ~35%