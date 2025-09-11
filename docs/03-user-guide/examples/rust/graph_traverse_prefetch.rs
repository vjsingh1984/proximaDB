// REST: Graph traversal with per-call prefetch overrides (Rust + reqwest)
// cargo add reqwest serde_json --features reqwest/json

use reqwest::blocking::Client;
use serde_json::json;

fn main() -> anyhow::Result<()> {
    let client = Client::new();
    let url = "http://localhost:5678/api/v1/graph/traverse";
    let body = json!({
        "start_node_id": "n1",
        "max_depth": 3,
        "edge_types": ["REL"],
        "algorithm": "BFS"
    });
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("x-graph-prefetch-enabled", "true")
        .header("x-graph-prefetch-budget", "8")
        .json(&body)
        .send()?;
    println!("{}", resp.text()?);
    Ok(())
}

