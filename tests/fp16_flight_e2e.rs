//! End-to-end fp16 collection validation through the **real Arrow Flight
//! transport** — boots an in-process ProximaDB server with a free Flight
//! port, connects via a real `arrow_flight::FlightServiceClient` over TCP,
//! invokes the `create_collection` `DoAction` with
//! `canonical_embedding_precision = "fp16"`, and verifies the catalog row
//! reflects fp16.
//!
//! Closes the transport gap above `arrow_ipc::service::do_action`, which
//! the unit tests in `src/network/arrow_ipc/service.rs` cover at the
//! in-process API level but never exercise across a real socket.

use std::net::TcpListener;
use std::time::Duration;

use arrow_flight::Action;
use arrow_flight::flight_service_client::FlightServiceClient;
use bytes::Bytes;
use proximadb::core::Config;
use proximadb::database::ProximaDB;
use tempfile::TempDir;
use tokio::time::sleep;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

struct FlightTestServer {
    flight_port: u16,
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl FlightTestServer {
    async fn start() -> anyhow::Result<Self> {
        unsafe {
            std::env::set_var("PROXIMADB_EMBED_PRECISION_SCHEMA_V2", "true");
        }

        let flight_port = free_port();
        let rest_port = free_port();
        let grpc_port = free_port();
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
                        anyhow::bail!(
                            "REST server didn't become ready on port {} within 15s",
                            rest_port
                        );
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }

        // Brief grace for the Arrow IPC listener to start accepting.
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

    fn rest_base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rest_port)
    }
}

impl Drop for FlightTestServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

/// Connect via a real Arrow Flight client and verify that a
/// `create_collection` DoAction with `canonical_embedding_precision = "fp16"`
/// lands on the catalog. Cross-protocol verifies via REST GET, which reads
/// from the same catalog row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flight_create_collection_with_canonical_embedding_precision_fp16() {
    let server = FlightTestServer::start().await.expect("server start");

    let channel = tonic::transport::Endpoint::from_shared(server.flight_url())
        .expect("flight endpoint")
        .connect()
        .await
        .expect("flight channel");
    let mut flight = FlightServiceClient::new(channel);

    let name = format!(
        "flight_fp16_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let body = serde_json::json!({
        "name": name,
        "dimension": 8,
        "engine": "sst",
        "distance_metric": "cosine",
        "canonical_embedding_precision": "fp16",
    });
    let action = Action {
        r#type: "create_collection".to_string(),
        body: Bytes::from(serde_json::to_vec(&body).unwrap()),
    };

    let mut stream = flight
        .do_action(action)
        .await
        .expect("flight do_action")
        .into_inner();

    // Drain the result stream so the server commits.
    use futures::StreamExt;
    while let Some(msg) = stream.next().await {
        msg.expect("flight result frame");
    }

    // Cross-protocol verify via REST GET.
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .unwrap();
    let get_url = format!("{}/api/v1/collections/{}", server.rest_base_url(), name);
    let resp = http_client
        .get(&get_url)
        .send()
        .await
        .expect("REST GET collection");
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    assert!(
        status.is_success(),
        "REST GET after flight create_collection failed: status={status}, body={body}"
    );

    let cfg = body
        .get("collection")
        .and_then(|c| c.get("config"))
        .or_else(|| body.get("config"))
        .unwrap_or_else(|| {
            panic!(
                "expected response to expose collection.config or config; \
                 actual body shape: {body}"
            )
        });
    let precision = cfg
        .get("canonical_embedding_precision")
        .expect("collection.config has canonical_embedding_precision after flight create_collection");

    let matches_fp16 = match precision {
        serde_json::Value::String(s) => {
            s == "EMBEDDING_PRECISION_FP16" || s == "FP16" || s == "Fp16" || s == "fp16"
        }
        serde_json::Value::Number(n) => n.as_i64() == Some(2),
        _ => false,
    };
    assert!(
        matches_fp16,
        "flight create_collection with canonical_embedding_precision='fp16' \
         must persist as Fp16 in the catalog row; REST GET returned: {precision:?}"
    );
}
