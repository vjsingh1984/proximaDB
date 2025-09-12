# Search Optimization Debugging Guide

## Overview
ProximaDB provides detailed logging of search optimization decisions to help understand and debug query performance.

## Enabling Optimization Logs

### 1. Basic Optimization Summary (INFO level)
```bash
RUST_LOG=info ./target/release/proximadb-server
```

Shows high-level decisions:
```
🔍 Optimizing search for collection products with goal Balanced
🎯 OPTIMIZATION_SUMMARY for products: method=Progressive, access=Parallel, quant=true, compression=UseQuantizedColumns, est_latency=50ms, est_recall=0.98
```

### 2. Detailed Decision Logging (DEBUG level)
```bash
RUST_LOG=debug ./target/release/proximadb-server
```

Shows decision summaries and strategy selection.

### 3. Full Decision Tree (TRACE level)
```bash
RUST_LOG=trace ./target/release/proximadb-server
```

Shows complete decision rationale:
```
🗺️ OPTIMIZATION_CONTEXT: collection=products, total_vectors=1000000, available_files=5, query_dims=768
🔍 COLLECTION_ANALYSIS: quantization=true, compression=true, indexes=true
🎯 DECISION: Balanced → Using cost-based optimization
📊 COST_BASED: Large dataset (1000000 vectors) + quantization → Progressive(3 stages: 1000→100→10)
📈 DATA_ACCESS selected: Parallel { num_threads: 4 } (based on 1000000 vectors and goal Balanced)
🧮 QUANTIZATION configured: type=PQ8, two_stage=true, candidates=10, rerank_k=10
🗂️ COMPRESSION strategy: UseQuantizedColumns (has_comp=true, has_quant=true)
📡 PERFORMANCE_ESTIMATE: latency=50ms, memory=100MB, recall=0.98, confidence=0.85
```

## Understanding Optimization Decisions

### Optimization Goals
- **MaximizeRecall**: Always uses DirectFP32 for 100% accuracy
- **MaximizeSpeed**: Uses Binary quantization or indexes
- **MinimizeMemory**: Uses PQ4 quantization for maximum compression
- **MinimizeLatency**: Uses progressive search on large datasets
- **MaximizeThroughput**: Uses PQ8 without reranking
- **Balanced**: Cost-based decision using dataset size

### Cost-Based Decisions (Balanced mode)
```
Dataset Size        | Strategy
--------------------|---------------------------
< 10K vectors       | DirectFP32
10K - 100K vectors  | HNSW index or Progressive(2 stages)
100K - 1M vectors   | Progressive(3 stages) or IVF
> 1M vectors        | Aggressive Progressive or Hybrid
```

### Example: Debugging Slow Queries

1. Enable trace logging for specific module:
```bash
RUST_LOG=proximadb::query::unified_search_optimizer=trace ./target/release/proximadb-server
```

2. Look for decision points:
```
🎯 DECISION: MaximizeSpeed + quantization → Binary quantized-only
📊 COST_BASED: Large dataset (1000000 vectors) + quantization → Progressive(3 stages: 1000→100→10)
```

3. Check performance estimates vs actual:
```
📡 PERFORMANCE_ESTIMATE: latency=50ms, memory=100MB, recall=0.98, confidence=0.85
```

## Python Client Example

```python
from proximadb import ProximaDBClient
from proximadb.models import SearchHints, OptimizationGoal

client = ProximaDBClient()

# Search with optimization hints
hints = SearchHints(
    goal=OptimizationGoal.MINIMIZE_LATENCY,
    recall_threshold=0.95,
    latency_budget_ms=100
)

# The optimizer will log its decisions
results = client.search_vectors(
    collection_id="products",
    query_vector=[0.1] * 768,
    top_k=10,
    runtime_hints=hints
)
```

## Common Optimization Patterns

### 1. Small Dataset, High Accuracy
```
Collection: users (5000 vectors)
Goal: MaximizeRecall
Decision: DirectFP32
Reason: Small dataset allows full precision search
```

### 2. Large Dataset, Fast Search
```
Collection: products (1M vectors)
Goal: MaximizeSpeed
Decision: Binary quantized-only
Reason: Binary sketches provide fastest filtering
```

### 3. Large Dataset, Balanced
```
Collection: documents (500K vectors)
Goal: Balanced
Decision: Progressive(3 stages: 1000→100→10)
Reason: Multi-stage filtering balances speed and accuracy
```

## Troubleshooting

### Query is Slow
- Check if quantization is enabled for the collection
- Look for "no quantization" in logs - suggests DirectFP32 on large dataset
- Consider adding indexes or enabling quantization

### Low Recall
- Check optimization goal - MaximizeSpeed trades accuracy for speed
- Look for "Binary quantized-only" - lowest accuracy method
- Switch to Balanced or MaximizeRecall goal

### High Memory Usage
- Check for "DirectFP32" on large datasets
- Look at MEMORY_ESTIMATE in logs
- Use MinimizeMemory goal or enable quantization

## SQL Query Optimization

SQL queries automatically use the same optimizer:

```sql
-- This query will be optimized based on collection characteristics
SELECT id, metadata.name
FROM products
WHERE metadata.category = 'electronics'
ORDER BY VECTOR_SIMILARITY(vector, '[0.1, 0.2, ...]', 'cosine')
LIMIT 10
```

The optimizer will log:
- Collection analysis (quantization, compression, indexes)
- Selected execution method
- Data access strategy
- Performance estimates

All paths (REST, gRPC, SQL) use the same optimization logic for consistency.