# ProximaDB Issues Tracking

*Last Updated: 2025-08-01*

This document tracks all issues, bugs, and gaps discovered during demo testing, along with detailed prompts for implementing fixes.

## 🔴 Critical Issues

### 1. SQL AND/OR Operators Not Implemented
**Status**: ✅ FIXED (2025-08-02)
**Issue**: SQL queries with AND/OR operators were returning "Complex conditions not supported yet"
**Location**: `/src/query/sql_engine/planner.rs` and `/src/query/sql_engine/parser.rs`
**Resolution**: 
- Implemented full support for AND/OR/NOT operators
- Added proper precedence handling and parentheses support
- All operators now convert to FilterExpression tree structure
- Comprehensive tests added in comprehensive_sql_tests.rs

### 2. SQL Comparison Operators Limited
**Status**: ✅ FIXED (2025-08-02)
**Issue**: Only equality (=) operator was working in SQL
**Location**: `/src/query/sql_engine/planner.rs`
**Resolution**:
- All comparison operators now implemented: <, >, <=, >=, !=
- Properly mapped to ComparisonOperator enum
- BETWEEN operator also implemented for range queries
- IN operator implemented for set membership
- Full test coverage in comprehensive tests

### 3. SQL LIKE Operator Not Supported
**Issue**: Pattern matching with LIKE returns "Operator Like not supported"
**Location**: `/src/query/sql_engine/planner.rs`
**Affected Engines**: Both VIPER and SST (SQL layer issue)

**Fix Prompt**:
```
Implement LIKE operator for SQL pattern matching:
1. Add LIKE case in parse_where_condition
2. Convert SQL LIKE patterns (%) to regex or contains logic
3. Map to appropriate FilterOperation (CONTAINS, STARTS_WITH, ENDS_WITH)
4. Test: WHERE product_name LIKE '%Pro%'
```

## 🟡 Major Issues

### 4. Metadata Filters Not Working Correctly
**Status**: ✅ FIXED (2025-08-01)
**Issue**: Metadata filters in REST/gRPC were returning incorrect results (wrong categories/brands)
**Symptoms**: 
- Filter for category='electronics' returned books
- Filter for brand='Apple' returned Samsung products
**Affected Engines**: VIPER (now fixed)
**Resolution**: 
- Implemented predicate pushdown optimization (60-90% I/O reduction)
- Fixed metadata extraction and comparison logic
- Centralized filter evaluation for consistency
**Note**: SST metadata filtering appears to work correctly

### 5. No Results Returned for Valid Filters
**Issue**: Many metadata filter queries return 0 results even when matching data exists
**Example**: IN operator, category filters often return empty
**Affected Engines**: Both VIPER and SST
**Note**: More prevalent in gRPC than REST

**Fix Prompt**:
```
Investigate why valid metadata filters return no results:
1. Add debug logging to filter evaluation
2. Check metadata serialization/deserialization
3. Verify filter field names match stored metadata keys
4. Test metadata storage and retrieval independently
5. Ensure proper type handling (string vs numeric comparisons)
```

### 6. SQL Subqueries Not Supported
**Issue**: Subqueries and UNION operations fail with parsing errors
**Error**: `Expected SELECT at position X`
**Affected Engines**: Both VIPER and SST (SQL parser issue)

**Fix Prompt**:
```
Add subquery support to SQL parser:
1. Extend parser to handle nested SELECT statements
2. Implement query composition for UNION/INTERSECT
3. Add execution planning for multi-stage queries
4. Test: SELECT * FROM collection WHERE id IN (SELECT id FROM...)
```

## 🟢 Minor Issues

### 7. SQL Query Caching Not Effective
**Issue**: Query caching shows minimal speedup (1.0x instead of expected 2-8x)
**Location**: `/src/query/sql_engine/query_cache.rs`
**Affected Engines**: Both VIPER and SST (SQL layer caching)

**Fix Prompt**:
```
Improve SQL query plan caching:
1. Verify cache key generation (should normalize query)
2. Check if cache is being properly hit/miss tracked
3. Ensure parsed queries are reused, not just stored
4. Add metrics for cache effectiveness
5. Test with identical queries in rapid succession
```

