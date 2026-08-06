#!/usr/bin/env bash
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
#
# Seed the non-relational modalities (timeseries + graph) through the REAL v2 REST
# API, under one tenant, so the pgwire cross-modal join in crossmodal.sql can read
# them back in a single SQL plan. The tenant here (X-Tenant-ID) MUST match the pgwire
# `dbname` the SQL connects with — the catalog is the tenant boundary (TD-064), and
# the UDTFs read under the connection tenant (TD-XMODAL-6).
set -euo pipefail

REST="${PROXIMADB_REST:-http://127.0.0.1:5678}"
TENANT="${PROXIMADB_TENANT:-de_demo}"
CURL=(curl -s --noproxy '*' -H "X-Tenant-ID: ${TENANT}" -H 'content-type: application/json')

echo "→ seeding under tenant '${TENANT}' at ${REST}"

# ── Timeseries: a per-account transaction-rate stream (acct-7 has a burst) ──
"${CURL[@]}" -X POST "${REST}/api/v2/timeseries/collections" \
  -d '{"name":"acct_txn","value_columns":[{"name":"rate"}]}' >/dev/null
"${CURL[@]}" -X POST "${REST}/api/v2/timeseries/collections/acct_txn/ingest" \
  -d '{"points":[
        {"timestamp":1000,"values":{"rate":12.0}},
        {"timestamp":2000,"values":{"rate":880.0}},
        {"timestamp":3000,"values":{"rate":33.0}}]}' >/dev/null
echo "  ✓ timeseries acct_txn (3 points)"

# ── Vector: a fraud-typology library; records keyed by account so the join keys on id ──
# 2-D behavioural signature [place-and-cancel burstiness, steady-flow]; acct-7 sits on the
# fraud-like axis, the others on the benign axis.
"${CURL[@]}" -X POST "${REST}/api/v2/collections" \
  -d '{"name":"patterns","dimension":2,"engine":"sst","distance_metric":"cosine"}' >/dev/null
"${CURL[@]}" -X POST "${REST}/api/v2/collections/patterns/records/batch" \
  -d '{"records":[
        {"id":"acct-7","vector":[0.95,0.05]},
        {"id":"acct-9","vector":[0.10,0.90]},
        {"id":"acct-3","vector":[0.20,0.80]}]}' >/dev/null
echo "  ✓ vector patterns (3 account signatures)"

# ── Graph: a money-flow chain acct-7 → acct-31 → acct-88 → acct-99 ──
"${CURL[@]}" -X POST "${REST}/api/v2/graphs" \
  -d '{"graph_id":"flows","name":"money flows"}' >/dev/null
for n in acct-7 acct-31 acct-88 acct-99; do
  "${CURL[@]}" -X POST "${REST}/api/v2/graphs/flows/nodes" \
    -d "{\"node\":{\"id\":\"${n}\",\"labels\":[\"account\"]}}" >/dev/null
done
i=0
for pair in "acct-7:acct-31" "acct-31:acct-88" "acct-88:acct-99"; do
  i=$((i + 1)); from="${pair%:*}"; to="${pair#*:}"
  "${CURL[@]}" -X POST "${REST}/api/v2/graphs/flows/edges" \
    -d "{\"edge\":{\"id\":\"e${i}\",\"from_node_id\":\"${from}\",\"to_node_id\":\"${to}\",\"edge_type\":\"sent_to\"}}" >/dev/null
done
echo "  ✓ graph flows (4 nodes, 3 sent_to edges)"

echo "→ seeded. Now run the SQL under the SAME tenant: psql \"dbname=${TENANT}\""
