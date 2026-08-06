#!/usr/bin/env bash
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
#
# Data-engineering demo: warehouse analytics + the zero-ETL cross-modal join, driven
# entirely over the PostgreSQL wire protocol against a REAL running ProximaDB.
#
# Prerequisites — a ProximaDB built from THIS code, with pgwire enabled:
#   cargo build --release -p proximadb-server        # or: make build-server
#   # run it with pgwire on 5433 and REST on 5678 (pg_port = 5433 in config.toml [api]):
#   ./target/release/proximadb-server -c config/config.toml
#   # (Docker: build the image from this code — the cross-modal UDTFs + pgwire OLAP
#   #  route only exist in a same-code build — then publish 5678 and 5433.)
#
# The cross-modal UDTFs read under the pgwire connection's tenant, and the catalog
# (the psql `dbname`) IS the tenant boundary — so we seed the vector/timeseries/graph
# data via REST under tenant "de_demo" and connect psql with dbname=de_demo. They MUST
# match (TD-XMODAL-6).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PROXIMADB_REST="${PROXIMADB_REST:-http://127.0.0.1:5678}"
export PROXIMADB_TENANT="${PROXIMADB_TENANT:-de_demo}"
PG_HOST="${PROXIMADB_PG_HOST:-127.0.0.1}"
PG_PORT="${PROXIMADB_PG_PORT:-5433}"
CONN="host=${PG_HOST} port=${PG_PORT} user=postgres dbname=${PROXIMADB_TENANT} sslmode=disable"

echo "▶ ProximaDB data-engineering demo (pgwire ${PG_HOST}:${PG_PORT}, REST ${PROXIMADB_REST}, tenant ${PROXIMADB_TENANT})"

# 1) Seed the non-relational modalities via the v2 REST API under the tenant.
bash "${HERE}/seed.sh"

# 2) Warehouse analytics — CREATE / INSERT / MATERIALIZE / GROUP BY over pgwire.
echo; echo "── warehouse.sql : relational analytics (routes to the DataFusion OLAP engine) ──"
psql "${CONN}" -f "${HERE}/warehouse.sql"

# 3) The moat: relational ⋈ vector ⋈ timeseries ⋈ graph in ONE data-local SQL plan.
echo; echo "── crossmodal.sql : the zero-ETL four-way cross-modal join ──"
psql "${CONN}" -f "${HERE}/crossmodal.sql"

echo; echo "✓ done — one engine, one query plane, zero ETL."
