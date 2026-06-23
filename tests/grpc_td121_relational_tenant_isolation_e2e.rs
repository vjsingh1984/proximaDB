//! TD-121 — relational SQL over gRPC is tenant-isolated, end-to-end.
//!
//! Companion to `pgwire_td064_write_tenant_isolation_e2e`. Two tenants create
//! the SAME table name with different rows via pgwire (tenant = startup
//! `database`). Then the gRPC `ProximaRecordService.ExecuteQuery` RPC runs
//! RELATIONAL SQL (`SELECT … FROM <table>`, `COUNT(*)`) carrying the tenant in
//! the `x-tenant-id` metadata. The reads must:
//!   1. actually execute (previously gRPC SQL returned "Relational execution not
//!      yet supported"), and
//!   2. return ONLY the calling tenant's rows — the same TD-064 partition scope
//!      pgwire enforces, now reached through the shared relational pipeline.

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use proximadb::proto::proximadb_v2::V2QueryRequest;
use proximadb::proto::proximadb_v2::proxima_record_service_client::ProximaRecordServiceClient;
use tempfile::TempDir;
use tokio::time::sleep;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

struct TestServer {
    pg_port: u16,
    grpc_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl TestServer {
    async fn start() -> anyhow::Result<Self> {
        let pg_port = free_port();
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
        config.api.pg_port = Some(pg_port);
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
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            match http_client.get(&health_url).send().await {
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
            pg_port,
            grpc_port,
            db: Some(db),
            _tmp_data: tmp_data,
        })
    }

    fn conn_string_for(&self, database: &str) -> String {
        format!(
            "host=127.0.0.1 port={} user=postgres dbname={} sslmode=disable",
            self.pg_port, database
        )
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

/// Connect a pgwire client scoped to `database` (= tenant) and run a statement.
async fn pg_run(server: &TestServer, database: &str, sql: &str) {
    let (client, connection) =
        tokio_postgres::connect(&server.conn_string_for(database), tokio_postgres::NoTls)
            .await
            .expect("tokio-postgres connect");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.simple_query(sql).await.expect("pgwire statement");
}

/// Run a relational SELECT over gRPC `ExecuteQuery` as `tenant` and return the
/// number of rows returned.
async fn grpc_select_row_count(grpc_port: u16, tenant: &str, query: &str) -> u64 {
    let mut client = ProximaRecordServiceClient::connect(format!("http://127.0.0.1:{grpc_port}"))
        .await
        .expect("gRPC connect");
    let mut request = tonic::Request::new(V2QueryRequest {
        query: query.to_string(),
        collection_id: String::new(),
        limit: None,
        offset: None,
    });
    request
        .metadata_mut()
        .insert("x-tenant-id", tenant.parse().expect("tenant metadata"));
    let response = client
        .execute_query(request)
        .await
        .expect("gRPC ExecuteQuery")
        .into_inner();
    response.rows.len() as u64
}

#[tokio::test]
async fn grpc_relational_select_is_tenant_isolated() {
    let server = TestServer::start().await.expect("server start");
    let table = format!("orders_{}", server.pg_port);
    let ddl = format!("CREATE TABLE {table} (id TEXT NOT NULL, note TEXT, PRIMARY KEY (id));");

    // Tenant acme: two rows. Tenant globex: one row (SAME table name + PK '1').
    pg_run(&server, "acme", &ddl).await;
    pg_run(
        &server,
        "acme",
        &format!("INSERT INTO {table} (id, note) VALUES ('1','acme-one'),('2','acme-two');"),
    )
    .await;
    pg_run(&server, "globex", &ddl).await;
    pg_run(
        &server,
        "globex",
        &format!("INSERT INTO {table} (id, note) VALUES ('1','globex-one');"),
    )
    .await;

    // Full scan over gRPC relational SQL is tenant-scoped: acme sees its 2 rows,
    // globex sees its 1 row — proving the SELECT executes (no "Relational
    // execution not yet supported") AND honors the calling tenant (TD-064).
    let acme_rows =
        grpc_select_row_count(server.grpc_port, "acme", &format!("SELECT id FROM {table}")).await;
    let globex_rows = grpc_select_row_count(
        server.grpc_port,
        "globex",
        &format!("SELECT id FROM {table}"),
    )
    .await;
    assert_eq!(
        acme_rows, 2,
        "acme must see only its own 2 rows over gRPC SQL"
    );
    assert_eq!(
        globex_rows, 1,
        "globex must see only its own 1 row over gRPC SQL"
    );

    // Aggregate (engages the relational engine) is likewise tenant-scoped and
    // returns one COUNT row per call.
    let acme_count = grpc_select_row_count(
        server.grpc_port,
        "acme",
        &format!("SELECT COUNT(*) AS c FROM {table}"),
    )
    .await;
    assert_eq!(acme_count, 1, "COUNT(*) returns a single aggregate row");
}
