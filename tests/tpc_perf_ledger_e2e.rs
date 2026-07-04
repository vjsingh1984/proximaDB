//! TPC-H / TPC-DS performance evidence-ledger harness (TD-OLAP-4, P0b skeleton).
//!
//! Promotes the TPC suites from count-only *conformance* (see
//! `tests/{tpch,tpcds}_pgwire_e2e.rs`) toward *measured* evidence (ADR-052
//! invariant 4): per query × route × temperature it records wall latency and
//! the per-query `observability::io_trace` snapshot (bytes read, ranged GETs,
//! footer hits, per-engine compute_ms), on BOTH routes — native/Volcano
//! (pre-MATERIALIZE) and DataFusion-on-Parquet (post-MATERIALIZE) — into a
//! versioned JSON ledger artifact.
//!
//! Methodology (pinned per TD-OLAP-4; regimes are reported separately, never
//! averaged):
//! - storage regime: `local-tempdir` (file:// object store on local disk).
//!   Object-store regimes are a separate ledger section when they land.
//! - datagen: `synthetic-tpc-shaped` — deterministic, seeded, TPC-schema
//!   row-ratio-scaled with key skew, referential integrity, and fact tables
//!   CLUSTERED by their date column (uniform data defeats zone-map pruning —
//!   see TPC_PERF_GATE_EVIDENCE_2026_07_04). It is NOT audited dbgen/dsdgen
//!   output; no TPC-official claim may cite this ledger.
//! - temperature: `first` vs `repeat` against a warm process (true cold-cache
//!   runs need a server restart per query — a future slice).
//! - scale: `TPC_PERF_SCALE` (default 0.001 ≈ 9k TPC-H rows; SF1 ≡ 1.0 —
//!   impractical over per-row INSERTs until a bulk-load path exists).
//!
//! `#[ignore]` — advisory, never a merge gate. Run on demand:
//!
//!   TPC_PERF_SCALE=0.001 cargo test --test tpc_perf_ledger_e2e -- --ignored --nocapture
//!
//! Output: `TPC_PERF_LEDGER_OUT` (default `target/tpc-perf-ledger/ledger.json`).
//!
//! Schemas and query texts are copied verbatim from the conformance suites
//! (`tests/tpch_pgwire_e2e.rs`, `tests/tpcds_pgwire_e2e.rs`) — the repo's
//! integration tests are self-contained by convention; keep them in sync.

use std::net::TcpListener;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use proximadb::observability::io_trace::{self, IoTraceSnapshot};
use tempfile::TempDir;
use tokio::time::sleep;
use tokio_postgres::{Client, SimpleQueryMessage};

/// One billing snapshot per query, pushed by the collecting observer at
/// io_trace scope close (pgwire wraps each statement in its own scope).
static CAPTURE: Mutex<Vec<IoTraceSnapshot>> = Mutex::new(Vec::new());

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

        // Override the process billing observer (installed by db.start()) with
        // a collector that captures each query's snapshot for this harness.
        io_trace::set_billing_observer(Some(Box::new(|snap: &IoTraceSnapshot, _tenant| {
            CAPTURE.lock().expect("capture lock").push(snap.clone());
        })));

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .no_proxy()
            .build()?;
        let health = format!("http://127.0.0.1:{rest_port}/health");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match http.get(&health).send().await {
                Ok(r) if r.status().is_success() => break,
                _ if Instant::now() > deadline => anyhow::bail!("REST not ready"),
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

    async fn shutdown(mut self) {
        if let Some(mut db) = self.db.take() {
            let _ = db.shutdown().await;
        }
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

// ---------------------------------------------------------------------------
// Deterministic seeded generator — synthetic-tpc-shaped (NOT dbgen/dsdgen).
// SplitMix64 keeps every run reproducible; key skew is power-law-ish so join
// and filter selectivities are non-uniform like real TPC data.
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    /// Power-law-skewed key in `1..=n` (quadratic bias toward low keys).
    fn skewed_key(&mut self, n: u64) -> u64 {
        let u = self.next() as f64 / u64::MAX as f64;
        ((u * u * n as f64) as u64).min(n.saturating_sub(1)) + 1
    }
    fn pick<'a>(&mut self, choices: &[&'a str]) -> &'a str {
        choices[self.below(choices.len() as u64) as usize]
    }
    fn date(&mut self) -> String {
        let y = 1992 + self.below(7);
        let m = 1 + self.below(12);
        let d = 1 + self.below(28);
        format!("DATE '{y}-{m:02}-{d:02}'")
    }
    fn money(&mut self, max: u64) -> String {
        format!("{}.{:02}", self.below(max), self.below(100))
    }
}

fn chunked_inserts(table: &str, cols: &str, values: Vec<String>, chunk: usize) -> Vec<String> {
    values
        .chunks(chunk)
        .map(|c| format!("INSERT INTO {table} ({cols}) VALUES {}", c.join(", ")))
        .collect()
}

/// Scale a canonical TPC row count, with a floor so every table joins.
fn scaled(base: u64, scale: f64, floor: u64) -> u64 {
    ((base as f64 * scale) as u64).max(floor)
}

// --- TPC-H (schema copied from tests/tpch_pgwire_e2e.rs) -------------------

