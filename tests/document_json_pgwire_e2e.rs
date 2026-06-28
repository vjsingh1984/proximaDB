//! Document (SQL/JSON) over pgwire — ACCURACY conformance harness (ADR-040).
//!
//! Documents are modeled as a relational table with a JSON column, queried over
//! the PostgreSQL wire protocol — the open, standard way to store + query
//! semi-structured documents in a SQL database. Same model as the TPC-H/TPC-DS
//! harnesses: CREATE → INSERT → MATERIALIZE → query one by one, routed by
//! ProximaDB.
//!
//! Per ADR-040 (TD-182), a query counts toward the ratchet only when it is
//! verified CORRECT, not merely that it executes without error:
//!   * **Anchored** — the materialized (DataFusion) result must equal a
//!     hand-computed expected answer derived from the deterministic seed.
//!   * **Differential** — when BOTH the native (pre-MATERIALIZE) and DataFusion
//!     (post-MATERIALIZE) engines return rows, they must agree after
//!     canonicalization; disagreement means at least one engine is wrong and the
//!     query does not count.
//!
//! The accuracy conversion exposed that JSON-path extraction in a SELECT
//! projection or WHERE filter returned 0 rows (the queries were misrouted to the
//! document store). Fixed incrementally: the routing fix (#480) repaired the
//! projections d04/d05/d08; the DataFusion `json_extract_path_text` alias UDF (this
//! PR) repaired the function-form projection d10. 8 of 11 are now accurate. The
//! filters d06/d07 (native filter path returns 0 rows for a JSON function) and d11
//! (DataFusion `Utf8 = Boolean` coercion) stay KNOWN-BAD under TD-183 — asserted to
//! still be wrong so the next fix trips the guard — and excluded from the ratchet.
//!
//!   RUST_LOG=proximadb=debug cargo test --test document_json_pgwire_e2e -- --nocapture

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use tempfile::TempDir;
use tokio::time::sleep;

/// Queries that must be verified CORRECT (anchored + differential) to count.
/// 8 of 11 are accurate: d01/d02/d03 (no JSON path), d09 (aggregation), d04/d05/d08
/// (projections, routing fix #480), and d10 (DataFusion `json_extract_path_text`
/// alias UDF, this PR). d06/d07/d11 remain known-bad under TD-183 (native filter
/// eval + DataFusion coercion); the ratchet rises as those land.
const DOC_ACCURACY_RATCHET: usize = 8;

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

fn explain_err(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        format!("[{}] {}", db.code().code(), db.message())
    } else {
        e.to_string()
    }
}

const SCHEMA: &[(&str, &str)] = &[(
    "docs",
    "CREATE TABLE docs (doc_id INT PRIMARY KEY, kind VARCHAR, doc JSON)",
)];

const DATA: &[&str] = &[
    "INSERT INTO docs (doc_id, kind, doc) VALUES (1, 'product', '{\"title\":\"alpha\",\"price\":10,\"in_stock\":true,\"tags\":[\"x\",\"y\"],\"author\":{\"name\":\"Ann\",\"country\":\"CA\"}}')",
    "INSERT INTO docs (doc_id, kind, doc) VALUES (2, 'product', '{\"title\":\"beta\",\"price\":25,\"in_stock\":false,\"tags\":[\"y\",\"z\"],\"author\":{\"name\":\"Bob\",\"country\":\"US\"}}')",
    "INSERT INTO docs (doc_id, kind, doc) VALUES (3, 'article', '{\"title\":\"gamma\",\"price\":5,\"in_stock\":true,\"tags\":[\"x\"],\"author\":{\"name\":\"Ann\",\"country\":\"CA\"}}')",
];

/// Document/JSON queries probing increasing capability. Numbered for reporting.
fn doc_queries() -> Vec<(&'static str, String)> {
    vec![
        // Baseline: store + retrieve the whole JSON document.
        ("d01_select_all", "select doc_id, kind, doc from docs order by doc_id".to_string()),
        // Filter on a normal relational column alongside the JSON blob.
        ("d02_filter_col", "select doc_id, doc from docs where kind = 'product' order by doc_id".to_string()),
        ("d03_count_by_kind", "select kind, count(*) as n from docs group by kind order by kind".to_string()),
        // JSON scalar field extraction (-> as json, ->> as text).
        ("d04_arrow_text", "select doc_id, doc->>'title' as title from docs order by doc_id".to_string()),
        ("d05_arrow_json", "select doc_id, doc->'price' as price from docs order by doc_id".to_string()),
        // Filter on an extracted JSON scalar.
        ("d06_filter_json_scalar", "select doc_id from docs where (doc->>'price')::int > 8 order by doc_id".to_string()),
        ("d07_filter_json_text", "select doc_id from docs where doc->>'title' = 'alpha'".to_string()),
        // Nested path.
        ("d08_nested_path", "select doc_id, doc->'author'->>'name' as author from docs order by doc_id".to_string()),
        // Group by an extracted JSON field.
        ("d09_group_by_json", "select doc->'author'->>'country' as country, count(*) as n from docs group by doc->'author'->>'country' order by country".to_string()),
        // json_extract_path_text function form (portable alternative to ->>).
        ("d10_json_extract_fn", "select doc_id, json_extract_path_text(doc, 'title') as title from docs order by doc_id".to_string()),
        // Boolean field extraction.
        ("d11_bool_field", "select doc_id from docs where (doc->>'in_stock')::boolean = true order by doc_id".to_string()),
    ]
}

