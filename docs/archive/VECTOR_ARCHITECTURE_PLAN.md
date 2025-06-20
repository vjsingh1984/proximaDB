# Vector Storage System Consolidation Plan

## Current Issues
- 21 files in `/storage/viper/` doing overlapping work
- Search operations scattered across 4+ locations  
- Indexing algorithms duplicated in multiple places
- No clear separation of concerns

## Target Architecture: Clean Separation with Design Patterns

### 1. Storage Layer (Heavy Data Operations)
```
/src/storage/
├── vector/                    # NEW: Consolidated vector operations
│   ├── engine.rs             # Main VectorStorageEngine  
│   ├── operations/           # CRUD operations
│   │   ├── insert.rs        # Vector insertion
│   │   ├── search.rs        # Vector search  
│   │   ├── delete.rs        # Vector deletion
│   │   └── update.rs        # Vector updates
│   ├── indexing/            # All indexing algorithms
│   │   ├── hnsw.rs          # HNSW implementation
│   │   ├── ivf.rs           # IVF implementation
│   │   ├── pq.rs            # Product Quantization
│   │   └── manager.rs       # Index management
│   └── storage/             # Storage backends
│       ├── lsm.rs           # LSM tree storage
│       ├── mmap.rs          # Memory-mapped storage
│       └── viper.rs         # VIPER storage (consolidated)
```

### 2. Compute Layer (Algorithms Only)
```
/src/compute/
├── distance.rs               # Distance metrics (cosine, euclidean, etc.)
├── algorithms.rs             # Core search algorithms  
├── hardware.rs               # Hardware optimizations (SIMD, GPU)
└── quantization.rs           # Vector quantization
```

### 3. Query Layer (High-level Interface)
```
/src/query/
├── vector_query.rs           # Query builder pattern
├── filters.rs                # Metadata filtering
└── results.rs                # Result processing
```

## Design Patterns to Apply

### 1. **Strategy Pattern** for Storage Backends
```rust
trait VectorStorage {
    async fn insert(&self, vectors: Vec<VectorRecord>) -> Result<()>;
    async fn search(&self, query: VectorQuery) -> Result<Vec<SearchResult>>;
    async fn delete(&self, ids: Vec<VectorId>) -> Result<()>;
}

struct VectorStorageEngine {
    storage_strategy: Box<dyn VectorStorage>,
    index_strategy: Box<dyn VectorIndex>, 
}
```

### 2. **Factory Pattern** for Index Selection
```rust
struct IndexFactory;
impl IndexFactory {
    fn create_index(algorithm: IndexAlgorithm, config: IndexConfig) -> Box<dyn VectorIndex> {
        match algorithm {
            IndexAlgorithm::HNSW => Box::new(HNSWIndex::new(config)),
            IndexAlgorithm::IVF => Box::new(IVFIndex::new(config)),
            IndexAlgorithm::PQ => Box::new(PQIndex::new(config)),
        }
    }
}
```

### 3. **Command Pattern** for Operations
```rust
trait VectorOperation {
    async fn execute(&self, engine: &VectorStorageEngine) -> Result<OperationResult>;
}

struct InsertOperation { vectors: Vec<VectorRecord> }
struct SearchOperation { query: VectorQuery }  
struct DeleteOperation { ids: Vec<VectorId> }
```

### 4. **Builder Pattern** for Queries
```rust
struct VectorQueryBuilder {
    query_vector: Option<Vec<f32>>,
    filters: Vec<MetadataFilter>,
    top_k: usize,
    distance_metric: DistanceMetric,
}

impl VectorQueryBuilder {
    fn with_vector(mut self, vector: Vec<f32>) -> Self { ... }
    fn with_filter(mut self, filter: MetadataFilter) -> Self { ... }
    fn top_k(mut self, k: usize) -> Self { ... }
    fn build(self) -> VectorQuery { ... }
}
```

## Migration Plan

### Phase 1: Consolidate VIPER (21 files → 4 files)
- Move `/storage/viper/*` → `/storage/vector/storage/viper.rs`
- Extract common patterns into traits
- Remove duplication

### Phase 2: Unify Search Operations
- Consolidate search implementations
- Single SearchEngine with pluggable algorithms

### Phase 3: Clean Index Management  
- Move all indexing to `/storage/vector/indexing/`
- Implement factory pattern for index selection
- Remove duplicate index implementations

### Phase 4: Command-based Operations
- Implement operation pattern for all vector operations
- Enable better testing and monitoring
- Clear separation of concerns