//! Examples of using the Unified Filter Evaluator across storage engines
//!
//! This file demonstrates how all storage engines can leverage the unified
//! filter evaluator for consistent filtering behavior.

use proximadb_filter_expression::{ComparisonOperator, FilterExpression};
use crate::storage::engines::core::filter_evaluator::*;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Example 1: Basic usage in any storage engine
pub fn basic_filter_example() {
    // Create a filter expression
    let expr = FilterExpression::Comparison {
        field: "status".to_string(),
        operator: ComparisonOperator::Equals,
        value: json!("active"),
    };

    // Method 1: Direct evaluation with JSON metadata
    let mut metadata = HashMap::new();
    metadata.insert("status".to_string(), json!("active"));
    metadata.insert("score".to_string(), json!(85));

    let result = evaluate_filter(&expr, &metadata);
    println!("Filter matched: {}", result);

    // Method 2: Using thread-safe evaluator for parallel operations
    let evaluator = UnifiedFilterEvaluator::new(Some(&expr))
        .unwrap()
        .unwrap();
    
    // Can be shared across threads
    let closure = evaluator.as_json_closure();
    let result = closure(&metadata);
    println!("Thread-safe filter matched: {}", result);
}

/// Example 2: SST Engine - Row-based filtering
pub fn sst_engine_example() {
    // SST uses JSON metadata directly
    let filter_expr = FilterExpression::And(vec![
        FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::In,
            value: json!(["electronics", "books"]),
        },
        FilterExpression::Comparison {
            field: "price".to_string(),
            operator: ComparisonOperator::Between,
            value: json!([10.0, 100.0]),
        },
    ]);

    // SST converts proto metadata to JSON and evaluates
    let mut metadata = HashMap::new();
    metadata.insert("category".to_string(), json!("electronics"));
    metadata.insert("price".to_string(), json!(49.99));

    // Direct evaluation
    let matches = crate::storage::engines::core::evaluate_filter(&filter_expr, &metadata);
    println!("SST row matches filter: {}", matches);
}

/// Example 3: VIPER Engine - Columnar filtering
pub fn viper_engine_example() {
    // VIPER works with columnar data but uses same filter evaluation
    let filter_expr = FilterExpression::Comparison {
        field: "timestamp".to_string(),
        operator: ComparisonOperator::GreaterThan,
        value: json!("2024-01-01"),
    };

    // VIPER extracts column values and evaluates
    let mut metadata = HashMap::new();
    metadata.insert("timestamp".to_string(), json!("2024-06-15"));

    let matches = crate::storage::engines::core::evaluate_filter(&filter_expr, &metadata);
    println!("VIPER column matches filter: {}", matches);
}

/// Example 4: HELIX Engine - Thread-safe parallel filtering
pub fn helix_engine_example() {
    // HELIX needs thread-safe filters for parallel SSTable search
    let filter_expr = FilterExpression::Comparison {
        field: "region".to_string(),
        operator: ComparisonOperator::Like,
        value: json!("us-%"),
    };

    // Create thread-safe filter for parallel operations
    let filter_fn = crate::storage::engines::core::create_filter_fn(Some(&filter_expr));
    
    if let Some(filter) = filter_fn {
        // This closure can be shared across tokio tasks
        let mut metadata = HashMap::new();
        metadata.insert("region".to_string(), "us-west-2".to_string());
        
        let matches = filter(&metadata);
        println!("HELIX parallel filter matches: {}", matches);
    }
}

/// Example 5: RAPTOR Engine - Using filter with HNSW traversal
pub fn raptor_engine_example() {
    // RAPTOR can use filters during HNSW graph traversal
    let filter_expr = FilterExpression::Not(Box::new(
        FilterExpression::Comparison {
            field: "deleted".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!(true),
        }
    ));

    // Create evaluator for use during graph search
    let evaluator = UnifiedFilterEvaluator::new(Some(&filter_expr))
        .unwrap()
        .unwrap();

    // During HNSW traversal, check each node
    let mut node_metadata = HashMap::new();
    node_metadata.insert("deleted".to_string(), json!(false));
    node_metadata.insert("vector_id".to_string(), json!("vec_123"));

    let should_visit = evaluator.evaluate(&node_metadata);
    println!("RAPTOR should visit node: {}", should_visit);
}