/// JSON-path bugs still open under TD-183, asserted to STAY wrong so the next
/// fix trips the guard and forces the ratchet up. Excluded from the ratchet.
///
/// Resolved so far: the routing fix (#480) fixed the projections d04/d05/d08; the
/// DataFusion `json_extract_path_text` alias UDF (this PR) fixed the function-form
/// projection d10. Remaining, all needing deeper work (separate follow-up):
///   * d06/d07 — the native (pre-MATERIALIZE) filter path returns 0 rows for a JSON
///     function in WHERE (it is not evaluated through the scalar kernel); registering
///     the native kernel does not help and regresses DataFusion via registry binding.
///   * d07/d11 — DataFusion coercion: `json_extract_text(Utf8,Utf8)` in a bare `=`
///     comparison and `Utf8 = Boolean` for `(… )::boolean` both fail type coercion.
const KNOWN_BAD_TD183: &[&str] = &[
    "d06_filter_json_scalar", // (doc->>'price')::int > 8       filter — native 0-rows
    "d07_filter_json_text",   // doc->>'title' = 'alpha'        filter — native 0-rows + DF coercion
    "d11_bool_field",         // (doc->>'in_stock')::boolean    filter — DF cast coercion
];

/// One result row as text cells (pgwire `simple_query` form); SQL NULL → "NULL".
type Rows = Vec<Vec<String>>;

/// Run a query, returning ordered text rows or the server error string.
async fn collect(client: &tokio_postgres::Client, sql: &str) -> Result<Rows, String> {
    match client.simple_query(sql).await {
        Ok(msgs) => Ok(msgs
            .into_iter()
            .filter_map(|m| match m {
                tokio_postgres::SimpleQueryMessage::Row(r) => Some(
                    (0..r.len())
                        .map(|i| {
                            r.get(i)
                                .map(str::to_string)
                                .unwrap_or_else(|| "NULL".into())
                        })
                        .collect(),
                ),
                _ => None,
            })
            .collect()),
        Err(e) => Err(explain_err(&e)),
    }
}

/// Normalize a cell: JSON values are reparsed + reserialized (default serde_json
/// sorts object keys) so whitespace / key-order differences across engines do not
/// matter; non-JSON text (e.g. `alpha`, `product`) passes through unchanged.
fn norm_cell(s: &str) -> String {
    serde_json::from_str::<serde_json::Value>(s)
        .map(|v| v.to_string())
        .unwrap_or_else(|_| s.to_string())
}

/// Canonicalize a result for set-comparison: normalize every cell, then sort rows
/// (order-independent; all anchored queries also carry ORDER BY, so this only
/// guards against cross-engine ordering quirks).
fn canon(rows: &Rows) -> Rows {
    let mut out: Rows = rows
        .iter()
        .map(|row| row.iter().map(|c| norm_cell(c)).collect())
        .collect();
    out.sort();
    out
}

/// Hand-computed CORRECT answer for each query id, derived from the seed `DATA`
/// (docs 1/2/3 = Ann-CA / Bob-US / Ann-CA). Used to anchor good queries and to
/// detect when a TD-183 known-bad query becomes correct.
fn expected(id: &str) -> Rows {
    let j1 = r#"{"title":"alpha","price":10,"in_stock":true,"tags":["x","y"],"author":{"name":"Ann","country":"CA"}}"#;
    let j2 = r#"{"title":"beta","price":25,"in_stock":false,"tags":["y","z"],"author":{"name":"Bob","country":"US"}}"#;
    let j3 = r#"{"title":"gamma","price":5,"in_stock":true,"tags":["x"],"author":{"name":"Ann","country":"CA"}}"#;
    let r = |cols: &[&str]| cols.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match id {
        "d01_select_all" => vec![
            r(&["1", "product", j1]),
            r(&["2", "product", j2]),
            r(&["3", "article", j3]),
        ],
        "d02_filter_col" => vec![r(&["1", j1]), r(&["2", j2])],
        "d03_count_by_kind" => vec![r(&["article", "1"]), r(&["product", "2"])],
        "d04_arrow_text" => vec![r(&["1", "alpha"]), r(&["2", "beta"]), r(&["3", "gamma"])],
        "d05_arrow_json" => vec![r(&["1", "10"]), r(&["2", "25"]), r(&["3", "5"])],
        "d06_filter_json_scalar" => vec![r(&["1"]), r(&["2"])],
        "d07_filter_json_text" => vec![r(&["1"])],
        "d08_nested_path" => vec![r(&["1", "Ann"]), r(&["2", "Bob"]), r(&["3", "Ann"])],
        "d09_group_by_json" => vec![r(&["CA", "2"]), r(&["US", "1"])],
        "d10_json_extract_fn" => vec![r(&["1", "alpha"]), r(&["2", "beta"]), r(&["3", "gamma"])],
        "d11_bool_field" => vec![r(&["1"]), r(&["3"])],
        other => panic!("no expected answer registered for {other}"),
    }
}

