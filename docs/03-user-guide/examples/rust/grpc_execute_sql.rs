// gRPC: ExecuteSql via tonic (requires generated client stubs)
// cargo add tonic prost tokio --features tokio/full

use proximadb_v1::sql_service_client::SqlServiceClient;
use proximadb_v1::{ExecuteSqlRequest, SqlValue};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = SqlServiceClient::connect("http://localhost:5679").await?;
    let vec = SqlValue { value: Some(proximadb_v1::sql_value::Value::NumberValue(0.1)) };
    let req = ExecuteSqlRequest {
        query: "SELECT id FROM my_collection ORDER BY COSINE_DISTANCE(vector, $1) LIMIT 5".to_string(),
        parameters: vec![SqlValue { value: Some(proximadb_v1::sql_value::Value::ArrayValue(proximadb_v1::SqlArray{ items: vec![vec] })) }],
        collection: Some("my_collection".to_string()),
        limit: None,
        offset: None,
    };
    let resp = client.execute_sql(req).await?;
    println!("{:?}", resp.into_inner());
    Ok(())
}

pub mod proximadb_v1 { include!("../../../../src/proto/proximadb.v1.rs"); }

