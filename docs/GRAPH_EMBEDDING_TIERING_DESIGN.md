# Graph Engine Embedding Tiering Design

## Executive Summary

This document outlines the design for optimizing ProximaDB's graph engine by:

1. **Keeping CSR lean** - Pure graph topology only (no embeddings)
2. **Embeddings are OPTIONAL** - Pure graph workloads don't need them
3. **Cold tier uses existing vector engines** - SST/HELIX/VIPER for embedding storage
4. **Consumer configurable** - Override to keep embeddings in memory for SKS-heavy workloads

## Current State Analysis

### Benchmark Results Summary

| Operation | ProximaDB | NetworkX | igraph | Neo4j |
|-----------|-----------|----------|--------|-------|
| Bulk Insert | 3,170 ops/sec | 964K ops/sec | 2.2M ops/sec | 281 ops/sec |
| Node Lookup | 663K ops/sec | 1.95M ops/sec | 2.25M ops/sec | 279 ops/sec |
| Neighbor Query | 366K ops/sec | 2.07M ops/sec | 398K ops/sec | 226 ops/sec |

### Key Finding: Memory Footprint Problem

Current Node structure stores embeddings inline:

```proto
message Node {
  string id = 1;
  repeated string labels = 2;
  map<string, PropertyValue> properties = 3;
  optional EmbeddingVersion embedding = 4;  // <- THE PROBLEM
  int64 created_at_ms = 5;
  int64 updated_at_ms = 6;
}

message EmbeddingVersion {
  string model_id = 1;
  string model_version = 2;
  repeated float vector = 3;     // 128-1536 dims * 4 bytes = 512-6144 bytes
  uint32 dimension = 4;
  ...
}
```

**Memory Impact** (1M node graph with 128-dim embeddings):

| Component | Size per Node | Total (1M nodes) |
|-----------|---------------|------------------|
| CSR topology | ~20 bytes | ~20 MB |
| Node ID + Labels | ~50 bytes | ~50 MB |
| Properties (avg) | ~100 bytes | ~100 MB |
| Embeddings (128-dim) | ~562 bytes | ~562 MB |
| **Total** | ~732 bytes | **~732 MB** |

**Embeddings are 77% of memory footprint!**

For 1536-dim embeddings (OpenAI text-embedding-3):
- Embedding size: ~6,200 bytes/node
- Total for 1M nodes: **~6.2 GB** just for embeddings!

## Current Engine Architecture

### ORION (In-Memory CSR)
- Stores entire `Node` including embeddings in `GraphMemoryPool`
- CSR stores `edge_ids` as String references
- All data in hot memory

### QUASAR (Hot/Cold Tiering)
- Uses ORION as hot tier
- Tiers **entire nodes** to cold storage
- Doesn't separate embeddings from graph topology

### PULSAR (Distributed)
- Shards nodes across ORION instances
- Same memory footprint per shard

## Design Goals

1. **CSR stays lean**: Only topology data (node IDs, edge connections) in hot memory
2. **Embeddings in cold tier**: Loaded on-demand for semantic search
3. **Fast graph traversal**: Pure CSR operations stay O(degree) with minimal cache misses
4. **Backward compatible**: Existing API unchanged
5. **Configurable**: Users can choose hot-only mode for small graphs

## Proposed Solution: Layered Architecture

### Core Principle: Separation of Concerns

```
┌─────────────────────────────────────────────────────────────────┐
│                 GRAPH LAYER (CSR - Always Hot)                   │
│                                                                   │
│  Pure Graph Operations:                                          │
│  ├── Traversal (BFS, DFS, shortest path)                        │
│  ├── Neighbor lookup                                             │
│  ├── Pattern matching                                            │
│  └── Property filtering                                          │
│                                                                   │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │  CSR Storage (Lean - NO EMBEDDINGS)                        │  │
│  │  ├── offsets: Vec<usize>      (~8 bytes × nodes)           │  │
│  │  ├── targets: Vec<usize>      (~8 bytes × edges)           │  │
│  │  └── edge_ids: Vec<EdgeId>    (~24 bytes × edges)          │  │
│  └────────────────────────────────────────────────────────────┘  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │  Node Metadata (Hot - NO EMBEDDINGS)                       │  │
│  │  ├── id: String               (~24 bytes)                  │  │
│  │  ├── labels: Vec<String>      (~50 bytes avg)              │  │
│  │  ├── properties: HashMap      (~100 bytes avg)             │  │
│  │  └── vector_id: Option<String> (~24 bytes, POINTER ONLY)   │  │
│  └────────────────────────────────────────────────────────────┘  │
└───────────────────────────────┬─────────────────────────────────┘
                                │
                    Optional link (for SKS only)
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│              VECTOR LAYER (Cold Tier - Existing Engines)         │
│                                                                   │
│  Reuses existing vector storage engines:                         │
│  ├── SST Engine (write-optimized, real-time)                    │
│  ├── HELIX Engine (locality-optimized, Hilbert curve)           │
│  ├── VIPER Engine (columnar Parquet, analytics)                 │
│  └── etc.                                                        │
│                                                                   │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │  Embedding Collection: "graph_{graph_id}_embeddings"       │  │
│  │  ├── VectorRecord.id = node_id                             │  │
│  │  ├── VectorRecord.vector = embedding (128-1536 dims)       │  │
│  │  └── VectorRecord.metadata = {source: "graph_node"}        │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                   │
│  Consumer Override (embedding_mode = "memory"):                  │
│  └── Cache embeddings in memory for SKS-heavy workloads         │
└─────────────────────────────────────────────────────────────────┘
```

