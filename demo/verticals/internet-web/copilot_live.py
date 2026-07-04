#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Internet / web *incident-response copilot* — a ProximaDB internet-vertical demo.

An SRE asks one question — "traffic to /pricing fell off a cliff, what happened?" — and
the copilot answers by chaining **four modalities in one engine** (no ETL between a
metrics store, a vector DB, a graph DB, and a search index):

  1. **timeseries** — per-entity 5xx-error streams; flag the error bursts
  2. **vector**     — match the alerting page's error signature to a known incident *typology*
  3. **graph**      — trace the service dependency chain to the root cause (Cypher multi-hop)
  4. **RAG**        — retrieve the postmortem / runbook with the fix

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
TYP_COLL = "web_incident_typologies"
PM_COLL = "web_postmortems"
BURST_THRESHOLD = 200.0  # a single-bucket 5xx volume this large is an incident, not noise
ALERT_PAGE = "/pricing"  # the page the SRE was paged about


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
    for tok in text.lower().replace("/", " ").replace(",", " ").replace(".", " ").split():
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
            label="load web-incident typology signatures")
    print(f"           ↳ {r.get('inserted_count', 0)} typologies")


def load_postmortems(postmortems) -> None:
    recs = [{"id": p["id"], "vector": embed(f"{p['typology']} {p['typology']} {p['text']}"),
             "props": {"typology": p["typology"], "resolution": p["resolution"]}}
            for p in postmortems]
    api("POST", "/api/v2/collections", {"name": PM_COLL, "dimension": EMB_DIM, "distance_metric": "cosine"})
    r = api("POST", f"/api/v2/collections/{PM_COLL}/records/batch", {"records": recs, "upsert": True},
            label="load postmortems + runbooks")
    print(f"           ↳ {r.get('inserted_count', 0)} postmortems")


def build_topology_graph(topology) -> None:
    """Pages + services + depends_on edges as a ProximaDB graph (untraced setup calls)."""
    api("POST", "/api/v2/graphs", {"graph_id": "web"})
    for n in topology["nodes"]:
        api("POST", "/api/v2/graphs/web/nodes",
            {"node": {"id": n["id"], "labels": ["Entity"],
                      "properties": {"kind": n["kind"], "tier": n["tier"]}}})
    for e in topology["edges"]:
        api("POST", "/api/v2/graphs/web/edges",
            {"edge": {"id": f"{e['from']}->{e['to']}", "from_node_id": e["from"],
                      "to_node_id": e["to"], "edge_type": "depends_on", "weight": 1.0}})
    print(f"     [graph] built dependency graph: {len(topology['nodes'])} entities (live)")


# ── analysis hops (all live ProximaDB calls) ────────────────────────────────────
def timeseries_scan(ts_entities) -> list[dict]:
    """Ingest each entity's 5xx-error stream into ProximaDB time-series, then aggregate SUM
    per 30-min bucket to flag error bursts. Real /api/v2/timeseries/*."""
    print(f"     [timeseries] ingesting {len(ts_entities)} error streams + aggregating (live API)…")
    flagged = []
    for ent in ts_entities:
        pts = ent["points"]
        if len(pts) < 3:
            continue
        coll = "ts_" + ent["entity"].strip("/").replace("/", "_").replace("-", "_") or "ts_root"
        api("POST", "/api/v2/timeseries/collections", {"name": coll})
        api("POST", f"/api/v2/timeseries/collections/{coll}/ingest",
            {"points": [{"timestamp": iso_ms(p["ts"]), "values": {"errors": p["errors"]}} for p in pts]})
        res = api("POST", f"/api/v2/timeseries/collections/{coll}/aggregate",
                  {"start_time": iso_ms(pts[0]["ts"]) - 1, "end_time": iso_ms(pts[-1]["ts"]) + 1,
                   "aggregation": "sum", "bucket_ms": 1_800_000})
        sums = [b["values"]["errors"] for b in res.get("buckets", []) if "errors" in b.get("values", {})]
        if not sums:
            continue
        peak = max(sums)
        if peak > BURST_THRESHOLD:
            flagged.append({"entity": ent["entity"], "peak": round(peak, 1),
                            "features": feature_vector(sums)})
    return sorted(flagged, key=lambda f: f["peak"], reverse=True)


