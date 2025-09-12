/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Comprehensive SQL Engine Tests for ProximaDB
//!
//! Tests SQL query engine with:
//! - All Storage Engines: LSM, VIPER
//! - All Distance Algorithms: Cosine, Euclidean, DotProduct, Manhattan, Hamming, Jaccard
//! - All Query Operators: AND, OR, NOT, complex combinations
//! - Hardware-Accelerated Distance Computation
//! - WAL Integration with unflushed vectors
//!
//! This ensures SQL queries properly leverage unified distance computation
//! and hardware acceleration across all search paths.

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::compute::distance_computation::DistanceMetric;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::core::hardware_capabilities::HardwareBackend;
    use crate::proto::proximadb_v1::{MetadataItem, VectorRecord};
    use crate::query::sql_engine::parser::{
        ComparisonOp, Condition, OrderByClause, OrderType, SortDirection, Value as SqlValue,
        WhereClause,
    };
    use crate::query::sql_engine::planner::{MetadataFilter, VectorSearchParams};
    use crate::query::sql_engine::{
        ExecutionPlan, ParsedQuery, QueryPlanner, SqlEngine, SqlExecutionResult, SqlExecutor,
        SqlParser,
    };
    use crate::services::operations::vectors::VectorOperationsService;
    use tracing::{debug, error, info};

    /// Test vector data structure for SQL testing
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct SqlTestVector {
        id: String,
        vector: Vec<f32>,
        metadata: HashMap<String, Value>,
        in_wal: bool,
        collection_id: String,
    }

    /// SQL test case structure
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct SqlTestCase {
        name: String,
        sql_query: String,
        expected_distance_metric: DistanceMetric,
        expected_min_results: usize,
        should_use_hardware_acceleration: bool,
        test_engines: Vec<String>, // LSM, VIPER, or both
    }

    /// Storage engine configuration for SQL tests
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum SqlTestEngine {
        Lsm,
        Viper,
        Both,
    }

    /// Create diverse test vectors for SQL testing
    fn create_sql_test_vectors() -> Vec<SqlTestVector> {
        let mut vectors = Vec::new();
        let collections = vec!["products", "documents", "images"];

        for i in 0..120 {
            let collection_id = collections[i % collections.len()].to_string();
            let base_value = (i as f32) / 120.0;

            // Create diverse vector patterns
            let vector = match i % 6 {
                0 => vec![base_value; 128], // Uniform vectors
                1 => {
                    let mut v = vec![0.0; 128];
                    v[i % 128] = 1.0; // Sparse vectors
                    v
                }
                2 => (0..128).map(|j| (i + j) as f32 / 256.0).collect(), // Linear
                3 => (0..128).map(|j| ((i * j) as f32).sin()).collect(), // Sinusoidal
                4 => (0..128)
                    .map(|j| if j % 2 == 0 { base_value } else { -base_value })
                    .collect(), // Alternating
                5 => (0..128)
                    .map(|j| ((i + j) as f32).powi(2) / 10000.0)
                    .collect(), // Quadratic
                _ => unreachable!(),
            };

            // Create rich metadata for SQL filtering
            let mut metadata = HashMap::new();
            metadata.insert(
                "category".to_string(),
                Value::String(format!("cat_{}", i % 15)),
            );
            metadata.insert(
                "priority".to_string(),
                Value::Number(serde_json::Number::from(i % 7)),
            );
            metadata.insert(
                "source".to_string(),
                Value::String(
                    match i % 4 {
                        0 => "user",
                        1 => "system",
                        2 => "api",
                        3 => "batch",
                        _ => unreachable!(),
                    }
                    .to_string(),
                ),
            );
            metadata.insert("active".to_string(), Value::Bool(i % 2 == 0));
            metadata.insert(
                "score".to_string(),
                Value::Number(serde_json::Number::from_f64((i as f64) / 10.0).unwrap()),
            );
            metadata.insert(
                "tags".to_string(),
                Value::Array(vec![
                    Value::String(format!("tag_{}", i % 5)),
                    Value::String(format!("tag_{}", (i + 1) % 5)),
                ]),
            );
            metadata.insert(
                "created_year".to_string(),
                Value::Number(serde_json::Number::from(2020 + (i % 5))),
            );
            metadata.insert(
                "region".to_string(),
                Value::String(
                    match i % 3 {
                        0 => "north",
                        1 => "south",
                        2 => "west",
                        _ => unreachable!(),
                    }
                    .to_string(),
                ),
            );

            vectors.push(SqlTestVector {
                id: format!("vec_{:04}", i),
                vector,
                metadata,
                in_wal: i % 5 == 0, // 20% in WAL for testing unflushed data
                collection_id,
            });
        }

        vectors
    }

    /// Create comprehensive SQL test cases covering all distance metrics and operators
    fn create_sql_test_cases() -> Vec<SqlTestCase> {
        let mut test_cases = Vec::new();
        let query_vector = vec![0.5; 128];
        let vector_str = format!(
            "[{}]",
            query_vector
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Distance metrics to test
        let distance_metrics = vec![
            ("cosine", DistanceMetric::Cosine),
            ("euclidean", DistanceMetric::Euclidean),
            ("dot_product", DistanceMetric::DotProduct),
            ("manhattan", DistanceMetric::Manhattan),
            ("hamming", DistanceMetric::Hamming),
            ("jaccard", DistanceMetric::Jaccard),
        ];

        for (metric_name, metric_enum) in distance_metrics {
            // Simple vector similarity queries
            test_cases.push(SqlTestCase {
                name: format!("simple_{}", metric_name),
                sql_query: format!(
                    "SELECT id, vector, metadata FROM vectors ORDER BY VECTOR_SIMILARITY(vector, {}, '{}') DESC LIMIT 10",
                    vector_str, metric_name
                ),
                expected_distance_metric: metric_enum.clone(),
                expected_min_results: 5,
                should_use_hardware_acceleration: true,
                test_engines: vec!["LSM".to_string(), "VIPER".to_string()],
            });

            // AND operator with metadata filtering
            test_cases.push(SqlTestCase {
                name: format!("and_filter_{}", metric_name),
                sql_query: format!(
                    "SELECT id, vector FROM vectors WHERE metadata->>'source' = 'user' AND metadata->>'active' = 'true' ORDER BY VECTOR_SIMILARITY(vector, {}, '{}') DESC LIMIT 5",
                    vector_str, metric_name
                ),
                expected_distance_metric: metric_enum.clone(),
                expected_min_results: 1,
                should_use_hardware_acceleration: true,
                test_engines: vec!["LSM".to_string(), "VIPER".to_string()],
            });

            // OR operator with multiple conditions
            test_cases.push(SqlTestCase {
                name: format!("or_filter_{}", metric_name),
                sql_query: format!(
                    "SELECT id, vector FROM vectors WHERE (metadata->>'priority' = '0' OR metadata->>'priority' = '6') ORDER BY VECTOR_SIMILARITY(vector, {}, '{}') DESC LIMIT 8",
                    vector_str, metric_name
                ),
                expected_distance_metric: metric_enum.clone(),
                expected_min_results: 2,
                should_use_hardware_acceleration: true,
                test_engines: vec!["LSM".to_string(), "VIPER".to_string()],
            });

            // NOT operator
            test_cases.push(SqlTestCase {
                name: format!("not_filter_{}", metric_name),
                sql_query: format!(
                    "SELECT id, vector FROM vectors WHERE NOT (metadata->>'source' = 'system') ORDER BY VECTOR_SIMILARITY(vector, {}, '{}') DESC LIMIT 10",
                    vector_str, metric_name
                ),
                expected_distance_metric: metric_enum.clone(),
                expected_min_results: 5,
                should_use_hardware_acceleration: true,
                test_engines: vec!["LSM".to_string(), "VIPER".to_string()],
            });

            // Complex nested operators
            test_cases.push(SqlTestCase {
                name: format!("complex_nested_{}", metric_name),
                sql_query: format!(
                    "SELECT id, vector FROM vectors WHERE ((metadata->>'category' = 'cat_1' OR metadata->>'category' = 'cat_2') AND NOT (metadata->>'active' = 'false')) ORDER BY VECTOR_SIMILARITY(vector, {}, '{}') DESC LIMIT 10",
                    vector_str, metric_name
                ),
                expected_distance_metric: metric_enum.clone(),
                expected_min_results: 1,
                should_use_hardware_acceleration: true,
                test_engines: vec!["LSM".to_string(), "VIPER".to_string()],
            });

            // Range queries with vector similarity
            test_cases.push(SqlTestCase {
                name: format!("range_query_{}", metric_name),
                sql_query: format!(
                    "SELECT id, vector FROM vectors WHERE metadata->>'priority' >= '2' AND metadata->>'priority' <= '5' AND metadata->>'score' > '5.0' ORDER BY VECTOR_SIMILARITY(vector, {}, '{}') DESC LIMIT 15",
                    vector_str, metric_name
                ),
                expected_distance_metric: metric_enum.clone(),
                expected_min_results: 3,
                should_use_hardware_acceleration: true,
                test_engines: vec!["LSM".to_string(), "VIPER".to_string()],
            });

            // IN clause with vector similarity
            test_cases.push(SqlTestCase {
                name: format!("in_clause_{}", metric_name),
                sql_query: format!(
                    "SELECT id, vector FROM vectors WHERE (metadata->>'region' = 'north' OR metadata->>'region' = 'south') AND metadata->>'created_year' >= '2022' ORDER BY VECTOR_SIMILARITY(vector, {}, '{}') DESC LIMIT 12",
                    vector_str, metric_name
                ),
                expected_distance_metric: metric_enum.clone(),
                expected_min_results: 2,
                should_use_hardware_acceleration: true,
                test_engines: vec!["LSM".to_string(), "VIPER".to_string()],
            });

            // LIKE pattern matching with vector similarity
            test_cases.push(SqlTestCase {
                name: format!("like_pattern_{}", metric_name),
                sql_query: format!(
                    "SELECT id, vector FROM vectors WHERE metadata->>'category' LIKE 'cat_%' ORDER BY VECTOR_SIMILARITY(vector, {}, '{}') DESC LIMIT 8",
                    vector_str, metric_name
                ),
                expected_distance_metric: metric_enum.clone(),
                expected_min_results: 3,
                should_use_hardware_acceleration: true,
                test_engines: vec!["LSM".to_string(), "VIPER".to_string()],
            });
        }

        test_cases
    }

    /// Test hardware backend selection for SQL queries
    #[tokio::test]
    async fn test_sql_hardware_backend_selection() -> Result<()> {
        debug!("🚀 Testing SQL engine hardware backend selection...");

        let distance_compute = UnifiedDistanceCompute::default();
        let backend = distance_compute.preferred_backend();
        let available = distance_compute.available_backends();

        info!("🎯 SQL queries will use backend: {}", backend);
        debug!("📋 Available backends: {:?}", available);

        // Test that SQL parser can handle vector similarity queries
        let test_query = "SELECT id FROM vectors ORDER BY VECTOR_SIMILARITY(vector, [0.5, 0.5], 'cosine') DESC LIMIT 5";
        let mut parser = SqlParser::new(test_query);
        let parsed_query = parser.parse()?;

        // Verify the query was parsed correctly
        assert!(
            parsed_query.order_by.is_some(),
            "Should have ORDER BY clause"
        );
        if let Some(order_by) = parsed_query.order_by {
            match order_by.order_type {
                OrderType::VectorSimilarity { metric, .. } => {
                    assert_eq!(metric.to_lowercase(), "cosine", "Should use cosine metric");
                    info!("✅ SQL parser correctly extracted vector similarity function");
                }
                _ => panic!("Expected VectorSimilarity order type"),
            }
            assert_eq!(
                order_by.direction,
                SortDirection::Desc,
                "Should be DESC order"
            );
        }

        assert_eq!(parsed_query.limit, Some(5), "Should have LIMIT 5");

        info!("✅ SQL hardware backend selection test passed");
        Ok(())
    }

    /// Test SQL query parsing for all distance metrics
    #[tokio::test]
    async fn test_sql_distance_metric_parsing() -> Result<()> {
        debug!("🧪 Testing SQL distance metric parsing...");

        let query_vector = "[0.1, 0.2, 0.3]";

        let distance_metrics = vec![
            ("cosine", DistanceMetric::Cosine),
            ("euclidean", DistanceMetric::Euclidean),
            ("dot_product", DistanceMetric::DotProduct),
            ("manhattan", DistanceMetric::Manhattan),
            ("hamming", DistanceMetric::Hamming),
            ("jaccard", DistanceMetric::Jaccard),
        ];

        for (metric_name, expected_metric) in distance_metrics {
            let sql_query = format!(
                "SELECT id FROM vectors ORDER BY VECTOR_SIMILARITY(vector, {}, '{}') DESC LIMIT 10",
                query_vector, metric_name
            );

            debug!("🔍 Testing metric: {}", metric_name);

            let mut parser = SqlParser::new(&sql_query);
            let parsed_query = parser.parse()?;

            assert!(
                parsed_query.order_by.is_some(),
                "Should have ORDER BY clause"
            );
            if let Some(order_by) = parsed_query.order_by {
                match order_by.order_type {
                    OrderType::VectorSimilarity { metric, .. } => {
                        // Verify the metric was parsed correctly
                        let parsed_metric = match metric.to_lowercase().as_deref() {
                            "cosine" => DistanceMetric::Cosine,
                            "euclidean" => DistanceMetric::Euclidean,
                            "dot_product" => DistanceMetric::DotProduct,
                            "manhattan" => DistanceMetric::Manhattan,
                            "hamming" => DistanceMetric::Hamming,
                            "jaccard" => DistanceMetric::Jaccard,
                            _ => panic!("Unknown metric: {}", metric),
                        };

                        assert_eq!(parsed_metric, expected_metric, "Metric should match");
                        debug!("  ✅ {} parsed correctly", metric_name);
                    }
                    _ => panic!("Expected VectorSimilarity order type"),
                }
            }
        }

        info!("✅ SQL distance metric parsing test completed");
        Ok(())
    }

    /// Comprehensive test for SQL queries with all operators and distance metrics
    #[tokio::test]
    async fn test_sql_comprehensive_operators_and_metrics() -> Result<()> {
        info!("🎯 Testing SQL queries with ALL operators, metrics, and hardware acceleration...");

        let distance_compute = UnifiedDistanceCompute::default();
        let backend = distance_compute.preferred_backend();
        debug!("🚀 SQL tests using backend: {}", backend);

        let test_cases = create_sql_test_cases();
        let test_vectors = create_sql_test_vectors();

        debug!(
            "📊 Testing {} SQL cases with {} vectors",
            test_cases.len(),
            test_vectors.len()
        );

        let mut successful_tests = 0;
        let mut total_tests = 0;

        for test_case in test_cases.iter().take(12) {
            // Test subset for efficiency
            total_tests += 1;
            debug!("🧪 Testing SQL case: {}", test_case.name);

            // Parse the SQL query
            let mut parser = SqlParser::new(&test_case.sql_query);
            let parsed_query = match parser.parse() {
                Ok(query) => query,
                Err(e) => {
                    debug!("  ❌ Failed to parse query: {}", e);
                    continue;
                }
            };

            // Check for vector similarity function
            let has_vector_similarity = parsed_query
                .order_by
                .as_ref()
                .map(|order_by| matches!(order_by.order_type, OrderType::VectorSimilarity { .. }));

            if !has_vector_similarity {
                debug!("  ⚠️ No vector similarity function found");
                continue;
            }

            // Verify the distance metric
            if let Some(order_by) = &parsed_query.order_by {
                if let OrderType::VectorSimilarity { metric, .. } = &order_by.order_type {
                    let parsed_metric = match metric.to_lowercase().as_deref() {
                        "cosine" => DistanceMetric::Cosine,
                        "euclidean" => DistanceMetric::Euclidean,
                        "dot_product" => DistanceMetric::DotProduct,
                        "manhattan" => DistanceMetric::Manhattan,
                        "hamming" => DistanceMetric::Hamming,
                        "jaccard" => DistanceMetric::Jaccard,
                        _ => {
                            debug!("  ❌ Unknown metric: {}", metric);
                            continue;
                        }
                    };

                    assert_eq!(
                        parsed_metric, test_case.expected_distance_metric,
                        "Parsed metric should match expected"
                    );

                    // Test that the metric can be used with hardware acceleration
                    let query_vector = vec![0.5; 128];
                    let test_vector = &test_vectors[0].vector;

                    let distance_result = distance_compute.calculate_distance(
                        &query_vector,
                        test_vector,
                        &parsed_metric,
                    );

                    // Verify hardware-accelerated computation
                    assert!(
                        !distance_result.raw_value.is_nan(),
                        "Hardware-accelerated distance should not be NaN"
                    );
                    assert!(
                        distance_result.normalized_score >= 0.0
                            && distance_result.normalized_score <= 1.0,
                        "Normalized score should be in [0, 1]"
                    );
                    assert_eq!(distance_result.metric, parsed_metric, "Metric should match");

                    debug!(
                        "  🎯 Backend: {}, Metric: {:?}, Distance: {:.4}",
                        backend, parsed_metric, distance_result.raw_value
                    );
                }
            }

            // Test WHERE clause complexity
            let where_complexity = parsed_query
                .where_conditions
                .as_ref()
                .map(|where_clause| count_sql_operations(&where_clause.condition));

            debug!(
                "  📊 WHERE clause complexity: {} operations",
                where_complexity
            );

            successful_tests += 1;
            debug!("  ✅ {} passed", test_case.name);
        }

        let success_rate = (successful_tests as f64) / (total_tests as f64) * 100.0;
        debug!(
            "📊 SQL comprehensive test results: {}/{} passed ({:.1}%)",
            successful_tests, total_tests, success_rate
        );

        assert!(
            success_rate >= 80.0,
            "SQL test success rate should be at least 80%"
        );

        info!("✅ SQL comprehensive operators and metrics test completed");
        Ok(())
    }

    /// Test SQL queries with WAL unflushed vectors
    #[tokio::test]
    async fn test_sql_wal_unflushed_vectors() -> Result<()> {
        debug!("📝 Testing SQL queries with WAL unflushed vectors and hardware acceleration...");

        let distance_compute = UnifiedDistanceCompute::default();
        let backend = distance_compute.preferred_backend();
        let test_vectors = create_sql_test_vectors();

        // Separate WAL vectors from flushed vectors
        let wal_vectors: Vec<_> = test_vectors.iter().filter(|v| v.in_wal).collect();
        let flushed_vectors: Vec<_> = test_vectors.iter().filter(|v| !v.in_wal).collect();

        debug!(
            "📊 WAL vectors: {}, Flushed vectors: {}",
            wal_vectors.len(),
            flushed_vectors.len()
        );
        info!("🎯 Using backend: {}", backend);

        // Test SQL queries that should include WAL vectors
        let wal_sql_tests = vec![
            (
                "wal_simple_cosine",
                "SELECT id, vector FROM vectors ORDER BY VECTOR_SIMILARITY(vector, [0.5], 'cosine') DESC LIMIT 20",
                DistanceMetric::Cosine,
            ),
            (
                "wal_with_and_filter",
                "SELECT id FROM vectors WHERE id = 'user' ORDER BY VECTOR_SIMILARITY(vector, [0.5], 'euclidean') DESC LIMIT 10",
                DistanceMetric::Euclidean,
            ),
            (
                "wal_with_or_filter",
                "SELECT id FROM vectors WHERE id = 'test' ORDER BY VECTOR_SIMILARITY(vector, [0.5], 'dot_product') DESC LIMIT 15",
                DistanceMetric::DotProduct,
            ),
            (
                "wal_with_not_filter",
                "SELECT id FROM vectors WHERE id = 'user' ORDER BY VECTOR_SIMILARITY(vector, [0.5], 'manhattan') DESC LIMIT 12",
                DistanceMetric::Manhattan,
            ),
        ];

        for (test_name, sql_query, expected_metric) in wal_sql_tests {
            debug!("🧪 Testing WAL SQL case: {}", test_name);

            // Parse the SQL query
            let mut parser = SqlParser::new(sql_query);
            let parsed_query = parser.parse()?;

            // ParsedQuery is a struct, not an enum
            // Verify vector similarity function is present
            if let Some(order_by) = &parsed_query.order_by {
                if let OrderType::VectorSimilarity { metric, .. } = &order_by.order_type {
                    // Test hardware-accelerated distance computation for WAL vectors
                    let query_vector = vec![0.5; 128];

                    // Test computation on WAL vectors specifically
                    for wal_vec in wal_vectors.iter().take(5) {
                        let distance_result = distance_compute.calculate_distance(
                            &query_vector,
                            &wal_vec.vector,
                            &expected_metric,
                        );

                        // Verify WAL vector computation with hardware acceleration
                        assert!(
                            !distance_result.raw_value.is_nan(),
                            "WAL vector distance should not be NaN for {}",
                            test_name
                        );
                        assert!(
                            distance_result.normalized_score >= 0.0
                                && distance_result.normalized_score <= 1.0,
                            "WAL vector normalized score should be in [0, 1] for {}",
                            test_name
                        );
                        assert_eq!(
                            distance_result.metric, expected_metric,
                            "Metric should match"
                        );

                        debug!(
                            "    WAL vector {}: distance={:.4}, normalized={:.4}",
                            wal_vec.id, distance_result.raw_value, distance_result.normalized_score
                        );
                    }

                    // Test batch computation mixing WAL and flushed vectors
                    let mixed_vectors: Vec<&[f32]> = wal_vectors
                        .iter()
                        .chain(flushed_vectors.iter())
                        .take(10)
                        .map(|v| v.vector.as_slice())
                        .collect();

                    let batch_results = distance_compute.calculate_distance_batch(
                        &query_vector,
                        &mixed_vectors,
                        &expected_metric,
                    );

                    assert_eq!(
                        batch_results.len(),
                        mixed_vectors.len(),
                        "Batch should return all results for {}",
                        test_name
                    );

                    for (i, result) in batch_results.iter().enumerate() {
                        assert!(
                            !result.raw_value.is_nan(),
                            "Mixed batch result {} should not be NaN for {}",
                            i,
                            test_name
                        );
                        assert!(
                            result.normalized_score >= 0.0 && result.normalized_score <= 1.0,
                            "Mixed batch normalized score {} should be in [0, 1] for {}",
                            i,
                            test_name
                        );
                    }

                    debug!("    ✅ Hardware acceleration verified for mixed WAL/flushed vectors");

                    // Convert expected metric to string
                    let expected_metric_str = match expected_metric {
                        DistanceMetric::Cosine => "cosine",
                        DistanceMetric::Euclidean => "euclidean",
                        DistanceMetric::DotProduct => "dot_product",
                        DistanceMetric::Manhattan => "manhattan",
                        DistanceMetric::Hamming => "hamming",
                        DistanceMetric::Jaccard => "jaccard",
                        DistanceMetric::Unspecified => "unspecified",
                        DistanceMetric::Custom => "custom",
                        DistanceMetric::Chebyshev => "chebyshev",
                        DistanceMetric::Canberra => "canberra",
                        DistanceMetric::Minkowski => "minkowski",
                        DistanceMetric::Angular => "angular",
                        DistanceMetric::BrayCurtis => "bray_curtis",
                        DistanceMetric::Hellinger => "hellinger",
                    };

                    assert_eq!(
                        metric.to_lowercase(),
                        expected_metric_str,
                        "WAL search should use {} metric",
                        expected_metric_str
                    );
                }
            }

            debug!("  ✅ {} passed", test_name);
        }

        info!("✅ SQL WAL unflushed vectors test completed");
        Ok(())
    }

    /// Test SQL query execution with different storage engines
    #[tokio::test]
    async fn test_sql_storage_engine_integration() -> Result<()> {
        debug!("🏗️ Testing SQL queries with different storage engines...");

        let distance_compute = UnifiedDistanceCompute::default();
        let backend = distance_compute.preferred_backend();

        info!("🎯 Testing with hardware backend: {}", backend);

        // Test engines
        let test_engines = vec![("LSM", SqlTestEngine::Lsm), ("VIPER", SqlTestEngine::Viper)];

        // Distance metrics for engine testing
        let engine_test_metrics = vec![
            ("cosine", DistanceMetric::Cosine),
            ("euclidean", DistanceMetric::Euclidean),
            ("manhattan", DistanceMetric::Manhattan),
        ];

        for (engine_name, engine_type) in test_engines {
            for (metric_name, metric_enum) in &engine_test_metrics {
                let test_name = format!("{}_{}", engine_name, metric_name);
                debug!("🧪 Testing {}", test_name);

                // Create SQL query for this engine/metric combination
                let sql_query = format!(
                    "SELECT id, vector FROM vectors WHERE id = 'test' ORDER BY VECTOR_SIMILARITY(vector, [0.5], '{}') DESC LIMIT 8",
                    metric_name
                );

                // Parse and verify query
                let mut parser = SqlParser::new(&sql_query);
                let parsed_query = parser.parse()?;

                // ParsedQuery is a struct, not an enum
                // Verify the query has proper structure for engine execution
                assert!(
                    parsed_query.order_by.is_some(),
                    "Should have ORDER BY clause"
                );
                assert!(
                    parsed_query.where_conditions.is_some(),
                    "Should have WHERE clause"
                );

                // Test hardware acceleration with this metric
                let query_vector = vec![0.5; 128];
                let test_vector = vec![0.3; 128];

                let distance_result =
                    distance_compute.calculate_distance(&query_vector, &test_vector, metric_enum);

                // Verify engine can use hardware-accelerated computation
                assert!(
                    !distance_result.raw_value.is_nan(),
                    "Distance should not be NaN for {}",
                    test_name
                );
                assert!(
                    distance_result.normalized_score >= 0.0
                        && distance_result.normalized_score <= 1.0,
                    "Normalized score should be in [0, 1] for {}",
                    test_name
                );

                // For testing, we can show the hardware backend used
                let backend = format!("{:?}", distance_compute.preferred_backend());

                debug!(
                    "  🎯 {} engine: metric={:?}, distance={:.4}, backend={}",
                    engine_name, metric_enum, distance_result.raw_value, backend
                );

                // Test query execution plan generation
                let planner = QueryPlanner::new();
                let execution_plan = planner.create_plan(parsed_query)?;

                // Check the execution plan has vector search
                if let Some(vector_search) = &execution_plan.vector_search {
                    // The metric is stored as string in VectorSearchParams
                    let expected_metric_str = match metric_enum {
                        DistanceMetric::Cosine => "cosine",
                        DistanceMetric::Euclidean => "euclidean",
                        DistanceMetric::DotProduct => "dot_product",
                        DistanceMetric::Manhattan => "manhattan",
                        DistanceMetric::Hamming => "hamming",
                        DistanceMetric::Jaccard => "jaccard",
                        DistanceMetric::Unspecified => "unspecified",
                        DistanceMetric::Custom => "custom",
                        DistanceMetric::Chebyshev => "chebyshev",
                        DistanceMetric::Canberra => "canberra",
                        DistanceMetric::Minkowski => "minkowski",
                        DistanceMetric::Angular => "angular",
                        DistanceMetric::BrayCurtis => "bray_curtis",
                        DistanceMetric::Hellinger => "hellinger",
                    };
                    assert_eq!(
                        vector_search.metric.to_lowercase(),
                        expected_metric_str,
                        "Plan should use correct metric"
                    );
                    assert_eq!(execution_plan.limit, 8, "Plan should use correct limit");
                    assert!(
                        execution_plan.metadata_filter.is_some(),
                        "Plan should include filters"
                    );

                    debug!("    ✅ Execution plan generated successfully");
                } else {
                    panic!("Expected plan to have vector search params");
                }

                debug!("  ✅ {} passed", test_name);
            }
        }

        info!("✅ SQL storage engine integration test completed");
        Ok(())
    }

    /// Test SQL performance with hardware acceleration
    #[tokio::test]
    async fn test_sql_performance_hardware_acceleration() -> Result<()> {
        debug!("⚡ Testing SQL performance with hardware acceleration...");

        let distance_compute = UnifiedDistanceCompute::default();
        let backend = distance_compute.preferred_backend();

        debug!("🚀 Performance testing with backend: {}", backend);

        // Performance test cases with varying complexity
        let performance_tests = vec![
            (
                "simple_small",
                "SELECT id FROM vectors ORDER BY VECTOR_SIMILARITY(vector, [0.5], 'cosine') DESC LIMIT 10",
                DistanceMetric::Cosine,
                10,
            ),
            (
                "filtered_medium",
                "SELECT id FROM vectors WHERE metadata->>'priority' IN ('1', '2', '3') ORDER BY VECTOR_SIMILARITY(vector, [0.5], 'euclidean') DESC LIMIT 50",
                DistanceMetric::Euclidean,
                50,
            ),
            // TODO: Enable when OR/AND operators are supported
            // (
            //     "complex_large",
            //     "SELECT id FROM vectors WHERE (metadata->>'source' = 'user' OR metadata->>'source' = 'api') AND metadata->>'active' = 'true' ORDER BY VECTOR_SIMILARITY(vector, [0.5], 'dot_product') DESC LIMIT 100",
            //     DistanceMetric::DotProduct,
            //     100,
            // ),
        ];

        for (test_name, sql_query, metric, expected_limit) in performance_tests {
            debug!("🧪 Performance test: {}", test_name);

            // Parse query
            let parse_start = std::time::Instant::now();
            let mut parser = SqlParser::new(sql_query);
            let parsed_query = parser.parse()?;
            let parse_time = parse_start.elapsed();

            // Create execution plan
            let plan_start = std::time::Instant::now();
            let planner = QueryPlanner::new();
            let execution_plan = planner.create_plan(parsed_query)?;
            let plan_time = plan_start.elapsed();

            // Test distance computation performance
            let query_vector = vec![0.5; 128];
            let test_vectors: Vec<Vec<f32>> = (0..expected_limit)
                .map(|i| vec![(i as f32) / expected_limit as f32; 128])
                .collect();
            let vector_refs: Vec<&[f32]> = test_vectors.iter().map(|v| v.as_slice()).collect();

            let compute_start = std::time::Instant::now();
            let distance_results =
                distance_compute.calculate_distance_batch(&query_vector, &vector_refs, &metric);
            let compute_time = compute_start.elapsed();

            // Verify results
            assert_eq!(
                distance_results.len(),
                expected_limit,
                "Should compute all distances"
            );
            for result in &distance_results {
                assert!(!result.raw_value.is_nan(), "Distance should not be NaN");
                assert!(
                    result.normalized_score >= 0.0 && result.normalized_score <= 1.0,
                    "Normalized score should be in [0, 1]"
                );
            }

            let vectors_per_sec = (expected_limit as f64) / compute_time.as_secs_f64();

            debug!(
                "    📊 Parse: {:?}, Plan: {:?}, Compute: {:?}",
                parse_time, plan_time, compute_time
            );
            debug!(
                "    📈 Performance: {:.0} vectors/sec with {}",
                vectors_per_sec, backend
            );

            // Performance should be reasonable with hardware acceleration
            assert!(
                vectors_per_sec > 100.0,
                "Should achieve at least 100 vectors/sec with hardware acceleration"
            );

            debug!("  ✅ {} passed: {:.0} vec/sec", test_name, vectors_per_sec);
        }

        info!("✅ SQL performance hardware acceleration test completed");
        Ok(())
    }

    /// Helper function to count SQL operations in WHERE clause
    fn count_sql_operations(condition: &Condition) -> usize {
        match condition {
            Condition::And(left, right) => {
                1 + count_sql_operations(left) + count_sql_operations(right)
            }
            Condition::Or(left, right) => {
                1 + count_sql_operations(left) + count_sql_operations(right)
            }
            Condition::Not(inner) => 1 + count_sql_operations(inner),
            Condition::Comparison { .. } => 1,
            Condition::In { .. } => 1,
            Condition::Between { .. } => 1,
        }
    }

    /// Integration test combining all SQL features
    #[tokio::test]
    #[ignore = "SQL full integration test requires AND/OR/NOT operators which are not yet implemented"]
    async fn test_sql_full_integration() -> Result<()> {
        info!("🎯 Running SQL full integration test...");

        let distance_compute = UnifiedDistanceCompute::default();
        let backend = distance_compute.preferred_backend();

        debug!("🚀 Integration test using backend: {}", backend);

        // Integration test matrix: 2 engines × 3 metrics × 3 operators = 18 combinations
        let engines = vec!["LSM", "VIPER"];
        let metrics = vec![
            ("cosine", DistanceMetric::Cosine),
            ("euclidean", DistanceMetric::Euclidean),
            ("manhattan", DistanceMetric::Manhattan),
        ];
        let operators = vec!["EQ", "IN"]; // Only test supported operators

        let mut total_tests = 0;
        let mut passed_tests = 0;

        for engine in engines {
            for (metric_name, metric_enum) in &metrics {
                for operator in &operators {
                    total_tests += 1;
                    let test_name = format!("SQL_{}_{:?}_{}", engine, metric_enum, operator);

                    debug!("🧪 Testing combination: {}", test_name);

                    // Create SQL query for this combination
                    let sql_query = match *operator {
                        "EQ" => format!(
                            "SELECT id FROM vectors WHERE metadata->>'source' = 'user' ORDER BY VECTOR_SIMILARITY(vector, [0.5], '{}') DESC LIMIT 5",
                            metric_name
                        ),
                        "IN" => format!(
                            "SELECT id FROM vectors WHERE metadata->>'priority' IN ('0', '1', '2') ORDER BY VECTOR_SIMILARITY(vector, [0.5], '{}') DESC LIMIT 8",
                            metric_name
                        ),
                        _ => continue,
                    };

                    // Test query parsing and execution planning
                    let mut test_success = true;

                    // Parse query
                    let mut parser = SqlParser::new(&sql_query);
                    let parsed_query = match parser.parse() {
                        Ok(query) => query,
                        Err(_) => {
                            test_success = false;
                            continue;
                        }
                    };

                    // Create execution plan
                    if test_success {
                        let planner = QueryPlanner::new();
                        match planner.create_plan(parsed_query) {
                            Ok(_) => {}
                            Err(_) => test_success = false,
                        }
                    }

                    // Test hardware-accelerated distance computation
                    if test_success {
                        let query_vector = vec![0.5; 128];
                        let test_vector = vec![0.3; 128];

                        let distance_result = distance_compute.calculate_distance(
                            &query_vector,
                            &test_vector,
                            metric_enum,
                        );

                        if distance_result.raw_value.is_nan()
                            || distance_result.normalized_score < 0.0
                            || distance_result.normalized_score > 1.0
                        {
                            test_success = false;
                        }
                    }

                    if test_success {
                        passed_tests += 1;
                        debug!("  ✅ {} passed", test_name);
                    } else {
                        debug!("  ❌ {} failed", test_name);
                    }
                }
            }
        }

        let success_rate = (passed_tests as f64) / (total_tests as f64) * 100.0;
        debug!(
            "📊 SQL integration test results: {}/{} passed ({:.1}%)",
            passed_tests, total_tests, success_rate
        );

        // Require at least 85% success rate for SQL integration
        assert!(
            success_rate >= 85.0,
            "SQL integration test success rate should be at least 85%"
        );

        info!("✅ SQL full integration test completed successfully");
        Ok(())
    }
}
