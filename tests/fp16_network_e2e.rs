//! End-to-end fp16 collection validation through the **real network
//! server**, not the in-process embedded shortcut.
//!
//! Boots an in-process `ProximaDB::new(config)` with random free ports
//! for REST/gRPC, waits for the server to be ready, then exercises:
//!
//! 1. **REST** `POST /v1/collections` with
//!    `canonical_embedding_precision: "EMBEDDING_PRECISION_FP16"`,
//!    `GET /v1/collections/{name}` to verify the field round-trips
//! 2. **REST** `GET /metrics/prometheus` to scrape the
//!    `proximadb_embedding_precision_canonical_bytes` family — the
//!    actual content the production endpoint emits
//! 3. **gRPC** `CreateCollection(CollectionConfig {
//!    canonical_embedding_precision: Fp16 })` via tonic to prove the
//!    proto path also persists the field
//!
//! What this proves over the in-process embedded test:
//! - Axum router actually serves `/v1/collections` with the new field
//! - serde-json round-trip of `EmbeddingPrecision` (kebab-case enum) works
//! - Prometheus exporter responds on the real TCP port
//! - tonic-generated `CollectionConfig` carries the new field byte-faithfully
//!
//! What this doesn't yet cover (separate tests in this file or
//! follow-ups):
//! - pgwire SQL DDL (needs grammar extension)
//! - AQL / UQL (need their own DDL surfaces)
//! - Arrow Flight DoPut (needs the schema-with-precision setup)
//! - Full ingest+flush+metric loop (this test focuses on the
//!   create-collection and gRPC reachability; the metric scrape
//!   asserts the endpoint surfaces the gauge family, the ingest
//!   loop is covered by `fp16_canonical_bytes_metric_e2e`)

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use tempfile::TempDir;
use tokio::time::sleep;

/// Allocate a free TCP port by binding to port 0 and reading back the
/// kernel-assigned port. There's an inherent race between releasing the
/// socket and the server binding it; the server's bind retry tolerates it.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener); // release before returning so the server can bind
    port
}

/// In-process test server holding a `ProximaDB` instance + the ports it
/// listens on. The `Drop` impl signals shutdown so port leaks across
/// tests are bounded by tokio's runtime drop.
struct NetworkTestServer {
    rest_port: u16,
    grpc_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl NetworkTestServer {
    async fn start() -> anyhow::Result<Self> {
        // Enable WAL schema v2 BEFORE anything touches the cached
        // EmbeddingPrecisionConfig — the v1 EmbeddingCell serializer
        // hard-refuses non-Fp32 records (INT-2.5b step 2 Q1). Without
        // this, REST inserts into a fp16 collection silently fail at
        // WAL flush and the per-precision metric stays empty.
        // SAFETY: set before any threads inspect the OnceLock-backed
        // cache. Each `tests/*.rs` file compiles as its own binary so
        // the env var is isolated.
        unsafe {
            std::env::set_var("PROXIMADB_EMBED_PRECISION_SCHEMA_V2", "true");
        }

        let rest_port = free_port();
        let grpc_port = free_port();
        let tmp_data = TempDir::new()?;

        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp_data.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = grpc_port;
        // Multi-port mode keeps REST and gRPC on distinct ports for clarity.
        config.api.unified_mode = false;
        // Point storage_locations at the temp dir so WAL + metadata
        // don't collide with other tests' /tmp/proximadb defaults.
        config.storage.storage_locations = vec![proximadb::core::config::StorageLocation {
            url: format!("file://{}", tmp_data.path().display()),
            ..Default::default()
        }];
        config.storage.wal_config.write_buffer_directory =
            format!("file://{}/wal", tmp_data.path().display());

        let mut db = ProximaDB::new(config).await?;
        db.start().await?;

        // Poll until the REST port answers — startup is async, the
        // `start()` call returns once listeners are bound but the
        // routers may take a moment to come up.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .no_proxy()
            .build()?;
        let health_url = format!("http://127.0.0.1:{}/health", rest_port);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            match client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => {
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!(
                            "REST server didn't become ready on port {} within 15s",
                            rest_port
                        );
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }

        Ok(Self {
            rest_port,
            grpc_port,
            db: Some(db),
            _tmp_data: tmp_data,
        })
    }

    fn rest_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rest_port)
    }

    fn grpc_endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.grpc_port)
    }
}

