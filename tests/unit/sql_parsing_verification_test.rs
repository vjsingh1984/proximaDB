//! SQL Parsing and Planning Verification Tests
//! Quick verification that SQL operators are correctly parsed and planned

#[cfg(test)]
mod tests {
    use proximadb::query::sql_engine::{
        SqlParser, QueryPlanner,
        parser::{ParsedQuery, Condition, ComparisonOp, Value as SqlValue, WhereClause},
        planner::{ExecutionPlan, MetadataFilter},
    };
    use proximadb::core::search::{FilterExpression, ComparisonOperator};
    
    fn parse_and_plan_sql(sql: &str) -> anyhow::Result<(ParsedQuery, ExecutionPlan)> {
        let mut parser = SqlParser::new(sql);
        let parsed = parser.parse()?;
        
        let planner = QueryPlanner::new();
        let plan = planner.create_plan(parsed.clone())?;
        
        Ok((parsed, plan))
    }
    
    #[test]
    fn test_sql_equality_parsing() {
        let sql = "SELECT * FROM products WHERE metadata->>'category' = 'electronics'";
        let (parsed, plan) = parse_and_plan_sql(sql).unwrap();
        
        assert_eq!(plan.collection, "products");
        assert!(plan.metadata_filter.is_some());
        
        let filter = plan.metadata_filter.unwrap();
        match filter.expression {
            FilterExpression::Comparison { field, operator, value } => {
                assert_eq!(field, "metadata.category");
                assert!(matches!(operator, ComparisonOperator::Equals));
                assert_eq!(value, serde_json::Value::String("electronics".to_string()));
            }
            _ => panic!("Expected simple comparison expression"),
        }
    }
    
    #[test]
    fn test_sql_and_operator_parsing() {
        let sql = "SELECT * FROM products WHERE metadata->>'category' = 'electronics' AND metadata->>'price' > 100";
        let (parsed, plan) = parse_and_plan_sql(sql).unwrap();
        
        assert!(plan.metadata_filter.is_some());
        let filter = plan.metadata_filter.unwrap();
        
        match filter.expression {
            FilterExpression::And(exprs) => {
                assert_eq!(exprs.len(), 2);
                
                // First expression should be category = 'electronics'
                match &exprs[0] {
                    FilterExpression::Comparison { field, operator, value } => {
                        assert_eq!(field, "metadata.category");
                        assert!(matches!(operator, ComparisonOperator::Equals));
                    }
                    _ => panic!("Expected comparison expression for first AND operand"),
                }
                
                // Second expression should be price > 100
                match &exprs[1] {
                    FilterExpression::Comparison { field, operator, value } => {
                        assert_eq!(field, "metadata.price");
                        assert!(matches!(operator, ComparisonOperator::GreaterThan));
                    }
                    _ => panic!("Expected comparison expression for second AND operand"),
                }
            }
            _ => panic!("Expected AND expression"),
        }
    }
    
    #[test]
    fn test_sql_or_operator_parsing() {
        let sql = "SELECT * FROM products WHERE metadata->>'brand' = 'Apple' OR metadata->>'brand' = 'Samsung'";
        let (parsed, plan) = parse_and_plan_sql(sql).unwrap();
        
        assert!(plan.metadata_filter.is_some());
        let filter = plan.metadata_filter.unwrap();
        
        match filter.expression {
            FilterExpression::Or(exprs) => {
                assert_eq!(exprs.len(), 2);
                
                // Both expressions should be brand comparisons
                for expr in &exprs {
                    match expr {
                        FilterExpression::Comparison { field, operator, value } => {
                            assert_eq!(field, "metadata.brand");
                            assert!(matches!(operator, ComparisonOperator::Equals));
                            assert!(value.is_string());
                        }
                        _ => panic!("Expected comparison expression in OR"),
                    }
                }
            }
            _ => panic!("Expected OR expression"),
        }
    }
    
    #[test]
    fn test_sql_in_operator_parsing() {
        let sql = "SELECT * FROM products WHERE metadata->>'brand' IN ('Apple', 'Samsung', 'Google')";
        let (parsed, plan) = parse_and_plan_sql(sql).unwrap();
        
        assert!(plan.metadata_filter.is_some());
        let filter = plan.metadata_filter.unwrap();
        
        // IN should be converted to OR of equality comparisons
        match filter.expression {
            FilterExpression::Or(exprs) => {
                assert_eq!(exprs.len(), 3);
                
                let expected_brands = vec!["Apple", "Samsung", "Google"];
                for (i, expr) in exprs.iter().enumerate() {
                    match expr {
                        FilterExpression::Comparison { field, operator, value } => {
                            assert_eq!(field, "metadata.brand");
                            assert!(matches!(operator, ComparisonOperator::Equals));
                            assert_eq!(value, &serde_json::Value::String(expected_brands[i].to_string()));
                        }
                        _ => panic!("Expected comparison expression in IN conversion"),
                    }
                }
            }
            _ => panic!("Expected OR expression from IN conversion"),
        }
    }
    