### 8. IN Operator Implementation Inconsistent
**Issue**: IN operator behavior varies between SQL and REST/gRPC APIs
**SQL**: Works partially
**REST/gRPC**: Unclear syntax, multiple formats attempted
**Affected Engines**: Both VIPER and SST
**Note**: Issue is in filter parsing, not engine-specific

**Fix Prompt**:
```
Standardize IN operator across all APIs:
1. Define canonical format: {"field": {"$in": [values]}} or {"field": [values]}
2. Update REST/gRPC handlers to parse IN correctly
3. Ensure SQL IN maps to same internal representation
4. Test: metadata filter with brand IN ('Apple', 'Samsung')
```

### 9. Multiple Vector Search Not Exposed
**Issue**: Proto supports batch search but not exposed in client APIs
**Location**: `VectorSearchRequest.queries` field unused
**Affected Engines**: Both VIPER and SST (API layer issue)

**Fix Prompt**:
```
Implement batch vector search in client APIs:
1. Update Python/REST/gRPC clients to accept multiple query vectors
2. Implement server-side batch processing
3. Return results grouped by query vector
4. Test: Search with 5 different query vectors in one request
```

## 📊 Performance Issues

### 10. Metadata Filter Performance
**Issue**: Filtered searches sometimes slower than unfiltered
**Expected**: Filters should reduce search space and improve performance
**Affected Engines**: 
- VIPER: Should be faster due to columnar storage
- SST: May have overhead due to row-based format
**Note**: Needs engine-specific optimization

**Fix Prompt**:
```
Optimize metadata filtering performance:
1. Profile filter evaluation overhead
2. Implement early termination for filtered searches
3. Use indexed metadata fields for fast filtering
4. Consider filter pushdown to storage layer
5. Benchmark: Compare filtered vs unfiltered search times
```

## 🔧 Implementation Gaps

### 11. Complex Filter Expressions
**Gap**: No support for nested filter expressions with mixed AND/OR
**Example**: (category='electronics' OR category='computers') AND brand='Apple'

**Implementation Prompt**:
```
Design and implement complex filter expression support:
1. Create filter expression AST structure
2. Implement expression evaluation engine
3. Support parentheses for grouping
4. Handle operator precedence correctly
5. Test complex nested expressions
```

### 12. Range Queries
**Gap**: No BETWEEN operator or efficient range query support
**Workaround**: Must use >= AND <= which may not optimize well

**Implementation Prompt**:
```
Add optimized range query support:
1. Implement BETWEEN operator in SQL parser
2. Create efficient range filter in storage engines
3. Optimize for common patterns (date ranges, price ranges)
4. Test: WHERE price BETWEEN 100 AND 500
```

### 13. Null Handling
**Gap**: IS NULL / IS NOT NULL operators not implemented
**Impact**: Cannot filter for missing metadata fields

**Implementation Prompt**:
```
Implement NULL handling in filters:
1. Add IS_NULL, IS_NOT_NULL to FilterOperation enum
2. Handle missing metadata fields correctly
3. Distinguish between null and empty string
4. Test: WHERE special_offer IS NOT NULL
```

## 🚀 Enhancement Opportunities

### 14. Aggregation Support
**Enhancement**: Add COUNT, AVG, MAX, MIN for analytics queries
**Use Case**: Analytics on vector search results

**Implementation Prompt**:
```
Add SQL aggregation functions:
1. Implement aggregation operators in query planner
2. Add result aggregation phase after search
3. Support GROUP BY for metadata fields
4. Test: SELECT category, COUNT(*), AVG(score) GROUP BY category
```

### 15. Multi-Stage Search
**Enhancement**: Support for search refinement and reranking
**Use Case**: Initial broad search followed by precise filtering

**Implementation Prompt**:
```
Implement multi-stage search pipeline:
1. Design pipeline stages (search -> filter -> rerank)
2. Allow different parameters per stage
3. Support result fusion from multiple searches
4. Test: Coarse search (k=1000) -> Fine filter -> Rerank (k=10)
```

