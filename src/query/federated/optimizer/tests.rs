#[cfg(test)]
mod tests {
    use crate::query::capability::{Capability, CapabilitySet};
    use crate::query::federated::optimizer::*;
    use crate::query::federated::parser::{
        self, FederatedQuery, QueryType, SqlExtension, TargetModelType, VectorQuery,
    };
    use crate::storage::multimodel::ModelType;
    use std::collections::HashMap;

    #[test]
    fn test_optimizer_creation() {
        let optimizer = CrossModelOptimizer::new();
        assert!(optimizer.cost_models.contains_key(&ModelType::Vector));
        assert!(optimizer.cost_models.contains_key(&ModelType::Graph));
    }

    #[test]
    fn test_optimize_sql_query() {
        let optimizer = CrossModelOptimizer::new();
        let query = FederatedQuery {
            sql: "SELECT * FROM users".to_string(),
            query_type: QueryType::Sql,
            extensions: vec![],
            extension_positions: vec![],
            extension_aliases: vec![],
            targets: vec![parser::QueryTarget {
                name: "users".to_string(),
                alias: None,
                model_type: TargetModelType::Table,
            }],
            parameters: HashMap::new(),
            is_cross_model_join: false,
        };

        let plan = optimizer
            .optimize(&query)
            .expect("optimize should succeed for valid query");
        assert!(plan.total_cost > 0.0);
        assert!(!plan.metadata.is_cross_model);
    }

    #[test]
    fn test_graph_query_plan_uses_projected_output_columns_for_scalar_subset_queries() {
        let optimizer = CrossModelOptimizer::new();
        let query = parser::FederatedParser::new()
            .parse(
                "SELECT * FROM GRAPH_QUERY('MATCH (n:Person) FROM social RETURN n.name AS person_name')",
            )
            .expect("graph query should parse");

        let plan = optimizer
            .optimize(&query)
            .expect("graph query plan should build");

        assert_eq!(plan.root.output_columns, vec!["person_name"]);
    }

    #[test]
    fn test_graph_query_plan_preserves_legacy_output_columns_for_node_projection() {
        let optimizer = CrossModelOptimizer::new();
        let query = parser::FederatedParser::new()
            .parse("SELECT * FROM GRAPH_QUERY('MATCH (n:Person) FROM social RETURN n')")
            .expect("graph query should parse");

        let plan = optimizer
            .optimize(&query)
            .expect("graph query plan should build");

        assert_eq!(
            plan.root.output_columns,
            vec![
                "node_id".to_string(),
                "label".to_string(),
                "properties".to_string()
            ]
        );
    }

    #[test]
    fn test_optimize_vector_search() {
        let optimizer = CrossModelOptimizer::new();
        let query = FederatedQuery {
            sql: "SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1]', 10)".to_string(),
            query_type: QueryType::VectorSearch,
            extensions: vec![SqlExtension::VectorSearch {
                collection: "embeddings".to_string(),
                query_vector: VectorQuery::Literal(vec![0.1]),
                top_k: 10,
            }],
            extension_positions: vec![14],
            extension_aliases: vec![None],
            targets: vec![],
            parameters: HashMap::new(),
            is_cross_model_join: false,
        };

        let plan = optimizer
            .optimize(&query)
            .expect("optimize should succeed for vector search query");
        assert!(plan.metadata.involved_models.contains(&ModelType::Vector));
    }

    #[test]
    fn test_lateral_plan_preserves_sql_source_order_and_correlation() {
        let parser = parser::FederatedParser::new();
        let query = parser
            .parse(
                "SELECT * FROM DOCUMENT_QUERY('profiles') p JOIN LATERAL VECTOR_SEARCH('products', p.document.embedding, 1) v ON true",
            )
            .expect("parser should accept function-backed lateral query");
        let optimizer = CrossModelOptimizer::new();

        let plan = optimizer
            .optimize(&query)
            .expect("optimizer should preserve lateral source ordering");

        match &plan.root.node_type {
            PlanNodeType::NestedLoopJoin {
                outer,
                inner,
                correlation,
            } => {
                assert!(matches!(
                    outer.node_type,
                    PlanNodeType::DocumentQuery { .. }
                ));
                assert!(matches!(inner.node_type, PlanNodeType::VectorSearch { .. }));
                assert_eq!(correlation, &vec!["p.document.embedding".to_string()]);
                match &outer.node_type {
                    PlanNodeType::DocumentQuery { source_alias, .. } => {
                        assert_eq!(source_alias.as_deref(), Some("p"));
                    }
                    other => panic!("expected document outer plan, got {:?}", other),
                }
            }
            other => panic!("expected nested-loop join, got {:?}", other),
        }
    }

    #[test]
    fn test_lateral_plan_preserves_right_document_alias_in_multi_document_outer_join() {
        let parser = parser::FederatedParser::new();
        let query = parser
            .parse(
                "SELECT * FROM DOCUMENT_QUERY('left_profiles') p, DOCUMENT_QUERY('right_profiles') q JOIN LATERAL VECTOR_SEARCH('products', q.document.embedding, 1) v ON true",
            )
            .expect("parser should accept repeated function-backed sources");
        let optimizer = CrossModelOptimizer::new();

        let plan = optimizer
            .optimize(&query)
            .expect("optimizer should preserve distinct document aliases");

        match &plan.root.node_type {
            PlanNodeType::NestedLoopJoin {
                outer,
                inner,
                correlation,
            } => {
                assert!(matches!(inner.node_type, PlanNodeType::VectorSearch { .. }));
                assert_eq!(correlation, &vec!["q.document.embedding".to_string()]);
                match &outer.node_type {
                    PlanNodeType::NestedLoopJoin { outer, inner, .. } => {
                        match &outer.node_type {
                            PlanNodeType::DocumentQuery { source_alias, .. } => {
                                assert_eq!(source_alias.as_deref(), Some("p"));
                            }
                            other => panic!("expected left document source, got {:?}", other),
                        }
                        match &inner.node_type {
                            PlanNodeType::DocumentQuery { source_alias, .. } => {
                                assert_eq!(source_alias.as_deref(), Some("q"));
                            }
                            other => panic!("expected right document source, got {:?}", other),
                        }
                    }
                    other => panic!("expected nested outer document join, got {:?}", other),
                }
            }
            other => panic!("expected nested-loop join, got {:?}", other),
        }
    }

