# Search Optimization Consolidation

## Executive Summary
Consolidated all search optimization logic into a single UnifiedSearchOptimizer, eliminating code duplication and ensuring consistent optimization across SQL, REST, and gRPC query paths.

## Problems Solved

### 1. Code Duplication
**Before:**
- CompressionAwareQueryPlanner: Compression and quantization routing
- UnifiedQueryPlanner: Attempted consolidation but duplicated enums and logic
- RuntimeOptimizer: Another layer of optimization decisions
- Result: 3 different places making similar decisions

**After:**
- UnifiedSearchOptimizer: Single source of truth
- All query types (SQL, REST, gRPC) use the same optimizer
- No duplicate enums or decision logic

### 2. Memory Inefficiency
**Before:**
- Each component maintained its own collection cache
- With 1000+ collections: 4x memory usage (CollectionService + VectorOps + Optimizer + AXIS)

**After:**
- VectorOperationsService maintains single cache per node
- Other components use read-only references
- Memory usage: 1x + minimal Arc overhead

### 3. Inconsistent Optimization
**Before:**
- SQL queries might get different optimization than REST
- Runtime hints only worked in some code paths

**After:**
- All queries flow through same optimization logic
- Consistent behavior regardless of entry point

## Architecture

```
┌─────────────────────────────────────────────┐
│           Query Entry Points                 │
├────────────┬──────────────┬─────────────────┤
│    SQL     │     REST     │      gRPC       │
└─────┬──────┴──────┬───────┴─────────────────┘
      │             │                          
      └─────────────▼─────────────┐            
              VectorOperationsService           
                      │                         
         ┌────────────┼────────────┐           
         │                         │           
         ▼                         ▼           
  search_vectors()         Collection Cache    
         │                   (Single Source)    
         │                         │           
         ▼                         │           
  UnifiedSearchOptimizer ◄─────────┘           
         │                                     
         ▼                                     
  Optimized Execution Strategy                 
```

## Key Components

### UnifiedSearchOptimizer (`/src/query/unified_search_optimizer.rs`)
- Analyzes collection characteristics (quantization, compression, indexes)
- Selects execution method based on optimization goal
- Configures data access, parallelism, and resource limits
- Provides detailed logging at different levels

### VectorOperationsService Updates
- Maintains single collection cache
- Provides `get_cached_collection()` for fetching
- Shares cache handle with AXIS and optimizer
- SQL queries convert to vector operations then optimize

### Optimization Goals
1. **MaximizeRecall**: Always DirectFP32
2. **MaximizeSpeed**: Binary quantization or indexes
3. **MinimizeMemory**: PQ4 maximum compression
4. **MinimizeLatency**: Progressive search for large datasets
5. **MaximizeThroughput**: PQ8 without reranking
6. **Balanced**: Cost-based using dataset size

## Debug Logging

### INFO Level
```
🎯 OPTIMIZATION_SUMMARY for products: method=Progressive, access=Parallel, quant=true
```

### DEBUG Level
```
🎯 Optimization strategy: method=Progressive, access=Parallel, parallelism=4
```

### TRACE Level (Full Decision Tree)
```
🗺️ OPTIMIZATION_CONTEXT: collection=products, total_vectors=1000000
🔍 COLLECTION_ANALYSIS: quantization=true, compression=true, indexes=true
🎯 DECISION: Balanced → Using cost-based optimization
📊 COST_BASED: Large dataset (1000000 vectors) + quantization → Progressive(3 stages)
📈 DATA_ACCESS selected: Parallel { num_threads: 4 }
🧮 QUANTIZATION configured: type=PQ8, two_stage=true
🗂️ COMPRESSION strategy: UseQuantizedColumns
📡 PERFORMANCE_ESTIMATE: latency=50ms, memory=100MB, recall=0.98
```

## Usage Example

```rust
// All paths use same optimizer
// REST/gRPC
let results = vector_ops_service.search_vectors(
    collection_id,
    query_vector,
    k,
    distance_metric,
    Some(&search_params),  // Can include runtime_hints
    include_vectors,
    include_metadata,
).await?;

// SQL automatically converts to vector operation
let results = vector_ops_service.execute_sql_with_planner(
    &parsed_query,
    collection_id,
).await?;
```

## Benefits

1. **Maintainability**: Single place to update optimization logic
2. **Consistency**: All query types get same optimization
3. **Memory Efficiency**: ~75% reduction in collection metadata memory
4. **Debuggability**: Clear logging shows why decisions were made
5. **Scalability**: Each node manages its own cache efficiently
6. **Flexibility**: Easy to add new optimization strategies

## Future Enhancements

1. **Learning Optimizer**: Track actual vs estimated performance
2. **Adaptive Strategies**: Adjust based on historical performance
3. **Query Plan Cache**: Cache optimization decisions for repeated queries
4. **Distributed Cache Sync**: Optional Redis/Hazelcast for cross-node consistency
5. **Cost Model Tuning**: Adjust thresholds based on hardware capabilities