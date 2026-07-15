// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-REL-LOWER-6 — a comma-join whose equi-join key appears only inside a
//! top-level `OR` of range predicates (TPC-H Q19 shape) must normalize to an
//! equi-join on the native route, not decline with `0A000 … unnormalized cross
//! join`. The planner factors the shared `c.id = o.cid` out of the disjunction:
//! `(c.id=o.cid ∧ p1) ∨ (c.id=o.cid ∧ p2)` ≡ `c.id=o.cid ∧ (p1 ∨ p2)`.

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
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
async fn rows(client: &tokio_postgres::Client, sql: &str) -> Vec<Vec<String>> {
    let msgs = client.simple_query(sql).await.unwrap_or_else(|e| {
        panic!(
            "{sql}\n  {}",
            e.as_db_error()
                .map(|d| format!("[{}] {}", d.code().code(), d.message()))
                .unwrap_or_else(|| e.to_string())
        )
    });
    msgs.iter()
        .filter_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => Some(
                (0..r.len())
                    .map(|i| r.get(i).unwrap_or("").to_string())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn native_comma_join_disjunction_equi_key() {
    std::thread::Builder::new()
        .name("rel-crossjoin-or-e2e-16m".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("rt")
                .block_on(body())
        })
        .expect("spawn")
        .join()
        .expect("panic");
}

async fn body() {
    let server = PgServer::start().await.expect("server");
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    for ddl in [
        "DROP TABLE IF EXISTS cust",
        "DROP TABLE IF EXISTS ord",
        "CREATE TABLE cust (id INT PRIMARY KEY, tier VARCHAR)",
        "CREATE TABLE ord (oid INT PRIMARY KEY, cid INT, amt INT)",
        "INSERT INTO cust VALUES (1,'A'),(2,'B'),(3,'C')",
        // cid=1(tier A): amt 100,40 ; cid=2(tier B): amt 60,30 ; cid=3(tier C): 200
        "INSERT INTO ord VALUES (10,1,100),(11,1,40),(12,2,60),(13,2,30),(14,3,200)",
    ] {
        client.simple_query(ddl).await.expect("ddl");
    }

    // Q19 shape: the join key `cust.id = ord.cid` appears in BOTH branches of a
    // top-level OR of (tier, amount) range predicates. Was declined pre-fix with
    // `0A000 … unnormalized cross join`.
    //   branch A: tier='A' AND amt>=100 → (1,10) amt 100
    //   branch B: tier='B' AND amt>=50  → (2,12) amt 60
    //   sum = 160
    let got = rows(
        &client,
        "SELECT sum(ord.amt) AS rev FROM cust, ord \
         WHERE (cust.id = ord.cid AND cust.tier = 'A' AND ord.amt >= 100) \
            OR (cust.id = ord.cid AND cust.tier = 'B' AND ord.amt >= 50)",
    )
    .await;
    assert_eq!(got.len(), 1, "global aggregate is a single row");
    let rev: i64 = got[0][0].parse().expect("i64");
    assert_eq!(
        rev, 160,
        "OR-of-ANDs comma join must equi-join and sum correctly"
    );

    eprintln!("✓ TD-REL-LOWER-6 disjunction-shared equi-key normalizes on the native route");
}