### Three Operating Modes

| Mode | Embeddings | Use Case | Memory |
|------|------------|----------|--------|
| `none` (DEFAULT) | Not stored | Pure graph workloads | Minimal |
| `cold` | In vector engine | SKS with large graphs | Graph only in RAM |
| `memory` | In memory | SKS-heavy, small graphs | Full (graph + embeddings) |

### New Data Structures

```rust
/// Graph embedding configuration
#[derive(Debug, Clone)]
pub struct GraphEmbeddingConfig {
    /// Embedding storage mode
    pub mode: EmbeddingMode,
    /// Vector engine for cold tier (if mode = Cold)
    pub vector_engine: Option<String>,  // "sst", "helix", "viper"
    /// Memory cache size for hot embeddings (if mode = Memory)
    pub memory_cache_mb: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum EmbeddingMode {
    /// No embeddings stored - pure graph workloads (DEFAULT)
    #[default]
    None,
    /// Embeddings in cold tier vector engine (SST/HELIX/VIPER)
    Cold,
    /// Embeddings cached in memory (consumer override for SKS-heavy)
    Memory,
}

/// Node in CSR - NEVER contains embedding data
/// Embeddings live in separate vector collection if needed
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub properties: HashMap<String, PropertyValue>,
    /// Optional pointer to vector in cold tier (NOT the embedding itself)
    pub vector_id: Option<String>,  // Points to VectorRecord in vector engine
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Graph node storage trait - abstracts embedding handling
pub trait GraphNodeStorage {
    /// Store node (embedding goes to vector engine if mode != None)
    async fn store_node(&self, node: Node) -> Result<Arc<GraphNode>>;

    /// Get node without embedding (fast, pure graph)
    fn get_node(&self, id: &NodeId) -> Result<Option<Arc<GraphNode>>>;

    /// Get node with embedding (fetches from vector engine if needed)
    async fn get_node_with_embedding(&self, id: &NodeId) -> Result<Option<Node>>;
}
```

### Memory Savings

| Component | Before | After | Savings |
|-----------|--------|-------|---------|
| Node (128-dim) | 732 bytes | 190 bytes | **74%** |
| Node (1536-dim) | 6,350 bytes | 190 bytes | **97%** |
| 1M nodes (128-dim) | 732 MB | 190 MB | **542 MB** |
| 1M nodes (1536-dim) | 6.35 GB | 190 MB | **6.16 GB** |

## Do Edges Need Embeddings?

**Answer: No, for most use cases.**

### Analysis

1. **Edges represent relationships**, not content:
   - "KNOWS", "WORKS_AT", "LOCATED_IN" are categorical
   - Edge semantics derived from connected node embeddings

2. **Semantic Knowledge Search (SKS)** workflow:
   - Start from semantically similar seed nodes (use node embeddings)
   - Traverse via edges (edge types + weights)
   - Filter by edge properties (no embeddings needed)
   - Score results by node semantic similarity

3. **Edge weight already captures similarity**:
   - `Edge.weight: Option<f64>` can store pre-computed semantic similarity
   - No need to store full embedding vector

4. **Exception: Edge Embedding Use Cases**:
   - Knowledge graph link prediction (rare)
   - Edge-centric semantic search (specialized)
   - Can be added as optional cold-tier extension

### Recommendation

- **Default**: Edges do NOT have embeddings
- **Optional**: Add `edge_embedding_store` for specialized use cases
- **SKS**: Use node embeddings + edge weights for semantic traversal

## Implementation Plan

### Phase 1: Embedding Store (Core)

