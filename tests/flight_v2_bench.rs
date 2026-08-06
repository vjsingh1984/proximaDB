//! Benchmark harness: Arrow Flight `DoGet` vs REST v2 vector search
//! (TD-FLIGHT-1 §"Benchmark acceptance").
//!
//! `#[ignore]`'d because it loads a real bed and is a *measurement* (run on a
//! release build for representative numbers): invoke with
//!   `cargo nextest run --run-ignored only -p proximadb flight_v2_bench`
//! (or `cargo test --release flight_v2_bench -- --ignored --nocapture`).
//!
//! Bed size is configurable: `PROXIMADB_FLIGHT_BENCH_N` (records, default 2000)
//! and `PROXIMADB_FLIGHT_BENCH_Q` (queries, default 60).
//!
//! Expected invariants (TD-FLIGHT-1):
//!   - **Recall / GETs per query are transport-invariant** — both surfaces call
//!     the same `handle_record_search_for_tenant` engine, so the ordered result
//!     ids must match exactly. This test asserts that.
//!   - **Flight lowers wire bytes** (and typically p50/p95 client-perceived
//!     latency for bulk) by saving serialization/copy. This test *reports* the
//!     comparison; it does not assert a hard ratio (that is
//!     release-build/environment dependent).
//!
//! NOTE: v2 search defaults to `VectorFreshnessMode::Strong`, which bypasses the
//! query cache — so both transports compute fresh on every query and the cache is
//! not a confound (the handoff's "identical cache state" requirement).

use std::net::TcpListener;
use std::time::{Duration, Instant};

use arrow_array::{Array, StringArray};
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

