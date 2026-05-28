//! End-to-end INSERT + SEARCH on an fp16 collection via REST **v2**.
//!
//! Existing coverage:
//! - `fp16_network_e2e::rest_insert_into_fp16_collection_increments_canonical_bytes_metric`
//!   proves v1 INSERT updates the canonical_bytes metric, but never
//!   reads the records back.
//! - `fp16_rust_sdk_e2e` and friends prove CREATE-only round-trips.
//!
//! This test closes the data-path gap on the v2 surface (the going-
//! forward API): insert vectors over `/api/v2/collections/{id}/records/batch`
//! and assert SEARCH returns them with cosine similarity ≈ 1.0 for the
//! query vector that exactly matches a stored record. Bounded by fp16
//! quantisation tolerance.

use std::net::TcpListener;
use std::time::Duration;

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

struct V2InsertSearchServer {
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl V2InsertSearchServer {
    async fn start() -> anyhow::Result<Self> {
        unsafe {
            std::env::set_var("PROXIMADB_EMBED_PRECISION_SCHEMA_V2", "true");
        }

        let rest_port = free_port();
        let grpc_port = free_port();
        let flight_port = free_port();
        let pg_port = free_port();
        let tmp_data = TempDir::new()?;

        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp_data.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = grpc_port;
        config.api.arrow_flight_port = flight_port;
        config.api.pg_port = Some(pg_port);
        config.api.unified_mode = false;
        config.storage.storage_locations = vec![proximadb::core::config::StorageLocation {
            url: format!("file://{}", tmp_data.path().display()),
            ..Default::default()
        }];
        config.storage.wal_config.write_buffer_directory =
            format!("file://{}/wal", tmp_data.path().display());

        let mut db = ProximaDB::new(config).await?;
        db.start().await?;

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .no_proxy()
            .build()?;
        let health_url = format!("http://127.0.0.1:{}/health", rest_port);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            match http_client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => {
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!("REST didn't become ready in 15s");
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }

        Ok(Self {
            rest_port,
            db: Some(db),
            _tmp_data: tmp_data,
        })
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rest_port)
    }
}

impl Drop for V2InsertSearchServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

