#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Manufacturing / industrial *factory copilot* — the flagship ProximaDB vertical demo.

An operator asks one question — "why did line throughput drop?" — and the copilot
answers by chaining **four modalities**, exactly the co-design pitch (one engine, no
glue):

  1. **timeseries** — scan sensor streams, flag the anomalous machine
  2. **vector**     — match the anomaly's signature to a known fault library
  3. **graph**      — traverse the asset topology for downstream impact
  4. **RAG**        — retrieve the maintenance log with the proven fix

This script is **self-contained and runnable with no server** — it runs the analysis
locally over the generated dataset so the copilot transcript always works, and prints,
for each step, the equivalent ProximaDB operation (the same call an AnvaiOps-governed
MCP copilot issues in production). Run ``generate_data.py`` first.

    python generate_data.py && python run_demo.py

See ``README.adoc`` for the production wiring (ProximaDB SDK + the MCP tools).
"""

from __future__ import annotations

import json
import math
from collections import defaultdict, deque
from pathlib import Path

DATA = Path(__file__).parent / "data"


def _load(name: str):
    return json.loads((DATA / name).read_text())


def cosine(a, b) -> float:
    dot = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a)) or 1.0
    nb = math.sqrt(sum(y * y for y in b)) or 1.0
    return dot / (na * nb)


# ── 1. timeseries: anomaly scan ─────────────────────────────────────────────────
def scan_anomalies(series) -> list[dict]:
    """Rolling z-score over each sensor stream; flag streams that break out.

    ProximaDB equivalent: a timeseries downsample/aggregate query per sensor with an
    anomaly predicate — `SELECT sensor_id FROM sensors WHERE zscore(value) > 3`.
    """
    flagged = []
    for s in series:
        values = [p["value"] for p in s["points"]]
        n = len(values)
        base = values[: int(n * 0.5)]
        mean = sum(base) / len(base)
        sd = (sum((v - mean) ** 2 for v in base) / len(base)) ** 0.5 or 1e-6
        tail = values[int(n * 0.6):]
        breakouts = sum(1 for v in tail if abs(v - mean) / sd > 3.0)
        peak_ratio = (max(values) / (mean or 1e-6))
        if breakouts >= max(3, len(tail) * 0.03):
            slope = (sum(tail[-20:]) / 20 - mean) / (mean or 1e-6)
            flagged.append({
                "sensor_id": s["sensor_id"], "machine": s["machine"], "metric": s["metric"],
                "breakouts": breakouts,
                # feature vector aligned with the fault-signature library
                "features": [round(sum(tail) / len(tail), 3), round(slope, 3),
                             round(peak_ratio, 3), round(breakouts / len(tail), 3)],
            })
    return flagged


# ── 2. vector: fault-signature similarity ───────────────────────────────────────
def match_signature(features, signatures) -> dict:
    """Cosine-match the anomaly's feature vector to the labelled fault library.

    ProximaDB equivalent: `client.search("fault_signatures", features, top_k=1)`.
    """
    ranked = sorted(
        ({**sig, "score": round(cosine(features, sig["features"]), 3)}
         for sig in signatures if sig["label"] != "nominal"),
        key=lambda r: r["score"], reverse=True,
    )
    return ranked[0]


# ── 3. graph: downstream impact traversal ───────────────────────────────────────
def downstream_impact(machine, assets) -> list[str]:
    """BFS the asset topology from the faulty machine along `feeds_into` line flow.

    ProximaDB equivalent: the `graph_walk` MCP tool / native traversal from the
    machine's line following `feeds_into` edges.
    """
    line_of = {n["id"]: n.get("line") for n in assets["nodes"] if n["kind"] == "machine"}
    my_line = f"line-{line_of.get(machine)}"
    flow = defaultdict(list)
    contains = defaultdict(list)
    for e in assets["edges"]:
        if e["rel"] == "feeds_into":
            flow[e["from"]].append(e["to"])
        if e["rel"] == "contains":
            contains[e["from"]].append(e["to"])
    impacted, q = [], deque([my_line])
    seen = {my_line}
    while q:
        line = q.popleft()
        for nxt in flow[line]:
            if nxt not in seen:
                seen.add(nxt)
                machines = [m for m in contains[nxt] if not m.startswith("line-")]
                impacted.append(f"{nxt} ({len(machines)} machines)")
                q.append(nxt)
    return impacted


# ── 4. RAG: maintenance-log retrieval ───────────────────────────────────────────
def retrieve_fix(fault_label, logs) -> dict | None:
    """Retrieve the maintenance log whose fault matches — the proven resolution.

    ProximaDB equivalent: hybrid search (`vector + BM25`) over the maintenance-log
    collection, or the MCP `search` tool scoped to that collection.
    """
    for log in logs:
        if log["fault"] == fault_label:
            return log
    return None


def say(role, text):
    tag = {"operator": "👷 operator", "copilot": "🤖 copilot"}[role]
    print(f"\n{tag}: {text}")


def main() -> None:
    if not DATA.exists():
        raise SystemExit("dataset missing — run `python generate_data.py` first")
    assets = _load("assets.json")
    series = _load("timeseries.json")
    signatures = _load("fault_signatures.json")
    logs = _load("maintenance_logs.json")

    print("=" * 74)
    print("  ProximaDB — Manufacturing / Industrial factory copilot")
    print("  one engine · timeseries + vector + graph + RAG · via the MCP surface")
    print("=" * 74)

    say("operator", "Throughput on the packaging line just dropped — what's going on?")

    # 1. timeseries
    anomalies = scan_anomalies(series)
    say("copilot", f"[timeseries] Scanned {len(series)} sensor streams — "
                   f"{len(anomalies)} breaking out of their baseline:")
    for a in anomalies:
        print(f"     • {a['machine']}  {a['metric']}  ({a['breakouts']} breakout points)")

    for a in anomalies:
        # 2. vector
        sig = match_signature(a["features"], signatures)
        say("copilot", f"[vector] {a['machine']}'s {a['metric']} signature matches "
                       f"**{sig['label']}** (cosine {sig['score']}).")

        # 3. graph
        impact = downstream_impact(a["machine"], assets)
        if impact:
            say("copilot", f"[graph] {a['machine']} feeds downstream → impacted: "
                           f"{', '.join(impact)}.")
        else:
            say("copilot", f"[graph] {a['machine']} has no downstream lines — contained.")

        # 4. RAG
        fix = retrieve_fix(sig["label"], logs)
        if fix:
            say("copilot", f"[RAG] Closest maintenance record → {fix['id']} on {fix['machine']}:\n"
                           f"       symptom:    {fix['symptom']}\n"
                           f"       resolution: {fix['resolution']}")

    say("copilot", "Recommendation: prioritise the bearing replacement on assembly-2 "
                   "(upstream, highest downstream impact), then the paint-3 cooling loop.")
    print("\n" + "-" * 74)
    print("In production every bracketed step is a governed ProximaDB operation issued")
    print("through the AnvaiOps MCP copilot — metered per tenant (KRU/KEU), entitlement-")
    print("checked, PII-redacted. Same engine, one query plane. See README.adoc.")


if __name__ == "__main__":
    main()
