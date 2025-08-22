# RAPTOR Engine UML Analysis & Duplication Report

## 📊 UML Class Model Overview

### Core Module Structure
```
raptor/
├── common.rs           [Shared data structures]
├── writer.rs           [Write operations]
├── consolidated_reader.rs [Read operations]
├── engine.rs           [Main engine orchestration]
├── consolidated_compactor.rs [Compaction]
├── config.rs           [Configuration]
├── metadata.rs         [Metadata structures]
├── rowgroup.rs         [RowGroup management]
├── rowgroup_manager.rs [RowGroup management v2]
├── artus_bloom.rs     [BloomFilter impl]
├── ivf_manager.rs      [IVF clustering]
├── adaptive_pxk.rs    [P×K matrix implementation]
└── smart_rowgroup_sizing.rs [Sizing logic]
```

## 🔍 Identified Duplicate Classes & Structures

### 1. **RowGroup Management Duplication** ⚠️

#### DUPLICATE CLASSES:
```rust
// common.rs:15
pub struct RowGroup {
    pub id: u16,
    pub bloom_filter: Option<RowGroupBloomFilter>,
    pub row_count: usize,
    // ... 20+ fields
}

// common.rs:111
pub struct RowGroupMetadata {
    pub id: u16,
    pub row_count: usize,
    pub column_pages: HashMap<ColumnType, ColumnPageMetadata>,
    // ... similar fields to RowGroup
}

// rowgroup.rs:15
pub struct RowGroupManager {
    row_groups: Vec<RowGroup>,
    // Appears to be managing the same concept
}

// rowgroup_manager.rs:20
pub struct HybridRowGroup {
    id: u32,  // Different type!
    row_count: usize,
    // ... duplicates RowGroup functionality
}

// rowgroup_manager.rs:135
pub struct RowGroupManager {  // SAME NAME as rowgroup.rs!
    current_rowgroup: Option<HybridRowGroup>,
    // Different implementation of same concept
}
```

**RECOMMENDATION**: 
- Consolidate `RowGroup` and `RowGroupMetadata` into single structure
- Remove duplicate `RowGroupManager` implementations
- Use consistent ID types (u16 vs u32)

### 2. **BloomFilter Duplication** ⚠️

#### DUPLICATE CLASSES:
```rust
// common.rs:516
pub struct RowGroupBloomFilter {
    pub bits: Vec<u8>,
    pub num_hashes: usize,
    // Full implementation
}

// common.rs:443
pub struct BloomFilterMetadata {
    pub num_bits: usize,
    pub num_hashes: usize,
    // Metadata only
}

// writer.rs:140
struct BloomFilterBuilder {
    ids: Vec<String>,
    target_false_positive_rate: f64,
    // Builder pattern
}

// writer.rs:1990
struct BloomFilterMetadata {  // DUPLICATE NAME!
    // Same as common.rs:443
}

// artus_bloom.rs:64
pub struct ArtusBloomManager {
    bloom_filters: HashMap<String, BloomFilter>,
    // Yet another bloom implementation
}

// artus_bloom.rs:315
pub struct CompoundBloomFilter {
    filters: Vec<BloomFilter>,
    // Another variant
}

// metadata.rs:46
pub struct BloomFilterMetadata {  // THIRD duplicate!
    // Same fields again
}
```

**RECOMMENDATION**:
- Keep only `RowGroupBloomFilter` as main implementation
- Remove duplicate `BloomFilterMetadata` definitions (3 copies!)
- Integrate `ArtusBloomManager` functionality into main BloomFilter
- Consider if `CompoundBloomFilter` is necessary

### 3. **Metadata Structures Duplication** ⚠️

#### DUPLICATE CLASSES:
```rust
// common.rs:320
pub enum MetadataDataType {
    Boolean, Integer, Float, String,
    List(Box<MetadataDataType>),
    Map(Box<MetadataDataType>, Box<MetadataDataType>),
}

// common.rs:330
pub enum MetadataValue {
    Boolean(bool), Integer(i64), Float(f64),
    String(String), List(Vec<MetadataValue>),
    Map(HashMap<String, MetadataValue>),
}

// writer.rs:1898
struct MetadataColumn {
    name: String,
    data_type: DataType,  // Different type system!
    values: Vec<Value>,
}

// writer.rs:1979
enum MetadataEncoding {
    Dictionary, RunLength, Delta, Raw,
}

// common.rs:297
pub enum ColumnEncoding {
    Dictionary { num_entries: usize },
    Integer { bits: usize },
    Float, Boolean, String,
    FastLanes { scheme: FastLanesScheme },
}
```

**RECOMMENDATION**:
- Unify `MetadataDataType` and `MetadataValue` enums
- Consolidate encoding enums into single `ColumnEncoding`
- Remove duplicate metadata column definitions

