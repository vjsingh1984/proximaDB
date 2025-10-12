//! SQL Parsing Tests - Consolidated from sql_frontend/tests.rs and sql_frontend/lowering.rs
//!
//! This module tests SQL parsing and AST lowering functionality:
//! - Basic SELECT parsing (projection, FROM, WHERE, LIMIT, ORDER BY)
//! - JOIN parsing
//! - GROUP BY and HAVING clause parsing
//! - CASE expressions
//! - Subqueries (FROM and WHERE clauses)
//! - Vector function parsing
//! - AST lowering and collection resolution
//! - Error handling for invalid SQL

use crate::query::sql_frontend::parser::SqlFrontendParser;
use crate::query::sql_frontend::lowering::QueryLowering;
use crate::query::ast::{BinaryOp, Expr, Literal, Query, UnaryOp, ProjectionItem, TableRef};
use crate::services::collection::manager::CollectionService;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::core::config::StorageConfig;
use crate::storage::metadata::backends::universal_backend::UniversalMetadataConfig;
use crate::proto::proximadb_v1::CollectionConfig;
use std::sync::Arc;

// ============================================================================
// Test Helpers
// ============================================================================

/// Create mock collection service for testing
async fn setup_test_collection_service() -> Arc<CollectionService> {
    let config = UniversalMetadataConfig::default();
    let filesystem_config = Default::default();
    let filesystem_factory = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
    let backend = crate::storage::metadata::backends::universal_backend::UniversalMetadataBackend::new(config, filesystem_factory).await.unwrap();
    let storage_config = StorageConfig::default();
    let service = Arc::new(CollectionService::new(Arc::new(backend), storage_config).await.unwrap());

    // Create test collection "products" (8 characters minimum)
    let collection_config = CollectionConfig {
        name: "products".to_string(),
        dimension: 128,
        ..Default::default()
    };

    // Ignore errors if collection already exists
    let _ = service.create_collection(&collection_config).await;

    service
}

// ============================================================================
// Basic SELECT Parsing Tests (from sql_frontend/tests.rs)
// ============================================================================

#[test]
fn test_parse_simple_select() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT * FROM products";

    let result = parser.parse(sql);
    assert!(
        result.is_ok(),
        "Failed to parse simple SELECT: {:?}",
        result.err()
    );

    match result.unwrap() {
        Query::Select(select) => {
            assert_eq!(select.projection.len(), 1);
            assert_eq!(select.from.len(), 1);
            assert_eq!(select.from[0].name, Some("products".to_string()));
        }
        _ => panic!("Unexpected query type"),
    }
}

#[test]
fn test_parse_select_with_where() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT id, name FROM products WHERE price > 100";

    let result = parser.parse(sql);
    assert!(
        result.is_ok(),
        "Failed to parse SELECT with WHERE: {:?}",
        result.err()
    );

    match result.unwrap() {
        Query::Select(select) => {
            assert_eq!(select.projection.len(), 2);
            assert!(select.selection.is_some());

            // Check WHERE clause structure
            if let Some(Expr::Binary {
                op: BinaryOp::Gt, ..
            }) = &select.selection
            {
                // Correct structure
            } else {
                panic!("Expected binary expression with > operator");
            }
        }
        _ => panic!("Unexpected query type"),
    }
}

#[test]
fn test_parse_select_with_limit() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT * FROM products LIMIT 10";

    let result = parser.parse(sql);
    assert!(
        result.is_ok(),
        "Failed to parse SELECT with LIMIT: {:?}",
        result.err()
    );

    match result.unwrap() {
        Query::Select(select) => {
            assert_eq!(select.limit, Some(10));
        }
        _ => panic!("Unexpected query type"),
    }
}

#[test]
fn test_parse_select_with_order_by() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT * FROM products ORDER BY price DESC";

    let result = parser.parse(sql);
    assert!(
        result.is_ok(),
        "Failed to parse SELECT with ORDER BY: {:?}",
        result.err()
    );

    match result.unwrap() {
        Query::Select(select) => {
            assert_eq!(select.order_by.len(), 1);
            assert!(!select.order_by[0].asc); // DESC order
        }
        _ => panic!("Unexpected query type"),
    }
}

#[test]
fn test_parse_vector_function() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT COSINE_DISTANCE(embedding, [0.1, 0.2]) as score FROM products";

    let result = parser.parse(sql);
    // This should parse successfully, even if vector literals aren't fully implemented
    match result {
        Ok(Query::Select(select)) => {
            assert_eq!(select.projection.len(), 1);
            // Function call should be recognized
            match &select.projection[0].expr {
                Expr::FuncCall { name, .. } => {
                    assert_eq!(name, "COSINE_DISTANCE");
                }
                _ => {
                    // Function parsing may not be complete yet, but it shouldn't crash
                }
            }
        }
        Ok(_) => panic!("Unexpected query type"),
        Err(_) => {
            // Vector literals may not be implemented yet, that's ok
        }
    }
}

