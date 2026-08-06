#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Monte Carlo option pricing from a NOTEBOOK — in-database compute over pgwire.

This is the notebook persona (the same workload as ``montecarlo.sql`` for the SQL editor):
a quant/data engineer connects a standard Postgres driver to ProximaDB's pgwire port and
drives a real Monte Carlo simulation *in the database* via the built-in ``mc_price`` scalar
UDF — the simulation runs where the data lives (compute-to-data), so there is no extract to
a separate Spark/pandas cluster.

The file is written as ``# %%`` cells: run it straight (``python montecarlo_notebook.py``)
or open it as a notebook (VS Code / Jupyter via jupytext). Only ``psycopg2`` is required.

Connection (env-overridable): PROXIMADB_PG_HOST=127.0.0.1  PROXIMADB_PG_PORT=5433
                              PROXIMADB_DB=quant  (the pgwire database == the tenant/catalog)
"""

# %% [markdown]
# ## 1. Connect to ProximaDB over pgwire
# The database name is the tenant/catalog boundary. Any standard Postgres driver works.

# %%
import os

import psycopg2

conn = psycopg2.connect(
    host=os.environ.get("PROXIMADB_PG_HOST", "127.0.0.1"),
    port=int(os.environ.get("PROXIMADB_PG_PORT", "5433")),
    dbname=os.environ.get("PROXIMADB_DB", "quant"),
    user="postgres",
)
conn.autocommit = True
cur = conn.cursor()
print("connected to ProximaDB pgwire")


# %% [markdown]
# ## 2. Load a derivatives book and MATERIALIZE it
# `MATERIALIZE` makes the table Parquet-backed, so SELECTs route to the DataFusion compute
# engine — where the `mc_price` Monte Carlo UDF lives.

# %%
cur.execute("DROP TABLE IF EXISTS option_book")
cur.execute(
    "CREATE TABLE option_book (option_id TEXT, underlying TEXT, spot DOUBLE PRECISION, "
    "strike DOUBLE PRECISION, vol DOUBLE PRECISION, rate DOUBLE PRECISION, "
    "t DOUBLE PRECISION, is_call BOOLEAN, qty BIGINT)"
)
cur.execute(
    "INSERT INTO option_book VALUES "
    "('o1','ACME',100.0,100.0,0.20,0.05,1.0,true,10),"
    "('o2','ACME',100.0,110.0,0.20,0.05,1.0,true,5),"
    "('o3','ACME',100.0,90.0,0.20,0.05,1.0,false,8),"
    "('o4','GLOBEX',50.0,55.0,0.35,0.05,0.5,true,20)"
)
cur.execute("ALTER TABLE option_book MATERIALIZE")
print("book loaded + materialized")


# %% [markdown]
# ## 3. Price every option — 200k Monte Carlo paths each, in the database

# %%
cur.execute(
    "SELECT option_id, underlying, "
    "  round(mc_price(spot,strike,vol,rate,t,is_call,200000)::numeric, 4) AS mc_price "
    "FROM option_book "
    "GROUP BY option_id, underlying, spot, strike, vol, rate, t, is_call "
    "ORDER BY option_id"
)
print(f"{'option':8} {'underlying':10} {'mc_price':>10}")
for opt, und, price in cur.fetchall():
    print(f"{opt:8} {und:10} {float(price):>10.4f}")


# %% [markdown]
# ## 4. Portfolio valuation — mark-to-model book value per underlying (price × position)

# %%
cur.execute(
    "SELECT underlying, count(*) AS n, "
    "  round(sum(mc_price(spot,strike,vol,rate,t,is_call,200000)*qty)::numeric, 2) AS book_value "
    "FROM option_book GROUP BY underlying ORDER BY book_value DESC"
)
print(f"{'underlying':10} {'n_options':>10} {'book_value':>12}")
for und, n, val in cur.fetchall():
    print(f"{und:10} {n:>10} {float(val):>12.2f}")


# %% [markdown]
# ## 5. Risk curve — reprice the WHOLE book across a volatility sweep, in one query
# Five volatility scenarios (0.5×…1.5× of book vol) repriced in-database — a vega curve
# for the desk. Only the five aggregates leave the engine.

# %%
cur.execute(
    "SELECT v.vol_mult, "
    "  round(sum(mc_price(spot,strike,vol*v.vol_mult,rate,t,is_call,100000)*qty)::numeric, 2) AS book_value "
    "FROM option_book "
    "CROSS JOIN (VALUES (0.5),(0.75),(1.0),(1.25),(1.5)) AS v(vol_mult) "
    "GROUP BY v.vol_mult ORDER BY v.vol_mult"
)
curve = [(float(m), float(val)) for m, val in cur.fetchall()]
peak = max(val for _, val in curve) or 1.0
print(f"{'vol x':>6}  {'book_value':>11}  risk curve")
for mult, val in curve:
    bar = "█" * int(round(40 * val / peak))
    print(f"{mult:>6.2f}  {val:>11.2f}  {bar}")


# %% [markdown]
# ## 6. Takeaway
# The Monte Carlo simulation ran **inside ProximaDB** over pgwire — no data left the engine
# for a separate compute cluster. One SQL surface serves both a data engineer (this notebook)
# and an analyst (the SQL editor / `montecarlo.sql`). Compute-to-data: the simulation runs
# where the book lives.

# %%
cur.close()
conn.close()
print("done — compute ran where the data lives.")
