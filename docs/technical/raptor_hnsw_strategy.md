# RAPTOR HNSW Strategy: Embedded vs External Design Analysis

## Executive Summary

After thorough analysis of ProximaDB's architecture and RAPTOR's requirements, we recommend a **Hybrid Approach** that leverages the existing AXIS infrastructure for in-memory HNSW while adding storage-aware graph segments for cost optimization.

## Current State Analysis

### Existing AXIS HNSW Infrastructure
- **Memory-based**: AXIS maintains HNSW indexes entirely in memory
- **EventLog Integration**: Async indexing via EventLog pattern
- **Zero-overhead vectors**: Lightweight vector representation without metadata
- **Proven performance**: <10ms search latency at scale

### RAPTOR Requirements
- **Cloud-native**: Optimized for S3/GCS/Azure storage
- **Row-aligned**: 10K vector RowGroups
- **Cost-conscious**: Minimize memory usage for static data
- **Compaction-aware**: Handle updates during compaction

## Design Options Analysis

### Option 1: Fully Embedded HNSW (Not Recommended)
```
Pros:
- Data locality with vectors
- No external dependencies
- Compaction preserves graphs

Cons:
- Large file sizes (graph adds 30-50% overhead)
- Complex update logic during compaction
- Memory pressure when loading RowGroups
- Duplicates AXIS functionality
```

### Option 2: Pure External AXIS (Current Implementation)
```
Pros:
- Reuses proven infrastructure
- No duplication of effort
- Centralized index management

Cons:
- All graphs in memory (expensive)
- No storage-based fallback
- Limited by available RAM
```

### Option 3: Hybrid Storage-Aware HNSW (Recommended)

## Recommended Design: Hybrid Storage-Aware HNSW

### Architecture Overview

```
┌─────────────────────────────────────────────────────┐
│                   RAPTOR Engine                      │
├─────────────────────────────────────────────────────┤
│                                                      │
│  Hot Data (10%)          │  Warm/Cold Data (90%)    │
│  ┌─────────────────┐    │  ┌─────────────────────┐ │
│  │  AXIS HNSW      │    │  │  Storage Segments   │ │
│  │  (In-Memory)    │    │  │  (Lazy-Loaded)      │ │
│  └─────────────────┘    │  └─────────────────────┘ │
│         ↓               │           ↓               │
│  ┌─────────────────┐    │  ┌─────────────────────┐ │
│  │  EventLog       │    │  │  RowGroup Files     │ │
│  │  Integration    │    │  │  with Graph Hints   │ │
│  └─────────────────┘    │  └─────────────────────┘ │
└─────────────────────────────────────────────────────┘
```

### Implementation Strategy

#### 1. Graph Hints in RowGroups (Not Full Graphs)
Instead of embedding complete HNSW graphs, store lightweight "graph hints":

```rust
pub struct GraphHints {
    /// Entry points for this RowGroup (top-level nodes)
    entry_points: Vec<u32>,
    
    /// Connectivity summary (not full edges)
    connectivity_sketch: BloomFilter,
    
    /// Centroid for fast pruning
    centroid: Vec<f32>,
    
    /// Quality metric for this segment
    graph_quality: f32,
    
    /// Links to neighboring RowGroups
    neighbor_rowgroups: Vec<u32>,
}
```

#### 2. Tiered Graph Management

```rust
pub enum GraphTier {
    /// Fully loaded in AXIS (hot data)
    Memory { 
        axis_index_id: String 
    },
    
    /// Graph hints only (warm data)
    StorageHints { 
        hints: GraphHints,
        file_offset: u64,
    },
    
    /// No graph, pure scan (cold data)
    NoGraph,
}
```

#### 3. Adaptive Loading Strategy

```rust
impl RaptorHnswStrategy {
    /// Decide graph tier based on access patterns
    pub fn determine_tier(&self, rowgroup: &RowGroup) -> GraphTier {
        let access_freq = self.get_access_frequency(rowgroup.id);
        let age = self.get_age(rowgroup);
        let size = rowgroup.vector_count;
        
        // Hot data: Full AXIS HNSW
        if access_freq > 100 || age < Duration::hours(1) {
            return GraphTier::Memory { 
                axis_index_id: self.get_or_create_axis_index(rowgroup)
            };
        }
        
        // Warm data: Storage hints
        if access_freq > 10 || age < Duration::days(7) {
            return GraphTier::StorageHints {
                hints: self.generate_hints(rowgroup),
                file_offset: rowgroup.offset,
            };
        }
        
        // Cold data: No graph
        GraphTier::NoGraph
    }
}
```