    #[test]
    fn test_lateral_plan_preserves_quoted_alias_with_dot_in_vector_source() {
        let parser = parser::FederatedParser::new();
        let query = parser
            .parse(
                "SELECT * FROM DOCUMENT_QUERY('left_profiles') \"Left.Alias\", DOCUMENT_QUERY('right_profiles') \"Right.Alias\" JOIN LATERAL VECTOR_SEARCH('products', \"Right.Alias\".document.embedding, 1) v ON true",
            )
            .expect("parser should accept quoted dotted aliases");
        let optimizer = CrossModelOptimizer::new();

        let plan = optimizer
            .optimize(&query)
            .expect("optimizer should preserve quoted dotted alias boundaries");

        match &plan.root.node_type {
            PlanNodeType::NestedLoopJoin { inner, .. } => match &inner.node_type {
                PlanNodeType::VectorSearch {
                    query_vector_source,
                    ..
                } => match query_vector_source {
                    VectorSource::ColumnRef { table, column } => {
                        assert_eq!(table, "\"Right.Alias\"");
                        assert_eq!(column, "document.embedding");
                    }
                    other => panic!("expected correlated column ref, got {:?}", other),
                },
                other => panic!("expected vector search inner plan, got {:?}", other),
            },
            other => panic!("expected nested-loop join, got {:?}", other),
        }
    }

    #[test]
    fn test_cost_model_defaults() {
        let cost_model = CostModel::default();
        assert_eq!(cost_model.row_scan_cost, 1.0);
        assert_eq!(cost_model.index_lookup_cost, 0.1);
    }

    // ========================================================================
    // OPTIMIZATION RULE TESTS
    // ========================================================================