const TPCH_SCHEMA: &[(&str, &str)] = &[
    (
        "region",
        "CREATE TABLE region (r_regionkey INT PRIMARY KEY, r_name VARCHAR, r_comment VARCHAR)",
    ),
    (
        "nation",
        "CREATE TABLE nation (n_nationkey INT PRIMARY KEY, n_name VARCHAR, n_regionkey INT, n_comment VARCHAR)",
    ),
    (
        "supplier",
        "CREATE TABLE supplier (s_suppkey INT PRIMARY KEY, s_name VARCHAR, s_address VARCHAR, s_nationkey INT, s_phone VARCHAR, s_acctbal DOUBLE PRECISION, s_comment VARCHAR)",
    ),
    (
        "part",
        "CREATE TABLE part (p_partkey INT PRIMARY KEY, p_name VARCHAR, p_mfgr VARCHAR, p_brand VARCHAR, p_type VARCHAR, p_size INT, p_container VARCHAR, p_retailprice DOUBLE PRECISION, p_comment VARCHAR)",
    ),
    (
        "partsupp",
        "CREATE TABLE partsupp (ps_partkey INT, ps_suppkey INT, ps_availqty INT, ps_supplycost DOUBLE PRECISION, ps_comment VARCHAR)",
    ),
    (
        "customer",
        "CREATE TABLE customer (c_custkey INT PRIMARY KEY, c_name VARCHAR, c_address VARCHAR, c_nationkey INT, c_phone VARCHAR, c_acctbal DOUBLE PRECISION, c_mktsegment VARCHAR, c_comment VARCHAR)",
    ),
    (
        "orders",
        "CREATE TABLE orders (o_orderkey INT PRIMARY KEY, o_custkey INT, o_orderstatus VARCHAR, o_totalprice DOUBLE PRECISION, o_orderdate DATE, o_orderpriority VARCHAR, o_clerk VARCHAR, o_shippriority INT, o_comment VARCHAR)",
    ),
    (
        "lineitem",
        "CREATE TABLE lineitem (l_orderkey INT, l_partkey INT, l_suppkey INT, l_linenumber INT, l_quantity DOUBLE PRECISION, l_extendedprice DOUBLE PRECISION, l_discount DOUBLE PRECISION, l_tax DOUBLE PRECISION, l_returnflag VARCHAR, l_linestatus VARCHAR, l_shipdate DATE, l_commitdate DATE, l_receiptdate DATE, l_shipinstruct VARCHAR, l_shipmode VARCHAR, l_comment VARCHAR)",
    ),
];

/// 5 regions / 25 nations mapped like the standard, including every nation
/// name the 22 queries reference by constant.
const REGIONS: &[&str] = &["AFRICA", "AMERICA", "ASIA", "EUROPE", "MIDDLE EAST"];
const NATIONS: &[(&str, u64)] = &[
    ("ALGERIA", 0),
    ("ARGENTINA", 1),
    ("BRAZIL", 1),
    ("CANADA", 1),
    ("EGYPT", 4),
    ("ETHIOPIA", 0),
    ("FRANCE", 3),
    ("GERMANY", 3),
    ("INDIA", 2),
    ("INDONESIA", 2),
    ("IRAN", 4),
    ("IRAQ", 4),
    ("JAPAN", 2),
    ("JORDAN", 4),
    ("KENYA", 0),
    ("MOROCCO", 0),
    ("MOZAMBIQUE", 0),
    ("PERU", 1),
    ("CHINA", 2),
    ("ROMANIA", 3),
    ("SAUDI ARABIA", 4),
    ("VIETNAM", 2),
    ("RUSSIA", 3),
    ("UNITED KINGDOM", 3),
    ("UNITED STATES", 1),
];

