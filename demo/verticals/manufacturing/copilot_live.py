#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Manufacturing factory copilot — **live against a running ProximaDB** (v2 REST API).

Unlike ``copilot_demo.py`` (which illustrates the flow with no server), this script
issues **real** ProximaDB operations and prints each call + its latency, so you can
*verify* the demo ran through the engine. The vector-similarity (fault match) and RAG
(maintenance-log retrieval) hops are real `POST …/records/batch` + `…/search` calls;
the timeseries anomaly scan and graph impact run over the mounted dataset (ProximaDB's
timeseries / graph engines wire the same way — see README.adoc).

Prereqs — a ProximaDB built from the SAME code (so the v2 API matches):

    cargo build --release -p proximadb-server
    docker build -f Dockerfile.prebuilt -t proximadb:develop .
    docker run -d --name proximadb-demo -p 5678:5678 \
        -v "$PWD/demo/verticals/manufacturing/data":/demo-data:ro proximadb:develop
    python generate_data.py && python copilot_live.py

Env: ``PROXIMADB_URL`` (default ``http://localhost:5678``), ``DATA`` (default ``./data``).
Pure stdlib (urllib) — no pip install.
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


def iso_ms(iso: str) -> int:
    """Parse a generate_data ISO timestamp to epoch milliseconds."""
    return int(datetime.fromisoformat(iso).timestamp() * 1000)

BASE = os.environ.get("PROXIMADB_URL", "http://localhost:5678").rstrip("/")
DATA = Path(os.environ.get("DATA", Path(__file__).parent / "data"))
EMB_DIM = 256
SIG_COLL = "mfg_fault_signatures"
LOG_COLL = "mfg_maintenance_logs"


# ── tiny REST client (stdlib) ───────────────────────────────────────────────────
def _req(method: str, path: str, body: dict | None = None) -> tuple[int, dict]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(f"{BASE}{path}", data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def api(method: str, path: str, body: dict | None = None, label: str = "") -> dict:
    t = time.time()
    status, resp = _req(method, path, body)
    ms = (time.time() - t) * 1000
    ok = 200 <= status < 300
    if label:
        mark = "✓" if ok else "✗"
        print(f"     [API {mark}] {method} {path}  ({status}, {ms:.0f} ms)")
    if not ok and status not in (404, 409):
        print(f"           ↳ {json.dumps(resp)[:200]}")
    return resp


def embed(text: str, dim: int = EMB_DIM) -> list[float]:
    """Deterministic bag-of-tokens hash embedding (demo only; production uses a real
    embedder). Stable across runs — unlike Python's salted hash()."""
    v = [0.0] * dim
    for tok in text.lower().replace(",", " ").replace(".", " ").split():
        h = int(hashlib.md5(tok.encode()).hexdigest(), 16)
        v[h % dim] += 1.0
    norm = math.sqrt(sum(x * x for x in v)) or 1.0
    return [round(x / norm, 6) for x in v]


def unwrap(v):
    """ProximaDB returns props as typed `{type, value}` wrappers — pull the value."""
    return v["value"] if isinstance(v, dict) and "value" in v else v


def _load(name: str):
    return json.loads((DATA / name).read_text())


# ── setup + load (REAL ProximaDB writes) ────────────────────────────────────────
def create_collection(name: str, dim: int) -> None:
    api("POST", "/api/v2/collections",
        {"name": name, "dimension": dim, "distance_metric": "cosine"},
        label=f"create {name} (dim {dim})")


def load_signatures(signatures) -> None:
    records = [{"id": s["id"], "vector": s["features"],
                "props": {"label": s["label"]}}
               for s in signatures if s["label"] != "nominal"]
    r = api("POST", f"/api/v2/collections/{SIG_COLL}/records/batch",
            {"records": records, "upsert": True}, label="insert fault signatures")
    print(f"           ↳ inserted {r.get('inserted_count', 0)} signatures")


def load_logs(logs) -> None:
    # Weight the fault label so the demo hash-embedding stays separable across faults.
    records = [{"id": log["id"],
                "vector": embed(f"{log['fault']} {log['fault']} {log['fault']} {log['symptom']}"),
                "props": {"machine": log["machine"], "fault": log["fault"],
                          "resolution": log["resolution"]}}
               for log in logs]
    r = api("POST", f"/api/v2/collections/{LOG_COLL}/records/batch",
            {"records": records, "upsert": True}, label="insert maintenance logs")
    print(f"           ↳ inserted {r.get('inserted_count', 0)} logs")


# ── analysis hops ───────────────────────────────────────────────────────────────
def timeseries_scan(series) -> list[dict]:
    """Ingest each sensor stream into ProximaDB time-series, then aggregate per 30-min
    bucket to flag anomalies — real `/api/v2/timeseries/*` calls (create + ingest +
    aggregate). Returns flagged sensors + a feature vector for the fault-signature match.
    (Bulk ingest calls are untraced to keep the transcript readable.)"""
    flagged = []
    print(f"     [timeseries] ingesting {len(series)} sensor streams + aggregating (live API)…")
    for s in series:
        coll = "ts_" + s["sensor_id"].replace(":", "_").replace("-", "_")
        metric = s["metric"]
        pts = s["points"][::6]  # ~6-min cadence keeps the demo snappy
        if len(pts) < 4:
            continue
        api("POST", "/api/v2/timeseries/collections", {"name": coll})
        points = [{"timestamp": iso_ms(p["ts"]), "values": {metric: p["value"]}} for p in pts]
        api("POST", f"/api/v2/timeseries/collections/{coll}/ingest", {"points": points})
        res = api("POST", f"/api/v2/timeseries/collections/{coll}/aggregate",
                  {"start_time": iso_ms(pts[0]["ts"]) - 1, "end_time": iso_ms(pts[-1]["ts"]) + 1,
                   "aggregation": "max", "bucket_ms": 1_800_000})
        peaks = [b["values"][metric] for b in res.get("buckets", []) if metric in b.get("values", {})]
        if len(peaks) < 4:
            continue
        # per-bucket MAX exposes the intermittent spike / drift that averaging smooths.
        # Flag when the late-window peak breaks out of the early-window baseline
        # distribution (z-score) — this adapts to each sensor's own noise, so a
        # spike (z≈30) and a slow drift (z≈9) both trip while noisy-but-normal
        # sensors (z≈2) do not, without a metric-specific threshold.
        half = len(peaks) // 2
        early = peaks[:half]
        base = sum(early) / len(early)
        std = max((sum((p - base) ** 2 for p in early) / len(early)) ** 0.5, base * 0.02)
        late_peak = max(peaks[half:])
        z = (late_peak - base) / std
        if z > 4.5:
            ratio = late_peak / base if base else 1.0
            hot = sum(1 for a in peaks[half:] if (a - base) / std > 4.5) / max(1, len(peaks[half:]))
            # feature[0] = baseline level so the metric magnitude picks the right fault
            # signature (vibration≈3 → bearing, temperature≈68 → cooling).
            flagged.append({
                "machine": s["machine"], "metric": metric,
                "features": [round(base, 3), round(ratio - 1, 3), round(ratio, 3), round(hot, 3)],
            })
    return flagged


def build_asset_graph(assets) -> None:
    """Create the asset topology as a ProximaDB graph — line/machine nodes + feeds_into
    / contains edges — via the real graph API (untraced setup calls)."""
    api("POST", "/api/v2/graphs", {"graph_id": "assets"})
    keep = set()
    for node in assets["nodes"]:
        if node["kind"] in ("line", "machine"):
            keep.add(node["id"])
            api("POST", "/api/v2/graphs/assets/nodes",
                {"node": {"id": node["id"], "labels": [node["kind"].capitalize()],
                          "properties": {"name": node.get("name", node["id"])}}})
    n_edges = 0
    for e in assets["edges"]:
        if e["rel"] in ("feeds_into", "contains") and e["from"] in keep and e["to"] in keep:
            api("POST", "/api/v2/graphs/assets/edges",
                {"edge": {"id": f"{e['from']}->{e['to']}", "from_node_id": e["from"],
                          "to_node_id": e["to"], "edge_type": e["rel"], "weight": 1.0}})
            n_edges += 1
    print(f"     [graph] built asset topology: {len(keep)} nodes, {n_edges} edges (live)")


def graph_impact(machine, assets) -> list[str]:
    """Downstream impact via a live Cypher query over the asset graph — from the faulty
    machine's line, follow `feeds_into` to the downstream lines."""
    line_of = {n["id"]: n.get("line") for n in assets["nodes"] if n["kind"] == "machine"}
    line = f"line-{line_of.get(machine)}"
    query = f"MATCH (l:Line {{id:'{line}'}})-[:feeds_into*1..5]->(dl:Line) RETURN dl"
    res = api("POST", "/api/v2/graphs/assets/query", {"query": query, "language": "cypher"},
              label=f"Cypher downstream-impact from {line}")
    rows = res.get("data", {}).get("rows", [])
    out = []
    for r in rows:
        node_id = r.get("node_id") or r.get("id")
        props = r.get("properties", {})
        out.append(props.get("name", node_id) if isinstance(props, dict) else node_id)
    return out


def main() -> None:
    if not DATA.exists():
        raise SystemExit("dataset missing — run `python generate_data.py` first")
    assets, series = _load("assets.json"), _load("timeseries.json")
    signatures, logs = _load("fault_signatures.json"), _load("maintenance_logs.json")

    print("=" * 76)
    print(f"  ProximaDB factory copilot — LIVE against {BASE}")
    print("=" * 76)
    caps = api("GET", "/api/v2/_meta/capabilities")
    print(f"  engine: v{caps.get('api_version','?')}  features: {', '.join(caps.get('features',[])[:6])}…\n")

    print("── setup (real ProximaDB writes across all four modalities) ──")
    create_collection(SIG_COLL, 4)
    create_collection(LOG_COLL, EMB_DIM)
    load_signatures(signatures)
    load_logs(logs)
    build_asset_graph(assets)

    print("\n── copilot ──")
    print("\n👷 operator: Throughput on the packaging line just dropped — what's going on?")

    anomalies = timeseries_scan(series)
    print(f"\n🤖 [timeseries] scanned {len(series)} sensor streams via ProximaDB — "
          f"{len(anomalies)} anomalous: "
          + ", ".join(f"{a['machine']}/{a['metric']}" for a in anomalies))

    for a in anomalies:
        # VECTOR — real ProximaDB ANN search
        res = api("POST", f"/api/v2/collections/{SIG_COLL}/search",
                  {"vector": a["features"], "top_k": 1, "include_vector": False},
                  label=f"search fault_signatures for {a['machine']}")
        hits = res.get("results", [])
        if not hits:
            continue
        fault = unwrap(hits[0]["props"].get("label", hits[0]["id"]))
        print(f"🤖 [vector] {a['machine']}/{a['metric']} → **{fault}** (score {hits[0]['score']:.3f}, live ANN)")

        # GRAPH — downstream impact via a live Cypher query
        impact = graph_impact(a["machine"], assets)
        print(f"🤖 [graph] downstream impact → {', '.join(impact) if impact else 'none (contained)'}")

        # RAG — real ProximaDB vector search over the log corpus
        res = api("POST", f"/api/v2/collections/{LOG_COLL}/search",
                  {"vector": embed(f"{fault} {fault} {fault} {a['metric']}"), "top_k": 1},
                  label="search maintenance_logs (RAG)")
        rhits = res.get("results", [])
        if rhits:
            print(f"🤖 [RAG] {rhits[0]['id']} (score {rhits[0]['score']:.3f}) → "
                  f"{unwrap(rhits[0]['props'].get('resolution', ''))}")

    print("\n" + "-" * 76)
    print("Every [API] line above is a real call to the running ProximaDB. In production")
    print("these run behind the governed MCP control plane — metered/entitlement-checked/redacted.")


if __name__ == "__main__":
    main()
