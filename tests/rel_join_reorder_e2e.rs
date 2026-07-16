// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-REL-LOWER-7 — an N-way comma join whose FROM order places two tables with
//! no direct equi-key adjacent (TPC-H Q9: `part, supplier` joined only through
//! `lineitem`) must reorder by equi-connectivity so the left-deep chain has a
//! join key at every level, instead of declining `0A000 … unnormalized cross
//! join`. Here `pt` and `sp` connect only through the hub `li`.

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
fn native_nway_comma_join_reorders_by_connectivity() {
    std::thread::Builder::new()
        .name("rel-join-reorder-e2e-16m".into())
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
        "DROP TABLE IF EXISTS pt",
        "DROP TABLE IF EXISTS sp",
        "DROP TABLE IF EXISTS li",
        "CREATE TABLE pt (pk INT PRIMARY KEY, pname VARCHAR)",
        "CREATE TABLE sp (sk INT PRIMARY KEY, snat VARCHAR)",
        "CREATE TABLE li (lid INT PRIMARY KEY, lpk INT, lsk INT)",
        "INSERT INTO pt VALUES (1,'A'),(2,'B')",
        "INSERT INTO sp VALUES (10,'X'),(20,'Y')",
        // li rows join pt on lpk=pk and sp on lsk=sk — all three match a pt+sp.
        "INSERT INTO li VALUES (100,1,10),(101,2,20),(102,1,20)",
    ] {
        client.simple_query(ddl).await.expect("ddl");
    }

    // Q9 shape: FROM order puts `pt, sp` adjacent, but they share NO direct
    // equi-key — both join only through the hub `li`. Was declined pre-fix with
    // `0A000 … unnormalized cross join`; the reorder makes it li-in-the-middle.
    let got = rows(
        &client,
        "SELECT count(*) AS n FROM pt, sp, li WHERE pt.pk = li.lpk AND sp.sk = li.lsk",
    )
    .await;
    assert_eq!(got.len(), 1);
    let n: i64 = got[0][0].parse().expect("i64");
    assert_eq!(n, 3, "each li row joins exactly one pt and one sp");

    // And with an extra filter, to exercise a residual over the reordered chain.
    let got2 = rows(
        &client,
        "SELECT count(*) AS n FROM pt, sp, li \
         WHERE pt.pk = li.lpk AND sp.sk = li.lsk AND pt.pname = 'A'",
    )
    .await;
    let n2: i64 = got2[0][0].parse().expect("i64");
    assert_eq!(n2, 2, "pt.pname='A' (pk=1) matches li rows 100 and 102");

    eprintln!(
        "✓ TD-REL-LOWER-7 N-way comma join reorders by equi-connectivity on the native route"
    );
}