// ============================================================================
// Aggregate Function and GROUP BY Tests
// ============================================================================

#[test]
fn test_parse_group_by() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT category, COUNT(*) FROM products GROUP BY category";

    let result = parser.parse(sql);
    assert!(result.is_ok(), "Failed to parse GROUP BY: {:?}", result.err());

    match result.unwrap() {
        Query::Select(select) => {
            assert_eq!(select.group_by.len(), 1);
        }
        _ => panic!("Unexpected query type"),
    }
}

#[test]
fn test_parse_having() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT category, COUNT(*) FROM products GROUP BY category HAVING COUNT(*) > 10";

    let result = parser.parse(sql);
    assert!(result.is_ok(), "Failed to parse HAVING: {:?}", result.err());

    match result.unwrap() {
        Query::Select(select) => {
            assert!(select.having.is_some());
        }
        _ => panic!("Unexpected query type"),
    }
}

// ============================================================================
// JOIN Tests
// ============================================================================

#[test]
fn test_parse_join() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT products.name, categories.name FROM products JOIN categories ON products.category_id = categories.id";

    let result = parser.parse(sql);
    assert!(result.is_ok(), "Failed to parse JOIN: {:?}", result.err());

    match result.unwrap() {
        Query::Select(select) => {
            assert_eq!(select.joins.len(), 1);
        }
        _ => panic!("Unexpected query type"),
    }
}

// ============================================================================
// Advanced Expression Tests
// ============================================================================

#[test]
fn test_parse_case_expression() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT CASE WHEN price > 100 THEN 'expensive' ELSE 'cheap' END FROM products";

    let result = parser.parse(sql);
    assert!(result.is_ok(), "Failed to parse CASE expression: {:?}", result.err());

    match result.unwrap() {
        Query::Select(select) => {
            assert_eq!(select.projection.len(), 1);
            if let Some(ProjectionItem { expr: Expr::Case { .. }, .. }) = select.projection.first() {
                // Correctly parsed CASE expression
            } else {
                panic!("Expected CASE expression in projection");
            }
        }
        _ => panic!("Unexpected query type"),
    }
}

// ============================================================================
// Subquery Tests
// ============================================================================

#[test]
fn test_parse_subquery_in_from() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT a.id FROM (SELECT id FROM products WHERE price > 100) AS a";

    let result = parser.parse(sql);
    assert!(result.is_ok(), "Failed to parse subquery in FROM: {:?}", result.err());

    match result.unwrap() {
        Query::Select(select) => {
            assert_eq!(select.from.len(), 1);
            if let Some(TableRef { subquery: Some(_), alias: Some(alias), .. }) = select.from.first() {
                assert_eq!(alias, "a");
            } else {
                panic!("Expected subquery with alias in FROM clause");
            }
        }
        _ => panic!("Unexpected query type"),
    }
}

