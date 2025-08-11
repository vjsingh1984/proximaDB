use anyhow::Result;
use proximadb::core::search::{FilterExpression, ComparisonOperator};
use tracing::{debug};

#[tokio::main]
async fn main() -> Result<()> {
    debug!("Testing SST filter evaluation...");
    
    // Create a simple filter
    let filter = FilterExpression::Comparison {
        field: "batch".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::Number(serde_json::Number::from(2)),
    };
    
    // Create test metadata
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("batch".to_string(), serde_json::json!(2));
    
    debug!("Filter: {:?}", filter);
    debug!("Metadata: {:?}", metadata);
    
    // Test the centralized filter evaluation
    let result = proximadb::core::search::json_comparison::evaluate_filter(&filter, &metadata);
    debug!("Filter evaluation result: {}", result);
    
    // Test with float value
    metadata.clear();
    metadata.insert("batch".to_string(), serde_json::json!(2.0));
    debug!("\nMetadata with float: {:?}", metadata);
    
    let result2 = proximadb::core::search::json_comparison::evaluate_filter(&filter, &metadata);
    debug!("Filter evaluation result with float: {}", result2);
    
    Ok(())
}