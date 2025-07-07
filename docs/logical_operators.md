# Logical Operators for Metadata Search in ProximaDB

## Overview

ProximaDB now supports advanced logical operators (AND, OR, NOT) for complex metadata filtering during vector search operations. This enables sophisticated business logic queries beyond simple equality matching.

## Features

### 1. Logical Operators
- **AND**: All conditions must be true
- **OR**: At least one condition must be true  
- **NOT**: Negates the condition

### 2. Comparison Operators
- **Equality**: `==`, `!=`
- **Numeric**: `>`, `>=`, `<`, `<=`
- **String**: `contains`, `startsWith`, `endsWith`
- **Array**: `in`, `notIn`
- **Existence**: `exists`, `notExists`
- **Regular Expression**: `regex`

### 3. Query Construction Methods

#### A. JSON-based Logical Queries
```json
{
  "and": [
    {"category": "electronics"},
    {"price": {"$lt": 500}}
  ]
}
```

#### B. gRPC Operator Fields
```
__and_1: {"category": "electronics"}
__and_2: {"price": {"$lt": 500}}
```

#### C. Programmatic Query Builder
```rust
MetadataQueryBuilder::new()
    .field_equals("category", json!("electronics"))
    .field_compare("price", ComparisonOperator::LessThan, json!(500.0))
    .build()
```

## Usage Examples

### Basic AND Query
Find electronics under $500:
```json
{
  "and": [
    {"category": "electronics"},
    {"price": {"$lt": 500}}
  ]
}
```

### Complex Business Logic
Premium products OR budget options:
```json
{
  "or": [
    {
      "and": [
        {"category": "electronics"},
        {"price": {"$gt": 500}},
        {"rating": {"$gte": 4.0}}
      ]
    },
    {
      "and": [
        {"price": {"$lt": 100}},
        {"in_stock": true}
      ]
    }
  ]
}
```

### NOT Operations
Exclude specific brands:
```json
{
  "not": {
    "or": [
      {"brand": "BadBrand"},
      {"warranty_years": {"$lt": 1}}
    ]
  }
}
```

### Range Queries
Price and rating ranges:
```rust
// Price range: 100 <= price <= 200
MetadataQuery::field_range("price", 100.0, 200.0)

// Rating >= 4.0
MetadataQuery::Field(FieldQuery {
    field: "rating".to_string(),
    operator: ComparisonOperator::GreaterThanOrEqual,
    value: json!(4.0),
})
```

### String Operations
Text searching:
```json
{
  "and": [
    {"description": {"$contains": "gaming"}},
    {"name": {"$startsWith": "Pro"}}
  ]
}
```

### Array Operations
Category filtering:
```json
{
  "category": {"$in": ["electronics", "computers", "gadgets"]}
}
```

## Integration Points

### 1. Multi-Tier Deduplication
The logical operators are integrated with ProximaDB's multi-tier search system:

```rust
// Create deduplicator with logical query
let query = MetadataQuery::And(vec![
    MetadataQuery::field_eq("category", json!("electronics")),
    MetadataQuery::Field(FieldQuery {
        field: "price".to_string(),
        operator: ComparisonOperator::LessThan,
        value: json!(300.0),
    }),
]);

let mut deduplicator = MultiTierDeduplicator::with_query(query);
```

### 2. gRPC Search Requests
Logical queries can be included in gRPC search requests via:

#### Special JSON Field
```
metadata_filter: {
  "__logical_query": "{\"and\": [{\"category\": \"electronics\"}, {\"price\": {\"$lt\": 500}}]}"
}
```

#### Operator Fields
```
metadata_filter: {
  "__and_1": "{\"category\": \"electronics\"}",
  "__and_2": "{\"price\": {\"$lt\": 500}}"
}
```

### 3. Backward Compatibility
Simple equality filters are still supported:
```
metadata_filter: {
  "category": "electronics",
  "brand": "TechCorp"
}
```

## Performance Considerations

### 1. Query Optimization
- Logical queries are evaluated efficiently using short-circuit evaluation
- AND operations stop at first false condition
- OR operations stop at first true condition

### 2. Regex Caching
- Regular expressions are compiled once and cached
- Reused across multiple evaluations for performance

### 3. Index Utilization
- Queries can leverage existing metadata indexes
- Field existence checks are optimized
- Numeric comparisons use efficient algorithms

## Error Handling

### 1. Type Safety
- Graceful handling of type mismatches
- Numeric operations fall back to string comparison
- Invalid regex patterns return clear error messages

### 2. Query Validation
- Malformed JSON queries are rejected with descriptive errors
- Unknown operators generate warnings
- Missing fields are handled according to operator semantics

## Testing

Comprehensive test suite covers:
- All logical operators (AND, OR, NOT)
- All comparison operators
- Complex nested queries
- Error conditions and edge cases
- Performance with large metadata sets
- Integration with search systems

Example test scenarios:
```rust
#[test]
fn test_complex_business_logic() {
    // E-commerce filtering: Premium OR Budget
    let query = MetadataQuery::Or(vec![
        // Premium: electronics + expensive + highly rated
        MetadataQuery::And(vec![
            MetadataQuery::field_eq("category", json!("electronics")),
            MetadataQuery::Field(FieldQuery {
                field: "price".to_string(),
                operator: ComparisonOperator::GreaterThan,
                value: json!(500.0),
            }),
            MetadataQuery::Field(FieldQuery {
                field: "rating".to_string(),
                operator: ComparisonOperator::GreaterThanOrEqual,
                value: json!(4.0),
            }),
        ]),
        // Budget: cheap + in stock
        MetadataQuery::And(vec![
            MetadataQuery::Field(FieldQuery {
                field: "price".to_string(),
                operator: ComparisonOperator::LessThan,
                value: json!(100.0),
            }),
            MetadataQuery::field_eq("in_stock", json!(true)),
        ]),
    ]);
}
```

## Migration Guide

### From Simple Filters
**Before:**
```rust
let filter = HashMap::new();
filter.insert("category".to_string(), "electronics".to_string());
let deduplicator = MultiTierDeduplicator::with_filters(filter);
```

**After:**
```rust
let query = MetadataQuery::field_eq("category", json!("electronics"));
let deduplicator = MultiTierDeduplicator::with_query(query);
```

### From Manual Logic
**Before:**
```rust
// Manual filtering in application code
let results = search_results.into_iter()
    .filter(|r| {
        r.metadata.get("category") == Some(&json!("electronics")) &&
        r.metadata.get("price").and_then(|p| p.as_f64())
            .map_or(false, |price| price < 500.0)
    })
    .collect();
```

**After:**
```rust
// Declarative query
let query = MetadataQuery::And(vec![
    MetadataQuery::field_eq("category", json!("electronics")),
    MetadataQuery::Field(FieldQuery {
        field: "price".to_string(),
        operator: ComparisonOperator::LessThan,
        value: json!(500.0),
    }),
]);
let deduplicator = MultiTierDeduplicator::with_query(query);
```

## Conclusion

The logical operators feature provides a powerful, flexible system for complex metadata filtering in ProximaDB. It supports:

- **Rich Query Language**: AND, OR, NOT with multiple comparison operators
- **Multiple Input Formats**: JSON, gRPC fields, and programmatic builders
- **High Performance**: Optimized evaluation with caching and short-circuiting
- **Backward Compatibility**: Existing simple filters continue to work
- **Type Safety**: Robust error handling and type coercion
- **Integration**: Seamless integration with existing search architecture

This enables sophisticated business logic queries while maintaining the performance and reliability of ProximaDB's vector search capabilities.