## 📝 Documentation Issues

### 16. Metadata Filter Format Unclear
**Issue**: Multiple filter formats attempted, unclear which is correct
**Formats Tried**:
- `{"field": "value"}`
- `{"field": {"$eq": "value"}}`
- `{"operator": "AND", "conditions": [...]}`

**Documentation Prompt**:
```
Document official metadata filter format:
1. Create comprehensive filter syntax guide
2. Show examples for each operator
3. Clarify REST vs gRPC differences
4. Add to API documentation
```

## 🔄 Testing Recommendations

1. **Create Comprehensive Filter Test Suite**
   - Test each operator individually
   - Test operator combinations
   - Test edge cases (empty results, null values)
   - Test performance with large datasets

2. **SQL Compliance Tests**
   - Compare against SQL standard where applicable
   - Test common query patterns
   - Validate error messages

3. **Multi-API Consistency Tests**
   - Ensure same results from REST/gRPC/SQL
   - Test filter portability across APIs
   - Validate response formats

## 🔧 Engine-Specific Issues

### SST Engine Issues

#### SST-1: Empty Vector ID Handling
**Issue**: SST engine had issues with empty/null IDs for append-only vectors
**Status**: FIXED (2025-08-01)
**Fix Applied**: Use sequence numbers as keys for append-only vectors

#### SST-2: Compaction BTreeMap Memory Usage
**Issue**: BTreeMap in compaction holds all records in memory
**Status**: FIXED (2025-08-01)
**Fix Applied**: Replaced with streaming merge-sort approach

#### SST-3: Serialization Mismatch
**Issue**: Flush used bincode but compaction reader expected custom format
**Status**: FIXED (2025-08-01)
**Fix Applied**: Standardized on bincode for data blocks

### VIPER Engine Issues

#### VIPER-1: Metadata Filter Accuracy
**Issue**: VIPER returns incorrect results for metadata filters
**Status**: OPEN
**Symptoms**: Returns data that doesn't match filter criteria
**Theory**: Columnar metadata storage may have indexing issues

#### VIPER-2: Complex Filter Performance
**Issue**: Complex metadata filters don't leverage columnar advantages
**Status**: OPEN
**Expected**: VIPER should excel at metadata filtering due to columnar format

### Cross-Engine Issues

#### CROSS-1: Filter Result Consistency
**Issue**: Same filter returns different results on VIPER vs SST
**Status**: NEEDS INVESTIGATION
**Test**: Run identical metadata filters on both engines and compare

#### CROSS-2: Search Result Ordering
**Issue**: Result ordering may differ between engines for same query
**Status**: NEEDS INVESTIGATION
**Impact**: Affects reproducibility and testing

## Priority Matrix

| Priority | Issue | Impact | Effort | Engines |
|----------|-------|--------|--------|---------|
| P0 | SQL AND/OR operators | High | Medium | Both |
| P0 | Metadata filters incorrect results | High | High | VIPER |
| P1 | SQL comparison operators | High | Low | Both |
| P1 | No results for valid filters | High | Medium | Both |
| P2 | SQL LIKE operator | Medium | Low | Both |
| P2 | IN operator consistency | Medium | Medium | Both |
| P2 | VIPER metadata accuracy | High | High | VIPER |
| P3 | Query caching | Low | Medium | Both |
| P3 | Null handling | Low | Low | Both |
| P3 | Cross-engine consistency | Medium | Medium | Both |

## Testing Strategy by Engine

### VIPER Testing Focus
1. Metadata filter accuracy (primary issue)
2. Complex filter performance
3. Columnar query optimization
4. Analytics query patterns

### SST Testing Focus
1. Write-heavy workload performance
2. Simple filter efficiency
3. Point lookup optimization
4. Compaction impact on queries

### Cross-Engine Testing
1. Identical query result validation
2. Performance comparison benchmarks
3. Metadata consistency checks
4. Filter behavior differences

---

This tracking file should be updated as issues are resolved or new ones are discovered.