fn gen_tpch(scale: f64) -> Vec<String> {
    let mut rng = Rng::new(0x7C_0FFE_E5EED);
    let n_supp = scaled(10_000, scale, 10);
    let n_part = scaled(200_000, scale, 20);
    let n_cust = scaled(150_000, scale, 15);
    let n_ord = scaled(1_500_000, scale, 150);

    let types = [
        "PROMO BRUSHED STEEL",
        "STANDARD POLISHED TIN",
        "SMALL PLATED COPPER",
        "MEDIUM POLISHED BRASS",
        "ECONOMY ANODIZED NICKEL",
    ];
    let containers = [
        "SM CASE", "SM BOX", "MED BOX", "MED BAG", "LG BOX", "WRAP PKG",
    ];
    let segments = [
        "BUILDING",
        "AUTOMOBILE",
        "MACHINERY",
        "HOUSEHOLD",
        "FURNITURE",
    ];
    let priorities = ["1-URGENT", "2-HIGH", "3-MEDIUM", "4-NOT SPECIFIED", "5-LOW"];
    let shipmodes = ["MAIL", "SHIP", "AIR", "AIR REG", "TRUCK", "RAIL", "FOB"];
    let instructs = [
        "DELIVER IN PERSON",
        "NONE",
        "TAKE BACK RETURN",
        "COLLECT COD",
    ];
    let name_words = ["forest", "green", "sky", "metal", "ivory", "rose", "navy"];
    let phone_codes = ["13", "31", "23", "29", "30", "18", "17", "27", "10"];

    let mut out = Vec::new();
    out.extend(chunked_inserts(
        "region",
        "r_regionkey, r_name, r_comment",
        REGIONS
            .iter()
            .enumerate()
            .map(|(i, r)| format!("({i}, '{r}', 'rc{i}')"))
            .collect(),
        200,
    ));
    out.extend(chunked_inserts(
        "nation",
        "n_nationkey, n_name, n_regionkey, n_comment",
        NATIONS
            .iter()
            .enumerate()
            .map(|(i, (n, r))| format!("({i}, '{n}', {r}, 'nc{i}')"))
            .collect(),
        200,
    ));
    out.extend(chunked_inserts(
        "supplier",
        "s_suppkey, s_name, s_address, s_nationkey, s_phone, s_acctbal, s_comment",
        (1..=n_supp)
            .map(|k| {
                let nat = rng.below(25);
                let comment = if rng.below(20) == 0 {
                    "Customer Complaints"
                } else {
                    "sc"
                };
                format!(
                    "({k}, 'Supplier#{k}', 'addr{k}', {nat}, '{}-{k:03}', {}, '{comment}')",
                    rng.pick(&phone_codes),
                    rng.money(10_000)
                )
            })
            .collect(),
        200,
    ));
    out.extend(chunked_inserts(
        "part",
        "p_partkey, p_name, p_mfgr, p_brand, p_type, p_size, p_container, p_retailprice, p_comment",
        (1..=n_part)
            .map(|k| {
                format!(
                    "({k}, '{} part {k}', 'Mfgr#{}', 'Brand#{}{}', '{}', {}, '{}', {}, 'pc{k}')",
                    rng.pick(&name_words),
                    1 + rng.below(5),
                    1 + rng.below(5),
                    1 + rng.below(5),
                    rng.pick(&types),
                    1 + rng.below(50),
                    rng.pick(&containers),
                    rng.money(2_000)
                )
            })
            .collect(),
        200,
    ));
    // 4 suppliers per part, like the standard 800k/200k ratio.
    out.extend(chunked_inserts(
        "partsupp",
        "ps_partkey, ps_suppkey, ps_availqty, ps_supplycost, ps_comment",
        (1..=n_part)
            .flat_map(|p| {
                (0..4)
                    .map(|_| {
                        format!(
                            "({p}, {}, {}, {}, 'psc')",
                            rng.skewed_key(n_supp),
                            1 + rng.below(9_999),
                            rng.money(1_000)
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect(),
        200,
    ));
    out.extend(chunked_inserts(
        "customer",
        "c_custkey, c_name, c_address, c_nationkey, c_phone, c_acctbal, c_mktsegment, c_comment",
        (1..=n_cust)
            .map(|k| {
                let comment = if rng.below(15) == 0 {
                    "special requests noted"
                } else {
                    "cc"
                };
                format!(
                    "({k}, 'Customer#{k}', 'caddr{k}', {}, '{}-{k:03}', {}, '{}', '{comment}')",
                    rng.below(25),
                    rng.pick(&phone_codes),
                    rng.money(10_000),
                    rng.pick(&segments)
                )
            })
            .collect(),
        200,
    ));
    // Fact tables are CLUSTERED by their date column before insertion.
    // Uniform per-row dates give every row group whole-domain min/max, so
    // zone-map pruning provably cannot skip (measured 0% in
    // docs/_internal/status/TPC_PERF_GATE_EVIDENCE_2026_07_04.adoc). Real TPC
    // data is time-ordered; sorting restores that property so row groups
    // carry tight bounds. The `DATE 'YYYY-MM-DD'` literal format sorts
    // lexicographically.
    let mut lineitems: Vec<(String, String)> = Vec::new();
    let mut orders: Vec<(String, String)> = (1..=n_ord)
        .map(|o| {
            let lines = 1 + rng.below(7);
            for l in 1..=lines {
                let ship = rng.date();
                let row = format!(
                    "({o}, {}, {}, {l}, {}, {}, 0.0{}, 0.0{}, '{}', '{}', {ship}, {}, {}, '{}', '{}', 'lc')",
                    rng.skewed_key(n_part),
                    rng.skewed_key(n_supp),
                    1 + rng.below(50),
                    rng.money(10_000),
                    rng.below(11),
                    rng.below(9),
                    rng.pick(&["N", "R", "A"]),
                    rng.pick(&["O", "F"]),
                    rng.date(),
                    rng.date(),
                    rng.pick(&instructs),
                    rng.pick(&shipmodes),
                );
                lineitems.push((ship, row));
            }
            let odate = rng.date();
            let row = format!(
                "({o}, {}, '{}', {}, {odate}, '{}', 'Clerk#{}', 0, 'oc')",
                rng.skewed_key(n_cust),
                rng.pick(&["O", "F", "P"]),
                rng.money(100_000),
                rng.pick(&priorities),
                1 + rng.below(100)
            );
            (odate, row)
        })
        .collect();
    orders.sort_by(|a, b| a.0.cmp(&b.0));
    lineitems.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend(chunked_inserts(
        "orders",
        "o_orderkey, o_custkey, o_orderstatus, o_totalprice, o_orderdate, o_orderpriority, o_clerk, o_shippriority, o_comment",
        orders.into_iter().map(|(_, row)| row).collect(),
        200,
    ));
    out.extend(chunked_inserts(
        "lineitem",
        "l_orderkey, l_partkey, l_suppkey, l_linenumber, l_quantity, l_extendedprice, l_discount, l_tax, l_returnflag, l_linestatus, l_shipdate, l_commitdate, l_receiptdate, l_shipinstruct, l_shipmode, l_comment",
        lineitems.into_iter().map(|(_, row)| row).collect(),
        200,
    ));
    out
}

// --- TPC-DS star subset (schema copied from tests/tpcds_pgwire_e2e.rs) -----

const TPCDS_SCHEMA: &[(&str, &str)] = &[
    (
        "date_dim",
        "CREATE TABLE date_dim (d_date_sk INT PRIMARY KEY, d_date DATE, d_year INT, d_moy INT, d_qoy INT, d_dom INT, d_dow INT)",
    ),
    (
        "item",
        "CREATE TABLE item (i_item_sk INT PRIMARY KEY, i_item_id VARCHAR, i_brand_id INT, i_brand VARCHAR, i_class_id INT, i_class VARCHAR, i_category_id INT, i_category VARCHAR, i_manufact_id INT, i_current_price DOUBLE PRECISION)",
    ),
    (
        "store",
        "CREATE TABLE store (s_store_sk INT PRIMARY KEY, s_store_id VARCHAR, s_store_name VARCHAR, s_state VARCHAR)",
    ),
    (
        "customer",
        "CREATE TABLE customer (c_customer_sk INT PRIMARY KEY, c_customer_id VARCHAR, c_first_name VARCHAR, c_last_name VARCHAR, c_current_addr_sk INT, c_birth_country VARCHAR)",
    ),
    (
        "customer_address",
        "CREATE TABLE customer_address (ca_address_sk INT PRIMARY KEY, ca_state VARCHAR, ca_city VARCHAR, ca_zip VARCHAR, ca_country VARCHAR, ca_gmt_offset DOUBLE PRECISION)",
    ),
    (
        "store_sales",
        "CREATE TABLE store_sales (ss_sold_date_sk INT, ss_item_sk INT, ss_store_sk INT, ss_customer_sk INT, ss_addr_sk INT, ss_ticket_number INT, ss_quantity INT, ss_sales_price DOUBLE PRECISION, ss_ext_sales_price DOUBLE PRECISION, ss_ext_discount_amt DOUBLE PRECISION, ss_net_profit DOUBLE PRECISION)",
    ),
];

fn gen_tpcds(scale: f64) -> Vec<String> {
    let mut rng = Rng::new(0xD5_0FFE_E5EED);
    let n_item = scaled(18_000, scale, 20);
    let n_cust = scaled(100_000, scale, 10);
    let n_sales = scaled(2_880_000, scale, 300);
    let n_store = 6u64;
    // Two years of dates, 1999-2000 (the queries filter d_year=2000, d_moy=11).
    let mut dates = Vec::new();
    let mut sk = 23_000u64;
    for y in [1999u64, 2000] {
        for m in 1..=12u64 {
            for d in 1..=28u64 {
                sk += 1;
                let dow = sk % 7;
                let qoy = m.div_ceil(3);
                dates.push(format!(
                    "({sk}, DATE '{y}-{m:02}-{d:02}', {y}, {m}, {qoy}, {d}, {dow})"
                ));
            }
        }
    }
    let n_dates = dates.len() as u64;
    let first_sk = 23_001u64;

    let categories = [
        ("Electronics", 1u64),
        ("Books", 2),
        ("Home", 3),
        ("Sports", 4),
        ("Music", 5),
    ];
    let states = ["CA", "NY", "TX", "WA", "IL", "GA"];

    let mut out = Vec::new();
    out.extend(chunked_inserts(
        "date_dim",
        "d_date_sk, d_date, d_year, d_moy, d_qoy, d_dom, d_dow",
        dates,
        200,
    ));
    out.extend(chunked_inserts(
        "item",
        "i_item_sk, i_item_id, i_brand_id, i_brand, i_class_id, i_class, i_category_id, i_category, i_manufact_id, i_current_price",
        (1..=n_item)
            .map(|k| {
                let (cat, cat_id) = categories[rng.below(5) as usize];
                let brand = 10 * cat_id + 1 + rng.below(5);
                format!(
                    "({k}, 'ITEM{k:05}', {brand}, 'brand{brand}', {cat_id}, 'class{}', {cat_id}, '{cat}', {}, {})",
                    1 + rng.below(3),
                    100 + rng.below(10),
                    rng.money(100)
                )
            })
            .collect(),
        200,
    ));
    out.extend(chunked_inserts(
        "store",
        "s_store_sk, s_store_id, s_store_name, s_state",
        (1..=n_store)
            .map(|k| {
                format!(
                    "({k}, 'STORE{k:02}', 'Store {k}', '{}')",
                    states[(k - 1) as usize % states.len()]
                )
            })
            .collect(),
        200,
    ));
    out.extend(chunked_inserts(
        "customer_address",
        "ca_address_sk, ca_state, ca_city, ca_zip, ca_country, ca_gmt_offset",
        (1..=n_cust)
            .map(|k| {
                format!(
                    "({k}, '{}', 'City{k}', '{:05}', 'United States', -{}.0)",
                    rng.pick(&states),
                    10_000 + rng.below(89_999),
                    5 + rng.below(4)
                )
            })
            .collect(),
        200,
    ));
    out.extend(chunked_inserts(
        "customer",
        "c_customer_sk, c_customer_id, c_first_name, c_last_name, c_current_addr_sk, c_birth_country",
        (1..=n_cust)
            .map(|k| {
                format!(
                    "({k}, 'CUST{k:05}', 'fn{k}', 'ln{k}', {}, '{}')",
                    1 + rng.below(n_cust),
                    rng.pick(&["CANADA", "MEXICO", "BRAZIL", "JAPAN"])
                )
            })
            .collect(),
        200,
    ));
    // Clustered by ss_sold_date_sk — see the gen_tpch clustering note.
    let mut sales: Vec<(u64, String)> = (1..=n_sales)
        .map(|t| {
            let qty = 1 + rng.below(10);
            let price = 1 + rng.below(100);
            let date_sk = first_sk + rng.below(n_dates);
            let row = format!(
                "({date_sk}, {}, {}, {}, {}, {t}, {qty}, {price}.00, {}.00, {}.00, {}.00)",
                rng.skewed_key(n_item),
                1 + rng.below(n_store),
                rng.skewed_key(n_cust),
                1 + rng.below(n_cust),
                qty * price,
                rng.below(10),
                rng.below(30)
            );
            (date_sk, row)
        })
        .collect();
    sales.sort_by_key(|(sk, _)| *sk);
    out.extend(chunked_inserts(
        "store_sales",
        "ss_sold_date_sk, ss_item_sk, ss_store_sk, ss_customer_sk, ss_addr_sk, ss_ticket_number, ss_quantity, ss_sales_price, ss_ext_sales_price, ss_ext_discount_amt, ss_net_profit",
        sales.into_iter().map(|(_, row)| row).collect(),
        200,
    ));
    out
}

// --- Queries (copied verbatim from the conformance suites) -----------------

fn tpch_queries() -> Vec<(&'static str, String)> {
    vec![
        ("Q1", "select l_returnflag, l_linestatus, sum(l_quantity) as sum_qty, sum(l_extendedprice) as sum_base_price, sum(l_extendedprice*(1-l_discount)) as sum_disc_price, sum(l_extendedprice*(1-l_discount)*(1+l_tax)) as sum_charge, avg(l_quantity) as avg_qty, avg(l_extendedprice) as avg_price, avg(l_discount) as avg_disc, count(*) as count_order from lineitem where l_shipdate <= DATE '1998-09-01' group by l_returnflag, l_linestatus order by l_returnflag, l_linestatus".to_string()),
        ("Q2", "select s_acctbal, s_name, n_name, p_partkey, p_mfgr, s_address, s_phone, s_comment from part, supplier, partsupp, nation, region where p_partkey = ps_partkey and s_suppkey = ps_suppkey and p_size = 15 and p_type like '%STEEL' and s_nationkey = n_nationkey and n_regionkey = r_regionkey and r_name = 'AMERICA' and ps_supplycost = (select min(ps_supplycost) from partsupp, supplier, nation, region where p_partkey = ps_partkey and s_suppkey = ps_suppkey and s_nationkey = n_nationkey and n_regionkey = r_regionkey and r_name = 'AMERICA') order by s_acctbal desc, n_name, s_name, p_partkey".to_string()),
        ("Q3", "select l_orderkey, sum(l_extendedprice*(1-l_discount)) as revenue, o_orderdate, o_shippriority from customer, orders, lineitem where c_mktsegment = 'BUILDING' and c_custkey = o_custkey and l_orderkey = o_orderkey and o_orderdate < DATE '1995-03-15' and l_shipdate > DATE '1995-03-15' group by l_orderkey, o_orderdate, o_shippriority order by revenue desc, o_orderdate".to_string()),
        ("Q4", "select o_orderpriority, count(*) as order_count from orders where o_orderdate >= DATE '1993-07-01' and o_orderdate < DATE '1993-10-01' and exists (select * from lineitem where l_orderkey = o_orderkey and l_commitdate < l_receiptdate) group by o_orderpriority order by o_orderpriority".to_string()),
        ("Q5", "select n_name, sum(l_extendedprice*(1-l_discount)) as revenue from customer, orders, lineitem, supplier, nation, region where c_custkey = o_custkey and l_orderkey = o_orderkey and l_suppkey = s_suppkey and c_nationkey = s_nationkey and s_nationkey = n_nationkey and n_regionkey = r_regionkey and r_name = 'AMERICA' and o_orderdate >= DATE '1994-01-01' and o_orderdate < DATE '1995-01-01' group by n_name order by revenue desc".to_string()),
        ("Q6", "select sum(l_extendedprice*l_discount) as revenue from lineitem where l_shipdate >= DATE '1994-01-01' and l_shipdate < DATE '1995-01-01' and l_discount between 0.05 and 0.07 and l_quantity < 24".to_string()),
        ("Q7", "select supp_nation, cust_nation, l_year, sum(volume) as revenue from (select n1.n_name as supp_nation, n2.n_name as cust_nation, extract(year from l_shipdate) as l_year, l_extendedprice*(1-l_discount) as volume from supplier, lineitem, orders, customer, nation n1, nation n2 where s_suppkey = l_suppkey and o_orderkey = l_orderkey and c_custkey = o_custkey and s_nationkey = n1.n_nationkey and c_nationkey = n2.n_nationkey and ((n1.n_name = 'GERMANY' and n2.n_name = 'FRANCE') or (n1.n_name = 'FRANCE' and n2.n_name = 'GERMANY')) and l_shipdate between DATE '1995-01-01' and DATE '1996-12-31') as shipping group by supp_nation, cust_nation, l_year order by supp_nation, cust_nation, l_year".to_string()),
        ("Q8", "select o_year, sum(case when nation = 'GERMANY' then volume else 0 end) / sum(volume) as mkt_share from (select extract(year from o_orderdate) as o_year, l_extendedprice*(1-l_discount) as volume, n2.n_name as nation from part, supplier, lineitem, orders, customer, nation n1, nation n2, region where p_partkey = l_partkey and s_suppkey = l_suppkey and l_orderkey = o_orderkey and o_custkey = c_custkey and c_nationkey = n1.n_nationkey and n1.n_regionkey = r_regionkey and r_name = 'EUROPE' and s_nationkey = n2.n_nationkey and o_orderdate between DATE '1995-01-01' and DATE '1996-12-31' and p_type = 'STANDARD POLISHED TIN') as all_nations group by o_year order by o_year".to_string()),
        ("Q9", "select nation, o_year, sum(amount) as sum_profit from (select n_name as nation, extract(year from o_orderdate) as o_year, l_extendedprice*(1-l_discount) - ps_supplycost*l_quantity as amount from part, supplier, lineitem, partsupp, orders, nation where s_suppkey = l_suppkey and ps_suppkey = l_suppkey and ps_partkey = l_partkey and p_partkey = l_partkey and o_orderkey = l_orderkey and s_nationkey = n_nationkey and p_name like '%green%') as profit group by nation, o_year order by nation, o_year desc".to_string()),
        ("Q10", "select c_custkey, c_name, sum(l_extendedprice*(1-l_discount)) as revenue, c_acctbal, n_name, c_address, c_phone, c_comment from customer, orders, lineitem, nation where c_custkey = o_custkey and l_orderkey = o_orderkey and o_orderdate >= DATE '1993-10-01' and o_orderdate < DATE '1994-01-01' and l_returnflag = 'R' and c_nationkey = n_nationkey group by c_custkey, c_name, c_acctbal, c_phone, n_name, c_address, c_comment order by revenue desc".to_string()),
        ("Q11", "select ps_partkey, sum(ps_supplycost*ps_availqty) as value from partsupp, supplier, nation where ps_suppkey = s_suppkey and s_nationkey = n_nationkey and n_name = 'GERMANY' group by ps_partkey having sum(ps_supplycost*ps_availqty) > (select sum(ps_supplycost*ps_availqty) * 0.0001 from partsupp, supplier, nation where ps_suppkey = s_suppkey and s_nationkey = n_nationkey and n_name = 'GERMANY') order by value desc".to_string()),
        ("Q12", "select l_shipmode, sum(case when o_orderpriority = '1-URGENT' or o_orderpriority = '2-HIGH' then 1 else 0 end) as high_line_count, sum(case when o_orderpriority <> '1-URGENT' and o_orderpriority <> '2-HIGH' then 1 else 0 end) as low_line_count from orders, lineitem where o_orderkey = l_orderkey and l_shipmode in ('MAIL', 'SHIP') and l_commitdate < l_receiptdate and l_shipdate < l_commitdate and l_receiptdate >= DATE '1994-01-01' and l_receiptdate < DATE '1995-01-01' group by l_shipmode order by l_shipmode".to_string()),
        ("Q13", "select c_count, count(*) as custdist from (select c_custkey, count(o_orderkey) as c_count from customer left outer join orders on c_custkey = o_custkey and o_comment not like '%special%requests%' group by c_custkey) as c_orders group by c_count order by custdist desc, c_count desc".to_string()),
        ("Q14", "select 100.00 * sum(case when p_type like 'PROMO%' then l_extendedprice*(1-l_discount) else 0 end) / sum(l_extendedprice*(1-l_discount)) as promo_revenue from lineitem, part where l_partkey = p_partkey and l_shipdate >= DATE '1995-09-01' and l_shipdate < DATE '1995-10-01'".to_string()),
        ("Q15", "select s_suppkey, s_name, s_address, s_phone, total_revenue from supplier, (select l_suppkey as supplier_no, sum(l_extendedprice*(1-l_discount)) as total_revenue from lineitem where l_shipdate >= DATE '1996-01-01' and l_shipdate < DATE '1996-04-01' group by l_suppkey) as revenue0 where s_suppkey = supplier_no order by s_suppkey".to_string()),
        ("Q16", "select p_brand, p_type, p_size, count(distinct ps_suppkey) as supplier_cnt from partsupp, part where p_partkey = ps_partkey and p_brand <> 'Brand#45' and p_type not like 'MEDIUM POLISHED%' and p_size in (49, 14, 23, 45, 19, 3, 36, 9) group by p_brand, p_type, p_size order by supplier_cnt desc, p_brand, p_type, p_size".to_string()),
        ("Q17", "select sum(l_extendedprice) / 7.0 as avg_yearly from lineitem, part where p_partkey = l_partkey and p_brand = 'Brand#23' and p_container = 'MED BOX' and l_quantity < (select 0.2 * avg(l_quantity) from lineitem where l_partkey = p_partkey)".to_string()),
        ("Q18", "select c_name, c_custkey, o_orderkey, o_orderdate, o_totalprice, sum(l_quantity) from customer, orders, lineitem where o_orderkey in (select l_orderkey from lineitem group by l_orderkey having sum(l_quantity) > 300) and c_custkey = o_custkey and o_orderkey = l_orderkey group by c_name, c_custkey, o_orderkey, o_orderdate, o_totalprice order by o_totalprice desc, o_orderdate".to_string()),
        ("Q19", "select sum(l_extendedprice*(1-l_discount)) as revenue from lineitem, part where (p_partkey = l_partkey and p_brand = 'Brand#12' and p_container in ('SM CASE', 'SM BOX', 'SM PACK', 'SM PKG') and l_quantity >= 1 and l_quantity <= 11 and p_size between 1 and 5 and l_shipmode in ('AIR', 'AIR REG') and l_shipinstruct = 'DELIVER IN PERSON') or (p_partkey = l_partkey and p_brand = 'Brand#23' and p_container in ('MED BAG', 'MED BOX', 'MED PKG', 'MED PACK') and l_quantity >= 10 and l_quantity <= 20 and p_size between 1 and 10 and l_shipmode in ('AIR', 'AIR REG') and l_shipinstruct = 'DELIVER IN PERSON')".to_string()),
        ("Q20", "select s_name, s_address from supplier, nation where s_suppkey in (select ps_suppkey from partsupp where ps_partkey in (select p_partkey from part where p_name like 'forest%') and ps_availqty > (select 0.5 * sum(l_quantity) from lineitem where l_partkey = ps_partkey and l_suppkey = ps_suppkey and l_shipdate >= DATE '1994-01-01' and l_shipdate < DATE '1995-01-01')) and s_nationkey = n_nationkey and n_name = 'CANADA' order by s_name".to_string()),
        ("Q21", "select s_name, count(*) as numwait from supplier, lineitem l1, orders, nation where s_suppkey = l1.l_suppkey and o_orderkey = l1.l_orderkey and o_orderstatus = 'F' and l1.l_receiptdate > l1.l_commitdate and exists (select * from lineitem l2 where l2.l_orderkey = l1.l_orderkey and l2.l_suppkey <> l1.l_suppkey) and not exists (select * from lineitem l3 where l3.l_orderkey = l1.l_orderkey and l3.l_suppkey <> l1.l_suppkey and l3.l_receiptdate > l3.l_commitdate) and s_nationkey = n_nationkey and n_name = 'SAUDI ARABIA' group by s_name order by numwait desc, s_name".to_string()),
        ("Q22", "select cntrycode, count(*) as numcust, sum(c_acctbal) as totacctbal from (select substring(c_phone from 1 for 2) as cntrycode, c_acctbal from customer where substring(c_phone from 1 for 2) in ('13', '31', '23', '29', '30', '18', '17') and c_acctbal > (select avg(c_acctbal) from customer where c_acctbal > 0.00 and substring(c_phone from 1 for 2) in ('13', '31', '23', '29', '30', '18', '17')) and not exists (select * from orders where o_custkey = c_custkey)) as custsale group by cntrycode order by cntrycode".to_string()),
    ]
}

fn tpcds_queries() -> Vec<(&'static str, String)> {
    vec![
        ("q42", "select dt.d_year, item.i_category_id, item.i_category, sum(ss_ext_sales_price) as revenue from date_dim dt, store_sales, item where dt.d_date_sk = store_sales.ss_sold_date_sk and store_sales.ss_item_sk = item.i_item_sk and item.i_manufact_id = 100 and dt.d_moy = 11 and dt.d_year = 2000 group by dt.d_year, item.i_category_id, item.i_category order by revenue desc, dt.d_year".to_string()),
        ("q52", "select dt.d_year, item.i_brand_id as brand_id, item.i_brand as brand, sum(ss_ext_sales_price) as ext_price from date_dim dt, store_sales, item where dt.d_date_sk = store_sales.ss_sold_date_sk and store_sales.ss_item_sk = item.i_item_sk and dt.d_moy = 11 and dt.d_year = 2000 group by dt.d_year, item.i_brand, item.i_brand_id order by dt.d_year, ext_price desc, brand_id".to_string()),
        ("q55", "select i_brand_id as brand_id, i_brand as brand, sum(ss_ext_sales_price) as ext_price from date_dim, store_sales, item where store_sales.ss_sold_date_sk = date_dim.d_date_sk and store_sales.ss_item_sk = item.i_item_sk and i_manufact_id = 100 and d_moy = 11 and d_year = 2000 group by i_brand, i_brand_id order by ext_price desc, i_brand_id".to_string()),
        ("q3", "select dt.d_year, item.i_brand_id as brand_id, item.i_brand as brand, sum(ss_ext_sales_price) as sum_agg from date_dim dt, store_sales, item where dt.d_date_sk = store_sales.ss_sold_date_sk and store_sales.ss_item_sk = item.i_item_sk and item.i_manufact_id = 100 and dt.d_moy = 11 group by dt.d_year, item.i_brand, item.i_brand_id order by dt.d_year, sum_agg desc, brand_id".to_string()),
        ("q98", "select i_item_id, i_category, i_class, i_current_price, sum(ss_ext_sales_price) as itemrevenue, sum(ss_ext_sales_price)*100/sum(sum(ss_ext_sales_price)) over (partition by i_class) as revenueratio from store_sales, item, date_dim where ss_item_sk = i_item_sk and i_category in ('Electronics', 'Books') and ss_sold_date_sk = d_date_sk and d_year = 2000 group by i_item_id, i_category, i_class, i_current_price order by i_category, i_class, i_item_id, revenueratio".to_string()),
        ("win_rank", "select i_category, i_brand, sum(ss_ext_sales_price) as rev, rank() over (partition by i_category order by sum(ss_ext_sales_price) desc) as rnk from store_sales, item where ss_item_sk = i_item_sk group by i_category, i_brand order by i_category, rnk".to_string()),
        ("win_running", "select d_date, sum(ss_ext_sales_price) as daily, sum(sum(ss_ext_sales_price)) over (order by d_date rows between unbounded preceding and current row) as running from store_sales, date_dim where ss_sold_date_sk = d_date_sk group by d_date order by d_date".to_string()),
        ("rollup", "select i_category, i_class, sum(ss_net_profit) as profit from store_sales, item where ss_item_sk = i_item_sk group by rollup(i_category, i_class) order by i_category, i_class".to_string()),
        ("grouping_sets", "select i_category, i_brand, sum(ss_quantity) as qty from store_sales, item where ss_item_sk = i_item_sk group by grouping sets ((i_category), (i_brand), ()) order by i_category, i_brand".to_string()),
        ("cube", "select d_year, i_category, sum(ss_ext_sales_price) as rev from store_sales, item, date_dim where ss_item_sk = i_item_sk and ss_sold_date_sk = d_date_sk group by cube(d_year, i_category) order by d_year, i_category".to_string()),
        ("intersect", "select ss_customer_sk from store_sales, item where ss_item_sk = i_item_sk and i_category = 'Electronics' intersect select ss_customer_sk from store_sales, item where ss_item_sk = i_item_sk and i_category = 'Books'".to_string()),
        ("except", "select ss_customer_sk from store_sales, item where ss_item_sk = i_item_sk and i_category = 'Electronics' except select ss_customer_sk from store_sales, item where ss_item_sk = i_item_sk and i_category = 'Books'".to_string()),
        ("cte", "with cat_rev as (select i_category, sum(ss_ext_sales_price) as rev from store_sales, item where ss_item_sk = i_item_sk group by i_category) select i_category, rev from cat_rev where rev > 10 order by rev desc".to_string()),
        ("count_distinct", "select i_category, count(distinct ss_customer_sk) as buyers from store_sales, item where ss_item_sk = i_item_sk group by i_category having count(distinct ss_customer_sk) >= 1 order by buyers desc, i_category".to_string()),
        ("correlated", "select i_item_sk, i_current_price from item where i_current_price > (select avg(i_current_price) from item i2 where i2.i_category = item.i_category) order by i_item_sk".to_string()),
        ("case_agg", "select i_category, sum(case when ss_net_profit > 10 then 1 else 0 end) as hi, sum(case when ss_net_profit <= 10 then 1 else 0 end) as lo from store_sales, item where ss_item_sk = i_item_sk group by i_category order by i_category".to_string()),
    ]
}

// --- Ledger shape ----------------------------------------------------------

#[derive(serde::Serialize)]
struct Methodology {
    engine: &'static str,
    engine_version: &'static str,
    git_sha: Option<String>,
    scale: f64,
    storage_regime: &'static str,
    datagen: &'static str,
    temperature_semantics: &'static str,
    routes: [&'static str; 2],
}

#[derive(serde::Serialize)]
struct LedgerRecord {
    benchmark: String,
    query: String,
    route: String,
    temperature: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    rows: usize,
    wall_ms: u128,
    #[serde(flatten)]
    snapshot: IoTraceSnapshot,
}

#[derive(serde::Serialize)]
struct Ledger {
    methodology: Methodology,
    records: Vec<LedgerRecord>,
}

/// Run one query: clear the capture, time the client round-trip, drain the
/// per-query billing snapshot (the observer fires server-side at scope close,
/// so poll briefly for it).
async fn measure(
    client: &Client,
    sql: &str,
) -> Result<(usize, u128, IoTraceSnapshot), (u128, String)> {
    CAPTURE.lock().expect("lock").clear();
    let t0 = Instant::now();
    let res = client.simple_query(sql).await;
    let wall_ms = t0.elapsed().as_millis();
    match res {
        Ok(msgs) => {
            let rows = msgs
                .iter()
                .filter(|m| matches!(m, SimpleQueryMessage::Row(_)))
                .count();
            let mut snap = IoTraceSnapshot::default();
            for _ in 0..60 {
                if let Some(s) = CAPTURE.lock().expect("lock").pop() {
                    snap = s;
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
            Ok((rows, wall_ms, snap))
        }
        Err(e) => Err((wall_ms, explain_err(&e))),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_record(
    out: &mut Vec<LedgerRecord>,
    benchmark: &str,
    query: &str,
    route: &str,
    temperature: &str,
    result: Result<(usize, u128, IoTraceSnapshot), (u128, String)>,
) {
    let rec = match result {
        Ok((rows, wall_ms, snapshot)) => LedgerRecord {
            benchmark: benchmark.into(),
            query: query.into(),
            route: route.into(),
            temperature: temperature.into(),
            ok: true,
            error: None,
            rows,
            wall_ms,
            snapshot,
        },
        Err((wall_ms, err)) => LedgerRecord {
            benchmark: benchmark.into(),
            query: query.into(),
            route: route.into(),
            temperature: temperature.into(),
            ok: false,
            error: Some(err),
            rows: 0,
            wall_ms,
            snapshot: IoTraceSnapshot::default(),
        },
    };
    out.push(rec);
}

async fn seed(client: &Client, schema: &[(&str, &str)], inserts: Vec<String>) {
    for (name, ddl) in schema {
        let _ = client
            .simple_query(&format!("DROP TABLE IF EXISTS {name}"))
            .await;
        client
            .simple_query(ddl)
            .await
            .unwrap_or_else(|e| panic!("CREATE {name}: {}", explain_err(&e)));
    }
    for sql in &inserts {
        client
            .simple_query(sql)
            .await
            .unwrap_or_else(|e| panic!("INSERT: {}", explain_err(&e)));
    }
}

/// Measure every query on both routes, first + repeat run each.
async fn run_benchmark(
    client: &Client,
    benchmark: &str,
    schema: &[(&str, &str)],
    inserts: Vec<String>,
    queries: Vec<(&'static str, String)>,
    out: &mut Vec<LedgerRecord>,
) {
    let n_inserts = inserts.len();
    seed(client, schema, inserts).await;
    eprintln!(
        "[{benchmark}] seeded ({n_inserts} insert batches across {} tables)",
        schema.len()
    );

    // Native/Volcano route (pre-MATERIALIZE).
    for (id, sql) in &queries {
        for temperature in ["first", "repeat"] {
            let r = measure(client, sql).await;
            push_record(out, benchmark, id, "native", temperature, r);
        }
    }

    // Flip to parquet-backed → DataFusion route.
    for (name, _) in schema {
        if let Err(e) = client
            .simple_query(&format!("ALTER TABLE {name} MATERIALIZE"))
            .await
        {
            eprintln!("[{benchmark}] · MATERIALIZE {name}: {}", explain_err(&e));
        }
    }

    // DataFusion route (post-MATERIALIZE).
    for (id, sql) in &queries {
        for temperature in ["first", "repeat"] {
            let r = measure(client, sql).await;
            push_record(out, benchmark, id, "datafusion", temperature, r);
        }
    }
}

async fn connect(server: &PgServer) -> Client {
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "perf evidence-ledger harness (TD-OLAP-4) — advisory; run with --ignored --nocapture"]
async fn tpc_perf_ledger() {
    let scale: f64 = std::env::var("TPC_PERF_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.001);
    eprintln!("=== tpc-perf-ledger harness (TPC_PERF_SCALE={scale}) ===");

    let mut records = Vec::new();

    // Fresh server per benchmark: TPC-H and TPC-DS both define `customer`.
    {
        let server = PgServer::start().await.expect("server start (tpch)");
        let client = connect(&server).await;
        run_benchmark(
            &client,
            "tpch",
            TPCH_SCHEMA,
            gen_tpch(scale),
            tpch_queries(),
            &mut records,
        )
        .await;
        server.shutdown().await;
    }
    {
        let server = PgServer::start().await.expect("server start (tpcds)");
        let client = connect(&server).await;
        run_benchmark(
            &client,
            "tpcds",
            TPCDS_SCHEMA,
            gen_tpcds(scale),
            tpcds_queries(),
            &mut records,
        )
        .await;
        server.shutdown().await;
    }

    // Console summary: per benchmark × route, pass count + medians.
    for benchmark in ["tpch", "tpcds"] {
        for route in ["native", "datafusion"] {
            let mut rows: Vec<&LedgerRecord> = records
                .iter()
                .filter(|r| {
                    r.benchmark == benchmark && r.route == route && r.temperature == "repeat"
                })
                .collect();
            rows.sort_by_key(|r| r.wall_ms);
            let ok = rows.iter().filter(|r| r.ok).count();
            let median = rows.get(rows.len() / 2).map(|r| r.wall_ms).unwrap_or(0);
            let bytes: u64 = rows.iter().map(|r| r.snapshot.bytes_read).sum();
            eprintln!(
                "[{benchmark}/{route}] ok {ok}/{} · median repeat wall {median} ms · total bytes_read {bytes}",
                rows.len()
            );
        }
    }

    let ledger = Ledger {
        methodology: Methodology {
            engine: "proximadb",
            engine_version: env!("CARGO_PKG_VERSION"),
            git_sha: std::env::var("GITHUB_SHA").ok(),
            scale,
            storage_regime: "local-tempdir",
            datagen: "synthetic-tpc-shaped (seeded, deterministic, date-clustered facts, non-audited)",
            temperature_semantics: "first vs repeat against a warm process (not cold page cache)",
            routes: [
                "native (Volcano, pre-MATERIALIZE)",
                "datafusion (Parquet, post-MATERIALIZE)",
            ],
        },
        records,
    };

    let out_path = std::env::var("TPC_PERF_LEDGER_OUT")
        .unwrap_or_else(|_| "target/tpc-perf-ledger/ledger.json".to_string());
    if let Some(dir) = std::path::Path::new(&out_path).parent() {
        std::fs::create_dir_all(dir).expect("create ledger dir");
    }
    std::fs::write(
        &out_path,
        serde_json::to_vec_pretty(&ledger).expect("serialize ledger"),
    )
    .expect("write ledger");
    eprintln!("ledger written: {out_path}");

    // Advisory skeleton: assert only harness integrity, never perf numbers.
    let n_queries = 22 + 16;
    assert_eq!(
        ledger.records.len(),
        n_queries * 2 /* routes */ * 2, /* temperatures */
        "one record per query x route x temperature"
    );
}