impl Drop for NetworkTestServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            // Spawn shutdown without awaiting — tokio will drop the
            // task when the test runtime exits. A graceful shutdown
            // here would block in `Drop` which is awkward.
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rest_create_fp16_collection_round_trips_canonical_precision() {
    let server = NetworkTestServer::start().await.expect("server start");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .unwrap();

    let collection_name = format!(
        "rest_fp16_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // Create the collection with fp16 canonical precision via REST.
    // The handler expects a CollectionRequest envelope per
    // crates/platform/proximadb-api/src/rest/v1/catalog.rs:
    //   { "operation": 1 (CollectionCreate),
    //     "collection_id": "...",
    //     "collection_config": { CollectionConfig fields... } }
    // Proto enum fields deserialize as i32 discriminants. The handler's
    // `apply_proto_enum_workarounds` also accepts string labels for
    // distance_metric / storage_engine / canonical_embedding_precision
    // (we exercise the string-label form in a sibling assertion below).
    let create_body = serde_json::json!({
        "name": collection_name,
        "dimension": 64,
        "canonical_embedding_precision": "fp16",
    });

    let create_url = format!("{}/api/v2/collections", server.rest_base_url());
    let resp = client
        .post(&create_url)
        .json(&create_body)
        .send()
        .await
        .expect("REST POST /v1/collections");
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    assert!(
        status.is_success(),
        "REST create collection failed: status={status}, body={body_text}"
    );

    // GET the collection back and verify the field round-trips. The
    // server's response is a `Collection` proto serialized as JSON;
    // `config.canonical_embedding_precision` should reflect what we
    // sent (as a string label or the underlying integer — accept either).
    let get_url = format!(
        "{}/api/v2/collections/{}",
        server.rest_base_url(),
        collection_name
    );
    let get_resp = client
        .get(&get_url)
        .send()
        .await
        .expect("REST GET /v2/collections/{name}");
    let get_status = get_resp.status();
    let get_body: serde_json::Value = get_resp.json().await.unwrap_or(serde_json::Value::Null);
    assert!(
        get_status.is_success(),
        "REST GET failed: status={get_status}, body={get_body}"
    );

    // v2 GET response shape: CollectionV2Response surfaces
    // `canonical_embedding_precision` as a top-level field (the v2 handler
    // maps the proto config discriminant to a stable string label). Accept
    // the legacy nested `collection.config` / `config` shapes too for
    // forward-compat.
    let precision = get_body
        .get("canonical_embedding_precision")
        .or_else(|| get_body.get("canonicalEmbeddingPrecision"))
        .or_else(|| {
            get_body
                .get("collection")
                .and_then(|c| c.get("config"))
                .or_else(|| get_body.get("config"))
                .and_then(|cfg| {
                    cfg.get("canonical_embedding_precision")
                        .or_else(|| cfg.get("canonicalEmbeddingPrecision"))
                })
        });
    assert!(
        precision.is_some(),
        "canonical_embedding_precision missing from GET response: {get_body}"
    );
    let precision = precision.unwrap();
    let matches_fp16 = match precision {
        serde_json::Value::String(s) => {
            s == "EMBEDDING_PRECISION_FP16" || s == "FP16" || s == "Fp16"
        }
        serde_json::Value::Number(n) => n.as_i64() == Some(2),
        _ => false,
    };
    assert!(
        matches_fp16,
        "GET response precision should denote Fp16 (=2). Got: {precision:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rest_metrics_prometheus_endpoint_serves_precision_gauge_family() {
    let server = NetworkTestServer::start().await.expect("server start");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .unwrap();

    // The endpoint must respond even before any records flow through;
    // the gauge family is registered at server boot via
    // init_precision_metrics. Asserting on family registration (not
    // specific collection values) is the stable signal — the in-process
    // embedded test (tests/fp16_canonical_bytes_metric_e2e.rs) covers
    // the full ingest-and-assert-value loop.
    let metrics_url = format!("{}/metrics/prometheus", server.rest_base_url());
    let resp = client
        .get(&metrics_url)
        .send()
        .await
        .expect("REST GET /metrics/prometheus");
    let status = resp.status();
    assert!(
        status.is_success(),
        "metrics endpoint must return 200; got {status}"
    );
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("proximadb_embedding_precision_canonical_bytes"),
        "metrics scrape must include the canonical_bytes family; sample: {}",
        &body[..body.len().min(500)]
    );
}

/// Full end-to-end through the real network: create fp16 collection
/// via REST → insert records via REST `/api/v1/vectors/batch` → scrape
/// `/metrics/prometheus` → assert canonical_bytes for the collection
/// reflects the inserted data with `precision="fp16"`.
///
/// Network-level equivalent of the embedded
/// `fp16_canonical_bytes_metric_e2e` — proves the metric pipeline
/// works through the production HTTP path (REST handler → bridge →
/// canonical_precision coercion → WAL flush → metric increment),
/// not just the in-process embedded shortcut.
///
/// Sync mode defaults to `PerBatch` (see
/// `src/storage/persistence/write_ahead_log/config.rs:133`), so each
/// REST insert triggers an immediate WAL flush + canonical_bytes
/// accumulation — no explicit flush endpoint needed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rest_insert_into_fp16_collection_increments_canonical_bytes_metric() {
    let server = NetworkTestServer::start().await.expect("server start");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();

    let collection_name = format!(
        "rest_fp16_ingest_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dim: usize = 16;
    let num_records: usize = 25;

    // 1. Create fp16 collection via REST.
    let create_body = serde_json::json!({
        "name": collection_name,
        "dimension": dim,
        "canonical_embedding_precision": "fp16",
    });
    let create_resp = client
        .post(format!("{}/api/v2/collections", server.rest_base_url()))
        .json(&create_body)
        .send()
        .await
        .expect("REST create");
    assert!(
        create_resp.status().is_success(),
        "create-collection failed: {} {}",
        create_resp.status(),
        create_resp.text().await.unwrap_or_default()
    );

    // 2. Insert records via POST /api/v2/collections/{collection}/records/batch.
    // The v2 records payload carries Vec<f32> vectors per ProximaRecord; the
    // bridge coerces to the collection's canonical fp16 at write time.
    let records: Vec<serde_json::Value> = (0..num_records)
        .map(|i| {
            let v: Vec<f32> = (0..dim)
                .map(|j| (i as f32) * 0.1 + (j as f32) * 0.01)
                .collect();
            serde_json::json!({
                "id": format!("rec-{:03}", i),
                "vector": v,
            })
        })
        .collect();
    let insert_body = serde_json::json!({
        "records": records,
    });
    let insert_resp = client
        .post(format!(
            "{}/api/v2/collections/{}/records/batch",
            server.rest_base_url(),
            collection_name
        ))
        .json(&insert_body)
        .send()
        .await
        .expect("REST insert");
    let insert_status = insert_resp.status();
    let insert_body_text = insert_resp.text().await.unwrap_or_default();
    assert!(
        insert_status.is_success(),
        "insert failed: {insert_status} {insert_body_text}"
    );

    // 3. Brief settling window — PerBatch sync mode flushes inside the
    // insert but the metric write may be on a different task.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 4. Scrape /metrics/prometheus and assert canonical_bytes for the
    // collection under precision="fp16".
    let metrics_resp = client
        .get(format!("{}/metrics/prometheus", server.rest_base_url()))
        .send()
        .await
        .expect("scrape");
    assert!(metrics_resp.status().is_success(), "metrics scrape failed");
    let scrape = metrics_resp.text().await.expect("body");

    // Expected: num_records × dim × bytes_per_element(fp16)
    // = 25 × 16 × 2 = 800 bytes
    let expected_bytes: i64 = (num_records as i64) * (dim as i64) * 2;

    let metric_prefix = "proximadb_embedding_precision_canonical_bytes";
    let coll_fragment = format!(r#"collection="{}""#, collection_name);
    let mut observed: Option<i64> = None;
    for line in scrape.lines() {
        if !line.starts_with(metric_prefix) {
            continue;
        }
        if !line.contains(&coll_fragment) || !line.contains(r#"precision="fp16""#) {
            continue;
        }
        if let Some((_, tail)) = line.split_once('}') {
            if let Ok(value) = tail.trim().parse::<i64>() {
                observed = Some(value);
                break;
            }
        }
    }
    assert_eq!(
        observed,
        Some(expected_bytes),
        "canonical_bytes{{collection={collection_name},precision=fp16}} = {observed:?}, \
         expected {expected_bytes} (= {num_records} × {dim} × 2 B/fp16). Scrape:\n{scrape}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_create_fp16_collection_persists_canonical_precision() {
    let server = NetworkTestServer::start().await.expect("server start");

    let channel = tonic::transport::Channel::from_shared(server.grpc_endpoint())
        .expect("valid grpc URL")
        .connect()
        .await
        .expect("grpc connect");
    let mut client =
        proximadb::proto::proximadb_v1::collection_service_client::CollectionServiceClient::new(
            channel,
        );

    let collection_name = format!(
        "grpc_fp16_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let config = proximadb::proto::proximadb_v1::CollectionConfig {
        name: collection_name.clone(),
        dimension: 64,
        canonical_embedding_precision: Some(
            proximadb::proto::proximadb_v1::EmbeddingPrecision::Fp16 as i32,
        ),
        ..Default::default()
    };

    let response = client
        .create_collection(tonic::Request::new(config))
        .await
        .expect("grpc CreateCollection");
    let collection = response.into_inner();
    let returned_cfg = collection.config.expect("returned Collection has config");
    assert_eq!(
        returned_cfg.canonical_embedding_precision,
        Some(proximadb::proto::proximadb_v1::EmbeddingPrecision::Fp16 as i32),
        "gRPC must persist + return Fp16 canonical precision"
    );
}
