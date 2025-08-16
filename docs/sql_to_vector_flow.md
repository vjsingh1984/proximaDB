# SQL to Vector Operations Flow

## Overview
SQL queries in ProximaDB are parsed, converted to vector operations, and then optimized using the same unified optimizer as REST/gRPC queries.

## Complete Flow

```
SQL Query
    ↓
SqlParser (parser.rs)
    ↓
ParsedQuery {
    - select_fields
    - from_collection  
    - where_conditions
    - order_by (VECTOR_SIMILARITY)
    - limit/offset
}
    ↓
SqlQueryPlanner (planner.rs)
    ↓
ExecutionPlan {
    - metadata_filter (FilterExpression)
    - vector_search (query_vector, metric, top_k)
    - select_fields
    - limit/offset
}
    ↓
VectorOperationsService::execute_sql_with_planner()
    ↓
SearchParams {
    - query_vectors
    - distance_metric
    - filter_expression
    - runtime_hints (None for SQL)
}
    ↓
VectorOperationsService::search_vectors()
    ↓
UnifiedSearchOptimizer
    ↓
Optimized Execution
```

## SQL Features Support

### 1. Vector Similarity Search
```sql
SELECT id, metadata.name, metadata.price
FROM products
WHERE metadata.category = 'electronics'
ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, ...], 'cosine')
LIMIT 10
```

**Parsed as:**
- `OrderType::VectorSimilarity { query_vector, metric }`
- Converted to `VectorSearchParams`
- Executes as vector search with filters

### 2. Metadata Filtering
```sql
SELECT *
FROM products
WHERE metadata.category = 'electronics'
  AND metadata.price BETWEEN 100 AND 1000
  AND metadata.brand IN ('Apple', 'Samsung')
```

**Converted to:**
- `FilterExpression::And` with nested conditions
- Supports: =, !=, <, >, <=, >=, BETWEEN, IN, AND, OR, NOT
- Applied during search via bloom filters and predicate pushdown

### 3. Distance Metrics Supported
- `'cosine'` → DistanceMetric::Cosine
- `'euclidean'` or `'l2'` → DistanceMetric::Euclidean
- `'dot_product'` or `'dot'` → DistanceMetric::DotProduct
- `'manhattan'` or `'l1'` → DistanceMetric::Manhattan
- `'hamming'` → DistanceMetric::Hamming
- `'jaccard'` → DistanceMetric::Jaccard

### 4. Complex Queries
```sql
SELECT id, metadata.name, metadata.score
FROM documents
WHERE metadata.status = 'published'
  AND metadata.score > 0.8
  AND (metadata.category = 'tech' OR metadata.category = 'science')
ORDER BY VECTOR_SIMILARITY(vector, [0.5, 0.3, ...], 'euclidean')
LIMIT 20
OFFSET 10
```

## Key Implementation Files

### SQL Engine Components
- `/src/query/sql_engine/parser.rs` - SQL parsing with VECTOR_SIMILARITY support
- `/src/query/sql_engine/planner.rs` - Converts ParsedQuery → ExecutionPlan
- `/src/query/sql_engine/executor.rs` - SQL execution logic

### Key Methods

#### SqlParser::parse()
- Parses SQL text into ParsedQuery
- Handles VECTOR_SIMILARITY in ORDER BY
- Parses vector literals `[0.1, 0.2, ...]`

#### SqlQueryPlanner::create_plan()
- Converts WHERE clause to FilterExpression
- Extracts vector search parameters
- Creates ExecutionPlan with all components

#### VectorOperationsService::execute_sql_with_planner()
- Uses SqlQueryPlanner to create plan
- Converts to SearchParams
- Routes to search_vectors() for optimization

## Optimization Benefits

1. **Consistent Optimization**: SQL queries get same optimization as REST/gRPC
2. **No Duplicate Logic**: Reuses existing SQL planner and parser
3. **Full Feature Support**: All SQL features already implemented
4. **Unified Path**: All queries flow through UnifiedSearchOptimizer

## Example Trace

With `RUST_LOG=trace`:
```
🔍 Executing SQL query for collection: products
📋 SQL execution plan created with vector search and 3 filters
🗺️ OPTIMIZATION_CONTEXT: collection=products, total_vectors=1000000
🎯 DECISION: Balanced → Using cost-based optimization
📊 COST_BASED: Large dataset + quantization → Progressive(3 stages)
✅ SQL query executed: 10 results in 45ms
```

## Future Enhancements

1. **VECTOR_SIMILARITY in WHERE**: Support similarity threshold filtering
   ```sql
   WHERE VECTOR_SIMILARITY(vector, [...], 'cosine') > 0.8
   ```

2. **Multiple Vector Columns**: Support searching different vector fields
   ```sql
   ORDER BY VECTOR_SIMILARITY(embedding, [...], 'cosine')
   ```

3. **KNN Join**: Join tables based on vector similarity
   ```sql
   SELECT * FROM products p
   JOIN reviews r ON VECTOR_KNN(p.vector, r.vector, 5)
   ```