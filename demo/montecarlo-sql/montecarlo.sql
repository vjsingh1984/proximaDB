-- Copyright (C) 2025 ProximaDB
-- SPDX-License-Identifier: Apache-2.0
--
-- Monte Carlo option pricing as an IN-DATABASE compute workload, over pgwire.
--
-- ProximaDB ships a vectorized Monte Carlo European-option pricer as a SQL scalar
-- function — `mc_price(spot, strike, vol, rate, t, is_call, n_paths)` — that runs on
-- the DataFusion compute engine. A quant / data engineer prices and stress-tests a
-- whole derivatives book with plain SQL: the simulation runs WHERE THE DATA LIVES
-- (compute-to-data), so nothing is extracted to a separate Spark/pandas cluster.
--
-- Routing note: `mc_price` is a DataFusion UDF, so a query must engage the OLAP
-- engine (an aggregate / GROUP BY over a MATERIALIZE'd, Parquet-backed table). A bare
-- scalar `SELECT mc_price(...)` stays on the row-wise path where the UDF isn't bound —
-- so we price per-row by grouping on the option id, and the portfolio by underlying.

DROP TABLE IF EXISTS option_book;

CREATE TABLE option_book (
    option_id  TEXT,
    underlying  TEXT,
    spot       DOUBLE PRECISION,
    strike     DOUBLE PRECISION,
    vol        DOUBLE PRECISION,   -- annualized volatility
    rate       DOUBLE PRECISION,   -- risk-free rate
    t          DOUBLE PRECISION,   -- time to expiry (years)
    is_call    BOOLEAN,
    qty        BIGINT              -- position size (contracts)
);

INSERT INTO option_book VALUES
    ('o1', 'ACME',   100.0, 100.0, 0.20, 0.05, 1.0, true,  10),  -- ATM call
    ('o2', 'ACME',   100.0, 110.0, 0.20, 0.05, 1.0, true,   5),  -- OTM call
    ('o3', 'ACME',   100.0,  90.0, 0.20, 0.05, 1.0, false,  8),  -- OTM put
    ('o4', 'GLOBEX',  50.0,  55.0, 0.35, 0.05, 0.5, true,  20);  -- OTM call, higher vol

-- Parquet-backed ⇒ SELECTs route to the DataFusion compute engine (where `mc_price` lives).
ALTER TABLE option_book MATERIALIZE;

-- 1) Per-option Monte Carlo price (200k paths each) — grouped by id so the UDF engages.
SELECT option_id,
       underlying,
       round(mc_price(spot, strike, vol, rate, t, is_call, 200000)::numeric, 4) AS mc_price
FROM option_book
GROUP BY option_id, underlying, spot, strike, vol, rate, t, is_call
ORDER BY option_id;

-- 2) Portfolio valuation — mark-to-model book value per underlying (price × position).
SELECT underlying,
       count(*)                                                                    AS n_options,
       round(sum(mc_price(spot, strike, vol, rate, t, is_call, 200000) * qty)::numeric, 2) AS book_value
FROM option_book
GROUP BY underlying
ORDER BY book_value DESC;

-- 3) Risk scenario — reprice the whole book under a +50% volatility shock, in ONE query.
--    (The stress simulation runs in-database; only the two aggregates leave.)
SELECT underlying,
       round(sum(mc_price(spot, strike, vol,       rate, t, is_call, 100000) * qty)::numeric, 2) AS base_value,
       round(sum(mc_price(spot, strike, vol * 1.5, rate, t, is_call, 100000) * qty)::numeric, 2) AS vol_stress_value
FROM option_book
GROUP BY underlying
ORDER BY underlying;
