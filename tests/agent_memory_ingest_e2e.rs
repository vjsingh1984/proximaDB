//! TD-101 sub-slice e2e — `POST /api/v1/memory/ingest` over the real REST
//! transport. Boots an in-process ProximaDB with a free REST port, creates a
//! collection, and posts an agent turn.
//!
//! CI reality: extraction + consolidation need an LLM backend, which the test
//! deployment does not configure (no API key). So this test asserts the
//! WIRING + availability guard: the route is mounted and returns a structured
//! "LLM backend required" error (HTTP 501) rather than 404/panic. If an LLM
//! key is present in the environment, the happy path (HTTP 200 + applied
//! actions) is asserted instead. The deterministic correctness of the
//! orchestration + parsing + record-building is covered by the unit tests in
//! `src/services/agent_memory.rs`.
//!
//! One ProximaDB boot per process (global WAL manifest is a set-once singleton).

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

struct RestTestServer {
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl RestTestServer {
    async fn start() -> anyhow::Result<Self> {
        let rest_port = free_port();
        let grpc_port = free_port();
        let tmp_data = TempDir::new()?;

        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp_data.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = grpc_port;
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
            .timeout(Duration::from_secs(3))
            .no_proxy()
            .build()?;
        let health = format!("http://127.0.0.1:{rest_port}/health");
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            match http.get(&health).send().await {
                Ok(r) if r.status().is_success() => break,
                _ => {
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!("REST not ready on {rest_port} within 15s");
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }
        sleep(Duration::from_millis(150)).await;

        Ok(Self {
            rest_port,
            db: Some(db),
            _tmp_data: tmp_data,
        })
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.rest_port)
    }
}

impl Drop for RestTestServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

/// Does the environment carry any LLM provider key the engine could use?
fn llm_configured() -> bool {
    ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "COHERE_API_KEY"]
        .iter()
        .any(|k| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_ingest_route_is_mounted_and_guarded() {
    let server = RestTestServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .expect("client");

    let collection = format!("mem_ingest_{}", uuid::Uuid::new_v4());
    let _ = http
        .post(format!("{}/api/v2/collections", server.base()))
        .json(&json!({"name": collection, "dimension": 384, "engine": "sst"}))
        .send()
        .await;

    let resp = http
        .post(format!("{}/api/v1/memory/ingest", server.base()))
        .json(&json!({
            "collection": collection,
            "tenant_id": "default",
            "actor": "test-agent",
            "session_id": "sess-e2e",
            "user": "I prefer dark mode and live in NYC.",
            "assistant": "Noted — dark mode on, based in New York."
        }))
        .send()
        .await
        .expect("ingest request sent");

    let status = resp.status();
    // The route must exist (never 404) and never 5xx-crash the server.
    assert_ne!(status.as_u16(), 404, "route should be mounted");

    if llm_configured() {
        assert!(
            status.is_success(),
            "with an LLM backend, ingest should succeed; got {status}"
        );
        let body: serde_json::Value = resp.json().await.expect("json body");
        assert!(
            body.get("applied").is_some(),
            "response carries applied actions"
        );
    } else {
        // No LLM backend in CI: the availability guard returns 501 Not Implemented.
        assert_eq!(
            status.as_u16(),
            501,
            "without an LLM backend the guard should return 501, got {status}"
        );
    }
}
