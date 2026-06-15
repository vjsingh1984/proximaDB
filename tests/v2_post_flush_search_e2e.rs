//! Characterization: does a v2-inserted record stay retrievable **after a
//! real WAL→storage flush**?
//!
//! Prior handoffs asserted a "v2 INSERT→search-index flush gap": records served
//! from the WAL memtable while unflushed, but dropped from `search`/`scan` once
//! flushed (because flush did not register them with the searchable index). That
//! claim was never pinned by a test — the cited `fp16_v2_insert_search_e2e`
//! waits only ~1s and never forces a flush, so it only ever exercised the
//! WAL-resident path.
//!
//! This test forces the flush explicitly (`ProximaDB::force_flush_collection`,
//! which drains the WAL so subsequent reads must come from persisted storage)
//! and re-runs the SAME search + filtered scan. It is the regression gate for
//! the post-flush read path on the going-forward v2 surface. If the flush gap is
//! real, the post-flush assertions fail and this file is the minimal repro; if
//! it passes, the post-flush path is locked in.
//!
//! Two collections are covered because the v2 write path forks on
//! `enable_proxima_record`:
//!   - `false` → canonical vector-record path (the fp16 round-trip path),
//!   - `true`  → ProximaRecord path (the metadata-filter path; what the hybrid
//!     filter enforcement reads via `list_all_records_with_tenant_context`).

use std::collections::BTreeSet;
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

