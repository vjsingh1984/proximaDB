#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Fraud & AML *compliance-analyst copilot* — the flagship ProximaDB financial-services demo.

An analyst asks one question — "account acct-007 tripped an alert, is it laundering?" —
and the copilot answers by chaining **four modalities in one engine** (no ETL between a
timeseries store, a vector DB, a graph DB, and a search index):

  1. **timeseries** — per-account transacted-amount streams; flag the volume bursts
  2. **vector**     — match each flagged account's behaviour to a known AML *typology*
  3. **graph**      — trace the money-laundering ring with a Cypher path query
  4. **RAG**        — retrieve the case note + sanctions guidance with the resolution

Every hop is a **real ProximaDB v2 API call** (printed with latency) — run it against a
ProximaDB built from the same code (timeseries + graph landed in #651):

    cargo build --release -p proximadb-server
    docker build -f Dockerfile.prebuilt -t proximadb:develop .
    docker run -d -p 5678:5678 proximadb:develop
    python generate_data.py && python copilot_live.py

Env: ``PROXIMADB_URL`` (default ``http://localhost:5678``). Pure stdlib (urllib).
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import time
import urllib.error
import urllib.request
from datetime import datetime
from pathlib import Path

BASE = os.environ.get("PROXIMADB_URL", "http://localhost:5678").rstrip("/")
DATA = Path(os.environ.get("DATA", Path(__file__).parent / "data"))
EMB_DIM = 256
TYP_COLL = "aml_typologies"
CASE_COLL = "aml_casenotes"
BURST_THRESHOLD = 5000.0  # a single-bucket transacted volume this large is suspicious


# ── tiny REST client (stdlib) ───────────────────────────────────────────────────
def _req(method: str, path: str, body: dict | None = None) -> tuple[int, dict]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(f"{BASE}{path}", data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def api(method: str, path: str, body: dict | None = None, label: str = "") -> dict:
    t = time.time()
    status, resp = _req(method, path, body)
    ms = (time.time() - t) * 1000
    if label:
        print(f"     [API {'✓' if 200 <= status < 300 else '✗'}] {method} {path}  ({status}, {ms:.0f} ms)")
    return resp


def embed(text: str, dim: int = EMB_DIM) -> list[float]:
    v = [0.0] * dim
    for tok in text.lower().replace(",", " ").replace(".", " ").split():
        v[int(hashlib.md5(tok.encode()).hexdigest(), 16) % dim] += 1.0
    norm = math.sqrt(sum(x * x for x in v)) or 1.0
    return [round(x / norm, 6) for x in v]


def unwrap(x):
    return x["value"] if isinstance(x, dict) and "value" in x else x


def iso_ms(iso: str) -> int:
    return int(datetime.fromisoformat(iso).timestamp() * 1000)


def _load(name: str):
    return json.loads((DATA / name).read_text())


# ── setup (real ProximaDB writes) ───────────────────────────────────────────────
def load_typologies(typologies) -> None:
    recs = [{"id": t["id"], "vector": t["features"],
             "props": {"label": t["label"], "case_ref": t["case_ref"] or ""}}
            for t in typologies if t["label"] != "nominal"]
    api("POST", "/api/v2/collections", {"name": TYP_COLL, "dimension": 4, "distance_metric": "cosine"})
    r = api("POST", f"/api/v2/collections/{TYP_COLL}/records/batch", {"records": recs, "upsert": True},
            label="load AML typology signatures")
    print(f"           ↳ {r.get('inserted_count', 0)} typologies")


def load_casenotes(cases) -> None:
    recs = [{"id": c["id"], "vector": embed(f"{c['typology']} {c['typology']} {c['text']}"),
             "props": {"typology": c["typology"], "resolution": c["resolution"]}}
            for c in cases]
    api("POST", "/api/v2/collections", {"name": CASE_COLL, "dimension": EMB_DIM, "distance_metric": "cosine"})
    r = api("POST", f"/api/v2/collections/{CASE_COLL}/records/batch", {"records": recs, "upsert": True},
            label="load AML case notes + sanctions")
    print(f"           ↳ {r.get('inserted_count', 0)} case notes")


def build_account_graph(accounts) -> None:
    """Accounts + sent_to transfers as a ProximaDB graph (untraced setup calls)."""
    api("POST", "/api/v2/graphs", {"graph_id": "aml"})
    for a in accounts["nodes"]:
        api("POST", "/api/v2/graphs/aml/nodes",
            {"node": {"id": a["id"], "labels": ["Account"],
                      "properties": {"kind": a["kind"], "country": a["country"]}}})
    for e in accounts["edges"]:
        # keep material flows (repeated or high-value) so the ring path stands out
        # from one-off background transfers.
        if (e["from"].startswith("acct-") and e["to"].startswith("acct-")
                and (e["count"] >= 2 or e["total"] >= 2000)):
            api("POST", "/api/v2/graphs/aml/edges",
                {"edge": {"id": f"{e['from']}->{e['to']}", "from_node_id": e["from"],
                          "to_node_id": e["to"], "edge_type": "sent_to", "weight": float(e["count"])}})
    print(f"     [graph] built transaction graph: {len(accounts['nodes'])} accounts (live)")


# ── analysis hops (all live ProximaDB calls) ────────────────────────────────────
def timeseries_scan(ts_accounts) -> list[dict]:
    """Ingest each account's transacted-amount stream into ProximaDB time-series, then
    aggregate SUM per 30-min bucket to flag volume bursts. Real /api/v2/timeseries/*."""
    print(f"     [timeseries] ingesting {len(ts_accounts)} account streams + aggregating (live API)…")
    flagged = []
    for acc in ts_accounts:
        pts = acc["points"]
        if len(pts) < 3:
            continue
        coll = "ts_" + acc["account"].replace("-", "_")
        api("POST", "/api/v2/timeseries/collections", {"name": coll})
        api("POST", f"/api/v2/timeseries/collections/{coll}/ingest",
            {"points": [{"timestamp": iso_ms(p["ts"]), "values": {"amount": p["amount"]}} for p in pts]})
        res = api("POST", f"/api/v2/timeseries/collections/{coll}/aggregate",
                  {"start_time": iso_ms(pts[0]["ts"]) - 1, "end_time": iso_ms(pts[-1]["ts"]) + 1,
                   "aggregation": "sum", "bucket_ms": 1_800_000})
        sums = [b["values"]["amount"] for b in res.get("buckets", []) if "amount" in b.get("values", {})]
        if not sums:
            continue
        peak = max(sums)
        if peak > BURST_THRESHOLD:  # a large single-window transacted volume
            mean = sum(sums) / len(sums)
            burst = sum(1 for s in sums if s > BURST_THRESHOLD) / len(sums)
            flagged.append({
                "account": acc["account"], "peak": round(peak, 1),
                "features": [round(peak, 3), round(peak / (mean or 1e-6), 3),
                             round(burst, 3), round(mean, 3)],
            })
    return sorted(flagged, key=lambda f: f["peak"], reverse=True)


def trace_ring(account) -> list[str]:
    """Trace the laundering ring downstream of an account via a live Cypher path query."""
    q = f"MATCH (a:Account {{id:'{account}'}})-[:sent_to*1..4]->(x:Account) RETURN x"
    res = api("POST", "/api/v2/graphs/aml/query", {"query": q, "language": "cypher"},
              label=f"Cypher trace laundering ring from {account}")
    rows = res.get("data", {}).get("rows", [])
    return [r.get("node_id") or r.get("id") for r in rows]


def say(role, text):
    print(f"\n{'🕵️  analyst' if role == 'analyst' else '🤖 copilot'}: {text}")


def main() -> None:
    if not DATA.exists():
        raise SystemExit("dataset missing — run `python generate_data.py` first")
    accounts = _load("accounts.json")
    ts_accounts = _load("transactions.json")
    typologies = _load("typologies.json")
    cases = _load("casenotes.json")

    print("=" * 78)
    print(f"  ProximaDB AML compliance copilot — LIVE against {BASE}")
    print("  one engine · timeseries + vector + graph + RAG")
    print("=" * 78)
    caps = api("GET", "/api/v2/_meta/capabilities")
    print(f"  engine features: {', '.join(caps.get('features', [])[-4:])}\n")

    print("── setup (real ProximaDB writes across all four modalities) ──")
    load_typologies(typologies)
    load_casenotes(cases)
    build_account_graph(accounts)

    print("\n── copilot ──")
    say("analyst", "Account acct-007 tripped a transaction-monitoring alert. Is it laundering?")

    flagged = timeseries_scan(ts_accounts)
    say("copilot", f"[timeseries] scanned {len(ts_accounts)} account streams via ProximaDB — "
                   f"{len(flagged)} with suspicious volume bursts: "
                   + ", ".join(f["account"] for f in flagged))

    # Investigate the alerted account (acct-007, the ring's placement head).
    target = next((f for f in flagged if f["account"] == "acct-007"), flagged[0] if flagged else None)
    for f in ([target] if target else []):
        acct = f["account"]
        # VECTOR — match behaviour to an AML typology
        res = api("POST", f"/api/v2/collections/{TYP_COLL}/search",
                  {"vector": f["features"], "top_k": 1}, label=f"match {acct} to AML typology")
        hits = res.get("results", [])
        typ = unwrap(hits[0]["props"].get("label", "?")) if hits else "?"
        say("copilot", f"[vector] {acct} behaviour → **{typ}** (score {hits[0]['score']:.3f}, live ANN)")

        # GRAPH — trace the ring with Cypher
        ring = trace_ring(acct)
        say("copilot", f"[graph] laundering ring downstream of {acct} → {' → '.join(ring) if ring else 'none'}")

        # RAG — retrieve the matching case note + resolution
        res = api("POST", f"/api/v2/collections/{CASE_COLL}/search",
                  {"vector": embed(f"{typ} {typ} {typ}"), "top_k": 1}, label="retrieve case note (RAG)")
        rhits = res.get("results", [])
        if rhits:
            p = rhits[0]["props"]
            say("copilot", f"[RAG] {rhits[0]['id']} → {unwrap(p.get('resolution', ''))}")

    print("\n" + "-" * 78)
    print("Every [API] line is a real ProximaDB call. In production this runs behind the")
    print("AnvaiOps MCP copilot — metered/entitlement-checked/PII-redacted per tenant.")


if __name__ == "__main__":
    main()