```rust
// src/graph/engines/orion/embedding_store.rs

impl EmbeddingStore {
    /// Create or open embedding store
    pub fn open(base_path: &Path) -> Result<Self>;

    /// Store embedding for a node (returns reference)
    pub fn store(&self, node_id: &NodeId, embedding: &[f32]) -> Result<EmbeddingRef>;

    /// Fetch embedding for a node
    pub fn fetch(&self, node_id: &NodeId) -> Result<Option<Vec<f32>>>;

    /// Batch fetch for semantic operations
    pub fn fetch_batch(&self, node_ids: &[NodeId]) -> Result<Vec<Option<Vec<f32>>>>;

    /// Prefetch embeddings for anticipated access (async)
    pub async fn prefetch(&self, node_ids: &[NodeId]) -> Result<()>;
}
```

### Phase 2: LeanNode Integration

```rust
// Modify Node insertion path
impl OrionGraphEngine {
    async fn insert_node(&self, node: Node) -> Result<Arc<Node>> {
        // Extract embedding if present
        let embedding_ref = if let Some(embedding) = &node.embedding {
            Some(self.embedding_store.store(&node.id, &embedding.vector)?)
        } else {
            None
        };

        // Store lean node in hot tier (no embedding data)
        let lean_node = LeanNode::from_node(&node, embedding_ref);
        self.memory_pool.insert_lean_node(lean_node);

        // Return full node for API compatibility
        Ok(Arc::new(node))
    }
}
```

### Phase 3: Semantic Operations

```rust
// Semantic search with on-demand embedding fetch
impl OrionGraphEngine {
    async fn semantic_neighbors(
        &self,
        node_id: &NodeId,
        query_embedding: &[f32],
        top_k: usize,
        min_similarity: f32,
    ) -> Result<Vec<(Arc<Node>, f32)>> {
        // Get structural neighbors from CSR (fast)
        let neighbors = self.get_neighbors(node_id, None)?;

        // Batch fetch embeddings from cold tier
        let neighbor_ids: Vec<_> = neighbors.iter().map(|n| &n.id).collect();
        let embeddings = self.embedding_store.fetch_batch(&neighbor_ids)?;

        // Compute similarities (SIMD-accelerated)
        let mut scored: Vec<_> = neighbors.into_iter()
            .zip(embeddings)
            .filter_map(|(node, emb)| {
                emb.map(|e| {
                    let sim = cosine_similarity(query_embedding, &e);
                    (node, sim)
                })
            })
            .filter(|(_, sim)| *sim >= min_similarity)
            .collect();

        // Sort by similarity and return top_k
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        scored.truncate(top_k);
        Ok(scored)
    }
}
```

### Phase 4: Configuration

```toml
# config/config.toml
[graph]
# Embedding storage mode (default: "none" for pure graph)
# "none"   - No embeddings stored (pure graph, best performance)
# "cold"   - Embeddings in vector engine (SST/HELIX/VIPER)
# "memory" - Embeddings in memory (consumer override for SKS-heavy)
embedding_mode = "none"

# Vector engine for cold tier embeddings (only if embedding_mode = "cold")
embedding_engine = "sst"  # or "helix", "viper"

# Collection name pattern for embeddings
embedding_collection_pattern = "graph_{graph_id}_embeddings"

# Memory cache for SKS (only if embedding_mode = "memory")
embedding_memory_cache_mb = 512

# Prefetch strategy for SKS operations
# "none"      - No prefetching
# "neighbors" - Prefetch 1-hop neighbor embeddings
# "bfs_2"     - Prefetch 2-hop BFS embeddings
embedding_prefetch = "none"
```

### Example Configurations

```toml
# Pure Graph Workload (DEFAULT - Best Performance)
[graph]
embedding_mode = "none"
# No embedding-related settings needed

# SKS with Large Graph (1M+ nodes)
[graph]
embedding_mode = "cold"
embedding_engine = "sst"
embedding_prefetch = "neighbors"

# SKS-Heavy Small Graph (Consumer Override)
[graph]
embedding_mode = "memory"
embedding_memory_cache_mb = 1024
embedding_prefetch = "bfs_2"
```

## Performance Projections

### Read Operations (No Change)

| Operation | Before | After | Notes |
|-----------|--------|-------|-------|
| Node Lookup | 663K ops/sec | 663K ops/sec | No change (lean node) |
| Neighbor Query | 366K ops/sec | 366K ops/sec | CSR unchanged |
| Edge Traversal | 127K ops/sec | 127K ops/sec | Topology-only |

### Write Operations (Improved)

| Operation | Before | After | Improvement |
|-----------|--------|-------|-------------|
| Bulk Insert | 3,170 ops/sec | ~30K ops/sec | ~10x (less data to copy) |
| Node Insert | ~120K ops/sec | ~200K ops/sec | ~1.7x |

