-- Copyright (C) 2025 ProximaDB
-- SPDX-License-Identifier: Apache-2.0
--
-- Step 1 — warehouse analytics over pgwire (the data-engineer's bread and butter).
--
-- Create a fact table, load it, MATERIALIZE it to Parquet (which routes the table to
-- the DataFusion OLAP engine behind ProximaDB's ComputeBackend seam), then run a
-- GROUP BY aggregate. This is plain ANSI SQL on a real PostgreSQL wire connection —
-- nothing ProximaDB-specific yet. The cross-modal payoff is in crossmodal.sql.

DROP TABLE IF EXISTS orders;

CREATE TABLE orders (
    order_id   TEXT,
    account_id TEXT,
    region     TEXT,
    amount     DOUBLE PRECISION
);

INSERT INTO orders VALUES
    ('o1', 'acct-7', 'emea', 100.0),
    ('o2', 'acct-7', 'emea', 9000.0),
    ('o3', 'acct-9', 'amer', 250.0),
    ('o4', 'acct-3', 'apac', 75.0),
    ('o5', 'acct-9', 'amer', 40.0);

-- Parquet-backed => the ComputeScheduler routes SELECTs over this table to the
-- DataFusion OLAP engine (vectorized aggregates), not the row-wise Volcano floor.
ALTER TABLE orders MATERIALIZE;

-- Pure relational analytics: gross revenue per region.
SELECT region,
       count(*)    AS n_orders,
       sum(amount) AS gross
FROM orders
GROUP BY region
ORDER BY gross DESC;
