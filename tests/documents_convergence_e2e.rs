//! ADR-009 document store-convergence — cross-surface visibility, e2e over the real
//! network transports (REST + gRPC), never calling an engine directly.
//!
//! The document modality historically had a **store-split**: REST v2 document ingest wrote
//! to the per-collection record/vector store, while the gRPC `ProximaDocumentService` +
//! `DocumentService` wrote to their own `document_wal` — so a document written on one
//! surface was invisible on the other. This suite proves the convergence:
//!
//! - **Gate ON** (`PROXIMADB_DOC_CANONICAL_VECTOR` = collection): a document ingested via
//!   REST v2 is visible to a gRPC `GetDocument` — one store, cross-surface. This is the
//!   headline convergence claim.
//! - **Gate OFF** (default): the gRPC/`DocumentService` legacy path round-trips unchanged
//!   (create → get), proving the default-OFF gate preserves today's behavior.
//!
//! One ProximaDB boot per process (the WAL manifest is a set-once singleton); nextest runs
//! each test in its own process, so the per-test env gate + boot are isolated.

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use proximadb::proto::proximadb_v2::proxima_document_service_client::ProximaDocumentServiceClient;
use proximadb::proto::proximadb_v2::{CreateDocumentRequest, GetDocumentRequest};
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
    grpc_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl TestServer {
    async fn start() -> anyhow::Result<Self> {
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
        config.api.unified_mode = false;
        config.api.pg_port = Some(pg_port);
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
        let health_url = format!("http://127.0.0.1:{rest_port}/health");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            match http.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => {
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!("REST server didn't become ready within 20s");
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }
        sleep(Duration::from_millis(300)).await;

        Ok(Self {
            rest_port,
            grpc_port,
            db: Some(db),
            _tmp_data: tmp_data,
        })
    }

    fn rest(&self) -> String {
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

/// Scope the process-global gate to `collection` for the test; removes it on drop.
struct GateGuard;
impl GateGuard {
    fn on(collection: &str) -> Self {
        // SAFETY (edition 2024): nextest runs each test in its own process, so this env
        // mutation is single-threaded for the process.
        unsafe { std::env::set_var("PROXIMADB_DOC_CANONICAL_VECTOR", collection) };
        Self
    }
}
impl Drop for GateGuard {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("PROXIMADB_DOC_CANONICAL_VECTOR") };
    }
}

/// Gate ON: a REST-v2-ingested document is visible to a gRPC `GetDocument` — one store,
/// cross-surface (the headline convergence claim).
#[tokio::test]
async fn rest_ingested_document_is_visible_via_grpc_when_gate_on() {
    let collection = "convdocs";
    let _gate = GateGuard::on(collection);
    let server = TestServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .expect("http client");

    // Create the vector collection the document route resolves against.
    let create = http
        .post(format!("{}/api/v2/collections", server.rest()))
        .json(&json!({"name": collection, "dimension": 8, "engine": "sst"}))
        .send()
        .await
        .expect("create collection");
    assert!(
        create.status().is_success(),
        "collection create should succeed, got {}",
        create.status()
    );

    // Ingest a document via REST v2 with an SDK-provided vector (no embedding backend needed).
    let ingest = http
        .post(format!(
            "{}/api/v2/collections/{collection}/documents",
            server.rest()
        ))
        .header("X-Embed-Source", "sdk-vector")
        .header("X-Ingest-Mode", "sync")
        .json(&json!({
            "records": [{
                "id": "doc-1",
                "vector": [0.1_f32, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8],
                "metadata": {"title": "Alpha"}
            }]
        }))
        .send()
        .await
        .expect("ingest document");
    assert!(
        ingest.status().is_success(),
        "document ingest should succeed, got {} body {:?}",
        ingest.status(),
        ingest.text().await.ok()
    );

    // Read it back over gRPC ProximaDocumentService — the other surface.
    let mut client =
        ProximaDocumentServiceClient::connect(format!("http://127.0.0.1:{}", server.grpc_port))
            .await
            .expect("gRPC connect");
    let resp = client
        .get_document(GetDocumentRequest {
            collection_id: collection.to_string(),
            id: "doc-1".to_string(),
        })
        .await
        .expect("gRPC GetDocument should find the REST-ingested doc (cross-surface)")
        .into_inner();
    let doc = resp.document.expect("document present");
    assert_eq!(doc.id, "doc-1", "gRPC sees the REST-ingested document id");
}

/// Gate OFF (default): the gRPC/DocumentService legacy path round-trips unchanged
/// (create → get), so the default-OFF gate preserves today's behavior bit-for-bit.
#[tokio::test]
async fn grpc_document_round_trips_on_legacy_path_when_gate_off() {
    let server = TestServer::start().await.expect("server start");
    let mut client =
        ProximaDocumentServiceClient::connect(format!("http://127.0.0.1:{}", server.grpc_port))
            .await
            .expect("gRPC connect");

    client
        .create_document(CreateDocumentRequest {
            collection_id: "legacydocs".to_string(),
            id: "doc-1".to_string(),
            ..Default::default()
        })
        .await
        .expect("gRPC CreateDocument (legacy path)");

    let resp = client
        .get_document(GetDocumentRequest {
            collection_id: "legacydocs".to_string(),
            id: "doc-1".to_string(),
        })
        .await
        .expect("gRPC GetDocument (legacy path)")
        .into_inner();
    let doc = resp.document.expect("document present via legacy path");
    assert_eq!(doc.id, "doc-1");
}
