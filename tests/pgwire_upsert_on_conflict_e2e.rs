//! TD-129 — pgwire `INSERT ... ON CONFLICT DO UPDATE` upsert, end-to-end.
//!
//! The code-graph re-index hot path re-pushes a file's symbol rows every time
//! the watcher fires. Without `ON CONFLICT` the idempotent re-push had to
//! read-then-write; this proves the SQL surface now does it in one statement and
//! that a re-push **updates the row in place** rather than duplicating it.
//!
//! Coverage:
//!  * `DO UPDATE SET col = excluded.col` — re-push with changed values leaves
//!    exactly one row, carrying the new values.
//!  * `DO NOTHING` — a conflicting re-push is skipped; the row is unchanged and
//!    still single.
//!  * the conflict key (PRIMARY KEY) drives identity, so no row is duplicated.

use std::net::TcpListener;
use std::time::Duration;

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

struct PgwireTestServer {
    pg_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl PgwireTestServer {
    async fn start() -> anyhow::Result<Self> {
        unsafe {
            std::env::set_var("PROXIMADB_EMBED_PRECISION_SCHEMA_V2", "true");
        }

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
                        anyhow::bail!(
                            "REST server didn't become ready on port {} within 20s",
                            rest_port
                        );
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }
        sleep(Duration::from_millis(200)).await;

        Ok(Self {
            pg_port,
            db: Some(db),
            _tmp_data: tmp_data,
        })
    }

    fn pg_connection_string(&self) -> String {
        format!(
            "host=127.0.0.1 port={} user=postgres dbname=proximadb sslmode=disable",
            self.pg_port
        )
    }
}

impl Drop for PgwireTestServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

/// Collect the data rows of a SELECT as strings (one inner `Vec` per row).
async fn query_rows(client: &tokio_postgres::Client, sql: &str) -> Vec<Vec<Option<String>>> {
    let messages = client.simple_query(sql).await.expect("pgwire query");
    let mut rows = Vec::new();
    for message in messages {
        if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
            let mut values = Vec::with_capacity(row.columns().len());
            for idx in 0..row.columns().len() {
                values.push(row.get(idx).map(|s| s.to_string()));
            }
            rows.push(values);
        }
    }
    rows
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pgwire_on_conflict_do_update_is_idempotent_and_in_place() {
    let server = PgwireTestServer::start().await.expect("server start");

    let (client, connection) =
        tokio_postgres::connect(&server.pg_connection_string(), tokio_postgres::NoTls)
            .await
            .expect("tokio-postgres connect");
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("pgwire connection error: {e}");
        }
    });

    let table = format!(
        "pgw_upsert_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    client
        .simple_query(&format!(
            "CREATE TABLE {table} (id VARCHAR PRIMARY KEY, title VARCHAR, hits BIGINT)"
        ))
        .await
        .expect("CREATE TABLE");

    // Initial push of the symbol row.
    client
        .simple_query(&format!(
            "INSERT INTO {table} (id, title, hits) VALUES ('sym-1', 'first', 1)"
        ))
        .await
        .expect("first INSERT");

    // Idempotent re-push with changed payload via ON CONFLICT DO UPDATE.
    client
        .simple_query(&format!(
            "INSERT INTO {table} (id, title, hits) VALUES ('sym-1', 'second', 5) \
             ON CONFLICT (id) DO UPDATE SET title = excluded.title, hits = excluded.hits"
        ))
        .await
        .expect("upsert DO UPDATE");

    sleep(Duration::from_millis(300)).await;

    // Exactly one row for the key, carrying the UPDATED values (not duplicated).
    let rows = query_rows(
        &client,
        &format!("SELECT title, hits FROM {table} WHERE id = 'sym-1'"),
    )
    .await;
    assert_eq!(
        rows.len(),
        1,
        "re-upsert must leave exactly one row for the key, got {rows:?}"
    );
    assert_eq!(
        rows[0][0].as_deref(),
        Some("second"),
        "DO UPDATE must apply excluded.title; got {rows:?}"
    );
    assert_eq!(
        rows[0][1].as_deref(),
        Some("5"),
        "DO UPDATE must apply excluded.hits; got {rows:?}"
    );

    let all = query_rows(&client, &format!("SELECT id FROM {table}")).await;
    assert_eq!(all.len(), 1, "table must hold a single row, got {all:?}");

    // DO NOTHING: a conflicting re-push is skipped, row stays as last updated.
    client
        .simple_query(&format!(
            "INSERT INTO {table} (id, title, hits) VALUES ('sym-1', 'ignored', 999) \
             ON CONFLICT (id) DO NOTHING"
        ))
        .await
        .expect("upsert DO NOTHING");

    sleep(Duration::from_millis(300)).await;

    let rows = query_rows(
        &client,
        &format!("SELECT title, hits FROM {table} WHERE id = 'sym-1'"),
    )
    .await;
    assert_eq!(rows.len(), 1, "DO NOTHING must not duplicate, got {rows:?}");
    assert_eq!(
        rows[0][0].as_deref(),
        Some("second"),
        "DO NOTHING must leave the row unchanged; got {rows:?}"
    );

    // A non-conflicting upsert inserts a brand-new row.
    client
        .simple_query(&format!(
            "INSERT INTO {table} (id, title, hits) VALUES ('sym-2', 'fresh', 7) \
             ON CONFLICT (id) DO UPDATE SET title = excluded.title"
        ))
        .await
        .expect("upsert new key");

    sleep(Duration::from_millis(300)).await;

    let all = query_rows(&client, &format!("SELECT id FROM {table}")).await;
    assert_eq!(
        all.len(),
        2,
        "a non-conflicting upsert must insert a new row, got {all:?}"
    );
}