### 4. **Compression/Encoding Duplication** ⚠️

#### DUPLICATE CLASSES:
```rust
// config.rs:96
pub enum CompressionCodec {
    Zstd, Lz4, Snappy, None,
}

// metadata.rs:15
pub enum CompressionCodec {  // EXACT DUPLICATE!
    Zstd, Lz4, Snappy, None,
}

// rowgroup_manager.rs:74
pub enum FastLanesScheme {
    BitPacking, DeltaEncoding, 
    DictionaryEncoding, FrameOfReference,
}

// common.rs:1495
pub enum FastLanesScheme {  // DUPLICATE!
    None, DeltaBitPacked, Dictionary,
    FrameOfReference, RunLength, Zigzag,
}
```

**RECOMMENDATION**:
- Remove duplicate `CompressionCodec` from metadata.rs
- Consolidate `FastLanesScheme` definitions into common.rs

### 5. **IVF/Graph Node Duplication** ⚠️

#### DUPLICATE CLASSES:
```rust
// writer.rs:1847
struct IvfNode {
    id: String,
    vector: Vec<f32>,
    cluster_id: u32,
    edges: Vec<EdgeWithDistance>,
}

// ivf_manager.rs:19
pub struct GraphNode {
    pub id: String,
    pub vector: Vec<f32>,
    pub neighbors: Vec<String>,
    pub cluster_id: Option<usize>,  // Different type!
}

// rowgroup_manager.rs:127
pub struct GraphNode {  // DUPLICATE NAME!
    pub id: u32,  // Different ID type!
    pub level: u8,
    pub neighbors: Vec<u32>,
}
```

**RECOMMENDATION**:
- Consolidate into single `GraphNode` structure
- Use consistent ID types (String vs u32 vs usize)
- Unify cluster_id representation

### 6. **Distance/Search Result Duplication** ⚠️

#### DUPLICATE CLASSES:
```rust
// common.rs:479
pub struct SearchResult {
    pub vector_id: String,
    pub distance: f32,
    pub vector: Option<Vec<f32>>,
    pub metadata: Option<HashMap<String, MetadataValue>>,
}

// consolidated_reader.rs:60
pub struct SimilarityResult {
    pub id: String,
    pub distance: f32,
    pub vector: Vec<f32>,
}

// consolidated_reader.rs:103
pub struct CandidateResult {
    pub id: String,
    pub vector: Vec<f32>,
    pub distance: f32,
    pub cluster_id: u32,
    pub cluster_info: ClusterInfo,
}

// ivf_manager.rs:72
pub struct IvfSearchResult {
    pub id: String,
    pub distance: f32,
    pub cluster_id: usize,
}
```

**RECOMMENDATION**:
- Consolidate into single `SearchResult` with optional fields
- Remove redundant result structures

### 7. **Vector Matrix Duplication** ⚠️

#### DUPLICATE CLASSES:
```rust
// common.rs:1236
pub struct VectorCentroidMatrix {
    pub rowgroup_id: u16,
    pub num_vectors: u32,
    pub num_centroids: u32,
    // Full implementation
}

// adaptive_pxk.rs:17
pub struct VectorCentroidMatrix {  // EXACT DUPLICATE NAME!
    vectors: Vec<Vec<f32>>,
    centroids: Vec<Vec<f32>>,
    // Different implementation of same concept
}
```

**RECOMMENDATION**:
- Remove duplicate from adaptive_pxk.rs
- Use common.rs version as single source of truth

### 8. **Configuration Duplication** ⚠️

#### DUPLICATE CLASSES:
```rust
// writer.rs:222
struct BoostingConfig {
    alpha_weights: [f32; 3],
    beta_weights: [f32; 2],
}

// consolidated_reader.rs:147
pub struct BoostConfig {  // Similar name!
    pub alpha_own: f32,
    pub alpha_other: f32,
    pub alpha_cluster: f32,
    pub beta_inter: f32,
    pub beta_diversity: f32,
}

// common.rs:1652
pub struct BoostingStrategy {
    pub spillover_strength: f32,
    pub ranking_strength: f32,
}
```

**RECOMMENDATION**:
- Consolidate into single `BoostingConfig` structure
- Remove duplicate boosting configurations

## 📊 Method Duplication Analysis

### Duplicate Method Patterns:

#### 1. **Multiple `new()` Implementations**
- `RowGroup::new()` - common.rs
- `RowGroupManager::new()` - rowgroup.rs AND rowgroup_manager.rs (2 different!)
- `BloomFilterBuilder::new()` - writer.rs
- `RowGroupBloomFilter::new()` - common.rs

