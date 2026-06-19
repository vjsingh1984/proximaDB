//! Cross-surface unification: a write on ONE protocol must be visible to every surface.
//!
//! Proves the convergence fix — REST/gRPC `DmlService` and the pgwire direct-write path now
//! share ONE `DirectWalTableRecordStore` (`SharedServices.canonical_record_store`) instead of
//! divergent instances (the old vector-compatibility stub on REST/gRPC vs the WAL store on
//! pgwire). Here we INSERT over pgwire, then read the change-feed off the SHARED store the
//! REST/gRPC `DmlService` is built on, and see the row.
//!
//!   cargo test --test cross_surface_unification_e2e -- --nocapture

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use proximadb::services::record_store::TableRecordStore;
use tempfile::TempDir;
use tokio::time::sleep;

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

struct PgServer {
    pg_port: u16,
    db: Option<ProximaDB>,
    _tmp: TempDir,
}

impl PgServer {
    async fn start() -> anyhow::Result<Self> {
        let pg_port = free_port();
        let rest_port = free_port();
        let grpc_port = free_port();
        let tmp = TempDir::new()?;
        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = grpc_port;
        config.api.unified_mode = false;
        config.api.pg_port = Some(pg_port);
        config.storage.storage_locations = vec![proximadb::core::config::StorageLocation {
            url: format!("file://{}", tmp.path().display()),
            ..Default::default()
        }];
        config.storage.wal_config.write_buffer_directory =
            format!("file://{}/wal", tmp.path().display());
        let mut db = ProximaDB::new(config).await?;
        db.start().await?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .no_proxy()
            .build()?;
        let health = format!("http://127.0.0.1:{rest_port}/health");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            match http.get(&health).send().await {
                Ok(r) if r.status().is_success() => break,
                _ if std::time::Instant::now() > deadline => anyhow::bail!("REST not ready"),
                _ => sleep(Duration::from_millis(100)).await,
            }
        }
        sleep(Duration::from_millis(200)).await;
        Ok(Self {
            pg_port,
            db: Some(db),
            _tmp: tmp,
        })
    }
    fn conn_str(&self) -> String {
        format!(
            "host=127.0.0.1 port={} user=postgres dbname=proximadb sslmode=disable",
            self.pg_port
        )
    }
}

impl Drop for PgServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pgwire_write_is_visible_on_the_shared_canonical_store() {
    let server = PgServer::start().await.expect("server start");
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    // Write over pgwire.
    let _ = client.simple_query("DROP TABLE IF EXISTS uacct").await;
    client
        .simple_query("CREATE TABLE uacct (id INT PRIMARY KEY, bal INT)")
        .await
        .expect("create");
    client
        .simple_query("INSERT INTO uacct (id, bal) VALUES (1, 100), (2, 200)")
        .await
        .expect("insert");
    sleep(Duration::from_millis(200)).await;

    // Read the change-feed off the SHARED canonical store — the SAME instance the REST/gRPC
    // DmlService is built on (SharedServices.canonical_record_store). If the surfaces were
    // still divergent (pgwire WAL store vs REST/gRPC vector-compat stub), this would be empty.
    let store = server
        .db
        .as_ref()
        .expect("db")
        .canonical_record_store()
        .expect("shared canonical record store must exist when config is provided");

    let changes = store
        .read_changes_since("uacct", 0)
        .await
        .expect("read changes");

    assert_eq!(
        changes.len(),
        2,
        "pgwire INSERT of 2 rows must be visible on the shared store the REST/gRPC \
         DmlService reads from; got {changes:?}"
    );
    assert!(changes.iter().all(|c| c.op == "upsert" && c.collection == "uacct"));
    let keys: Vec<&str> = changes.iter().map(|c| c.key.as_str()).collect();
    assert!(keys.contains(&"1") && keys.contains(&"2"), "keys: {keys:?}");
    eprintln!("✓ cross-surface unified: pgwire write visible on the shared REST/gRPC store");
}