fn fmt(res: &Result<Rows, String>) -> String {
    match res {
        Ok(rows) => format!("{} rows {:?}", rows.len(), rows),
        Err(e) => format!("ERR {e}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn document_json_pgwire_accuracy() {
    let server = PgServer::start().await.expect("server start");
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    for (name, ddl) in SCHEMA {
        let _ = client
            .simple_query(&format!("DROP TABLE IF EXISTS {name}"))
            .await;
        client
            .simple_query(ddl)
            .await
            .unwrap_or_else(|e| panic!("CREATE {name}: {}", explain_err(&e)));
    }
    eprintln!("✓ schema: {} tables created", SCHEMA.len());

    for sql in DATA {
        client
            .simple_query(sql)
            .await
            .unwrap_or_else(|e| panic!("INSERT failed: {}\n  sql: {sql}", explain_err(&e)));
    }
    eprintln!("✓ data: {} documents loaded", DATA.len());

    let queries = doc_queries();

    // Phase 1 — NATIVE engine (pre-MATERIALIZE).
    let mut native: Vec<(&str, Result<Rows, String>)> = Vec::new();
    for (id, sql) in &queries {
        native.push((*id, collect(&client, sql).await));
    }

    // MATERIALIZE flips the table parquet-backed → DataFusion route.
    let mut materialized = 0;
    for (name, _) in SCHEMA {
        match client
            .simple_query(&format!("ALTER TABLE {name} MATERIALIZE"))
            .await
        {
            Ok(_) => materialized += 1,
            Err(e) => eprintln!("  · MATERIALIZE {name}: {}", explain_err(&e)),
        }
    }
    eprintln!("✓ materialize: {materialized}/{} tables", SCHEMA.len());

    // Phase 2 — DataFusion engine (post-MATERIALIZE).
    let mut df: Vec<(&str, Result<Rows, String>)> = Vec::new();
    for (id, sql) in &queries {
        df.push((*id, collect(&client, sql).await));
    }

    // Classify each query: anchored (DataFusion == expected) AND differential
    // (native == DataFusion when both returned rows).
    let mut accurate: Vec<&str> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    eprintln!("\n=== Document accuracy (native | DataFusion vs expected) ===");
    for ((id, nat), (_, dfr)) in native.iter().zip(df.iter()) {
        let exp = canon(&expected(id));
        let df_ok = matches!(dfr, Ok(rows) if canon(rows) == exp);
        // Differential holds when native errors (single-engine) or matches DF.
        let diff_ok = match (nat, dfr) {
            (Ok(n), Ok(d)) => canon(n) == canon(d),
            (Err(_), _) => true,
            (_, Err(_)) => false,
        };
        let known_bad = KNOWN_BAD_TD183.contains(id);
        let is_accurate = df_ok && diff_ok;

        let tag = if known_bad {
            if is_accurate {
                "TD183-FIXED?"
            } else {
                "td183-known"
            }
        } else if is_accurate {
            "OK"
        } else {
            "MISMATCH"
        };
        eprintln!(
            "  [{tag:>12}] {id}\n      native: {}\n      datafu: {}",
            fmt(nat),
            fmt(dfr)
        );

        if known_bad {
            // The guard: a known-bad query must STAY wrong. When PR2 fixes the
            // projection path it becomes accurate → this fires → flip it to a
            // normal anchored query and raise DOC_ACCURACY_RATCHET.
            if is_accurate {
                violations.push(format!(
                    "{id}: TD-183 known-bad query now passes — remove it from \
                     KNOWN_BAD_TD183 and raise DOC_ACCURACY_RATCHET"
                ));
            }
            continue;
        }
        if is_accurate {
            accurate.push(id);
        } else {
            violations.push(format!(
                "{id}: not accurate (anchored={df_ok}, differential={diff_ok})\n      \
                 native:   {}\n      datafu:   {}\n      expected: {:?}",
                fmt(nat),
                fmt(dfr),
                exp
            ));
        }
    }

    eprintln!(
        "\n=== accurate: {}/{} (ratchet {}; {} known-bad under TD-183) ===",
        accurate.len(),
        queries.len(),
        DOC_ACCURACY_RATCHET,
        KNOWN_BAD_TD183.len()
    );
    eprintln!("  accurate: {accurate:?}");
    if !violations.is_empty() {
        eprintln!("violations:");
        for v in &violations {
            eprintln!("  ✗ {v}");
        }
    }

    assert!(
        violations.is_empty(),
        "Document accuracy violations ({}):\n{}",
        violations.len(),
        violations.join("\n")
    );
    assert!(
        accurate.len() >= DOC_ACCURACY_RATCHET,
        "Document accuracy regressed: {} accurate < ratchet {}",
        accurate.len(),
        DOC_ACCURACY_RATCHET
    );
}
