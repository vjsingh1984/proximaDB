//! pgwire relational pipeline (PATH B) over REAL data — joins & aggregates.
//!
//! The legacy single-table SELECT path can't do JOIN / GROUP BY / aggregates.
//! This test proves the gated, additive wiring: queries that engage those
//! features route through the algebra engine (frontend → planner → executor)
//! reading REAL `RecordStorage` rows inserted over pgwire, while simple
//! single-table SELECTs stay on the legacy path. Default config (the new
//! pipeline is on by default; it falls through to legacy for queries the gate
//! doesn't engage and for pg-specific syntax).

use std::collections::BTreeSet;
use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio_postgres::SimpleQueryMessage;

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

/// First value of column `col` across the rows, as text.
fn scalar(messages: &[SimpleQueryMessage], col: &str) -> Option<String> {
    messages.iter().find_map(|msg| match msg {
        SimpleQueryMessage::Row(row) => row.get(col).map(|s| s.to_string()),
        _ => None,
    })
}

/// Collect column `col` across all rows into a sorted set.
fn col_set(messages: &[SimpleQueryMessage], col: &str) -> BTreeSet<String> {
    messages
        .iter()
        .filter_map(|msg| match msg {
            SimpleQueryMessage::Row(row) => row.get(col).map(|s| s.to_string()),
            _ => None,
        })
        .collect()
}

/// Collect `(a, b)` column pairs across all rows into a sorted set.
fn pair_set(messages: &[SimpleQueryMessage], a: &str, b: &str) -> BTreeSet<(String, String)> {
    messages
        .iter()
        .filter_map(|msg| match msg {
            SimpleQueryMessage::Row(row) => {
                Some((row.get(a)?.to_string(), row.get(b)?.to_string()))
            }
            _ => None,
        })
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pgwire_relational_engine_serves_joins_and_aggregates_over_real_data() {
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

    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let inv = format!("inv_{suffix}");
    let dept = format!("dept_{suffix}");
    let emp = format!("emp_{suffix}");

    // --- inv: single-table aggregate / GROUP BY + simple-OR regression ---
    client
        .simple_query(&format!(
            "CREATE TABLE {inv} (id VARCHAR PRIMARY KEY, status VARCHAR, qty BIGINT)"
        ))
        .await
        .expect("CREATE inv");
    for (id, status, qty) in [
        ("i1", "active", 5),
        ("i2", "active", 15),
        ("i3", "idle", 25),
        ("i4", "idle", 35),
    ] {
        client
            .simple_query(&format!(
                "INSERT INTO {inv} (id, status, qty) VALUES ('{id}', '{status}', {qty})"
            ))
            .await
            .expect("INSERT inv");
    }

    // --- dept / emp: 2-table INNER JOIN ---
    client
        .simple_query(&format!(
            "CREATE TABLE {dept} (id BIGINT PRIMARY KEY, dname VARCHAR)"
        ))
        .await
        .expect("CREATE dept");
    client
        .simple_query(&format!(
            "CREATE TABLE {emp} (id BIGINT PRIMARY KEY, dept_id BIGINT, ename VARCHAR)"
        ))
        .await
        .expect("CREATE emp");
    for (id, dname) in [(1, "eng"), (2, "sales")] {
        client
            .simple_query(&format!(
                "INSERT INTO {dept} (id, dname) VALUES ({id}, '{dname}')"
            ))
            .await
            .expect("INSERT dept");
    }
    for (id, dept_id, ename) in [(10, 1, "ann"), (11, 1, "bob"), (12, 2, "cas")] {
        client
            .simple_query(&format!(
                "INSERT INTO {emp} (id, dept_id, ename) VALUES ({id}, {dept_id}, '{ename}')"
            ))
            .await
            .expect("INSERT emp");
    }

    sleep(Duration::from_millis(500)).await;

    // (1) COUNT(*) over real rows → 4.
    let rows = client
        .simple_query(&format!("SELECT COUNT(*) AS n FROM {inv}"))
        .await
        .expect("SELECT COUNT(*)");
    assert_eq!(
        scalar(&rows, "n").as_deref(),
        Some("4"),
        "COUNT(*) over real data"
    );

    // (2) GROUP BY over real rows → active:2, idle:2.
    let rows = client
        .simple_query(&format!(
            "SELECT status, COUNT(*) AS n FROM {inv} GROUP BY status"
        ))
        .await
        .expect("SELECT GROUP BY");
    assert_eq!(
        pair_set(&rows, "status", "n"),
        [
            ("active".to_string(), "2".to_string()),
            ("idle".to_string(), "2".to_string())
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "GROUP BY status counts"
    );

    // (2b) Filtered GROUP BY — the WHERE is pushed into the inv scan (the
    // reader is the sole predicate applier), so only qty >= 15 rows are
    // materialized: active:1 (i2), idle:2 (i3,i4).
    let rows = client
        .simple_query(&format!(
            "SELECT status, COUNT(*) AS n FROM {inv} WHERE qty >= 15 GROUP BY status"
        ))
        .await
        .expect("SELECT filtered GROUP BY");
    assert_eq!(
        pair_set(&rows, "status", "n"),
        [
            ("active".to_string(), "1".to_string()),
            ("idle".to_string(), "2".to_string())
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "predicate pushed into the scan yields correct filtered groups"
    );

    // (2c) PK-equality WHERE under an aggregate → the planner pushes id='i2'
    // into the inv scan and rewrites it to ScanAccess::PkLookup (point lookup),
    // exercising the relational reader's lookup_pk over real data. i2 is active,
    // so the single matched row groups as {(active, 1)}.
    let rows = client
        .simple_query(&format!(
            "SELECT status, COUNT(*) AS n FROM {inv} WHERE id = 'i2' GROUP BY status"
        ))
        .await
        .expect("SELECT PK-lookup aggregate");
    assert_eq!(
        pair_set(&rows, "status", "n"),
        [("active".to_string(), "1".to_string())]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "PK point lookup feeds the aggregate with the correct single row"
    );

    // (3) INNER JOIN over real rows from two tables → 3 joined rows.
    let rows = client
        .simple_query(&format!(
            "SELECT ename, dname FROM {emp} JOIN {dept} ON {emp}.dept_id = {dept}.id"
        ))
        .await
        .expect("SELECT JOIN");
    assert_eq!(
        pair_set(&rows, "ename", "dname"),
        [
            ("ann".to_string(), "eng".to_string()),
            ("bob".to_string(), "eng".to_string()),
            ("cas".to_string(), "sales".to_string()),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "INNER JOIN emp×dept"
    );

    // (4) Regression: a simple single-table OR SELECT still returns correct rows
    // (the gate keeps it on the hardened legacy path, not PATH B).
    let rows = client
        .simple_query(&format!(
            "SELECT id FROM {inv} WHERE status = 'active' OR qty >= 35"
        ))
        .await
        .expect("SELECT simple OR");
    assert_eq!(
        col_set(&rows, "id"),
        set(&["i1", "i2", "i4"]),
        "simple OR stays correct on the legacy path"
    );
}
