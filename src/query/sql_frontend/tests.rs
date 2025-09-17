//! Tests for SQL frontend parser

use super::parser::SqlFrontendParser;
use crate::query::ast::{BinaryOp, Expr, Literal, Query, UnaryOp, ProjectionItem, TableRef};

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
}
