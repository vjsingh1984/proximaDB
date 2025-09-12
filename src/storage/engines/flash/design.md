# FLASH Engine - Implementation Design

## Executive Summary
FLASH (Fast Lookup And Search Hierarchy) is a memory-optimized storage engine designed for sub-millisecond vector similarity search on hot data.

## Architecture Overview

```rust
// Core structure fitting into existing ProximaDB architecture
pub struct FlashEngine {
    // Reuse existing components
    base_sst: SstStorage,                    // Persistent backing
    transaction_coordinator: Arc<TransactionCoordinator>,
    distance_compute: Arc<UnifiedDistanceCompute>,
    
    // New components
    memory_graph: Arc<RwLock<HnswIndex>>,    // In-memory HNSW
    vector_cache: Arc<DashMap<VectorId, CachedVector>>,
    prefetch_engine: PrefetchEngine,
    tier_manager: FlashTierManager,
}
```

## Integration with Existing Infrastructure

### 1. Reuse Existing Components
```rust
// Leverage existing HNSW implementation
use crate::index::hnsw::HnswIndex;
use crate::storage::engines::sst::SstStorage;
use crate::storage::transaction_coordinator::TransactionCoordinator;

impl FlashEngine {
    pub fn new(config: FlashConfig) -> Result<Self> {
        // Build on top of SST for persistence
        let sst = SstStorage::new(config.sst_config)?;
        
        // Use existing HNSW with modifications
        let graph = HnswIndex::new(HnswConfig {
            m: 16,              // Connections per node
            ef_construction: 200,
            max_elements: config.max_memory_vectors,
            distance_metric: config.distance_metric,
        });
        
        Ok(Self {
            base_sst: sst,
            memory_graph: Arc::new(RwLock::new(graph)),
            ..Default::default()
        })
    }
}
```

### 2. Implement UnifiedStorageEngine Trait
```rust
#[async_trait]
impl UnifiedStorageEngine for FlashEngine {
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        // Step 1: Add to in-memory graph
        let graph = self.memory_graph.write().await;
        for vector in &params.vector_records {
            graph.insert(vector.id, &vector.vector)?;
            self.vector_cache.insert(vector.id, CachedVector::from(vector));
        }
        
        // Step 2: Async persist to SST (non-blocking)
        let sst = self.base_sst.clone();
        let params = params.clone();
        tokio::spawn(async move {
            sst.do_flush(&params).await
        });
        
        Ok(FlushResult {
            success: true,
            entries_flushed: params.vector_records.len(),
            ..Default::default()
        })
    }
    
    async fn search_vectors_unified(&self, params: SearchParams) -> Result<Vec<SearchResult>> {
        // Use HNSW for fast navigation
        let graph = self.memory_graph.read().await;
        let candidates = graph.search(&params.query_vector, params.k * 2)?;
        
        // Rerank with exact distance
        let mut results = Vec::new();
        for candidate_id in candidates {
            if let Some(vector) = self.vector_cache.get(&candidate_id) {
                let distance = self.distance_compute.compute(
                    &params.query_vector,
                    &vector.data
                );
                results.push((distance, candidate_id));
            }
        }
        
        results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        results.truncate(params.k);
        
        // Convert to SearchResult
        Ok(results.into_iter().map(|(dist, id)| {
            SearchResult {
                id,
                score: dist,
                ..Default::default()
            }
        }).collect())
    }
}
```

## Memory Management Strategy

### Tiered Memory Architecture
```rust
struct MemoryTiers {
    // L1: Ultra-hot (HNSW graph nodes)
    l1_graph: Arc<HnswIndex>,         // ~100MB for 100K vectors
    
    // L2: Hot vectors (full precision)
    l2_vectors: Arc<DashMap<VectorId, Vec<f32>>>, // ~4GB for 1M vectors
    
    // L3: Warm vectors (quantized)
    l3_quantized: Arc<DashMap<VectorId, QuantizedVector>>, // ~500MB for 1M vectors
    
    // L4: Memory-mapped from SST
    l4_mmap: Arc<MmapRegion>,         // Virtual memory, OS managed
}

struct QuantizedVector {
    pq_codes: [u8; 32],    // Product quantization
    residual: Option<Box<[i8]>>, // Optional residual for accuracy
}
```

### Eviction Policy
```rust
impl FlashTierManager {
    async fn manage_memory(&self) {
        loop {
            let memory_usage = self.get_memory_usage();
            
            if memory_usage > self.config.max_memory {
                // Evict from L2 to L3 (quantize)
                let victims = self.select_eviction_candidates(100);
                for victim in victims {
                    let vector = self.l2_vectors.remove(&victim);
                    let quantized = self.quantize(vector);
                    self.l3_quantized.insert(victim, quantized);
                }
            }
            
            if memory_usage > self.config.critical_memory {
                // Evict from L3 to L4 (disk)
                let victims = self.select_cold_vectors(1000);
                for victim in victims {
                    self.l3_quantized.remove(&victim);
                    // Will be loaded from mmap on demand
                }
            }
            
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}
```