/// Example 6: NOVA Engine - Hybrid filtering with quantized data
pub fn nova_engine_example() {
    // NOVA can filter both raw and quantized vectors
    let filter_expr = FilterExpression::Comparison {
        field: "quantization_level".to_string(),
        operator: ComparisonOperator::In,
        value: json!(["INT8", "PQ4", "PQ8"]),
    };

    // Check if vector should be included in search
    let mut metadata = HashMap::new();
    metadata.insert("quantization_level".to_string(), json!("INT8"));
    metadata.insert("dimensions".to_string(), json!(768));

    let matches = crate::storage::engines::core::evaluate_filter(&filter_expr, &metadata);
    println!("NOVA quantized vector matches: {}", matches);
}

/// Example 7: SWIFT Engine - Fast filtering with hierarchical blocks
pub fn swift_engine_example() {
    // SWIFT uses filters at superblock level for early pruning
    let filter_expr = FilterExpression::Comparison {
        field: "block_level".to_string(),
        operator: ComparisonOperator::LessThanOrEqual,
        value: json!(2),
    };

    // Check superblock metadata
    let mut block_metadata = HashMap::new();
    block_metadata.insert("block_level".to_string(), json!(1));
    block_metadata.insert("num_vectors".to_string(), json!(1000));

    let should_scan = crate::storage::engines::core::evaluate_filter(&filter_expr, &block_metadata);
    println!("SWIFT should scan block: {}", should_scan);
}

/// Example 8: PRISM Engine - Tree-based filtering
pub fn prism_engine_example() {
    // PRISM filters during tree traversal
    let filter_expr = FilterExpression::And(vec![
        FilterExpression::Comparison {
            field: "node_type".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("leaf"),
        },
        FilterExpression::Comparison {
            field: "min_distance".to_string(),
            operator: ComparisonOperator::LessThan,
            value: json!(0.5),
        },
    ]);

    // Evaluate tree node
    let mut node_metadata = HashMap::new();
    node_metadata.insert("node_type".to_string(), json!("leaf"));
    node_metadata.insert("min_distance".to_string(), json!(0.3));

    let should_explore = crate::storage::engines::core::evaluate_filter(&filter_expr, &node_metadata);
    println!("PRISM should explore node: {}", should_explore);
}

/// Example 9: Complex filter with all operators
pub fn complex_filter_example() {
    let complex_filter = FilterExpression::Or(vec![
        FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "category".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("premium"),
            },
            FilterExpression::Comparison {
                field: "score".to_string(),
                operator: ComparisonOperator::GreaterThanOrEqual,
                value: json!(90),
            },
        ]),
        FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "tags".to_string(),
                operator: ComparisonOperator::Contains,
                value: json!("featured"),
            },
            FilterExpression::Not(Box::new(
                FilterExpression::Comparison {
                    field: "status".to_string(),
                    operator: ComparisonOperator::Equals,
                    value: json!("archived"),
                }
            )),
        ]),
    ]);

    // Test with various metadata
    let mut metadata1 = HashMap::new();
    metadata1.insert("category".to_string(), json!("premium"));
    metadata1.insert("score".to_string(), json!(95));
    
    let mut metadata2 = HashMap::new();
    metadata2.insert("tags".to_string(), json!("new,featured,hot"));
    metadata2.insert("status".to_string(), json!("active"));

    println!("Complex filter test 1: {}", 
        crate::storage::engines::core::evaluate_filter(&complex_filter, &metadata1));
    println!("Complex filter test 2: {}", 
        crate::storage::engines::core::evaluate_filter(&complex_filter, &metadata2));
}

/// Example 10: Performance optimization with cached evaluator
pub fn performance_example() {
    // For repeated evaluations, create evaluator once
    let filter_expr = FilterExpression::Comparison {
        field: "active".to_string(),
        operator: ComparisonOperator::Equals,
        value: json!(true),
    };

    // Create evaluator once
    let evaluator = UnifiedFilterEvaluator::new(Some(&filter_expr))
        .unwrap()
        .unwrap();

    // Reuse for many evaluations
    let mut results = Vec::new();
    for i in 0..1000 {
        let mut metadata = HashMap::new();
        metadata.insert("active".to_string(), json!(i % 2 == 0));
        metadata.insert("id".to_string(), json!(i));
        
        if evaluator.evaluate(&metadata) {
            results.push(i);
        }
    }
    
    println!("Found {} matching records", results.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_examples() {
        basic_filter_example();
        sst_engine_example();
        viper_engine_example();
        helix_engine_example();
        raptor_engine_example();
        nova_engine_example();
        swift_engine_example();
        prism_engine_example();
        complex_filter_example();
        performance_example();
    }
}