//! End-to-end fp16 collection validation through the **Rust SDK** against
//! a real ProximaDB server.
//!
//! Closes the cross-language gap on the Rust side: the Rust SDK has unit
//! tests on the builder/payload shape (clients/rust/src/collection.rs)
//! but no integration coverage that proves a live SDK call round-trips
//! through the server with fp16 preserved. This test boots an in-process
//! server on a free REST port and drives the actual `CollectionBuilder`.

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use proximadb_sdk::{EmbeddingPrecision, ProximaClient};
use tempfile::TempDir;
use tokio::time::sleep;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

struct RustSdkTestServer {
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl RustSdkTestServer {
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
                        anyhow::bail!("REST server didn't become ready within 15s");
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

impl Drop for RustSdkTestServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

/// Drive `ProximaClient::create_collection().precision(Fp16).execute()`
/// against a live server and cross-verify the catalog row reports Fp16.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rust_sdk_create_collection_with_fp16_precision_round_trips() {
    let server = RustSdkTestServer::start().await.expect("server start");

    let client = ProximaClient::connect(server.base_url()).expect("ProximaClient");

    let name = format!(
        "rust_sdk_fp16_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    client
        .create_collection(&name)
        .dimension(8)
        .precision(EmbeddingPrecision::Fp16)
        .execute()
        .await
        .expect("SDK create_collection");

    // Cross-protocol verify via raw REST so we assert the SERVER's row,
    // not whatever the SDK echoed back from the request shape.
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .unwrap();
    let resp = http_client
        .get(format!("{}/api/v1/collections/{}", server.base_url(), name))
        .send()
        .await
        .expect("REST GET");
    assert!(
        resp.status().is_success(),
        "REST GET failed: {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);

    let cfg = body
        .get("collection")
        .and_then(|c| c.get("config"))
        .or_else(|| body.get("config"))
        .unwrap_or_else(|| panic!("missing collection.config in body: {body}"));
    let precision = cfg
        .get("canonical_embedding_precision")
        .expect("collection.config has canonical_embedding_precision after Rust SDK create");

    let matches_fp16 = match precision {
        serde_json::Value::String(s) => {
            s == "EMBEDDING_PRECISION_FP16" || s == "FP16" || s == "Fp16" || s == "fp16"
        }
        serde_json::Value::Number(n) => n.as_i64() == Some(2),
        _ => false,
    };
    assert!(
        matches_fp16,
        "Rust SDK create_collection with EmbeddingPrecision::Fp16 must persist as Fp16; \
         REST GET returned: {precision:?}"
    );
}