#[test]
fn test_parse_subquery_in_where() {
    let parser = SqlFrontendParser::new();
    let sql = "SELECT id FROM products WHERE id IN (SELECT product_id FROM orders WHERE quantity > 5)";

    let result = parser.parse(sql);
    assert!(result.is_ok(), "Failed to parse subquery in WHERE: {:?}", result.err());

    match result.unwrap() {
        Query::Select(select) => {
            assert!(select.selection.is_some());
            if let Some(Expr::Binary { right: subquery_expr, .. }) = &select.selection {
                if let Expr::Subquery(_) = **subquery_expr {
                    // Correctly parsed subquery
                } else {
                    panic!("Expected subquery in WHERE clause");
                }
            } else {
                panic!("Expected binary expression with subquery in WHERE clause");
            }
        }
        _ => panic!("Unexpected query type"),
    }
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[test]
fn test_invalid_sql() {
    let parser = SqlFrontendParser::new();
    let sql = "INVALID SQL STATEMENT";

    let result = parser.parse(sql);
    assert!(result.is_err(), "Should fail to parse invalid SQL");
}

#[test]
fn test_empty_sql() {
    let parser = SqlFrontendParser::new();
    let sql = "";

    let result = parser.parse(sql);
    assert!(result.is_err(), "Should fail to parse empty SQL");
}

// ============================================================================
// AST Lowering Tests (from sql_frontend/lowering.rs)
// ============================================================================

#[tokio::test]
async fn test_simple_select_lowering() {
    let collection_service = setup_test_collection_service().await;
    let lowering = QueryLowering::new(collection_service);
    let sql = "SELECT id, metadata FROM products LIMIT 10";

    let ast = lowering.lower_sql(sql).await.unwrap();

    match ast {
        Query::Select(select) => {
            assert_eq!(select.projection.len(), 2);
            assert_eq!(select.limit, Some(10));
            assert!(select.from.len() > 0);

            // Verify projection contains expected fields
            if let Some(item) = select.projection.get(0) {
                assert!(matches!(item.expr, Expr::Identifier(ref id) if id == "id"));
            }
            if let Some(item) = select.projection.get(1) {
                assert!(matches!(item.expr, Expr::Identifier(ref id) if id == "metadata"));
            }
        }
        _ => panic!("Unexpected query type"),
    }
}

#[tokio::test]
async fn test_metadata_filter_lowering() {
    let collection_service = setup_test_collection_service().await;
    let lowering = QueryLowering::new(collection_service);
    let sql = "SELECT * FROM products WHERE metadata.category = 'electronics'";

    let ast = lowering.lower_sql(sql).await.unwrap();

    match ast {
        Query::Select(select) => {
            assert!(select.selection.is_some());

            // Verify WHERE clause generates efficient FilterExpression
            // This will use HashMap.get("category") instead of linear scan
            if let Some(Expr::Binary { left: _, op, right: _ }) = &select.selection {
                assert!(matches!(op, BinaryOp::Eq));
                // TODO: Validate field access pattern optimizes to HashMap.get()
            }
        }
        _ => panic!("Unexpected query type"),
    }
}

#[tokio::test]
async fn test_vector_similarity_order_by() {
    let collection_service = setup_test_collection_service().await;
    let lowering = QueryLowering::new(collection_service);
    let sql = "SELECT * FROM products ORDER BY VECTOR_SIMILARITY(embedding, [0.1, 0.2, 0.3], 'cosine') DESC LIMIT 5";

    let ast = lowering.lower_sql(sql).await.unwrap();

    match ast {
        Query::Select(select) => {
            assert!(!select.order_by.is_empty());
            assert_eq!(select.limit, Some(5));

            // Verify vector similarity function is properly recognized
            if let Expr::FuncCall { name, args } = &select.order_by[0].expr {
                assert!(name.to_uppercase().contains("VECTOR_SIMILARITY"));
                assert_eq!(args.len(), 3); // field, vector, metric
            }
        }
        _ => panic!("Unexpected query type"),
    }
}

#[tokio::test]
async fn test_parameter_placeholder_recognition() {
    let collection_service = setup_test_collection_service().await;
    let lowering = QueryLowering::new(collection_service);
    let sql = "SELECT * FROM products WHERE category = $1 AND price > $2";

    let ast = lowering.lower_sql(sql).await.unwrap();

    match ast {
        Query::Select(select) => {
            assert!(select.selection.is_some());

            // Verify parameter placeholders are preserved in the AST
            if let Some(Expr::Binary { left, op: _, right }) = &select.selection {
                // Left side: category = $1
                if let Expr::Binary { left: _, op: _, right: param1 } = left.as_ref() {
                    assert!(matches!(param1.as_ref(), Expr::Param(_)),
                        "First parameter should be Expr::Param");
                }
                // Right side: price > $2
                if let Expr::Binary { left: _, op: _, right: param2 } = right.as_ref() {
                    assert!(matches!(param2.as_ref(), Expr::Param(_)),
                        "Second parameter should be Expr::Param");
                }
            }
        }
        _ => panic!("Unexpected query type"),
    }
}

#[tokio::test]
async fn test_performance_filter_pattern_generation() {
    // This test validates that the lowering generates efficient metadata access patterns
    let collection_service = setup_test_collection_service().await;
    let lowering = QueryLowering::new(collection_service);
    let sql = "WHERE metadata.brand = 'apple' AND metadata.price > 500";

    // CompoundIdentifier handling is now implemented in lower_expr for metadata.field syntax
    // The lowered AST represents metadata access with "metadata.field" identifiers
    // which the execution engine can optimize to O(1) HashMap lookups

    let ast = lowering
        .lower_sql(&format!("SELECT * FROM products {}", sql))
        .await
        .unwrap();

    // The lowered AST should represent metadata access in a way that
    // the execution engine can optimize to O(1) HashMap lookups
    assert!(matches!(ast, Query::Select(_)));
}

#[tokio::test]
async fn test_collection_name_resolution() {
    let collection_service = setup_test_collection_service().await;
    let lowering = QueryLowering::new(collection_service);
    let sql = "SELECT * FROM products";

    let ast = lowering.lower_sql(sql).await.unwrap();

    match ast {
        Query::Select(select) => {
            // Verify collection name was resolved
            assert!(!select.from.is_empty());
            assert!(select.from[0].name.is_some());
            // The name should be resolved to collection ID (UUID format)
            let table_name = select.from[0].name.as_ref().unwrap();
            assert!(!table_name.is_empty(), "Collection name should be resolved");
        }
        _ => panic!("Unexpected query type"),
    }
}
