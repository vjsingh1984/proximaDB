//! TD-135 — relational WRITES (DDL + DML) over gRPC SQL are tenant-isolated, e2e.
//!
//! Companion to `grpc_td121_relational_tenant_isolation_e2e` (which proved reads).
//! Here every statement — `CREATE TABLE`, `INSERT`, `DELETE`, `SELECT` — is driven
//! over the gRPC `ProximaRecordService.ExecuteQuery` RPC with the tenant carried in
//! the `x-tenant-id` metadata. Two tenants create the SAME table name and INSERT the
//! SAME primary key; each must see only its own rows, and a DELETE by one tenant must
//! not affect the other — i.e. writes route through the tenant-scoped DDL/DML seams
//! (TD-064 partition scope), not a shared/default partition.

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
            grpc_port,
            db: Some(db),
            _tmp_data: tmp_data,
        })
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

/// Run a SQL statement over gRPC `ExecuteQuery` as `tenant`; returns the response.
async fn grpc_exec(
    grpc_port: u16,
    tenant: &str,
    query: &str,
) -> Result<proximadb::proto::proximadb_v2::V2QueryResponse, tonic::Status> {
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
    client.execute_query(request).await.map(|r| r.into_inner())
}

/// Number of rows returned by a SELECT over gRPC as `tenant`.
async fn grpc_select_count(grpc_port: u16, tenant: &str, query: &str) -> u64 {
    grpc_exec(grpc_port, tenant, query)
        .await
        .expect("gRPC SELECT")
        .rows
        .len() as u64
}

#[tokio::test]
async fn grpc_relational_writes_are_tenant_isolated() {
    let server = TestServer::start().await.expect("server start");
    let g = server.grpc_port;
    let table = format!("orders_{g}");
    let ddl = format!("CREATE TABLE {table} (id TEXT NOT NULL, note TEXT, PRIMARY KEY (id))");

    // DDL over gRPC, per tenant (same table name in each tenant's catalog).
    grpc_exec(g, "acme", &ddl).await.expect("acme CREATE TABLE");
    grpc_exec(g, "globex", &ddl)
        .await
        .expect("globex CREATE TABLE");

    // DML over gRPC: same PK '1' in both tenants must coexist independently.
    grpc_exec(
        g,
        "acme",
        &format!("INSERT INTO {table} (id, note) VALUES ('1','acme-one'),('2','acme-two')"),
    )
    .await
    .expect("acme INSERT");
    grpc_exec(
        g,
        "globex",
        &format!("INSERT INTO {table} (id, note) VALUES ('1','globex-one')"),
    )
    .await
    .expect("globex INSERT (same PK as acme, different tenant)");

    // Reads over gRPC are tenant-scoped: each sees only its own rows.
    assert_eq!(
        grpc_select_count(g, "acme", &format!("SELECT id FROM {table}")).await,
        2,
        "acme must see only its own 2 rows"
    );
    assert_eq!(
        grpc_select_count(g, "globex", &format!("SELECT id FROM {table}")).await,
        1,
        "globex must see only its own 1 row"
    );

    // DELETE over gRPC is tenant-scoped: acme deletes its '2'; globex is untouched.
    grpc_exec(g, "acme", &format!("DELETE FROM {table} WHERE id = '2'"))
        .await
        .expect("acme DELETE");
    assert_eq!(
        grpc_select_count(g, "acme", &format!("SELECT id FROM {table}")).await,
        1,
        "acme now has 1 row after deleting id='2'"
    );
    assert_eq!(
        grpc_select_count(g, "globex", &format!("SELECT id FROM {table}")).await,
        1,
        "globex still has its 1 row — acme's DELETE did not cross tenants"
    );

    // Same PK '1' resolves to each tenant's OWN row (no cross-tenant bleed).
    let acme_note = grpc_exec(
        g,
        "acme",
        &format!("SELECT note FROM {table} WHERE id = '1'"),
    )
    .await
    .expect("acme point read");
    let globex_note = grpc_exec(
        g,
        "globex",
        &format!("SELECT note FROM {table} WHERE id = '1'"),
    )
    .await
    .expect("globex point read");
    assert_eq!(acme_note.rows.len(), 1);
    assert_eq!(globex_note.rows.len(), 1);
}