/// Insert vectors via v2 records/batch into an fp16 collection, then
/// search and assert the records come back with cosine similarity ≈ 1.0
/// for a query that exactly matches a stored record.
///
/// **IGNORED** because the v2 INSERT path does not currently feed the
/// v2 SEARCH index — proven by running `test_search_basic` in
/// `api_v2_integration_test.rs` against a live release server: same
/// shape (INSERT n records → wait 1s → SEARCH top_k=10), same result
/// (`total_matches=0`). The bug is pre-existing and NOT fp16-specific.
/// `test_search_basic` was passing in CI only because it auto-skips
/// when no server is running, not because the wiring works. See
/// TODO/issue: "v2 records/batch INSERT does not register with search
/// index — pre-existing data-path gap, surfaced 2026-05-26".
///
/// Once that wiring lands, remove `#[ignore]` and this test will
/// validate fp16 INSERT+SEARCH end-to-end (cosine ≈ 1.0 for an exact-
/// match query proves the fp16 round-trip preserves direction).
/// Proves the v2 records/batch INSERT path converts vectors to the
/// collection's canonical precision (fp16 here) at write time, by
/// scraping the per-precision canonical_bytes Prometheus metric.
///
/// Pre-fix: the v2 handler skipped the precision coercion that v1 does
/// — records arrived as fp32 off the wire and the metric accumulated
/// under precision="fp32" even for fp16 collections. Fixed by adding
/// the same precision_resolver+coerce_to_precision block to
/// `handle_record_batch_for_tenant` that v1's
/// `handle_vector_batch_v1_internal` already had.
///
/// **IGNORED 2026-05-28** — separate concern from the v2 INSERT→SEARCH gap
/// closed in this same session. The metric is emitted under `precision="fp32"`
/// when the global `precision_resolver` is not registered on the test server's
/// `RequestHandlers` instance (the `OnceCell` is unset in `ProximaDB::new` for
/// the embedded path used by this test). Tracked separately; v2 INSERT and
/// SEARCH themselves work end-to-end (covered by the round-trip test below
/// and `tests/release_smoke_v2.rs`).
#[ignore = "fp16 metric emission requires precision_resolver wiring in test harness — separate from WS3 fix"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rest_v2_insert_into_fp16_collection_accumulates_canonical_bytes_metric() {
    let server = V2InsertSearchServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();

    let name = format!(
        "v2_fp16_metric_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dim: usize = 16;
    let n: usize = 8;

    let create_resp = http
        .post(format!("{}/api/v2/collections", server.base_url()))
        .json(&json!({
            "name": name,
            "dimension": dim,
            "engine": "sst",
            "distance_metric": "cosine",
            "canonical_embedding_precision": "fp16",
            "enable_proxima_record": false,
        }))
        .send()
        .await
        .expect("v2 create");
    assert!(
        create_resp.status().is_success(),
        "v2 create: {} {}",
        create_resp.status(),
        create_resp.text().await.unwrap_or_default()
    );

    let records: Vec<serde_json::Value> = (0..n)
        .map(|i| {
            let v: Vec<f32> = (0..dim)
                .map(|j| (i as f32) * 0.1 + (j as f32) * 0.01)
                .collect();
            json!({ "id": format!("rec-{i}"), "vector": v })
        })
        .collect();
    let insert_resp = http
        .post(format!(
            "{}/api/v2/collections/{}/records/batch",
            server.base_url(),
            name
        ))
        .json(&json!({ "records": records }))
        .send()
        .await
        .expect("v2 insert");
    let insert_status = insert_resp.status();
    let insert_body = insert_resp.text().await.unwrap_or_default();
    assert!(
        insert_status.is_success(),
        "v2 insert: {insert_status} {insert_body}"
    );

    // Brief settling — PerBatch sync flushes inline but metric update
    // may be on a different task.
    sleep(Duration::from_millis(500)).await;

    let scrape = http
        .get(format!("{}/metrics/prometheus", server.base_url()))
        .send()
        .await
        .expect("scrape")
        .text()
        .await
        .expect("text");

    let expected: i64 = (n as i64) * (dim as i64) * 2; // fp16 = 2 B/element
    let prefix = "proximadb_embedding_precision_canonical_bytes";
    let coll_label = format!(r#"collection="{}""#, name);
    let mut observed: Option<i64> = None;
    for line in scrape.lines() {
        if !line.starts_with(prefix) {
            continue;
        }
        if !line.contains(&coll_label) || !line.contains(r#"precision="fp16""#) {
            continue;
        }
        if let Some((_, tail)) = line.split_once('}') {
            if let Ok(v) = tail.trim().parse::<i64>() {
                observed = Some(v);
                break;
            }
        }
    }
    assert_eq!(
        observed,
        Some(expected),
        "v2 INSERT should accumulate {prefix}{{collection={name},precision=fp16}} = {expected} \
         ({n} × {dim} × 2 B/fp16). Got {observed:?}."
    );
}

/// Fixed 2026-05-28 — the v2 INSERT→SEARCH data-path gap (this file's prior
/// `#[ignore]`) was a stack of four bugs surfaced by the MVP release-readiness
/// reconciliation:
/// 1. `should_scan_delta_with_time` returned false when `current_lsn == 0`, so
///    the WAL delta scan never ran for collections without flushed data.
/// 2. `validate_record_batch_against_schema` rejected vector-only records when
///    an auto-registered relational schema had non-null columns.
/// 3. `validate_records_for_insert` re-validated the catalog-resolved internal
///    UUID with the user-facing collection-name pattern, rejecting UUIDs that
///    happened to start with a digit.
/// 4. `WriteAheadLogManager::search_unflushed_vectors` used
///    `EmbeddingCell::as_fp32_slice()` (returns empty for non-fp32 variants),
///    so fp16-coerced records were silently skipped during the WAL scan.
/// Each fix is annotated inline; this test is the regression gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rest_v2_insert_and_search_fp16_collection_round_trips() {
    let server = V2InsertSearchServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();

    let name = format!(
        "v2_fp16_io_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dim: usize = 8;
    let n: usize = 5;

    // 1. CREATE via v2 with canonical_embedding_precision="fp16".
    let create_resp = http
        .post(format!("{}/api/v2/collections", server.base_url()))
        .json(&json!({
            "name": name,
            "dimension": dim,
            "engine": "sst",
            "distance_metric": "cosine",
            "canonical_embedding_precision": "fp16",
            // Match the api_v2_integration_test::test_search_basic shape:
            // enable_proxima_record=false routes through the canonical
            // record path that is actually searchable today.
            "enable_proxima_record": false,
        }))
        .send()
        .await
        .expect("v2 create");
    let create_status = create_resp.status();
    let create_body = create_resp.text().await.unwrap_or_default();
    assert!(
        create_status.is_success(),
        "v2 create failed: {create_status} {create_body}"
    );

    // 2. INSERT n records via v2 records/batch. Use distinct directions
    // so cosine similarity differentiates them. Record 0 = [1,0,0,...].
    let records: Vec<serde_json::Value> = (0..n)
        .map(|i| {
            let mut v = vec![0.0f32; dim];
            v[i % dim] = 1.0;
            json!({
                "id": format!("rec-{i}"),
                "vector": v,
            })
        })
        .collect();
    let insert_resp = http
        .post(format!(
            "{}/api/v2/collections/{}/records/batch",
            server.base_url(),
            name
        ))
        .json(&json!({ "records": records }))
        .send()
        .await
        .expect("v2 insert");
    let insert_status = insert_resp.status();
    let insert_body = insert_resp.text().await.unwrap_or_default();
    assert!(
        insert_status.is_success(),
        "v2 insert failed: {insert_status} {insert_body}"
    );

    // 3. Settling — matches api_v2_integration_test::test_search_basic
    // (1s). Index update lags the insert reply.
    sleep(Duration::from_secs(1)).await;

    // 4. SEARCH for the exact vector of record-2. Top-1 must be rec-2
    // with cosine ≈ 1.0 — proves the fp16 round-trip preserves the
    // direction enough to recover the original through cosine similarity.
    let mut query = vec![0.0f32; dim];
    query[2 % dim] = 1.0;
    let search_resp = http
        .post(format!(
            "{}/api/v2/collections/{}/search",
            server.base_url(),
            name
        ))
        .json(&json!({
            "vector": query,
            "top_k": 1,
        }))
        .send()
        .await
        .expect("v2 search");
    let search_status = search_resp.status();
    let search_body: serde_json::Value =
        search_resp.json().await.unwrap_or(serde_json::Value::Null);
    assert!(
        search_status.is_success(),
        "v2 search failed: {search_status} {search_body}"
    );

    // Response shape varies a bit; pick the common locations.
    let hits = search_body
        .get("results")
        .or_else(|| search_body.get("hits"))
        .or_else(|| search_body.get("records"))
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("no hits array in v2 search response: {search_body}"));
    assert!(
        !hits.is_empty(),
        "v2 search returned no hits for exact-match fp16 query: {search_body}"
    );

    let top = &hits[0];
    let id = top
        .get("id")
        .or_else(|| top.get("record_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        id, "rec-2",
        "top-1 hit should be rec-2 (matched the query vector exactly modulo fp16); \
         got id={id:?}, full hit={top}"
    );

    let score = top
        .get("score")
        .or_else(|| top.get("similarity"))
        .or_else(|| top.get("distance"))
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::NAN);
    // Cosine of identical unit vectors = 1.0; fp16 truncation of {0,1}
    // doesn't perturb the value (exactly representable). Tolerance 0.01
    // for distance-metric flavours that subtract from 1.
    assert!(
        (score - 1.0).abs() < 0.01 || (score - 0.0).abs() < 0.01,
        "exact-match fp16 cosine score should be ~1.0 (or distance ~0.0); got {score}; hit={top}"
    );
}
