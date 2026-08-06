-- Copyright (C) 2025 ProximaDB
-- SPDX-License-Identifier: Apache-2.0
--
-- Step 2 — the zero-ETL cross-modal join (the moat).
--
-- ONE SQL statement, over a real PostgreSQL wire connection, joins the relational
-- warehouse fact table against THREE non-relational modalities living in the SAME
-- engine — no ETL between a metrics store, a vector DB, and a graph DB:
--
--   relational  (orders)              — the warehouse fact table (Parquet / DataFusion)
--   ⋈ vector    vector_search(...)    — similarity of the account's behaviour to a
--                                       library of known fraud patterns (live ANN)
--   ⋈ timeseries timeseries_range(...) — the account's peak transaction rate (live TST)
--   ⋈ graph     graph_traverse(...)   — the money-flow accounts downstream of it (live ORION)
--
-- The cross-modal sources are DataFusion table-valued functions (ADR-053); the
-- ComputeScheduler routes the whole plan data-local, so what ships to the client is the
-- ANSWER, not four intermediate result sets glued together in application code.
--
-- Scenario: a data engineer triaging high-value orders asks, for account 'acct-7',
-- "how fraud-like is its behaviour, how hot is its transaction stream, and where does
--  its money flow?" — resolved in a single query the analyst actually types.

SELECT
    o.account_id,
    o.region,
    sum(o.amount)   AS order_gross,          -- relational: the account's booked orders
    v.score         AS fraud_pattern_score,  -- vector: cosine similarity to the fraud typology
    ts.peak_txn_rate,                         -- timeseries: peak per-window transaction rate
    g.node_id       AS downstream_account,   -- graph: an account money flows to
    g.depth         AS hops_downstream       -- graph: how many hops away
FROM orders o
JOIN vector_search('patterns', '[0.95,0.05]', 5) v
    ON o.account_id = v.id
CROSS JOIN (
    SELECT max(value) AS peak_txn_rate
    FROM timeseries_range('acct_txn', 0, 9999999)
) ts
JOIN graph_traverse('flows', 'acct-7', 'sent_to', 4) g
    ON true
WHERE o.account_id = 'acct-7'
GROUP BY o.account_id, o.region, v.score, ts.peak_txn_rate, g.node_id, g.depth
ORDER BY g.depth;