struct TestServer {
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl TestServer {
    async fn start() -> anyhow::Result<Self> {
        let rest_port = free_port();
        let tmp_data = TempDir::new()?;

        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp_data.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = free_port();
        config.api.arrow_flight_port = free_port();
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

    /// Drain the collection's WAL/memtable into persisted storage so the next
    /// read cannot be served from memory.
    async fn force_flush(&self, collection: &str) -> anyhow::Result<()> {
        self.db
            .as_ref()
            .expect("db present")
            .force_flush_collection(collection)
            .await
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

fn ids(values: &[serde_json::Value]) -> BTreeSet<String> {
    values
        .iter()
        .filter_map(|v| {
            v.get("id")
                .or_else(|| v.get("record_id"))
                .and_then(|id| id.as_str())
                .map(str::to_string)
        })
        .collect()
}

/// enable_proxima_record=false (canonical vector path): exact-match search must
/// return the same top-1 hit before AND after a forced flush.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_vector_search_survives_flush() {
    let server = TestServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let base = server.base_url();
    let name = format!(
        "v2_flush_vec_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dim: usize = 8;
    let n: usize = 5;

    let create = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": name,
            "dimension": dim,
            "engine": "sst",
            "distance_metric": "cosine",
            "enable_proxima_record": false,
        }))
        .send()
        .await
        .expect("create");
    assert!(
        create.status().is_success(),
        "create: {} {}",
        create.status(),
        create.text().await.unwrap_or_default()
    );

    // One-hot directions so cosine cleanly separates them; rec-2 = e_2.
    let records: Vec<serde_json::Value> = (0..n)
        .map(|i| {
            let mut v = vec![0.0f32; dim];
            v[i % dim] = 1.0;
            json!({ "id": format!("rec-{i}"), "vector": v })
        })
        .collect();
    let insert = http
        .post(format!("{base}/api/v2/collections/{name}/records/batch"))
        .json(&json!({ "records": records }))
        .send()
        .await
        .expect("insert");
    assert!(
        insert.status().is_success(),
        "insert: {} {}",
        insert.status(),
        insert.text().await.unwrap_or_default()
    );
    sleep(Duration::from_millis(500)).await;

    let mut query = vec![0.0f32; dim];
    query[2 % dim] = 1.0;
    let search = |q: Vec<f32>| {
        let http = http.clone();
        let url = format!("{base}/api/v2/collections/{name}/search");
        async move {
            let resp = http
                .post(url)
                .json(&json!({ "vector": q, "top_k": 1 }))
                .send()
                .await
                .expect("search");
            assert!(resp.status().is_success(), "search status {}", resp.status());
            let body: serde_json::Value = resp.json().await.expect("search json");
            let hits = body
                .get("results")
                .or_else(|| body.get("hits"))
                .or_else(|| body.get("records"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            (hits, body)
        }
    };

    // Pre-flush sanity: WAL-resident search returns rec-2 (proven path).
    let (pre_hits, pre_body) = search(query.clone()).await;
    let pre_top = pre_hits
        .first()
        .and_then(|h| h.get("id").or_else(|| h.get("record_id")))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(pre_top, "rec-2", "pre-flush top-1 should be rec-2: {pre_body}");

    // Force the WAL→storage flush, then re-run the identical search.
    server.force_flush(&name).await.expect("force flush");
    sleep(Duration::from_millis(500)).await;

    let (post_hits, post_body) = search(query).await;
    assert!(
        !post_hits.is_empty(),
        "POST-FLUSH search returned no hits — the flush gap is real: {post_body}"
    );
    let post_top = post_hits
        .first()
        .and_then(|h| h.get("id").or_else(|| h.get("record_id")))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        post_top, "rec-2",
        "POST-FLUSH top-1 should still be rec-2: {post_body}"
    );
}

/// enable_proxima_record=true (ProximaRecord path): a metadata-filtered scan —
/// the exact read path the hybrid BM25 filter enforcement reuses — must return
/// the same matching set before AND after a forced flush.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_filtered_scan_survives_flush() {
    let server = TestServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let base = server.base_url();
    let name = format!(
        "v2_flush_scan_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let create = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": name,
            "dimension": 1,
            "enable_proxima_record": true,
            "schema": {
                "columns": [
                    { "name": "account_id", "data_type": "text", "indexed": true, "filterable": true }
                ],
                "enforcement": "hybrid",
                "allow_additional_fields": true
            }
        }))
        .send()
        .await
        .expect("create");
    assert!(
        create.status().is_success(),
        "create: {} {}",
        create.status(),
        create.text().await.unwrap_or_default()
    );

    let insert = http
        .post(format!("{base}/api/v2/collections/{name}/records/batch"))
        .json(&json!({
            "records": [
                { "id": "a1", "vector": [0.0], "props": { "account_id": "acctA" } },
                { "id": "a2", "vector": [0.0], "props": { "account_id": "acctA" } },
                { "id": "b1", "vector": [0.0], "props": { "account_id": "acctB" } }
            ],
            "upsert": true
        }))
        .send()
        .await
        .expect("insert");
    assert!(
        insert.status().is_success(),
        "insert: {} {}",
        insert.status(),
        insert.text().await.unwrap_or_default()
    );
    sleep(Duration::from_millis(500)).await;

    let expected: BTreeSet<String> = ["a1".to_string(), "a2".to_string()].into_iter().collect();
    let scan = || {
        let http = http.clone();
        let url = format!("{base}/api/v2/collections/{name}/records/scan");
        async move {
            let resp = http
                .post(url)
                .json(&json!({ "limit": 10, "filter": { "account_id": "acctA" } }))
                .send()
                .await
                .expect("scan");
            assert!(resp.status().is_success(), "scan status {}", resp.status());
            let body: serde_json::Value = resp.json().await.expect("scan json");
            let recs = body
                .get("records")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            (ids(&recs), body)
        }
    };

    // Pre-flush: WAL-resident filtered scan returns exactly acctA.
    let (pre_ids, pre_body) = scan().await;
    assert_eq!(pre_ids, expected, "pre-flush filtered scan: {pre_body}");

    // Force the WAL→storage flush, then re-run the identical filtered scan.
    server.force_flush(&name).await.expect("force flush");
    sleep(Duration::from_millis(500)).await;

    let (post_ids, post_body) = scan().await;
    assert_eq!(
        post_ids, expected,
        "POST-FLUSH filtered scan must still return exactly acctA records \
         (the path hybrid filter enforcement reuses): {post_body}"
    );
}
