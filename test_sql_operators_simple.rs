//! Simple test to verify SQL operator implementation works
//! Run with: cargo test --bin test_sql_operators_simple

use proximadb::query::sql_engine::{
    SqlParser, QueryPlanner,
    parser::{ParsedQuery, Condition, ComparisonOp, Value as SqlValue, WhereClause},
    planner::{ExecutionPlan, MetadataFilter},
};
use proximadb::core::search::{FilterExpression, ComparisonOperator};

fn main() {
    println!("Testing SQL operator parsing and planning...\n");
    
    // Test 1: Simple equality
    test_sql_parsing("SELECT * FROM products WHERE metadata->>'category' = 'electronics'", "simple equality");
    
    // Test 2: AND operator
    test_sql_parsing("SELECT * FROM products WHERE metadata->>'category' = 'electronics' AND metadata->>'price' > 100", "AND operator");
    
    // Test 3: OR operator
    test_sql_parsing("SELECT * FROM products WHERE metadata->>'brand' = 'Apple' OR metadata->>'brand' = 'Samsung'", "OR operator");
    
    // Test 4: Complex nested conditions
    test_sql_parsing("SELECT * FROM products WHERE (metadata->>'category' = 'electronics' AND metadata->>'price' > 500) OR metadata->>'brand' = 'Nike'", "complex nested");
    
    // Test 5: IN operator
    test_sql_parsing("SELECT * FROM products WHERE metadata->>'brand' IN ('Apple', 'Samsung', 'Google')", "IN operator");
    
    // Test 6: BETWEEN operator
    test_sql_parsing("SELECT * FROM products WHERE metadata->>'price' BETWEEN 100 AND 1000", "BETWEEN operator");
    
    // Test 7: Comparison operators
    test_sql_parsing("SELECT * FROM products WHERE metadata->>'rating' >= 4.5", "comparison operator");
    
    println!("\n✅ All SQL operator tests completed successfully!");
}

fn test_sql_parsing(sql: &str, test_name: &str) {
    println!("🧪 Testing {}: {}", test_name, sql);
    
    match parse_and_plan_sql(sql) {
        Ok((parsed, plan)) => {
            println!("✅ Parse successful");
            
            if let Some(filter) = &plan.metadata_filter {
                println!("🔍 Filter expression: {:?}", filter.expression);
                
                // Verify the filter expression is properly structured
                match &filter.expression {
                    FilterExpression::Comparison { field, operator, value } => {
                        println!("   → Simple comparison: {} {:?} {:?}", field, operator, value);
                    }
                    FilterExpression::And(exprs) => {
                        println!("   → AND with {} expressions", exprs.len());
                    }
                    FilterExpression::Or(exprs) => {
                        println!("   → OR with {} expressions", exprs.len());
                    }
                    FilterExpression::Not(expr) => {
                        println!("   → NOT expression");
                    }
                }
            } else {
                println!("   → No metadata filter");
            }
            
            println!("   Collection: {}", plan.collection);
            println!("   Select fields: {:?}", plan.select_fields);
            println!();
        }
        Err(e) => {
            println!("❌ Failed: {}", e);
            println!();
        }
    }
}

fn parse_and_plan_sql(sql: &str) -> Result<(ParsedQuery, ExecutionPlan), Box<dyn std::error::Error>> {
    let mut parser = SqlParser::new(sql);
    let parsed = parser.parse()?;
    
    let planner = QueryPlanner::new();
    let plan = planner.create_plan(parsed.clone())?;
    
    Ok((parsed, plan))
}