## Performance Optimizations

### 1. SIMD-Accelerated Distance Computation
```rust
// Reuse existing SIMD infrastructure
use crate::compute::distance_computation::platform::SimdEngine;

impl FlashEngine {
    fn batch_distance(&self, query: &[f32], vectors: &[Vec<f32>]) -> Vec<f32> {
        // Process 8 vectors at once with AVX-512
        let mut distances = Vec::with_capacity(vectors.len());
        
        for chunk in vectors.chunks(8) {
            let chunk_distances = self.simd_engine.compute_distances_x8(query, chunk);
            distances.extend_from_slice(&chunk_distances);
        }
        
        distances
    }
}
```

### 2. Prefetching Strategy
```rust
struct PrefetchEngine {
    access_pattern: AccessPatternTracker,
    prefetch_queue: Arc<SegQueue<VectorId>>,
}

impl PrefetchEngine {
    async fn prefetch_related(&self, accessed_id: VectorId) {
        // Find vectors frequently accessed together
        let related = self.access_pattern.get_correlated(accessed_id, 0.8);
        
        for related_id in related {
            if !self.vector_cache.contains_key(&related_id) {
                self.prefetch_queue.push(related_id);
            }
        }
    }
}
```

## Migration from/to Other Engines

### Automatic Promotion from SST
```rust
impl FlashEngine {
    async fn promote_from_sst(&self, vector_ids: Vec<VectorId>) -> Result<()> {
        // Batch read from SST
        let vectors = self.base_sst.get_vectors(vector_ids).await?;
        
        // Add to memory graph
        let mut graph = self.memory_graph.write().await;
        for vector in vectors {
            graph.insert(vector.id, &vector.data)?;
            self.vector_cache.insert(vector.id, CachedVector::from(vector));
        }
        
        Ok(())
    }
}
```

### Demotion to SST/VIPER
```rust
impl FlashEngine {
    async fn demote_cold_vectors(&self) -> Result<()> {
        let cold_vectors = self.identify_cold_vectors(
            Duration::from_hours(24)
        );
        
        // Remove from memory
        let mut graph = self.memory_graph.write().await;
        for id in cold_vectors {
            graph.remove(id)?;
            self.vector_cache.remove(&id);
        }
        
        // Already persisted in SST, nothing else needed
        Ok(())
    }
}
```

## Implementation Phases

### Phase 1: Basic In-Memory Index (Week 1-2)
- [ ] Create FlashEngine struct
- [ ] Integrate with existing HNSW
- [ ] Implement basic UnifiedStorageEngine trait
- [ ] Add to engine factory

### Phase 2: Memory Management (Week 3-4)
- [ ] Implement tiered memory architecture
- [ ] Add eviction policies
- [ ] Implement quantization for L3
- [ ] Add memory monitoring

### Phase 3: Performance Optimization (Week 5-6)
- [ ] Integrate SIMD acceleration
- [ ] Implement prefetching
- [ ] Add batch operations
- [ ] Optimize graph traversal

### Phase 4: Auto-Tiering (Week 7-8)
- [ ] Implement promotion logic
- [ ] Add demotion logic
- [ ] Integrate with TieringOrchestrator
- [ ] Add metrics and monitoring

## Configuration
```toml
[storage.flash]
enabled = true
max_memory_gb = 8
max_vectors = 1000000

[storage.flash.graph]
m = 16
ef_construction = 200
ef_search = 100

[storage.flash.tiering]
l1_size_mb = 100
l2_size_gb = 4
l3_size_mb = 500
eviction_batch_size = 100
eviction_interval_secs = 10

[storage.flash.promotion]
access_threshold = 10  # Accesses per hour
promotion_batch_size = 100
promotion_interval_secs = 60
```

## Expected Performance

| Metric | Current (SST) | FLASH | Improvement |
|--------|--------------|-------|-------------|
| Search Latency (p50) | 5ms | 0.5ms | 10x |
| Search Latency (p99) | 20ms | 2ms | 10x |
| QPS | 1,000 | 10,000 | 10x |
| Memory Usage | 1GB | 8GB | 8x |
| Write Latency | 10ms | 1ms | 10x |

## Risk Mitigation

1. **Memory Pressure**: Automatic eviction and quantization
2. **Cold Start**: Warm up from SST on startup
3. **Crash Recovery**: Periodic snapshots to SST
4. **Memory Leaks**: Bounded caches with TTL
5. **Graph Degradation**: Periodic rebalancing

## Testing Strategy

1. **Unit Tests**: Each component in isolation
2. **Integration Tests**: With SST backend
3. **Stress Tests**: Max memory scenarios
4. **Performance Tests**: Benchmark vs SST
5. **Migration Tests**: Promotion/demotion logic

## Conclusion

FLASH engine provides 10x performance improvement for hot data with manageable memory overhead. By reusing existing components (HNSW, SST, SIMD) and adding memory management layer, we can implement this in 8 weeks with minimal risk.