    #[test]
    fn test_sql_between_operator_parsing() {
        let sql = "SELECT * FROM products WHERE metadata->>'price' BETWEEN 100 AND 1000";
        let (parsed, plan) = parse_and_plan_sql(sql).unwrap();
        
        assert!(plan.metadata_filter.is_some());
        let filter = plan.metadata_filter.unwrap();
        
        // BETWEEN should be converted to AND of >= and <= comparisons
        match filter.expression {
            FilterExpression::And(exprs) => {
                assert_eq!(exprs.len(), 2);
                
                // First should be price >= 100
                match &exprs[0] {
                    FilterExpression::Comparison { field, operator, value } => {
                        assert_eq!(field, "metadata.price");
                        assert!(matches!(operator, ComparisonOperator::GreaterThanOrEqual));
                        assert_eq!(value, &serde_json::Value::Number(serde_json::Number::from_f64(100.0).unwrap()));
                    }
                    _ => panic!("Expected >= comparison for BETWEEN lower bound"),
                }
                
                // Second should be price <= 1000
                match &exprs[1] {
                    FilterExpression::Comparison { field, operator, value } => {
                        assert_eq!(field, "metadata.price");
                        assert!(matches!(operator, ComparisonOperator::LessThanOrEqual));
                        assert_eq!(value, &serde_json::Value::Number(serde_json::Number::from_f64(1000.0).unwrap()));
                    }
                    _ => panic!("Expected <= comparison for BETWEEN upper bound"),
                }
            }
            _ => panic!("Expected AND expression from BETWEEN conversion"),
        }
    }
    
    #[test]
    fn test_sql_complex_nested_conditions() {
        let sql = "SELECT * FROM products WHERE (metadata->>'category' = 'electronics' AND metadata->>'price' > 500) OR metadata->>'brand' = 'Nike'";
        let (parsed, plan) = parse_and_plan_sql(sql).unwrap();
        
        assert!(plan.metadata_filter.is_some());
        let filter = plan.metadata_filter.unwrap();
        
        // Should be an OR with two operands
        match filter.expression {
            FilterExpression::Or(exprs) => {
                assert_eq!(exprs.len(), 2);
                
                // First operand should be an AND expression
                match &exprs[0] {
                    FilterExpression::And(and_exprs) => {
                        assert_eq!(and_exprs.len(), 2);
                        // Verify it contains category and price comparisons
                    }
                    _ => panic!("Expected AND expression as first OR operand"),
                }
                
                // Second operand should be brand comparison
                match &exprs[1] {
                    FilterExpression::Comparison { field, operator, value } => {
                        assert_eq!(field, "metadata.brand");
                        assert!(matches!(operator, ComparisonOperator::Equals));
                        assert_eq!(value, &serde_json::Value::String("Nike".to_string()));
                    }
                    _ => panic!("Expected comparison expression as second OR operand"),
                }
            }
            _ => panic!("Expected OR expression for complex nested condition"),
        }
    }
    
    #[test]
    fn test_sql_comparison_operators() {
        let test_cases = vec![
            ("metadata->>'price' > 100", ComparisonOperator::GreaterThan),
            ("metadata->>'price' >= 100", ComparisonOperator::GreaterThanOrEqual),
            ("metadata->>'price' < 100", ComparisonOperator::LessThan),
            ("metadata->>'price' <= 100", ComparisonOperator::LessThanOrEqual),
            ("metadata->>'price' != 100", ComparisonOperator::NotEquals),
            ("metadata->>'price' <> 100", ComparisonOperator::NotEquals),
        ];
        
        for (condition, expected_op) in test_cases {
            let sql = format!("SELECT * FROM products WHERE {}", condition);
            let (parsed, plan) = parse_and_plan_sql(&sql).unwrap();
            
            assert!(plan.metadata_filter.is_some());
            let filter = plan.metadata_filter.unwrap();
            
            match filter.expression {
                FilterExpression::Comparison { field, operator, value } => {
                    assert_eq!(field, "metadata.price");
                    assert!(matches!(operator, expected_op));
                }
                _ => panic!("Expected comparison expression for condition: {}", condition),
            }
        }
    }
    
    #[test]
    fn test_sql_like_operator_error() {
        let sql = "SELECT * FROM products WHERE metadata->>'name' LIKE '%test%'";
        let result = parse_and_plan_sql(sql);
        
        // LIKE operator should return an error as it's not yet implemented
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("LIKE operator not yet implemented"));
    }
    
    #[test]
    fn test_sql_empty_in_clause_error() {
        let sql = "SELECT * FROM products WHERE metadata->>'brand' IN ()";
        let result = parse_and_plan_sql(sql);
        
        // Empty IN clause should return an error
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("IN clause cannot be empty"));
    }
}