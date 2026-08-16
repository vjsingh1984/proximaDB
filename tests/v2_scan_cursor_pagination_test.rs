//! Multi-page REST scan pagination — regression coverage for the
//! cursor-identity mismatch ("HTTP 400 after 10000 rows").
//!
//! Cursors were MINTED with the resolved internal `CollectionObjectId`
//! (a decimal u64) but VALIDATED against the URL-path collection name, so
//! the very first follow-up page of every scan failed `CollectionMismatch`
//! → HTTP 400. No test walked page 2 over REST; the cursor crate's own
//! tests pass a consistent id to both sides and cannot see the split.
//!
//! This test walks a 25-record collection in pages of 10 to exhaustion and
//! asserts every record is seen exactly once with no error — then proves
//! the mismatch check still has teeth by replaying a cursor against a
//! different collection (400 expected).

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

async fn create_collection(http: &reqwest::Client, base: &str, name: &str) {
    let create = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": name,
            "dimension": 1,
            "enable_proxima_record": true
        }))
        .send()
        .await
        .expect("create");
    assert!(
        create.status().is_success(),
        "create {name}: {} {}",
        create.status(),
        create.text().await.unwrap_or_default()
    );
}

async fn insert_records(http: &reqwest::Client, base: &str, name: &str, count: usize) {
    let records: Vec<serde_json::Value> = (0..count)
        .map(|i| json!({ "id": format!("r{i:03}"), "vector": [0.0] }))
        .collect();
    let insert = http
        .post(format!("{base}/api/v2/collections/{name}/records/batch"))
        .json(&json!({ "records": records, "upsert": true }))
        .send()
        .await
        .expect("insert");
    assert!(
        insert.status().is_success(),
        "insert {name}: {} {}",
        insert.status(),
        insert.text().await.unwrap_or_default()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_cursor_walks_every_page_to_exhaustion() {
    let server = TestServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let base = server.base_url();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let collection = format!("scanpage_{nanos}");
    let other = format!("scanother_{nanos}");

    create_collection(&http, &base, &collection).await;
    create_collection(&http, &base, &other).await;
    insert_records(&http, &base, &collection, 25).await;
    insert_records(&http, &base, &other, 3).await;
    sleep(Duration::from_millis(500)).await;

    // Walk pages of 10 to exhaustion. Before the identity fix, the second
    // request (first one carrying a cursor) answered 400 CollectionMismatch.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0usize;
    let mut first_cursor: Option<String> = None;
    loop {
        let mut body = json!({ "limit": 10, "include_vector": false });
        if let Some(c) = &cursor {
            body["cursor"] = json!(c);
        }
        let resp = http
            .post(format!(
                "{base}/api/v2/collections/{collection}/records/scan"
            ))
            .json(&body)
            .send()
            .await
            .expect("scan page");
        assert!(
            resp.status().is_success(),
            "page {} must not fail: {} {}",
            pages + 1,
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
        let payload: serde_json::Value = resp.json().await.expect("scan payload");
        let records = payload["records"].as_array().expect("records array");
        for record in records {
            let id = record["id"].as_str().expect("record id").to_string();
            assert!(seen.insert(id.clone()), "record {id} served twice");
        }
        pages += 1;
        assert!(pages <= 10, "runaway pagination — cursor never terminates");

        match payload["next_cursor"].as_str() {
            Some(next) if !next.is_empty() => {
                if first_cursor.is_none() {
                    first_cursor = Some(next.to_string());
                }
                cursor = Some(next.to_string());
            }
            _ => break,
        }
    }

    assert_eq!(seen.len(), 25, "every record exactly once across pages");
    assert!(pages >= 3, "25 records at limit 10 must take >= 3 pages");

    // Teeth: the mismatch check must still reject cross-collection reuse.
    let stolen = first_cursor.expect("multi-page walk produced a cursor");
    let cross = http
        .post(format!("{base}/api/v2/collections/{other}/records/scan"))
        .json(&json!({ "limit": 10, "cursor": stolen }))
        .send()
        .await
        .expect("cross-collection scan");
    assert_eq!(
        cross.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "a cursor minted for one collection must not be honored by another"
    );
}