#### 4. Compaction-Aware Updates

```rust
impl CompactionStrategy {
    pub fn compact_with_graphs(&mut self, rowgroups: Vec<RowGroup>) -> Result<RowGroup> {
        let mut merged = RowGroup::new();
        
        for rg in rowgroups {
            match self.get_graph_tier(&rg) {
                GraphTier::Memory { axis_index_id } => {
                    // Notify AXIS to update index
                    self.send_axis_update(axis_index_id, CompactionEvent::Merging);
                },
                GraphTier::StorageHints { hints, .. } => {
                    // Merge hints efficiently
                    merged.merge_hints(hints);
                },
                GraphTier::NoGraph => {
                    // Just merge vectors
                    merged.add_vectors(rg.vectors);
                }
            }
        }
        
        // Generate new hints for merged RowGroup
        merged.graph_hints = self.generate_merged_hints(&merged);
        
        Ok(merged)
    }
}
```

### Search Pipeline

```rust
pub async fn search_with_hybrid_hnsw(
    &self,
    query: &[f32],
    k: usize,
) -> Result<Vec<SearchResult>> {
    let mut results = Vec::new();
    
    // Phase 1: Search hot data via AXIS
    if let Some(axis_results) = self.search_axis_indexes(query, k).await? {
        results.extend(axis_results);
        
        if results.len() >= k {
            return Ok(results);
        }
    }
    
    // Phase 2: Use graph hints for warm data
    let candidate_rowgroups = self.select_rowgroups_via_hints(query);
    
    for rg_id in candidate_rowgroups {
        let rg_results = self.search_rowgroup_with_hints(rg_id, query, k).await?;
        results.extend(rg_results);
    }
    
    // Phase 3: Fallback to scan for cold data (if needed)
    if results.len() < k {
        let cold_results = self.scan_cold_rowgroups(query, k - results.len()).await?;
        results.extend(cold_results);
    }
    
    // Sort and return top-k
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    results.truncate(k);
    
    Ok(results)
}
```

### Benefits of Hybrid Approach

1. **Cost Optimization**: 90% reduction in memory usage vs pure AXIS
2. **Performance**: Hot data maintains <10ms latency
3. **Scalability**: Can handle datasets larger than available RAM
4. **Compatibility**: Leverages existing AXIS infrastructure
5. **Flexibility**: Adaptive to access patterns

### Implementation Phases

#### Phase 1: Graph Hints (2 weeks)
- Implement GraphHints structure
- Add hint generation during flush
- Integrate with RowGroup metadata

#### Phase 2: Tiered Management (1 week)
- Implement tier determination logic
- Add access frequency tracking
- Create promotion/demotion system

#### Phase 3: Compaction Integration (1 week)
- Update compaction to preserve hints
- Implement hint merging logic
- Add AXIS notification system

#### Phase 4: Search Pipeline (1 week)
- Implement hybrid search
- Add hint-based pruning
- Optimize fallback scanning

## Comparison with Pure Approaches

| Aspect | Fully Embedded | Pure External (AXIS) | Hybrid (Recommended) |
|--------|---------------|---------------------|---------------------|
| Memory Usage | High (all graphs) | High (all in AXIS) | Low (10% in memory) |
| Search Latency | 5-10ms | 5-10ms | 5-15ms (adaptive) |
| Storage Overhead | +30-50% | 0% | +2-5% (hints only) |
| Compaction Complexity | Very High | Low | Medium |
| Cost at Scale | $$$ | $$$ | $ |
| Implementation Time | 8-10 weeks | 1 week | 5 weeks |

## Recommendation

Implement the **Hybrid Storage-Aware HNSW** approach because:

1. **Cost Effective**: 90% memory reduction while maintaining performance
2. **Pragmatic**: Reuses AXIS for hot data, adds hints for warm/cold
3. **Scalable**: Can handle petabyte-scale datasets
4. **Compatible**: Works with existing EventLog and AXIS patterns
5. **Incremental**: Can be implemented in phases

## Alternative: Simplified Graph-Free Design

If timeline is critical, consider a simplified approach:
- Use AXIS for all graph operations
- Store only centroids and bloom filters in RowGroups
- Rely on RAPTOR's clustering for pruning
- Implement graph hints in Phase 2

This would reduce implementation to 2 weeks while still providing cost benefits.