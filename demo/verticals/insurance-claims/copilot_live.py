#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Insurance *SIU claims-fraud copilot* — a ProximaDB insurance-vertical demo.

A Special Investigations Unit analyst asks one question — "claim from clmt-007 was flagged,
is it a staged-accident ring?" — and the copilot answers by chaining **four modalities in
one engine** (no ETL between a metrics store, a vector DB, a graph DB, and a search index):

  1. **timeseries** — per-claimant claimed-amount streams; flag the amount bursts
  2. **vector**     — match the flagged claimant's burst signature to a known fraud *typology*
  3. **graph**      — trace the fraud ring through the shared crooked provider (Cypher multi-hop)
  4. **RAG**        — retrieve the SIU case note + fraud-indicator guidance with the resolution

Every hop is a **real ProximaDB v2 API call** (printed with latency). Run it against a
ProximaDB built from the same code (timeseries + multi-hop Cypher):

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
TYP_COLL = "ins_fraud_typologies"
CASE_COLL = "ins_casenotes"
BURST_THRESHOLD = 8000.0  # a single-window claimed volume this large is suspicious
ALERT_CLAIMANT = "clmt-007"  # the claimant the fraud model flagged


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
    for tok in text.lower().replace(",", " ").replace(".", " ").replace("-", " ").split():
        v[int(hashlib.md5(tok.encode()).hexdigest(), 16) % dim] += 1.0
    norm = math.sqrt(sum(x * x for x in v)) or 1.0
    return [round(x / norm, 6) for x in v]


def unwrap(x):
    return x["value"] if isinstance(x, dict) and "value" in x else x


def iso_ms(iso: str) -> int:
    return int(datetime.fromisoformat(iso).timestamp() * 1000)


def _load(name: str):
    return json.loads((DATA / name).read_text())


def feature_vector(sums: list[float]) -> list[float]:
    """Scale-normalised shape of a claimed-amount burst — must match the typology vectors in
    generate_data.py. Scaling each dimension to O(1..10) lets cosine discriminate on burst
    *shape* (a tight staged-ring spike vs sustained provider collusion) not raw dollars."""
    peak = max(sums)
    total = sum(sums) or 1e-6
    mean = total / len(sums)
    spread = sum(1 for s in sums if s > BURST_THRESHOLD) / len(sums)
    return [round(peak / 10000.0, 3), round(peak / (mean or 1e-6), 3),
            round(spread * 10.0, 3), round(peak / total * 10.0, 3)]


# ── setup (real ProximaDB writes) ───────────────────────────────────────────────
def load_typologies(typologies) -> None:
    recs = [{"id": t["id"], "vector": t["features"],
             "props": {"label": t["label"], "case_ref": t["case_ref"] or ""}}
            for t in typologies if t["label"] != "nominal"]
    api("POST", "/api/v2/collections", {"name": TYP_COLL, "dimension": 4, "distance_metric": "cosine"})
    r = api("POST", f"/api/v2/collections/{TYP_COLL}/records/batch", {"records": recs, "upsert": True},
            label="load fraud-typology signatures")
    print(f"           ↳ {r.get('inserted_count', 0)} typologies")


def load_casenotes(cases) -> None:
    recs = [{"id": c["id"], "vector": embed(f"{c['typology']} {c['typology']} {c['text']}"),
             "props": {"typology": c["typology"], "resolution": c["resolution"]}}
            for c in cases]
    api("POST", "/api/v2/collections", {"name": CASE_COLL, "dimension": EMB_DIM, "distance_metric": "cosine"})
    r = api("POST", f"/api/v2/collections/{CASE_COLL}/records/batch", {"records": recs, "upsert": True},
            label="load SIU case notes")
    print(f"           ↳ {r.get('inserted_count', 0)} case notes")


def build_party_graph(parties) -> None:
    """Claimants + providers + `linked` edges as a ProximaDB graph (untraced setup calls)."""
    api("POST", "/api/v2/graphs", {"graph_id": "ins"})
    for n in parties["nodes"]:
        api("POST", "/api/v2/graphs/ins/nodes",
            {"node": {"id": n["id"], "labels": ["Party"], "properties": {"kind": n["kind"]}}})
    for e in parties["edges"]:
        api("POST", "/api/v2/graphs/ins/edges",
            {"edge": {"id": f"{e['from']}->{e['to']}", "from_node_id": e["from"],
                      "to_node_id": e["to"], "edge_type": "linked", "weight": 1.0}})
    print(f"     [graph] built claim network: {len(parties['nodes'])} parties (live)")