def feature_vector(sums: list[float]) -> list[float]:
    """Scale-normalised shape of an error burst — must match the typology vectors in
    generate_data.py. Scaling each dimension to O(1..10) lets cosine discriminate on the
    burst *shape* (spiky vs spread, concentrated vs diffuse) rather than raw magnitude."""
    peak = max(sums)
    total = sum(sums) or 1e-6
    mean = total / len(sums)
    spread = sum(1 for s in sums if s > BURST_THRESHOLD) / len(sums)
    return [round(peak / 100.0, 3), round(peak / (mean or 1e-6), 3),
            round(spread * 10.0, 3), round(peak / total * 10.0, 3)]


def trace_dependency_chain(entity) -> list[str]:
    """Trace the dependency chain downstream of an entity via a live Cypher multi-hop query."""
    q = f"MATCH (a:Entity {{id:'{entity}'}})-[:depends_on*1..4]->(x:Entity) RETURN x"
    res = api("POST", "/api/v2/graphs/web/query", {"query": q, "language": "cypher"},
              label=f"Cypher trace dependency chain from {entity}")
    rows = res.get("data", {}).get("rows", [])
    return [r.get("node_id") or r.get("id") for r in rows]


def say(role, text):
    print(f"\n{'🧑‍💻 SRE' if role == 'sre' else '🛰️  copilot'}: {text}")


def main() -> None:
    if not DATA.exists():
        raise SystemExit("dataset missing — run `python generate_data.py` first")
    topology = _load("topology.json")
    ts_entities = _load("errorstreams.json")
    typologies = _load("typologies.json")
    postmortems = _load("postmortems.json")

    print("=" * 78)
    print(f"  ProximaDB web incident copilot — LIVE against {BASE}")
    print("  one engine · timeseries + vector + graph + RAG")
    print("=" * 78)
    caps = api("GET", "/api/v2/_meta/capabilities")
    print(f"  engine features: {', '.join(caps.get('features', [])[-4:])}\n")

    print("── setup (real ProximaDB writes across all four modalities) ──")
    load_typologies(typologies)
    load_postmortems(postmortems)
    build_topology_graph(topology)

    print("\n── copilot ──")
    say("sre", f"Traffic to {ALERT_PAGE} fell off a cliff and 5xx is spiking. What happened?")

    flagged = timeseries_scan(ts_entities)
    say("copilot", f"[timeseries] scanned {len(ts_entities)} error streams via ProximaDB — "
                   f"{len(flagged)} with error bursts: " + ", ".join(f["entity"] for f in flagged))

    target = next((f for f in flagged if f["entity"] == ALERT_PAGE), flagged[0] if flagged else None)
    for f in ([target] if target else []):
        ent = f["entity"]
        # VECTOR — match error signature to an incident typology
        res = api("POST", f"/api/v2/collections/{TYP_COLL}/search",
                  {"vector": f["features"], "top_k": 1}, label=f"match {ent} to incident typology")
        hits = res.get("results", [])
        typ = unwrap(hits[0]["props"].get("label", "?")) if hits else "?"
        say("copilot", f"[vector] {ent} error signature → **{typ}** (score {hits[0]['score']:.3f}, live ANN)")

        # GRAPH — trace the dependency chain (multi-hop Cypher), then JOIN with the failing
        # set from timeseries: the root cause is the deepest failing dependency — a sink in
        # the failing sub-graph (a failing entity that depends on no other failing entity).
        chain = trace_dependency_chain(ent)
        flagged_ids = {f["entity"] for f in flagged}
        dep_edges = {(e["from"], e["to"]) for e in topology["edges"]}
        failing = [n for n in chain if n in flagged_ids]  # reachable ∩ failing, in depth order
        root = next((n for n in failing
                     if not any((n, m) in dep_edges for m in flagged_ids if m != n)),
                    failing[-1] if failing else "?")
        say("copilot",
            f"[graph] {ent} reaches {len(chain)} deps; failing chain "
            f"{' → '.join([ent] + failing) if failing else 'none'} → root cause **{root}**")

        # RAG — retrieve the matching postmortem + resolution
        res = api("POST", f"/api/v2/collections/{PM_COLL}/search",
                  {"vector": embed(f"{typ} {typ} {typ}"), "top_k": 1}, label="retrieve postmortem (RAG)")
        rhits = res.get("results", [])
        if rhits:
            p = rhits[0]["props"]
            say("copilot", f"[RAG] {rhits[0]['id']} → {unwrap(p.get('resolution', ''))}")

    print("\n" + "-" * 78)
    print("Every [API] line is a real ProximaDB call. In production this runs behind the")
    print("AnvaiOps MCP copilot — metered/entitlement-checked/PII-redacted per tenant.")


if __name__ == "__main__":
    main()
