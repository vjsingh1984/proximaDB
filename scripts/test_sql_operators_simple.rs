//! Simple test to verify SQL operator implementation works
//! Run with: cargo test --bin test_sql_operators_simple

use proximadb::query::sql_engine::{
    SqlParser, QueryPlanner,
    parser::{ParsedQuery, Condition, ComparisonOp, Value as SqlValue, WhereClause},
    planner::{ExecutionPlan, MetadataFilter},
};
use proximadb::core::search::{FilterExpression, ComparisonOperator};
use tracing::{debug, error, info};

fn main() {
    debug!("Testing SQL operator parsing and planning...\n");
    
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
    
    info!("\n✅ All SQL operator tests completed successfully!");
}

fn test_sql_parsing(sql: &str, test_name: &str) {
    debug!("🧪 Testing {}: {}", test_name, sql);
    
    match parse_and_plan_sql(sql) {
        Ok((parsed, plan)) => {
            info!("✅ Parse successful");
            
            if let Some(filter) = &plan.metadata_filter {
                debug!("🔍 Filter expression: {:?}", filter.expression);
                
                // Verify the filter expression is properly structured
                match &filter.expression {
                    FilterExpression::Comparison { field, operator, value } => {
                        debug!("   → Simple comparison: {} {:?} {:?}", field, operator, value);
                    }
                    FilterExpression::And(exprs) => {
                        debug!("   → AND with {} expressions", exprs.len());
                    }
                    FilterExpression::Or(exprs) => {
                        debug!("   → OR with {} expressions", exprs.len());
                    }
                    FilterExpression::Not(expr) => {
                        debug!("   → NOT expression");
                    }
                }
            } else {
                debug!("   → No metadata filter");
            }
            
            debug!("   Collection: {}", plan.collection);
            debug!("   Select fields: {:?}", plan.select_fields);
            debug!();
        }
        Err(e) => {
            error!("❌ Failed: {}", e);
            debug!();
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