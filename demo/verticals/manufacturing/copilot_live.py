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
from collections import defaultdict, deque
from pathlib import Path

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
def scan_anomalies(series) -> list[dict]:
    """Timeseries anomaly scan over the mounted dataset (client-side z-score).
    Production: a ProximaDB timeseries aggregate query per sensor."""
    flagged = []
    for s in series:
        vals = [p["value"] for p in s["points"]]
        n = len(vals)
        base = vals[: n // 2]
        mean = sum(base) / len(base)
        sd = (sum((v - mean) ** 2 for v in base) / len(base)) ** 0.5 or 1e-6
        tail = vals[int(n * 0.6):]
        breaks = sum(1 for v in tail if abs(v - mean) / sd > 3.0)
        if breaks >= max(3, len(tail) * 0.03):
            slope = (sum(tail[-20:]) / 20 - mean) / (mean or 1e-6)
            flagged.append({
                "machine": s["machine"], "metric": s["metric"], "breakouts": breaks,
                "features": [round(sum(tail) / len(tail), 3), round(slope, 3),
                             round(max(vals) / (mean or 1e-6), 3), round(breaks / len(tail), 3)],
            })
    return flagged


def downstream_impact(machine, assets) -> list[str]:
    """Graph impact over the mounted topology (BFS on feeds_into).
    Production: the ProximaDB `graph_walk` / impact-analysis endpoint."""
    line_of = {n["id"]: n.get("line") for n in assets["nodes"] if n["kind"] == "machine"}
    flow, contains = defaultdict(list), defaultdict(list)
    for e in assets["edges"]:
        (flow if e["rel"] == "feeds_into" else contains if e["rel"] == "contains" else defaultdict(list))[e["from"]].append(e["to"])
    start = f"line-{line_of.get(machine)}"
    impacted, q, seen = [], deque([start]), {start}
    while q:
        for nxt in flow[q.popleft()]:
            if nxt not in seen:
                seen.add(nxt)
                machines = [m for m in contains[nxt] if not m.startswith("line-")]
                impacted.append(f"{nxt} ({len(machines)} machines)")
                q.append(nxt)
    return impacted


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

    print("── setup (real ProximaDB writes) ──")
    create_collection(SIG_COLL, 4)
    create_collection(LOG_COLL, EMB_DIM)
    load_signatures(signatures)
    load_logs(logs)

    print("\n── copilot ──")
    print("\n👷 operator: Throughput on the packaging line just dropped — what's going on?")

    anomalies = scan_anomalies(series)
    print(f"\n🤖 [timeseries] {len(anomalies)} sensor(s) off baseline (mounted data): "
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

        # GRAPH — impact over mounted topology
        impact = downstream_impact(a["machine"], assets)
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
    print("these run behind the AnvaiOps MCP copilot — metered/entitlement-checked/redacted.")


if __name__ == "__main__":
    main()