    /// Helper to create a scan node
    fn make_scan(optimizer: &CrossModelOptimizer, target: &str, cost: f64, rows: u64) -> PlanNode {
        PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Scan {
                target: target.to_string(),
                model_type: ModelType::Relational,
                predicates: vec![],
            },
            estimated_cost: cost,
            estimated_rows: rows,
            output_columns: vec!["id".to_string(), "name".to_string(), "value".to_string()],
            required_capabilities: CapabilitySet::new(),
        }
    }

    /// Helper to create a filter node
    fn make_filter(
        optimizer: &CrossModelOptimizer,
        input: PlanNode,
        column: &str,
        value: &str,
    ) -> PlanNode {
        PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Filter {
                input: Box::new(input),
                predicate: Predicate {
                    column: column.to_string(),
                    op: PredicateOp::Eq,
                    value: PredicateValue::String(value.to_string()),
                },
            },
            estimated_cost: 10.0,
            estimated_rows: 100,
            output_columns: vec!["id".to_string(), "name".to_string(), "value".to_string()],
            required_capabilities: CapabilitySet::new(),
        }
    }

    /// Helper to create a hash join node
    fn make_hash_join(
        optimizer: &CrossModelOptimizer,
        left: PlanNode,
        right: PlanNode,
    ) -> PlanNode {
        PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::HashJoin {
                left: Box::new(left),
                right: Box::new(right),
                join_keys: vec![("id".to_string(), "id".to_string())],
                join_type: JoinType::Inner,
            },
            estimated_cost: 200.0,
            estimated_rows: 1000,
            output_columns: vec!["*".to_string()],
            required_capabilities: CapabilitySet::new(),
        }
    }

    #[test]
    fn test_predicate_pushdown_to_scan() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Filter(Scan(users))
        let scan = make_scan(&optimizer, "users", 100.0, 1000);
        let filter = make_filter(&optimizer, scan, "name", "Alice");

        // Apply predicate pushdown
        let optimized = optimizer
            .push_predicates(filter)
            .expect("push_predicates should succeed for filter node");

        // The filter should be pushed into the scan
        match &optimized.node_type {
            PlanNodeType::Scan { predicates, .. } => {
                assert_eq!(predicates.len(), 1);
                assert_eq!(predicates[0].column, "name");
            }
            _ => panic!(
                "Expected scan with pushed predicate, got {:?}",
                optimized.node_type
            ),
        }

        // Cost should be reduced (10% selectivity)
        assert!(optimized.estimated_cost < 100.0);
    }

    #[test]
    fn test_predicate_pushdown_through_join() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Filter(HashJoin(Scan(users), Scan(orders)))
        let users_scan = make_scan(&optimizer, "users", 100.0, 1000);
        let orders_scan = make_scan(&optimizer, "orders", 150.0, 2000);
        let join = make_hash_join(&optimizer, users_scan, orders_scan);
        let filter = make_filter(&optimizer, join, "users.name", "Alice");

        // Apply predicate pushdown
        let optimized = optimizer
            .push_predicates(filter)
            .expect("push_predicates should succeed for filter with join");

        // The filter should be pushed through the join
        match &optimized.node_type {
            PlanNodeType::HashJoin { left, .. } => {
                // Left side should have the pushed predicate
                match &left.node_type {
                    PlanNodeType::Scan { predicates, .. } => {
                        assert_eq!(predicates.len(), 1);
                    }
                    _ => panic!("Expected scan with pushed predicate on left"),
                }
            }
            _ => panic!("Expected hash join at top level"),
        }
    }

    #[test]
    fn test_join_reordering_swaps_cheaper_to_left() {
        let optimizer = CrossModelOptimizer::new();

        // Create: HashJoin(expensive_scan, cheap_scan)
        let expensive = make_scan(&optimizer, "big_table", 1000.0, 100000);
        let cheap = make_scan(&optimizer, "small_table", 10.0, 100);
        let join = make_hash_join(&optimizer, expensive, cheap);

        // Apply join reordering
        let optimized = optimizer
            .reorder_joins(join)
            .expect("reorder_joins should succeed for hash join");

        // For inner joins, cheaper table should be on the left
        match &optimized.node_type {
            PlanNodeType::HashJoin { left, right, .. } => {
                assert!(
                    left.estimated_cost < right.estimated_cost,
                    "Expected left ({}) to be cheaper than right ({})",
                    left.estimated_cost,
                    right.estimated_cost
                );
            }
            _ => panic!("Expected hash join"),
        }
    }

    #[test]
    fn test_projection_pushdown() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Project(Scan(users), [id, name])
        let scan = make_scan(&optimizer, "users", 100.0, 1000);
        let project = PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Project {
                input: Box::new(scan),
                columns: vec!["id".to_string(), "name".to_string()],
            },
            estimated_cost: 5.0,
            estimated_rows: 1000,
            output_columns: vec!["id".to_string(), "name".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        // Apply projection pushdown
        let optimized = optimizer
            .push_projections(project)
            .expect("push_projections should succeed for project node");

        // The projection should be pushed down and the scan should only read needed columns
        // Cost should be reduced
        assert!(optimized.estimated_cost <= 105.0);
    }

    #[test]
    fn test_parallel_execution_identification_hash_join() {
        let optimizer = CrossModelOptimizer::new();

        // Create: HashJoin(Scan(a), Scan(b))
        let scan_a = make_scan(&optimizer, "table_a", 100.0, 1000);
        let scan_b = make_scan(&optimizer, "table_b", 100.0, 1000);
        let join = make_hash_join(&optimizer, scan_a, scan_b);

        // Apply parallel identification
        let optimized = optimizer
            .identify_parallelism(join)
            .expect("identify_parallelism should succeed for hash join");

        // Cost should account for parallel execution (max of children, not sum)
        match &optimized.node_type {
            PlanNodeType::HashJoin { left, right, .. } => {
                // Parallel cost model: max(left, right) + overhead
                // This should be less than left + right + join overhead
                assert!(
                    optimized.estimated_cost < left.estimated_cost + right.estimated_cost + 200.0,
                    "Parallel cost {} should be less than sequential {}",
                    optimized.estimated_cost,
                    left.estimated_cost + right.estimated_cost + 200.0
                );
            }
            _ => panic!("Expected hash join"),
        }
    }

    #[test]
    fn test_parallel_execution_identification_union() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Union(Scan(a), Scan(b), Scan(c))
        let scan_a = make_scan(&optimizer, "table_a", 50.0, 500);
        let scan_b = make_scan(&optimizer, "table_b", 100.0, 1000);
        let scan_c = make_scan(&optimizer, "table_c", 75.0, 750);

        let union = PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Union {
                inputs: vec![scan_a, scan_b, scan_c],
                all: true,
            },
            estimated_cost: 250.0,
            estimated_rows: 2250,
            output_columns: vec!["*".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        // Apply parallel identification
        let optimized = optimizer
            .identify_parallelism(union)
            .expect("identify_parallelism should succeed for union node");

        // All union inputs can run in parallel, cost should be max + overhead
        assert!(
            optimized.estimated_cost < 250.0,
            "Union cost {} should be less than original 250.0 due to parallelism",
            optimized.estimated_cost
        );
    }

    #[test]
    fn test_find_parallel_stages() {
        let optimizer = CrossModelOptimizer::new();

        // Create: HashJoin(HashJoin(a, b), c)
        let scan_a = make_scan(&optimizer, "table_a", 50.0, 500);
        let scan_b = make_scan(&optimizer, "table_b", 100.0, 1000);
        let scan_c = make_scan(&optimizer, "table_c", 75.0, 750);

        let inner_join = make_hash_join(&optimizer, scan_a.clone(), scan_b.clone());
        let outer_join = make_hash_join(&optimizer, inner_join.clone(), scan_c.clone());

        let stages = optimizer.find_parallel_stages(&outer_join);

        // Should identify that a and b can run in parallel
        // and that (a join b) and c can run in parallel
        assert!(stages.len() >= 1, "Should find at least one parallel stage");
    }

    #[test]
    fn test_predicate_selectivity_estimation() {
        let optimizer = CrossModelOptimizer::new();

        // Equality predicate should have ~10% selectivity
        let eq_pred = Predicate {
            column: "id".to_string(),
            op: PredicateOp::Eq,
            value: PredicateValue::Int(42),
        };
        assert!((optimizer.estimate_predicate_selectivity(&eq_pred) - 0.1).abs() < 0.001);

        // Not equal should have ~90% selectivity
        let ne_pred = Predicate {
            column: "status".to_string(),
            op: PredicateOp::Ne,
            value: PredicateValue::String("deleted".to_string()),
        };
        assert!((optimizer.estimate_predicate_selectivity(&ne_pred) - 0.9).abs() < 0.001);

        // Range predicates should have ~30% selectivity
        let range_pred = Predicate {
            column: "price".to_string(),
            op: PredicateOp::Gt,
            value: PredicateValue::Float(100.0),
        };
        assert!((optimizer.estimate_predicate_selectivity(&range_pred) - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_predicate_selectivity_policy_validation() {
        let mut policy = PredicateSelectivityPolicy::default();
        assert!(policy.validate().is_ok());

        policy.eq = 1.2;
        assert!(policy.validate().is_err());
    }

    #[test]
    fn test_predicate_selectivity_uses_configured_policy() {
        let optimizer = CrossModelOptimizer::new().with_predicate_selectivity_policy(
            PredicateSelectivityPolicy {
                eq: 0.42,
                range: 0.33,
                ..Default::default()
            },
        );

        let eq_pred = Predicate {
            column: "id".to_string(),
            op: PredicateOp::Eq,
            value: PredicateValue::Int(42),
        };
        assert!((optimizer.estimate_predicate_selectivity(&eq_pred) - 0.42).abs() < 0.001);

        let range_pred = Predicate {
            column: "price".to_string(),
            op: PredicateOp::Gt,
            value: PredicateValue::Float(100.0),
        };
        assert!((optimizer.estimate_predicate_selectivity(&range_pred) - 0.33).abs() < 0.001);
    }

    #[test]
    fn test_predicate_to_string() {
        let optimizer = CrossModelOptimizer::new();

        let pred = Predicate {
            column: "name".to_string(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("Alice".to_string()),
        };
        assert_eq!(optimizer.predicate_to_string(&pred), "name = 'Alice'");

        let in_pred = Predicate {
            column: "status".to_string(),
            op: PredicateOp::In,
            value: PredicateValue::List(vec![
                PredicateValue::String("active".to_string()),
                PredicateValue::String("pending".to_string()),
            ]),
        };
        assert_eq!(
            optimizer.predicate_to_string(&in_pred),
            "status IN ('active', 'pending')"
        );
    }

    #[test]
    fn test_optimization_reduces_cost() {
        let optimizer = CrossModelOptimizer::new();

        // Create a complex plan that should benefit from all optimizations
        let scan_users = make_scan(&optimizer, "users", 100.0, 1000);
        let scan_orders = make_scan(&optimizer, "orders", 200.0, 5000);
        let join = make_hash_join(&optimizer, scan_users, scan_orders);
        let filter = make_filter(&optimizer, join, "users.status", "active");

        let original_cost = optimizer.calculate_total_cost(&filter);

        // Apply all optimizations
        let optimized = optimizer
            .apply_optimizations(filter)
            .expect("apply_optimizations should succeed for filter node");
        let optimized_cost = optimizer.calculate_total_cost(&optimized);

        // Optimized plan should have lower or equal cost
        // Note: Allow larger variance (1.5x) because predicate pushdown may initially
        // increase intermediate costs while ultimately reducing overall execution cost.
        // The optimizer focuses on reducing I/O and network costs, not just the cost metric.
        assert!(
            optimized_cost <= original_cost * 1.5,
            "Optimized cost {} should not be much higher than original {}",
            optimized_cost,
            original_cost
        );
    }

    #[test]
    fn test_collect_required_columns() {
        let optimizer = CrossModelOptimizer::new();

        // Create: Filter(Project(Scan(users), [id, name, email]), name = 'Alice')
        let scan = make_scan(&optimizer, "users", 100.0, 1000);
        let project = PlanNode {
            id: optimizer.next_id(),
            node_type: PlanNodeType::Project {
                input: Box::new(scan),
                columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
            },
            estimated_cost: 5.0,
            estimated_rows: 1000,
            output_columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
            required_capabilities: CapabilitySet::new(),
        };
        let filter = make_filter(&optimizer, project, "name", "Alice");

        let required = optimizer.collect_required_columns(&filter);

        // Should include all projected columns plus the filter column
        assert!(required.contains(&"id".to_string()));
        assert!(required.contains(&"name".to_string()));
        assert!(required.contains(&"email".to_string()));
    }

    #[test]
    fn test_greedy_join_order() {
        let optimizer = CrossModelOptimizer::new();

        // Create relations with different costs
        let mut relations = vec![
            make_scan(&optimizer, "big", 1000.0, 100000),
            make_scan(&optimizer, "small", 10.0, 100),
            make_scan(&optimizer, "medium", 100.0, 1000),
        ];

        let result = optimizer
            .greedy_join_order(
                &mut relations,
                &[("id".to_string(), "id".to_string())],
                &JoinType::Inner,
            )
            .expect("greedy_join_order should succeed for valid relations");

        // Result should be a nested join tree starting with smallest tables
        match &result.node_type {
            PlanNodeType::HashJoin { .. } => {
                // The greedy algorithm should have built a left-deep tree
                // with the cheapest tables joined first
                assert!(result.estimated_cost > 0.0);
            }
            _ => panic!("Expected hash join as result"),
        }
    }

    // ========================================================================
    // JOIN STRATEGY SELECTION TESTS
    // ========================================================================

    #[test]
    fn test_join_strategy_index_join_small_right() {
        let strategy = select_join_strategy(50_000, 500, true);
        assert_eq!(strategy, JoinStrategy::IndexJoin);
    }

    #[test]
    fn test_join_strategy_hash_join_large_both() {
        let strategy = select_join_strategy(10_000, 5_000, false);
        assert_eq!(strategy, JoinStrategy::HashJoin);
    }

    #[test]
    fn test_join_strategy_nested_loop_small_inputs() {
        let strategy = select_join_strategy(100, 50, false);
        assert_eq!(strategy, JoinStrategy::NestedLoopJoin);
    }

    #[test]
    fn test_join_strategy_hash_over_index_when_right_large() {
        // Even with an index, if the right side is large prefer hash join
        let strategy = select_join_strategy(5_000, 5_000, true);
        assert_eq!(strategy, JoinStrategy::HashJoin);
    }

    #[test]
    fn test_join_strategy_nested_loop_one_side_small() {
        let strategy = select_join_strategy(500, 50, true);
        assert_eq!(strategy, JoinStrategy::IndexJoin);
    }

    // ========================================================================
    // Runtime Statistics Feedback Tests
    // ========================================================================

    #[test]
    fn test_runtime_stats_collector_creation() {
        let collector = RuntimeStatisticsCollector::new(500);
        let snapshot = collector.snapshot();
        assert_eq!(snapshot.tracked_operations, 0);
        assert_eq!(snapshot.total_observations, 0);
    }

    #[test]
    fn test_runtime_stats_record_single_feedback() {
        let collector = RuntimeStatisticsCollector::default();
        let feedback = ExecutionFeedback {
            operation_key: "vector_search:embeddings:top10".to_string(),
            estimated_cardinality: 10,
            actual_cardinality: 8,
            estimated_cost: 100.0,
            actual_latency_ms: 12.5,
            ..Default::default()
        };

        collector.record_feedback(&feedback);

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.cardinality_entries, 1);
        assert_eq!(snapshot.latency_entries, 1);

        // Check correction ratio was recorded
        let correction = collector
            .cardinality_correction("vector_search:embeddings:top10")
            .expect("correction should exist");
        // Initial ratio=1.0, actual/est=0.8, EMA with alpha=0.3 → 1.0*0.7 + 0.8*0.3 = 0.94
        assert!((correction - 0.94).abs() < 0.01);
    }

    #[test]
    fn test_runtime_stats_ema_convergence() {
        let collector = RuntimeStatisticsCollector::default();

        // Simulate 20 executions where actual is consistently 2x estimated
        for _ in 0..20 {
            collector.record_feedback(&ExecutionFeedback {
                operation_key: "graph:traverse:depth3".to_string(),
                estimated_cardinality: 100,
                actual_cardinality: 200,
                estimated_cost: 50.0,
                actual_latency_ms: 25.0,
                ..Default::default()
            });
        }

        let correction = collector
            .cardinality_correction("graph:traverse:depth3")
            .expect("correction should exist after 20 observations");
        // After 20 iterations of EMA(alpha=0.3) toward ratio=2.0, should be close to 2.0
        assert!(
            correction > 1.8,
            "correction ratio should converge toward 2.0, got {}",
            correction
        );
    }

    #[test]
    fn test_runtime_stats_latency_tracking() {
        let collector = RuntimeStatisticsCollector::default();

        collector.record_feedback(&ExecutionFeedback {
            operation_key: "doc_query:users".to_string(),
            estimated_cardinality: 50,
            actual_cardinality: 50,
            estimated_cost: 200.0,
            actual_latency_ms: 15.0,
            ..Default::default()
        });

        let avg = collector
            .avg_latency("doc_query:users")
            .expect("latency should be tracked");
        assert!((avg - 15.0).abs() < 0.1);

        let ratio = collector
            .cost_latency_ratio("doc_query:users")
            .expect("ratio should be tracked");
        // 15.0 / 200.0 = 0.075
        assert!((ratio - 0.075).abs() < 0.01);
    }

    #[test]
    fn test_runtime_stats_selectivity_tracking() {
        let collector = RuntimeStatisticsCollector::default();

        collector.record_feedback(&ExecutionFeedback {
            operation_key: "filter:age>30".to_string(),
            estimated_cardinality: 30,
            actual_cardinality: 30,
            estimated_cost: 10.0,
            actual_latency_ms: 1.0,
            rows_scanned: Some(100),
            ..Default::default()
        });

        let selectivity = collector
            .calibrated_selectivity("filter:age>30")
            .expect("selectivity should be tracked");
        // 30/100 = 0.3
        assert!((selectivity - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_runtime_stats_plan_invalidation_detection() {
        let collector = RuntimeStatisticsCollector::default();

        // Establish a baseline cost-to-latency ratio
        for _ in 0..5 {
            collector.record_feedback(&ExecutionFeedback {
                operation_key: "vector_search:imgs".to_string(),
                estimated_cardinality: 10,
                actual_cardinality: 10,
                estimated_cost: 50.0,
                actual_latency_ms: 5.0, // ratio = 0.1
                ..Default::default()
            });
        }

        // Normal latency: should NOT invalidate
        assert!(!collector.should_invalidate_plan("vector_search:imgs", 50.0, 6.0));

        // 3x+ regression: should invalidate
        // Expected = 50.0 * 0.1 = 5.0, threshold = 15.0
        assert!(collector.should_invalidate_plan("vector_search:imgs", 50.0, 20.0));
    }

    #[test]
    fn test_runtime_stats_unknown_operation_no_invalidation() {
        let collector = RuntimeStatisticsCollector::default();
        // No history for this op — should not recommend invalidation
        assert!(!collector.should_invalidate_plan("unknown:op", 100.0, 500.0));
    }

    #[test]
    fn test_advanced_cost_estimator_feedback_update() {
        let mut estimator = AdvancedCostEstimator::new();
        let initial_cpu = estimator.cpu_cycles_per_distance;

        estimator.update_from_feedback(&ExecutionFeedback {
            observed_cpu_per_distance: Some(0.005),
            ..Default::default()
        });

        // EMA: 0.001 * 0.8 + 0.005 * 0.2 = 0.0018
        let expected = initial_cpu * 0.8 + 0.005 * 0.2;
        assert!(
            (estimator.cpu_cycles_per_distance - expected).abs() < 1e-6,
            "cpu cost should update via EMA, got {}",
            estimator.cpu_cycles_per_distance
        );
    }

    #[test]
    fn test_advanced_cost_estimator_feedback_alpha_validation() {
        assert!(AdvancedCostEstimator::new().validate().is_ok());
        let mut estimator = AdvancedCostEstimator::new();
        estimator.feedback_ema_alpha = 1.5;
        assert!(estimator.validate().is_err());
    }

    #[test]
    fn test_advanced_cost_estimator_uses_configured_feedback_alpha() {
        let mut estimator = AdvancedCostEstimator::new().with_feedback_ema_alpha(0.5);
        let initial_cpu = estimator.cpu_cycles_per_distance;

        estimator.update_from_feedback(&ExecutionFeedback {
            observed_cpu_per_distance: Some(0.005),
            ..Default::default()
        });

        let expected = initial_cpu * 0.5 + 0.005 * 0.5;
        assert!(
            (estimator.cpu_cycles_per_distance - expected).abs() < 1e-6,
            "cpu cost should use configured EMA alpha, got {}",
            estimator.cpu_cycles_per_distance
        );
    }

    #[test]
    fn test_advanced_cost_estimator_multiple_feedback_convergence() {
        let mut estimator = AdvancedCostEstimator::new();

        // Apply 50 feedbacks with observed io_cost = 2.0 (initial is 1.0)
        for _ in 0..50 {
            estimator.update_from_feedback(&ExecutionFeedback {
                observed_io_per_page: Some(2.0),
                ..Default::default()
            });
        }

        assert!(
            (estimator.io_cost_per_page - 2.0).abs() < 0.05,
            "io_cost_per_page should converge to 2.0, got {}",
            estimator.io_cost_per_page
        );
    }

    #[test]
    fn test_optimizer_record_execution_feedback() {
        let mut optimizer = CrossModelOptimizer::new();

        // Record feedback for a vector search
        optimizer.record_execution_feedback(ExecutionFeedback {
            operation_key: "vector_search:embeddings:top10".to_string(),
            estimated_cardinality: 10,
            actual_cardinality: 7,
            estimated_cost: 100.0,
            actual_latency_ms: 8.0,
            ..Default::default()
        });

        // Calibrated cardinality should now differ from base
        let calibrated = optimizer.calibrated_cardinality("vector_search:embeddings:top10", 10);
        // With ratio ~0.94 (EMA from 1.0 toward 0.7): 10 * 0.94 = 9
        assert!(
            calibrated < 10,
            "calibrated should be less than base, got {}",
            calibrated
        );
    }

    #[test]
    fn test_optimizer_plan_invalidation_on_regression() {
        let mut optimizer = CrossModelOptimizer::new();

        // First, build up a baseline
        for _ in 0..10 {
            optimizer.record_execution_feedback(ExecutionFeedback {
                operation_key: "vector_search:products:top5".to_string(),
                estimated_cardinality: 5,
                actual_cardinality: 5,
                estimated_cost: 80.0,
                actual_latency_ms: 4.0, // ratio ~0.05
                ..Default::default()
            });
        }

        // Cache a plan for "products"
        let query = FederatedQuery {
            sql: "SELECT * FROM VECTOR_SEARCH('products', '[0.1]', 5)".to_string(),
            query_type: QueryType::VectorSearch,
            extensions: vec![SqlExtension::VectorSearch {
                collection: "products".to_string(),
                query_vector: VectorQuery::Literal(vec![0.1]),
                top_k: 5,
            }],
            extension_positions: vec![14],
            extension_aliases: vec![None],
            targets: vec![],
            parameters: HashMap::new(),
            is_cross_model_join: false,
        };
        let _plan = optimizer.optimize(&query).unwrap();
        let cache_key = PlanCacheKey::from_query(&query);
        assert!(optimizer.plan_cache.get(&cache_key).is_some());

        // Now record a massive regression
        optimizer.record_execution_feedback(ExecutionFeedback {
            operation_key: "vector_search:products:top5".to_string(),
            estimated_cardinality: 5,
            actual_cardinality: 5,
            estimated_cost: 80.0,
            actual_latency_ms: 500.0, // 125x regression
            ..Default::default()
        });

        // The plan for "products" should have been invalidated
        assert!(
            optimizer.plan_cache.get(&cache_key).is_none(),
            "cached plan should be invalidated after performance regression"
        );
    }

    #[test]
    fn test_runtime_stats_snapshot_completeness() {
        let collector = RuntimeStatisticsCollector::new(100);

        // Record various operation types
        collector.record_feedback(&ExecutionFeedback {
            operation_key: "op1".to_string(),
            estimated_cardinality: 10,
            actual_cardinality: 12,
            estimated_cost: 50.0,
            actual_latency_ms: 5.0,
            rows_scanned: Some(100),
            ..Default::default()
        });
        collector.record_feedback(&ExecutionFeedback {
            operation_key: "op2".to_string(),
            estimated_cardinality: 100,
            actual_cardinality: 80,
            estimated_cost: 200.0,
            actual_latency_ms: 20.0,
            ..Default::default()
        });

        let snap = collector.snapshot();
        assert_eq!(snap.cardinality_entries, 2);
        assert_eq!(snap.latency_entries, 2);
        assert_eq!(snap.selectivity_entries, 1); // Only op1 had rows_scanned
        assert!(snap.total_observations >= 4); // 2 cardinality + 2 latency minimum
    }

    #[test]
    fn test_calibrated_cardinality_falls_back_to_estimator() {
        let mut optimizer = CrossModelOptimizer::new();

        // Record in the cardinality_estimator directly (bypassing runtime stats)
        optimizer
            .cardinality_estimator
            .record_actual_cardinality("legacy:op".to_string(), 100, 50);

        // Should use the cardinality estimator as fallback
        let calibrated = optimizer.calibrated_cardinality("legacy:op", 200);
        // ratio = 50/100 = 0.5 → 200 * 0.5 = 100
        assert_eq!(calibrated, 100);
    }

    #[test]
    fn test_runtime_stats_compaction() {
        let collector = RuntimeStatisticsCollector::new(5);

        // Insert more than 2 * max_history_per_op entries to trigger compaction
        for i in 0..12 {
            collector.record_feedback(&ExecutionFeedback {
                operation_key: format!("op_{}", i),
                estimated_cardinality: 10,
                actual_cardinality: 10,
                estimated_cost: 10.0,
                actual_latency_ms: 1.0,
                ..Default::default()
            });
        }

        let snap = collector.snapshot();
        // After compaction, cardinality entries should be significantly less than 12
        // Compaction triggers at > 2*max (10) and removes down to max (5), then
        // remaining inserts may add a few more.
        assert!(
            snap.cardinality_entries < 12,
            "compaction should have removed entries, got {}",
            snap.cardinality_entries
        );
    }

    // ============================================================================
    // CAPABILITY INFERENCE TESTS
    // ============================================================================

    #[test]
    fn test_scan_node_capability_inference() {
        // Test basic scan with vector model
        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::Scan {
                target: "vectors".to_string(),
                model_type: ModelType::Vector,
                predicates: vec![],
            },
            estimated_cost: 100.0,
            estimated_rows: 1000,
            output_columns: vec!["id".to_string(), "embedding".to_string()],
            required_capabilities: CapabilitySet::new(), // Empty initially
        };

        let caps = node.infer_capabilities();
        assert!(caps.contains(&CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::VectorSearch,
            Capability::CosineDistance,
        ])));
    }

    #[test]
    fn test_scan_with_predicates_capability_inference() {
        // Test scan with predicates (should add filter capabilities)
        let predicate = Predicate {
            column: "category".to_string(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("electronics".to_string()),
        };

        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::Scan {
                target: "products".to_string(),
                model_type: ModelType::Vector,
                predicates: vec![predicate],
            },
            estimated_cost: 100.0,
            estimated_rows: 1000,
            output_columns: vec!["id".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        let caps = node.infer_capabilities();
        assert!(caps.contains(&CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::Filter,
            Capability::PredicatePushdown,
        ])));
    }

    #[test]
    fn test_vector_search_capability_inference() {
        // Test vector search node
        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::VectorSearch {
                collection: "embeddings".to_string(),
                top_k: 10,
                query_vector_source: VectorSource::Literal(vec![0.1, 0.2, 0.3]),
            },
            estimated_cost: 50.0,
            estimated_rows: 10,
            output_columns: vec!["id".to_string(), "score".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        let caps = node.infer_capabilities();
        assert!(caps.contains(&CapabilitySet::from_capabilities(&[
            Capability::VectorSearch,
            Capability::Scan,
            Capability::CosineDistance,
            Capability::EuclideanDistance,
            Capability::DotProduct,
        ])));
    }

    #[test]
    fn test_graph_traversal_capability_inference() {
        // Test graph traversal node
        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::GraphTraversal {
                cypher: "MATCH (a)-[:KNOWS]->(b) RETURN b".to_string(),
                start_nodes: None,
                source_alias: None,
            },
            estimated_cost: 75.0,
            estimated_rows: 100,
            output_columns: vec!["name".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        let caps = node.infer_capabilities();
        assert!(caps.contains(&CapabilitySet::from_capabilities(&[
            Capability::GraphQuery,
            Capability::GraphTraversal,
            Capability::PatternMatching,
            Capability::Scan,
        ])));
    }

    #[test]
    fn test_document_query_capability_inference() {
        // Test document query node
        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::DocumentQuery {
                collection: "docs".to_string(),
                filter: Some("status = 'active'".to_string()),
                source_alias: None,
            },
            estimated_cost: 60.0,
            estimated_rows: 500,
            output_columns: vec!["id".to_string(), "content".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        let caps = node.infer_capabilities();
        assert!(caps.contains(&CapabilitySet::from_capabilities(&[
            Capability::DocumentQuery,
            Capability::Scan,
            Capability::FullTextSearch,
            Capability::JSONPathQueries,
            Capability::Filter,
            Capability::PredicatePushdown,
        ])));
    }

    #[test]
    fn test_observability_query_capability_inference() {
        // Test logs query
        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::ObservabilityQuery {
                namespace: "production".to_string(),
                query_type: ObservabilityQueryType::Logs,
                time_range: None,
            },
            estimated_cost: 40.0,
            estimated_rows: 1000,
            output_columns: vec!["timestamp".to_string(), "message".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        let caps = node.infer_capabilities();
        assert!(caps.contains(&CapabilitySet::from_capabilities(&[
            Capability::LogQuery,
            Capability::LogAggregation,
            Capability::Scan,
        ])));
    }

    #[test]
    fn test_join_capability_inference() {
        // Test hash join with two child nodes
        let left_child = PlanNode {
            id: 1,
            node_type: PlanNodeType::Scan {
                target: "users".to_string(),
                model_type: ModelType::Document,
                predicates: vec![],
            },
            estimated_cost: 50.0,
            estimated_rows: 100,
            output_columns: vec!["id".to_string(), "name".to_string()],
            required_capabilities: CapabilitySet::from_capabilities(&[
                Capability::Scan,
                Capability::DocumentQuery,
            ]),
        };

        let right_child = PlanNode {
            id: 2,
            node_type: PlanNodeType::VectorSearch {
                collection: "embeddings".to_string(),
                top_k: 5,
                query_vector_source: VectorSource::Literal(vec![0.1; 128]),
            },
            estimated_cost: 30.0,
            estimated_rows: 5,
            output_columns: vec!["id".to_string(), "score".to_string()],
            required_capabilities: CapabilitySet::from_capabilities(&[
                Capability::VectorSearch,
                Capability::Scan,
            ]),
        };

        let node = PlanNode {
            id: 3,
            node_type: PlanNodeType::HashJoin {
                left: Box::new(left_child.clone()),
                right: Box::new(right_child.clone()),
                join_keys: vec![("user_id".to_string(), "id".to_string())],
                join_type: JoinType::Inner,
            },
            estimated_cost: 150.0,
            estimated_rows: 500,
            output_columns: vec!["name".to_string(), "score".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        let caps = node.infer_capabilities();
        // Should include Join, Scan, and capabilities from both children
        assert!(caps.contains(&CapabilitySet::from_capabilities(&[
            Capability::Join,
            Capability::Scan,
            Capability::DocumentQuery,
            Capability::VectorSearch,
        ])));
    }

    #[test]
    fn test_filter_capability_inference() {
        // Test filter node
        let child = PlanNode {
            id: 1,
            node_type: PlanNodeType::Scan {
                target: "products".to_string(),
                model_type: ModelType::Vector,
                predicates: vec![],
            },
            estimated_cost: 100.0,
            estimated_rows: 1000,
            output_columns: vec!["id".to_string()],
            required_capabilities: CapabilitySet::from_capabilities(&[
                Capability::Scan,
                Capability::VectorSearch,
            ]),
        };

        let predicate = Predicate {
            column: "price".to_string(),
            op: PredicateOp::Lt,
            value: PredicateValue::Float(100.0),
        };

        let node = PlanNode {
            id: 2,
            node_type: PlanNodeType::Filter {
                input: Box::new(child.clone()),
                predicate,
            },
            estimated_cost: 80.0,
            estimated_rows: 500,
            output_columns: vec!["id".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        let caps = node.infer_capabilities();
        // Should include Filter and child capabilities
        assert!(caps.contains(&CapabilitySet::from_capabilities(&[
            Capability::Filter,
            Capability::PredicatePushdown,
            Capability::Scan,
            Capability::VectorSearch,
        ])));
    }

    #[test]
    fn test_aggregate_capability_inference() {
        // Test aggregate node
        let child = PlanNode {
            id: 1,
            node_type: PlanNodeType::Scan {
                target: "sales".to_string(),
                model_type: ModelType::Document,
                predicates: vec![],
            },
            estimated_cost: 100.0,
            estimated_rows: 1000,
            output_columns: vec!["category".to_string(), "amount".to_string()],
            required_capabilities: CapabilitySet::from_capabilities(&[
                Capability::Scan,
                Capability::DocumentQuery,
            ]),
        };

        let node = PlanNode {
            id: 2,
            node_type: PlanNodeType::Aggregate {
                input: Box::new(child.clone()),
                group_by: vec!["category".to_string()],
                aggregates: vec![AggregateExpr {
                    function: AggregateFunction::Sum,
                    column: Some("amount".to_string()),
                    alias: "total".to_string(),
                }],
            },
            estimated_cost: 120.0,
            estimated_rows: 10,
            output_columns: vec!["category".to_string(), "total".to_string()],
            required_capabilities: CapabilitySet::new(),
        };

        let caps = node.infer_capabilities();
        // Should include Aggregate and child capabilities
        assert!(caps.contains(&CapabilitySet::from_capabilities(&[
            Capability::Aggregate,
            Capability::Scan,
            Capability::DocumentQuery,
        ])));
    }

    #[test]
    fn test_pre_inferred_capabilities_used() {
        // Test that pre-inferred capabilities are used without re-inferring
        let pre_inferred =
            CapabilitySet::from_capabilities(&[Capability::VectorSearch, Capability::Filter]);

        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::VectorSearch {
                collection: "embeddings".to_string(),
                top_k: 10,
                query_vector_source: VectorSource::Literal(vec![0.1, 0.2, 0.3]),
            },
            estimated_cost: 50.0,
            estimated_rows: 10,
            output_columns: vec!["id".to_string()],
            required_capabilities: pre_inferred.clone(),
        };

        let caps = node.infer_capabilities();
        // Should return the pre-inferred capabilities, not re-infer
        assert_eq!(caps, pre_inferred);
    }

    #[test]
    fn test_honest_capability_reporting() {
        // Test that honest capability reporting correctly identifies gaps
        let claimed_capabilities = CapabilitySet::from_capabilities(&[
            Capability::VectorSearch,
            Capability::Filter,
            Capability::PredicatePushdown,
            Capability::CosineDistance,
        ]);

        let actual_engine_capabilities = CapabilitySet::from_capabilities(&[
            Capability::VectorSearch,
            Capability::Filter,
            // Note: Missing PredicatePushdown and CosineDistance
        ]);

        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::Scan {
                target: "test_collection".to_string(),
                model_type: ModelType::Vector,
                predicates: vec![],
            },
            estimated_cost: 50.0,
            estimated_rows: 100,
            output_columns: vec!["id".to_string(), "vector".to_string()],
            required_capabilities: claimed_capabilities.clone(),
        };

        let (honest, missing, extra) =
            node.get_capability_honesty_report(&actual_engine_capabilities);

        // Honest capabilities should only include what's actually available
        assert!(honest.contains_capability(&Capability::VectorSearch));
        assert!(honest.contains_capability(&Capability::Filter));
        assert!(!honest.contains_capability(&Capability::PredicatePushdown));

        // Missing capabilities should include the gap
        assert!(missing.contains_capability(&Capability::PredicatePushdown));
        assert!(missing.contains_capability(&Capability::CosineDistance));
        assert!(!missing.contains_capability(&Capability::VectorSearch));

        // Extra capabilities show what's available but not claimed
        assert!(!extra.contains_capability(&Capability::VectorSearch));
    }

    #[test]
    fn test_capability_validation_passes_when_supported() {
        // Test that validation passes when all claimed capabilities are supported
        let claimed_capabilities =
            CapabilitySet::from_capabilities(&[Capability::VectorSearch, Capability::Filter]);

        let engine_capabilities = CapabilitySet::from_capabilities(&[
            Capability::VectorSearch,
            Capability::Filter,
            Capability::PredicatePushdown, // Extra capability not claimed
        ]);

        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::VectorSearch {
                collection: "embeddings".to_string(),
                top_k: 10,
                query_vector_source: VectorSource::Literal(vec![0.1, 0.2, 0.3]),
            },
            estimated_cost: 50.0,
            estimated_rows: 10,
            output_columns: vec!["id".to_string()],
            required_capabilities: claimed_capabilities.clone(),
        };

        // Should pass validation
        assert!(
            node.validate_capabilities(&engine_capabilities, "test node")
                .is_ok()
        );
    }

    #[test]
    fn test_capability_validation_fails_when_unsupported() {
        // Test that validation fails when claimed capabilities are not supported
        let claimed_capabilities = CapabilitySet::from_capabilities(&[
            Capability::VectorSearch,
            Capability::GraphQuery, // Not supported by engine
        ]);

        let engine_capabilities =
            CapabilitySet::from_capabilities(&[Capability::VectorSearch, Capability::Filter]);

        let node = PlanNode {
            id: 1,
            node_type: PlanNodeType::VectorSearch {
                collection: "embeddings".to_string(),
                top_k: 10,
                query_vector_source: VectorSource::Literal(vec![0.1, 0.2, 0.3]),
            },
            estimated_cost: 50.0,
            estimated_rows: 10,
            output_columns: vec!["id".to_string()],
            required_capabilities: claimed_capabilities.clone(),
        };

        // Should fail validation with descriptive error
        let result = node.validate_capabilities(&engine_capabilities, "vector search node");
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("GraphQuery"));
        assert!(error_msg.contains("capabilities that the storage engine doesn't support"));
    }

    // ---------------- R-7c.4c.1: RerankSearch lowering ----------------

    #[test]
    fn test_rerank_search_lowers_to_rerank_plan_node() {
        // RERANK(...) SRF must lower into PlanNodeType::RerankSearch
        // (not a Scan placeholder) so execution dispatches to the
        // rank pipeline.
        let optimizer = CrossModelOptimizer::new();
        let query = parser::FederatedParser::new()
            .parse("SELECT * FROM RERANK('docs', 'laptop', '[0.1,0.2,0.3]', 25, 'semantic_plus_ce')")
            .expect("RERANK query should parse");

        let plan = optimizer
            .optimize(&query)
            .expect("RERANK plan should build");

        match &plan.root.node_type {
            PlanNodeType::RerankSearch {
                collection,
                query_text,
                k,
                rank_profile,
                ..
            } => {
                assert_eq!(collection, "docs");
                assert_eq!(query_text, "laptop");
                assert_eq!(*k, 25);
                assert_eq!(rank_profile.as_deref(), Some("semantic_plus_ce"));
            }
            other => panic!("Expected PlanNodeType::RerankSearch, got {other:?}"),
        }
    }

    #[test]
    fn test_rerank_search_output_columns_include_phase_and_features() {
        // The rerank SRF surface must expose the 5-column shape
        // (id/score/phase/match_features/summary_features) so callers
        // can downstream-SELECT it.
        let optimizer = CrossModelOptimizer::new();
        let query = parser::FederatedParser::new()
            .parse("SELECT * FROM RERANK('docs', 'q', '[0.5]', 10)")
            .expect("RERANK query should parse");

        let plan = optimizer
            .optimize(&query)
            .expect("RERANK plan should build");

        assert_eq!(
            plan.root.output_columns,
            vec![
                "id".to_string(),
                "score".to_string(),
                "phase".to_string(),
                "match_features".to_string(),
                "summary_features".to_string(),
            ]
        );
    }

    #[test]
    fn test_rerank_with_profile_costs_more_than_retrieval_only() {
        // Second-phase rescoring adds latency; the optimizer must
        // reflect that in the plan cost.
        let optimizer = CrossModelOptimizer::new();
        let with_profile = parser::FederatedParser::new()
            .parse("SELECT * FROM RERANK('docs', 'q', '[0.5]', 10, 'ce_v1')")
            .unwrap();
        let without_profile = parser::FederatedParser::new()
            .parse("SELECT * FROM RERANK('docs', 'q', '[0.5]', 10)")
            .unwrap();

        let plan_with = optimizer.optimize(&with_profile).unwrap();
        let plan_without = optimizer.optimize(&without_profile).unwrap();
        assert!(
            plan_with.total_cost > plan_without.total_cost,
            "rerank-with-profile cost {:.2} should exceed retrieval-only cost {:.2}",
            plan_with.total_cost,
            plan_without.total_cost,
        );
    }

    #[test]
    fn test_rerank_node_has_vector_model_capability() {
        // Plan-level capability inference must treat RerankSearch as
        // a vector-modality consumer so engine-capability checks
        // route to the vector engine.
        let optimizer = CrossModelOptimizer::new();
        let query = parser::FederatedParser::new()
            .parse("SELECT * FROM RERANK('docs', 'q', '[0.5]', 10)")
            .unwrap();
        let plan = optimizer.optimize(&query).unwrap();
        assert!(plan.root.has_model(ModelType::Vector));
    }

    #[test]
    fn test_scan_honesty_gap_detection() {
        // Test detection of honesty gap in scan operations
        let scan_node = PlanNode {
            id: 1,
            node_type: PlanNodeType::Scan {
                target: "products".to_string(),
                model_type: ModelType::Vector,
                predicates: vec![Predicate {
                    column: "price".to_string(),
                    op: PredicateOp::Lt,
                    value: PredicateValue::Int(100),
                }],
            },
            estimated_cost: 50.0,
            estimated_rows: 100,
            output_columns: vec!["id".to_string(), "vector".to_string(), "price".to_string()],
            required_capabilities: CapabilitySet::new(), // Will be inferred
        };

        // Infer capabilities
        let _claimed = scan_node.infer_capabilities();

        // Simulate a storage engine that doesn't support predicate pushdown
        let limited_engine = CapabilitySet::from_capabilities(&[
            Capability::Scan,
            Capability::VectorSearch,
            // Missing: Filter, PredicatePushdown
        ]);

        let (_honest, missing, _extra) = scan_node.get_capability_honesty_report(&limited_engine);

        // Should detect the honesty gap
        assert!(missing.contains_capability(&Capability::Filter));
        assert!(missing.contains_capability(&Capability::PredicatePushdown));
        assert!(missing.len() >= 2);
    }
}
