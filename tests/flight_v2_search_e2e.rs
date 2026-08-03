//! End-to-end convergence of Arrow Flight vector search onto the canonical v2
//! service (TD-FLIGHT-1).
//!
//! Boots an in-process ProximaDB with a free Flight port, inserts records over
//! REST v2, then runs the SAME search over both REST v2 and Arrow Flight `DoGet`
//! (the new `type:"vector_search"` ticket) and asserts they return identical
//! ordered ids — proving Flight now routes through the same `RecordSearchPort`
//! authority as REST v2 rather than the deprecated v1 contract. A second test
//! verifies the clean-break ticket routing: a non-file/non-graph/non-v2 ticket
//! is rejected with `InvalidArgument` instead of being silently coerced to v1.

use std::net::TcpListener;
use std::time::Duration;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_flight::Ticket;
use arrow_flight::decode::FlightRecordBatchStream;
use arrow_flight::error::FlightError;
use arrow_flight::flight_service_client::FlightServiceClient;
use futures::TryStreamExt;
use proximadb::core::Config;
use proximadb::database::ProximaDB;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::sleep;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

struct FlightServer {
    flight_port: u16,
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl FlightServer {
    async fn start() -> anyhow::Result<Self> {
        let flight_port = free_port();
        let rest_port = free_port();
        let tmp_data = TempDir::new()?;

        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp_data.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = free_port();
        config.api.arrow_flight_port = flight_port;
        config.api.pg_port = Some(free_port());
        config.api.unified_mode = false;
        config.storage.storage_locations = vec![proximadb::core::config::StorageLocation {
            url: format!("file://{}", tmp_data.path().display()),
            ..Default::default()
        }];
        config.storage.wal_config.write_buffer_directory =
            format!("file://{}/wal", tmp_data.path().display());

        let mut db = ProximaDB::new(config).await?;
        db.start().await?;

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()?;
        let health_url = format!("http://127.0.0.1:{}/health", rest_port);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            match http.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => {
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!("server didn't become ready in 15s");
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }
        // Grace for the Arrow IPC listener to start accepting.
        sleep(Duration::from_millis(300)).await;

        Ok(Self {
            flight_port,
            rest_port,
            db: Some(db),
            _tmp_data: tmp_data,
        })
    }

    fn flight_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.flight_port)
    }

    fn rest_base(&self) -> String {
        format!("http://127.0.0.1:{}", self.rest_port)
    }
}

impl Drop for FlightServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

/// Build a unit-norm vector whose cosine to `e_0` is exactly `first` — so five
/// records with distinct `first` components have a strict, deterministic cosine
/// ordering against the query `e_0`.
fn unit_with_first(first: f32, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    v[0] = first;
    v[1] = (1.0 - first * first).sqrt();
    v
}

/// Flight `DoGet` with the canonical v2 ticket must return the SAME ordered ids
/// as REST v2 for the same collection/query/top_k — the core TD-FLIGHT-1
/// convergence gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flight_v2_doget_matches_rest_v2() {
    let server = FlightServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let base = server.rest_base();
    let name = format!(
        "flight_v2_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dim: usize = 8;

    let create = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": name, "dimension": dim, "engine": "sst",
            "distance_metric": "cosine", "enable_proxima_record": false,
        }))
        .send()
        .await
        .expect("create");
    assert!(create.status().is_success(), "create failed");

    // Distinct cosine sims against e_0: rec-0=1.0, rec-1=0.9, ... rec-4=0.6.
    let records: Vec<serde_json::Value> = (0..5)
        .map(|i| {
            let first = 1.0 - 0.1 * i as f32;
            json!({ "id": format!("rec-{i}"), "vector": unit_with_first(first, dim) })
        })
        .collect();
    let insert = http
        .post(format!("{base}/api/v2/collections/{name}/records/batch"))
        .json(&json!({ "records": records }))
        .send()
        .await
        .expect("insert");
    assert!(insert.status().is_success(), "insert failed");
    sleep(Duration::from_millis(500)).await;

    let query: Vec<f32> = unit_with_first(1.0, dim); // == e_0
    // REST v2 oracle.
    let rest_resp = http
        .post(format!("{base}/api/v2/collections/{name}/search"))
        .json(&json!({ "vector": query, "top_k": 3 }))
        .send()
        .await
        .expect("rest search");
    assert!(rest_resp.status().is_success(), "rest search status");
    let rest_body: serde_json::Value = rest_resp.json().await.expect("rest json");
    let rest_ids: Vec<String> = rest_body
        .get("results")
        .or_else(|| rest_body.get("hits"))
        .and_then(|v| v.as_array())
        .expect("rest results array")
        .iter()
        .filter_map(|h| {
            h.get("id")
                .or_else(|| h.get("record_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        rest_ids,
        vec!["rec-0", "rec-1", "rec-2"],
        "rest oracle order: {rest_body}"
    );

    // Flight DoGet with the canonical v2 ticket.
    let channel = tonic::transport::Endpoint::from_shared(server.flight_url())
        .expect("flight endpoint")
        .connect()
        .await
        .expect("flight channel");
    let mut flight = FlightServiceClient::new(channel);

    let ticket = Ticket {
        ticket: serde_json::to_vec(&json!({
            "type": "vector_search",
            "collection_id": name,
            "query_vector": query,
            "top_k": 3,
            "include_vector": false,
        }))
        .unwrap()
        .into(),
    };
    let response = flight
        .do_get(ticket)
        .await
        .expect("flight do_get")
        .into_inner();
    let record_stream = FlightRecordBatchStream::new_from_flight_data(
        response.map_err(|s| FlightError::Tonic(Box::new(s))),
    );
    let batches: Vec<RecordBatch> = record_stream.try_collect().await.expect("decode batches");

    let mut flight_ids: Vec<String> = Vec::new();
    for b in &batches {
        let id_col = b
            .column_by_name("id")
            .expect("id column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("id is StringArray");
        for i in 0..id_col.len() {
            flight_ids.push(id_col.value(i).to_string());
        }
    }

    assert_eq!(
        flight_ids, rest_ids,
        "Flight DoGet must return the same ordered ids as REST v2 (TD-FLIGHT-1)"
    );
}

/// A ticket that is neither arrow_file, graph, nor vector_search is rejected
/// with an error — no silent coercion into the removed v1 search fallback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flight_v2_unknown_ticket_rejected() {
    let server = FlightServer::start().await.expect("server start");
    let channel = tonic::transport::Endpoint::from_shared(server.flight_url())
        .expect("flight endpoint")
        .connect()
        .await
        .expect("flight channel");
    let mut flight = FlightServiceClient::new(channel);

    let ticket = Ticket {
        ticket: serde_json::to_vec(&json!({ "type": "bogus_unknown_ticket" }))
            .unwrap()
            .into(),
    };
    let result = flight.do_get(ticket).await;
    assert!(
        result.is_err(),
        "an unrecognized ticket must be rejected, not silently coerced to v1 search"
    );
}