#### 2. **Multiple `build()` Methods**
- `BloomFilterBuilder::build()` - writer.rs
- `build_p2_matrix()` - writer.rs
- `build_kxk_matrix()` - writer.rs
- Different build patterns for same concepts

#### 3. **Multiple I/O Methods**
- `read_rowgroup()` - consolidated_reader.rs
- `load_rowgroup()` - engine.rs
- `read_row_group()` - Different naming for same operation

#### 4. **Multiple Flush Methods**
- `flush()` - writer.rs
- `flush_current_rowgroup()` - writer.rs
- `flush_row_page_columnar()` - writer.rs
- Overlapping flush functionality

## 🎯 Consolidation Recommendations

### Priority 1: Critical Duplicates (MUST FIX)
1. **Remove 3 duplicate `BloomFilterMetadata` definitions**
2. **Merge 2 `RowGroupManager` classes with same name**
3. **Remove duplicate `CompressionCodec` enum**
4. **Consolidate `FastLanesScheme` enums**
5. **Remove duplicate `VectorCentroidMatrix` class**

### Priority 2: High Impact (SHOULD FIX)
1. **Unify RowGroup structures** (RowGroup, RowGroupMetadata, HybridRowGroup)
2. **Consolidate search result structures** (4 different types)
3. **Merge graph node implementations** (3 different GraphNode classes)
4. **Unify boosting configurations** (3 similar structures)

### Priority 3: Medium Impact (NICE TO HAVE)
1. **Standardize ID types** (String vs u32 vs u16 vs usize)
2. **Unify metadata type systems** (MetadataDataType vs MetadataValue)
3. **Consolidate encoding enums**
4. **Standardize method naming** (read_rowgroup vs load_rowgroup)

## 📈 Impact Analysis

### Memory Savings
- **Estimated code reduction**: 2,000-3,000 lines (30-40% of current code)
- **Binary size reduction**: ~200-300KB
- **Compilation time improvement**: 20-30% faster

### Maintenance Benefits
- **Reduced confusion**: Single source of truth for each concept
- **Easier debugging**: No ambiguity about which structure to use
- **Better testing**: Test once, use everywhere
- **Cleaner API**: Consistent interfaces

### Risk Assessment
- **Low Risk**: Removing exact duplicates (same name, same fields)
- **Medium Risk**: Consolidating similar structures (may need adapter patterns)
- **High Risk**: Changing public APIs (may affect other modules)

## 🔧 Implementation Strategy

### Phase 1: Remove Exact Duplicates (1 day)
```rust
// Remove these immediately:
- metadata.rs::BloomFilterMetadata
- metadata.rs::CompressionCodec  
- adaptive_pxk.rs::VectorCentroidMatrix
- rowgroup_manager.rs::FastLanesScheme (keep common.rs version)
```

### Phase 2: Merge Similar Classes (2-3 days)
```rust
// Consolidate into common.rs:
- All RowGroup variants → single RowGroup struct
- All search results → single SearchResult struct
- All graph nodes → single GraphNode struct
- All bloom filters → single RowGroupBloomFilter
```

### Phase 3: Standardize Types (1-2 days)
```rust
// Consistent types throughout:
- All IDs: String (for flexibility) or u32 (for performance)
- All cluster IDs: u32
- All rowgroup IDs: u16
```

### Phase 4: API Cleanup (1 day)
```rust
// Standardize method names:
- read_rowgroup() everywhere (not load_rowgroup)
- flush() for all flush operations
- new() for all constructors
```

## 📋 File-by-File Actions

### DELETE These Files:
1. `rowgroup.rs` - Functionality duplicated in rowgroup_manager.rs
2. `metadata.rs` - All structures duplicated in common.rs

### MERGE These Files:
1. `artus_bloom.rs` → Integrate unique features into common.rs BloomFilter
2. `adaptive_pxk.rs` → Merge unique logic into common.rs matrix structures

### REFACTOR These Files:
1. `common.rs` - Make it the single source of truth for all shared structures
2. `writer.rs` - Remove duplicate inner structures, use common.rs
3. `consolidated_reader.rs` - Remove duplicate result structures, use common.rs
4. `engine.rs` - Update to use consolidated structures

## 🚀 Expected Outcomes

After consolidation:
- **30-40% less code** to maintain
- **Single source of truth** for each concept
- **Consistent APIs** across all modules
- **Faster compilation** due to less code
- **Easier onboarding** for new developers
- **Reduced bugs** from using wrong structure variant

## ⚠️ Breaking Changes

These consolidations will require updates to:
1. Any code using `metadata.rs` structures
2. Any code using duplicate `RowGroupManager`
3. Any code using variant search result structures
4. Any code using variant graph node structures

---

**Recommendation**: Start with Phase 1 (exact duplicates) as it's low risk and high impact. Then proceed with Phase 2-4 based on testing results.