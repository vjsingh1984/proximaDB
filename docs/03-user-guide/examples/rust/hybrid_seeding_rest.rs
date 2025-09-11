// REST: Hybrid query with seeding strategy (Rust + reqwest)
// cargo add reqwest serde_json --features reqwest/json

use reqwest::blocking::Client;
use serde_json::json;

fn main() -> anyhow::Result<()> {
    let client = Client::new();
    let url = "http://localhost:5678/api/v1/sql/execute";
    let body = json!({
        "query": "-- SEEDING: AVERAGE\nSELECT id FROM my_collection ORDER BY COSINE_DISTANCE(vector, $1) LIMIT 10",
        "parameters": [ { "value": { "array_value": { "items": [ {"value": {"number_value": 0.1}}, {"value": {"number_value": 0.2}} ] } } } ],
        "collection": "my_collection",
        "seeding": "average"
    });
    let resp = client.post(url).json(&body).send()?;
    println!("{}", resp.text()?);
    Ok(())
}

