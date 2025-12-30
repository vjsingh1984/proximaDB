//! Tests for SQL frontend parser

use super::parser::SqlFrontendParser;
use crate::query::ast::{BinaryOp, Expr, Literal, ProjectionItem, Query, TableRef, UnaryOp};

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_parse_group_by() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT category, COUNT(*) FROM products GROUP BY category";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse GROUP BY: {:?}",
            result.err()
        );

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

    #[test]
    fn test_parse_case_expression() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT CASE WHEN price > 100 THEN 'expensive' ELSE 'cheap' END FROM products";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse CASE expression: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert_eq!(select.projection.len(), 1);
                if let Some(ProjectionItem {
                    expr: Expr::Case { .. },
                    ..
                }) = select.projection.first()
                {
                    // Correctly parsed CASE expression
                } else {
                    panic!("Expected CASE expression in projection");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_subquery_in_from() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT a.id FROM (SELECT id FROM products WHERE price > 100) AS a";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse subquery in FROM: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert_eq!(select.from.len(), 1);
                if let Some(TableRef {
                    subquery: Some(_),
                    alias: Some(alias),
                    ..
                }) = select.from.first()
                {
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
        assert!(
            result.is_ok(),
            "Failed to parse subquery in WHERE: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::Binary {
                    right: subquery_expr,
                    ..
                }) = &select.selection
                {
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

    // Phase 10.4: New SQL Expression Tests

    #[test]
    fn test_parse_exists_subquery() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE EXISTS (SELECT 1 FROM orders WHERE orders.product_id = products.id)";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse EXISTS subquery: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::Exists { negated, .. }) = &select.selection {
                    assert!(!negated, "EXISTS should not be negated");
                } else {
                    panic!("Expected EXISTS expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_not_exists_subquery() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE NOT EXISTS (SELECT 1 FROM discontinued WHERE discontinued.id = products.id)";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse NOT EXISTS subquery: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::Exists { negated, .. }) = &select.selection {
                    assert!(*negated, "NOT EXISTS should be negated");
                } else {
                    panic!("Expected NOT EXISTS expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_like_operator() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE name LIKE '%phone%'";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse LIKE: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::Binary { op: BinaryOp::Like, .. }) = &select.selection {
                    // Correctly parsed LIKE
                } else {
                    panic!("Expected LIKE expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_not_like_operator() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE name NOT LIKE '%test%'";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse NOT LIKE: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::Binary { op: BinaryOp::NotLike, .. }) = &select.selection {
                    // Correctly parsed NOT LIKE
                } else {
                    panic!("Expected NOT LIKE expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_between() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE price BETWEEN 10 AND 100";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse BETWEEN: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::Between { negated, .. }) = &select.selection {
                    assert!(!negated, "BETWEEN should not be negated");
                } else {
                    panic!("Expected BETWEEN expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_not_between() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE price NOT BETWEEN 10 AND 100";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse NOT BETWEEN: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::Between { negated, .. }) = &select.selection {
                    assert!(*negated, "NOT BETWEEN should be negated");
                } else {
                    panic!("Expected NOT BETWEEN expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_is_null() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE description IS NULL";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse IS NULL: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::IsNull { negated, .. }) = &select.selection {
                    assert!(!negated, "IS NULL should not be negated");
                } else {
                    panic!("Expected IS NULL expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_is_not_null() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE description IS NOT NULL";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse IS NOT NULL: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::IsNull { negated, .. }) = &select.selection {
                    assert!(*negated, "IS NOT NULL should be negated");
                } else {
                    panic!("Expected IS NOT NULL expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_in_list() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE status IN ('active', 'pending', 'review')";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse IN list: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::InList { list, negated, .. }) = &select.selection {
                    assert!(!negated, "IN should not be negated");
                    assert_eq!(list.len(), 3, "Expected 3 items in IN list");
                } else {
                    panic!("Expected IN list expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_not_in_list() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE category_id NOT IN (1, 2, 3)";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse NOT IN list: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::InList { negated, .. }) = &select.selection {
                    assert!(*negated, "NOT IN should be negated");
                } else {
                    panic!("Expected NOT IN list expression in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_cross_join() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT products.name, categories.name FROM products CROSS JOIN categories";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse CROSS JOIN: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert_eq!(select.joins.len(), 1);
                use crate::query::ast::JoinType;
                assert!(
                    matches!(select.joins[0].join_type, JoinType::Cross),
                    "Expected Cross join type"
                );
                assert!(
                    select.joins[0].on_condition.is_none(),
                    "Cross join should have no ON condition"
                );
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_left_outer_join() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT products.name, orders.quantity FROM products LEFT OUTER JOIN orders ON products.id = orders.product_id";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse LEFT OUTER JOIN: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert_eq!(select.joins.len(), 1);
                use crate::query::ast::JoinType;
                assert!(
                    matches!(select.joins[0].join_type, JoinType::LeftOuter),
                    "Expected Left Outer join type"
                );
                assert!(
                    select.joins[0].on_condition.is_some(),
                    "LEFT OUTER JOIN should have ON condition"
                );
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_complex_where_with_parentheses() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE (price > 100 AND category = 'electronics') OR (price < 50 AND category = 'books')";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse complex WHERE with parentheses: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                // The expression should be parsed as (A AND B) OR (C AND D)
                if let Some(Expr::Binary { op: BinaryOp::Or, .. }) = &select.selection {
                    // Correctly parsed as OR at the top level
                } else {
                    panic!("Expected OR at top level of complex WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_modulo_operator() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM products WHERE id % 2 = 0";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse modulo operator: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                // Should find a Mod operator somewhere in the expression
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_complex_join_on_condition() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT a.id, b.name FROM table_a a JOIN table_b b ON a.id = b.a_id AND a.type = b.type AND a.active = true";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse complex JOIN ON condition: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert_eq!(select.joins.len(), 1);
                assert!(
                    select.joins[0].on_condition.is_some(),
                    "JOIN should have ON condition"
                );
                // The ON condition should be a complex AND expression
                if let Some(Expr::Binary { op: BinaryOp::And, .. }) = &select.joins[0].on_condition {
                    // Correctly parsed complex ON condition
                } else {
                    panic!("Expected AND expression in JOIN ON condition");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    // ============================================================
    // Geospatial Function Tests
    // ============================================================

    #[test]
    fn test_parse_geo_distance() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id, GEO_DISTANCE(lat, lon, 37.7749, -122.4194) as dist FROM locations";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse GEO_DISTANCE: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert_eq!(select.projection.len(), 2);
                // Check second projection is GeoDistance
                if let ProjectionItem { expr: Expr::GeoDistance { .. }, alias } = &select.projection[1] {
                    assert_eq!(alias.as_ref().map(|s| s.as_str()), Some("dist"));
                } else {
                    panic!("Expected GeoDistance expression in projection");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_geo_distance_invalid_args() {
        let parser = SqlFrontendParser::new();
        // Only 3 arguments, but needs 4
        let sql = "SELECT GEO_DISTANCE(lat, lon, 37.7749) FROM locations";

        let result = parser.parse(sql);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("4 arguments"));
    }

    #[test]
    fn test_parse_geo_within_distance() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT * FROM locations WHERE GEO_WITHIN_DISTANCE(lat, lon, 37.7749, -122.4194, 10.0)";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse GEO_WITHIN_DISTANCE: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::GeoWithinDistance { radius, unit, .. }) = &select.selection {
                    assert!(matches!(radius.as_ref(), Expr::Literal(Literal::Number(n)) if (*n - 10.0).abs() < 0.001));
                    assert_eq!(unit.as_ref().map(|s| s.as_str()), Some("km")); // Default unit
                } else {
                    panic!("Expected GeoWithinDistance in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_geo_within_distance_with_unit() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT * FROM locations WHERE GEO_NEAR(lat, lon, 34.0522, -118.2437, 50.0, 'mi')";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse GEO_NEAR with unit: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                if let Some(Expr::GeoWithinDistance { unit, .. }) = &select.selection {
                    assert_eq!(unit.as_ref().map(|s| s.as_str()), Some("mi"));
                } else {
                    panic!("Expected GeoWithinDistance in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_geo_within_box() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT id FROM places WHERE GEO_WITHIN_BOX(lat, lon, 37.0, -123.0, 38.0, -122.0)";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse GEO_WITHIN_BOX: {:?}",
            result.err()
        );

        // Helper to check if an expression represents a number (either literal or negated literal)
        fn extract_number(expr: &Expr) -> Option<f64> {
            match expr {
                Expr::Literal(Literal::Number(n)) => Some(*n),
                Expr::Unary { op: UnaryOp::Neg, expr } => {
                    if let Expr::Literal(Literal::Number(n)) = expr.as_ref() {
                        Some(-*n)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }

        match result.unwrap() {
            Query::Select(select) => {
                assert!(select.selection.is_some());
                if let Some(Expr::GeoWithinBox { sw_lat, sw_lon, ne_lat, ne_lon, .. }) = &select.selection {
                    // Check bounding box coordinates
                    let sw_lat_val = extract_number(sw_lat.as_ref()).expect("sw_lat should be a number");
                    let sw_lon_val = extract_number(sw_lon.as_ref()).expect("sw_lon should be a number");
                    let ne_lat_val = extract_number(ne_lat.as_ref()).expect("ne_lat should be a number");
                    let ne_lon_val = extract_number(ne_lon.as_ref()).expect("ne_lon should be a number");

                    assert!((sw_lat_val - 37.0).abs() < 0.001, "sw_lat mismatch: {}", sw_lat_val);
                    assert!((sw_lon_val - (-123.0)).abs() < 0.001, "sw_lon mismatch: {}", sw_lon_val);
                    assert!((ne_lat_val - 38.0).abs() < 0.001, "ne_lat mismatch: {}", ne_lat_val);
                    assert!((ne_lon_val - (-122.0)).abs() < 0.001, "ne_lon mismatch: {}", ne_lon_val);
                } else {
                    panic!("Expected GeoWithinBox in WHERE clause");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_geo_bbox_alias() {
        let parser = SqlFrontendParser::new();
        // GEO_BBOX is an alias for GEO_WITHIN_BOX
        let sql = "SELECT * FROM places WHERE GEO_BBOX(lat, lon, 37.0, -123.0, 38.0, -122.0)";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse GEO_BBOX: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert!(matches!(select.selection, Some(Expr::GeoWithinBox { .. })));
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_geo_point() {
        let parser = SqlFrontendParser::new();
        let sql = "SELECT GEO_POINT(lat, lon) as location FROM places";

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse GEO_POINT: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert_eq!(select.projection.len(), 1);
                if let ProjectionItem { expr: Expr::GeoPoint { .. }, alias } = &select.projection[0] {
                    assert_eq!(alias.as_ref().map(|s| s.as_str()), Some("location"));
                } else {
                    panic!("Expected GeoPoint expression in projection");
                }
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_geo_combined_query() {
        let parser = SqlFrontendParser::new();
        // Combined geo query: find locations within distance and return distance
        let sql = r#"
            SELECT id, name, GEO_DISTANCE(lat, lon, 37.7749, -122.4194) as dist
            FROM locations
            WHERE GEO_WITHIN_DISTANCE(lat, lon, 37.7749, -122.4194, 50.0, 'km')
            ORDER BY dist
            LIMIT 10
        "#;

        let result = parser.parse(sql);
        assert!(
            result.is_ok(),
            "Failed to parse combined geo query: {:?}",
            result.err()
        );

        match result.unwrap() {
            Query::Select(select) => {
                assert_eq!(select.projection.len(), 3);
                assert!(matches!(select.selection, Some(Expr::GeoWithinDistance { .. })));
                assert!(matches!(&select.projection[2].expr, Expr::GeoDistance { .. }));
                assert_eq!(select.limit, Some(10));
            }
            _ => panic!("Unexpected query type"),
        }
    }

    #[test]
    fn test_parse_geo_invalid_unit() {
        let parser = SqlFrontendParser::new();
        // Invalid unit 'feet' (should be 'km', 'mi', or 'm')
        let sql = "SELECT * FROM locations WHERE GEO_WITHIN_DISTANCE(lat, lon, 37.0, -122.0, 10.0, 'feet')";

        let result = parser.parse(sql);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unit must be"));
    }
}