# ── analysis hops (all live ProximaDB calls) ────────────────────────────────────
def timeseries_scan(ts_claimants) -> list[dict]:
    """Ingest each claimant's claimed-amount stream into ProximaDB time-series, then aggregate
    SUM per 30-min bucket to flag amount bursts. Real /api/v2/timeseries/*."""
    print(f"     [timeseries] ingesting {len(ts_claimants)} claim streams + aggregating (live API)…")
    flagged = []
    for acc in ts_claimants:
        pts = acc["points"]
        if len(pts) < 3:
            continue
        coll = "ts_" + acc["claimant"].replace("-", "_")
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
        if peak > BURST_THRESHOLD:
            flagged.append({"claimant": acc["claimant"], "peak": round(peak, 1),
                            "features": feature_vector(sums)})
    return sorted(flagged, key=lambda f: f["peak"], reverse=True)


def trace_ring(party) -> list[str]:
    """Trace the fraud ring downstream of a claimant via a live Cypher multi-hop query."""
    q = f"MATCH (a:Party {{id:'{party}'}})-[:linked*1..4]->(x:Party) RETURN x"
    res = api("POST", "/api/v2/graphs/ins/query", {"query": q, "language": "cypher"},
              label=f"Cypher trace fraud ring from {party}")
    rows = res.get("data", {}).get("rows", [])
    return [r.get("node_id") or r.get("id") for r in rows]


def say(role, text):
    print(f"\n{'🕵️  SIU' if role == 'siu' else '🤖 copilot'}: {text}")


def main() -> None:
    if not DATA.exists():
        raise SystemExit("dataset missing — run `python generate_data.py` first")
    parties = _load("parties.json")
    ts_claimants = _load("claims.json")
    typologies = _load("typologies.json")
    cases = _load("casenotes.json")

    print("=" * 78)
    print(f"  ProximaDB insurance SIU copilot — LIVE against {BASE}")
    print("  one engine · timeseries + vector + graph + RAG")
    print("=" * 78)
    caps = api("GET", "/api/v2/_meta/capabilities")
    print(f"  engine features: {', '.join(caps.get('features', [])[-4:])}\n")

    print("── setup (real ProximaDB writes across all four modalities) ──")
    load_typologies(typologies)
    load_casenotes(cases)
    build_party_graph(parties)

    print("\n── copilot ──")
    say("siu", f"Claim from {ALERT_CLAIMANT} was flagged by the fraud model. Is it a staged-accident ring?")

    flagged = timeseries_scan(ts_claimants)
    say("copilot", f"[timeseries] scanned {len(ts_claimants)} claim streams via ProximaDB — "
                   f"{len(flagged)} with suspicious amount bursts: "
                   + ", ".join(f["claimant"] for f in flagged))

    target = next((f for f in flagged if f["claimant"] == ALERT_CLAIMANT), flagged[0] if flagged else None)
    for f in ([target] if target else []):
        clmt = f["claimant"]
        # VECTOR — match burst signature to a fraud typology
        res = api("POST", f"/api/v2/collections/{TYP_COLL}/search",
                  {"vector": f["features"], "top_k": 1}, label=f"match {clmt} to fraud typology")
        hits = res.get("results", [])
        typ = unwrap(hits[0]["props"].get("label", "?")) if hits else "?"
        say("copilot", f"[vector] {clmt} claim signature → **{typ}** (score {hits[0]['score']:.3f}, live ANN)")

        # GRAPH — trace the ring through the shared crooked provider (multi-hop Cypher)
        ring = trace_ring(clmt)
        say("copilot", f"[graph] fraud ring downstream of {clmt} → {' → '.join(ring) if ring else 'none'}")

        # RAG — retrieve the matching SIU case note + resolution
        res = api("POST", f"/api/v2/collections/{CASE_COLL}/search",
                  {"vector": embed(f"{typ} {typ} {typ}"), "top_k": 1}, label="retrieve SIU case note (RAG)")
        rhits = res.get("results", [])
        if rhits:
            p = rhits[0]["props"]
            say("copilot", f"[RAG] {rhits[0]['id']} → {unwrap(p.get('resolution', ''))}")

    print("\n" + "-" * 78)
    print("Every [API] line is a real ProximaDB call. In production this runs behind the")
    print("governed MCP control plane — metered/entitlement-checked/PII-redacted per tenant.")


if __name__ == "__main__":
    main()