### Semantic Operations (New Overhead)

| Operation | Time | Notes |
|-----------|------|-------|
| Embedding fetch (1 node) | ~50μs | Memory-mapped read |
| Embedding fetch (100 batch) | ~200μs | Batched I/O |
| Semantic neighbor (k=10) | ~500μs | CSR + embedding fetch |

### Memory Usage

| Graph Size | Before | After | Savings |
|------------|--------|-------|---------|
| 100K nodes (128-dim) | 73 MB | 19 MB | 74% |
| 1M nodes (128-dim) | 732 MB | 190 MB | 74% |
| 1M nodes (1536-dim) | 6.35 GB | 190 MB | 97% |

## TDD Test Cases

### Unit Tests

```rust
#[cfg(test)]
mod embedding_store_tests {
    #[test]
    fn test_store_and_fetch_embedding() {
        let store = EmbeddingStore::open_temp().unwrap();
        let embedding = vec![0.1f32; 128];

        let ref_ = store.store("node1", &embedding).unwrap();
        let fetched = store.fetch("node1").unwrap().unwrap();

        assert_eq!(embedding, fetched);
    }

    #[test]
    fn test_batch_fetch_performance() {
        let store = EmbeddingStore::open_temp().unwrap();

        // Store 1000 embeddings
        for i in 0..1000 {
            store.store(&format!("node{}", i), &vec![0.1f32; 128]).unwrap();
        }

        // Batch fetch should be faster than individual fetches
        let ids: Vec<_> = (0..1000).map(|i| format!("node{}", i)).collect();
        let start = Instant::now();
        let _ = store.fetch_batch(&ids).unwrap();
        let batch_time = start.elapsed();

        assert!(batch_time.as_millis() < 10, "Batch fetch should be fast");
    }

    #[test]
    fn test_lean_node_memory_size() {
        let lean = LeanNode {
            id: "test".to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::new(),
            embedding_ref: Some(EmbeddingRef {
                offset: 0,
                dimension: 128,
                model_id: "test".to_string(),
            }),
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        let size = std::mem::size_of_val(&lean);
        assert!(size < 200, "LeanNode should be under 200 bytes");
    }
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_semantic_search_with_cold_embeddings() {
    let engine = OrionGraphEngine::with_cold_embeddings("/tmp/test").await.unwrap();

    // Insert nodes with embeddings
    for i in 0..1000 {
        let node = Node {
            id: format!("n{}", i),
            labels: vec!["Entity".to_string()],
            properties: HashMap::new(),
            embedding: Some(create_test_embedding(128)),
            ..Default::default()
        };
        engine.insert_node(node).await.unwrap();
    }

    // Verify CSR is lean (embeddings not in memory)
    let memory_usage = engine.hot_memory_usage();
    assert!(memory_usage < 500_000, "Hot tier should be under 500KB for 1K nodes");

    // Semantic search still works
    let query_emb = vec![0.5f32; 128];
    let results = engine.semantic_neighbors("n0", &query_emb, 10, 0.5).await.unwrap();
    assert!(!results.is_empty(), "Should find semantic neighbors");
}

#[tokio::test]
async fn test_graph_traversal_without_embedding_fetch() {
    let engine = OrionGraphEngine::with_cold_embeddings("/tmp/test").await.unwrap();

    // Create graph with embeddings
    // ... (setup code)

    // Pure graph traversal should NOT fetch embeddings
    let start = Instant::now();
    for _ in 0..10000 {
        let _ = engine.get_neighbors("n0", None).unwrap();
    }
    let elapsed = start.elapsed();

    // Should be < 100ms for 10K traversals (no cold tier access)
    assert!(elapsed.as_millis() < 100, "Graph traversal should be fast: {:?}", elapsed);
}
```

## Migration Path

### For Existing Users

1. **Default behavior unchanged**: `embedding_storage.mode = "inline"` is default
2. **Opt-in to cold tier**: Set `embedding_storage.mode = "cold"` in config
3. **Migration tool**: `proximadb migrate-embeddings --to-cold`

### API Compatibility

- `Node` proto unchanged
- `get_node()` returns full node with embedding (fetched from cold if needed)
- New API: `get_node_lean()` returns node without embedding (fast)

## Conclusion

By separating embeddings into a cold tier:

1. **CSR stays lean**: 74-97% memory reduction
2. **Graph ops stay fast**: No change to traversal performance
3. **Semantic search works**: On-demand embedding fetch
4. **Edges don't need embeddings**: Use node embeddings + edge weights
5. **Backward compatible**: Opt-in via configuration

This design enables ProximaDB to scale to billion-node graphs while maintaining competitive graph traversal performance.