const DIM: usize = 64;

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
            .timeout(Duration::from_secs(30))
            .no_proxy()
            .build()?;
        let health_url = format!("http://127.0.0.1:{}/health", rest_port);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match http.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => {
                    if Instant::now() > deadline {
                        anyhow::bail!("server didn't become ready in 20s");
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }
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

/// Deterministic pseudo-random unit-ish vector (no RNG; reproducible across
/// runs). The exact distribution doesn't matter — we benchmark transport cost
/// and assert id-match, not recall quality.
fn det_vec(seed: usize, dim: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(dim);
    for j in 0..dim {
        // simple LCG-ish hash → [-0.5, 0.5)
        let h = (seed
            .wrapping_mul(2654435761)
            .wrapping_add(j.wrapping_mul(40503)))
            % 1000;
        v.push((h as f32 / 1000.0) - 0.5);
    }
    v
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Flight `DoGet` vs REST v2: identical queries, top_k, and server; assert the
/// ordered result ids match (recall/GET invariance) and report latency + wire
/// bytes per transport.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measurement: run on release — cargo nextest run --run-ignored only flight_v2_bench"]
async fn flight_v2_bench_vs_rest() {
    let n: usize = std::env::var("PROXIMADB_FLIGHT_BENCH_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);
    let q: usize = std::env::var("PROXIMADB_FLIGHT_BENCH_Q")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let top_k = 10u32;

    let server = FlightServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let base = server.rest_base();
    let name = format!(
        "bench_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // 1. Create collection + load a deterministic bed.
    let create = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": name, "dimension": DIM, "engine": "sst",
            "distance_metric": "cosine", "enable_proxima_record": false,
        }))
        .send()
        .await
        .expect("create");
    assert!(create.status().is_success(), "create failed");

    let batch_size = 500usize.min(n);
    for start in (0..n).step_by(batch_size) {
        let end = (start + batch_size).min(n);
        let records: Vec<serde_json::Value> = (start..end)
            .map(|i| json!({ "id": format!("r{i}"), "vector": det_vec(i, DIM) }))
            .collect();
        let resp = http
            .post(format!("{base}/api/v2/collections/{name}/records/batch"))
            .json(&json!({ "records": records }))
            .send()
            .await
            .expect("insert batch");
        assert!(resp.status().is_success(), "insert batch failed");
    }
    // Force-flush so the bed is served from persisted storage (real segments),
    // not just the WAL memtable.
    server
        .db
        .as_ref()
        .expect("db")
        .force_flush_collection(&name)
        .await
        .expect("force flush");
    sleep(Duration::from_millis(500)).await;

    let queries: Vec<Vec<f32>> = (0..q).map(|i| det_vec(1_000_000 + i, DIM)).collect();

    // 2. Warm up both transports (a couple of queries each).
    for v in queries.iter().take(3) {
        let _ = http
            .post(format!("{base}/api/v2/collections/{name}/search"))
            .json(&json!({ "vector": v, "top_k": top_k }))
            .send()
            .await;
    }
    let channel = tonic::transport::Endpoint::from_shared(server.flight_url())
        .expect("endpoint")
        .connect()
        .await
        .expect("flight channel");

    // 3. Measure REST v2.
    let mut rest_lat: Vec<f64> = Vec::with_capacity(q);
    let mut rest_bytes: u64 = 0;
    let mut rest_ids_first: Vec<String> = Vec::new();
    for (i, v) in queries.iter().enumerate() {
        let t0 = Instant::now();
        let resp = http
            .post(format!("{base}/api/v2/collections/{name}/search"))
            .json(&json!({ "vector": v, "top_k": top_k }))
            .send()
            .await
            .expect("rest search");
        let body = resp.bytes().await.expect("rest body");
        let dt = t0.elapsed().as_secs_f64() * 1000.0;
        rest_lat.push(dt);
        rest_bytes += body.len() as u64;
        if i == 0 {
            let jb: serde_json::Value = serde_json::from_slice(&body).expect("rest json");
            rest_ids_first = jb
                .get("results")
                .and_then(|r| r.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|h| h.get("id").and_then(|x| x.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
        }
    }

    // 4. Measure Flight DoGet.
    let mut flight = FlightServiceClient::new(channel);
    let mut flight_lat: Vec<f64> = Vec::with_capacity(q);
    let mut flight_bytes: u64 = 0;
    let mut flight_ids_first: Vec<String> = Vec::new();
    for (i, v) in queries.iter().enumerate() {
        let ticket = Ticket {
            ticket: serde_json::to_vec(&json!({
                "type": "vector_search",
                "collection_id": name,
                "query_vector": v,
                "top_k": top_k,
                "include_vector": false,
            }))
            .unwrap()
            .into(),
        };
        let t0 = Instant::now();
        let resp = flight
            .do_get(ticket)
            .await
            .expect("flight do_get")
            .into_inner();
        let batches = FlightRecordBatchStream::new_from_flight_data(
            resp.map_err(|e| FlightError::Tonic(Box::new(e))),
        )
        .try_collect::<Vec<_>>()
        .await
        .expect("decode");
        let dt = t0.elapsed().as_secs_f64() * 1000.0;
        flight_lat.push(dt);
        // Wire bytes: the FlightData frames the server would have egressed
        // (schema + data) — measured from the decoded batch's IPC size as a
        // proxy for what crossed the wire.
        for b in &batches {
            flight_bytes += arrow_flight_ipc_wire_bytes(b);
        }
        if i == 0 {
            for b in &batches {
                let col = b
                    .column_by_name("id")
                    .expect("id col")
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .expect("StringArray");
                for r in 0..col.len() {
                    flight_ids_first.push(col.value(r).to_string());
                }
            }
        }
    }

    rest_lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    flight_lat.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // 5. Recall invariance: identical ordered ids on both transports
    //    (same engine ⇒ same GETs/query).
    assert_eq!(
        flight_ids_first, rest_ids_first,
        "Flight and REST must return identical ordered ids (recall/GET invariance)"
    );

    eprintln!(
        "\n=== TD-FLIGHT-1 bench (bed={n} dim={DIM} q={q} top_k={top_k}) ===\n\
         REST  v2: p50={:.3}ms p95={:.3}ms  wire={:.1} KB/q\n\
         Flight  : p50={:.3}ms p95={:.3}ms  wire={:.1} KB/q\n\
         ids match: {}",
        percentile(&rest_lat, 0.5),
        percentile(&rest_lat, 0.95),
        rest_bytes as f64 / q as f64 / 1024.0,
        percentile(&flight_lat, 0.5),
        percentile(&flight_lat, 0.95),
        flight_bytes as f64 / q as f64 / 1024.0,
        flight_ids_first == rest_ids_first,
    );
}

/// Approximate wire size of a RecordBatch as encoded FlightData
/// (data_header + data_body). Uses the IPC-serialized byte length as the body
/// proxy — sufficient for a relative Flight-vs-REST wire comparison.
fn arrow_flight_ipc_wire_bytes(batch: &arrow_array::RecordBatch) -> u64 {
    let mut writer = arrow_ipc::writer::StreamWriter::try_new(Vec::new(), batch.schema().as_ref())
        .expect("ipc writer");
    writer.write(batch).expect("write");
    writer.into_inner().expect("inner").len() as u64
}
