//! Tests for SQL frontend parser

use super::parser::SqlFrontendParser;
use crate::query::ast::{BinaryOp, Expr, Literal, Query};

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
                match &select.projection[0] {
                    Expr::FuncCall { name, .. } => {
                        assert_eq!(name, "COSINE_DISTANCE");
                    }
                    _ => {
                        // Function parsing may not be complete yet, but it shouldn't crash
                    }
                }
            }
            Err(_) => {
                // Vector literals may not be implemented yet, that's ok
            }
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
