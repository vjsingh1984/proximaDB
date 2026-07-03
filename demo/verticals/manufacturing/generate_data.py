#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Synthetic factory dataset for the ProximaDB *manufacturing / industrial* vertical demo.

Generates a small, self-contained, deterministic dataset that exercises all four
modalities the demo showcases — with NO external data or network:

  * **graph**       asset topology: plant -> lines -> machines -> sensors (+ line flow)
  * **timeseries**  per-sensor readings (temperature / vibration / pressure) with
                    injected anomalies on a couple of machines
  * **vector**      a small library of labelled fault "signatures" for similarity
  * **text/RAG**    maintenance logs (symptom + resolution) as a retrieval corpus

Everything is seeded, so re-running produces the same dataset. Outputs JSON under
``--out`` (default ``./data``). This file is pure stdlib and is meant to be run and
verified on its own; ``run_demo.py`` loads the output into ProximaDB.
"""

from __future__ import annotations

import argparse
import json
import math
import random
from datetime import datetime, timedelta, timezone
from pathlib import Path

# ── shape of the plant ──────────────────────────────────────────────────────────
LINES = ["assembly", "paint", "packaging"]
MACHINES_PER_LINE = 4
SENSORS = [
    ("temperature", 65.0, 4.0, "degC"),   # (metric, baseline, noise_sd, unit)
    ("vibration", 2.5, 0.4, "mm_s"),
    ("pressure", 6.0, 0.5, "bar"),
]
POINTS_PER_SENSOR = 480          # ~8h at 1-minute cadence
CADENCE = timedelta(minutes=1)

# Machines that develop a fault partway through the window (id -> (metric, kind)).
FAULTS = {
    "assembly-2": ("vibration", "spike"),   # bearing wear -> vibration spikes
    "paint-3": ("temperature", "drift"),     # cooling loss -> temperature drift
}

# Labelled fault signatures (the vector-similarity library). Each is a short
# feature vector [mean, slope, peak_ratio, spikiness] the demo can match against.
FAULT_SIGNATURES = [
    {"id": "sig-bearing-wear", "label": "bearing wear", "metric": "vibration",
     "features": [3.1, 0.02, 2.4, 0.9], "remedy_ref": "log-bearing-01"},
    {"id": "sig-cooling-loss", "label": "cooling loss", "metric": "temperature",
     "features": [78.0, 0.15, 1.2, 0.1], "remedy_ref": "log-cooling-01"},
    {"id": "sig-seal-leak", "label": "seal leak", "metric": "pressure",
     "features": [4.8, -0.08, 1.1, 0.2], "remedy_ref": "log-seal-01"},
    {"id": "sig-nominal", "label": "nominal", "metric": "any",
     "features": [1.0, 0.0, 1.05, 0.05], "remedy_ref": None},
]

MAINTENANCE_LOGS = [
    {"id": "log-bearing-01", "machine": "assembly-2", "fault": "bearing wear",
     "symptom": "Vibration on the main spindle rising above 3 mm/s with periodic spikes; audible whine under load.",
     "resolution": "Replaced worn spindle bearing, re-greased housing, re-balanced. Vibration returned to ~2.5 mm/s baseline."},
    {"id": "log-cooling-01", "machine": "paint-3", "fault": "cooling loss",
     "symptom": "Enclosure temperature drifting upward past 75 degC over a shift; coolant flow reads low.",
     "resolution": "Cleared blocked coolant filter and topped up glycol; verified pump pressure. Temperature stabilised at 65 degC."},
    {"id": "log-seal-01", "machine": "packaging-1", "fault": "seal leak",
     "symptom": "Line pressure slowly falling below 5 bar; faint hiss near the pneumatic manifold.",
     "resolution": "Replaced perished O-ring on the manifold seal and re-torqued fittings. Pressure held at 6 bar."},
    {"id": "log-general-01", "machine": "assembly-1", "fault": "routine",
     "symptom": "Scheduled preventive maintenance; no abnormal readings.",
     "resolution": "Lubrication and inspection completed; all sensors nominal."},
]


def _machine_id(line: str, idx: int) -> str:
    return f"{line}-{idx}"


def build_assets() -> dict:
    """Asset topology as nodes + edges (the graph modality)."""
    nodes = [{"id": "plant-1", "kind": "plant", "name": "Riverside Plant"}]
    edges = []
    for li, line in enumerate(LINES):
        line_id = f"line-{line}"
        nodes.append({"id": line_id, "kind": "line", "name": line})
        edges.append({"from": "plant-1", "to": line_id, "rel": "contains"})
        # Line flow: assembly -> paint -> packaging (downstream impact traversal).
        if li > 0:
            edges.append({"from": f"line-{LINES[li - 1]}", "to": line_id, "rel": "feeds_into"})
        for m in range(1, MACHINES_PER_LINE + 1):
            mid = _machine_id(line, m)
            nodes.append({"id": mid, "kind": "machine", "name": mid, "line": line})
            edges.append({"from": line_id, "to": mid, "rel": "contains"})
            for metric, *_ in SENSORS:
                sid = f"{mid}:{metric}"
                nodes.append({"id": sid, "kind": "sensor", "metric": metric, "machine": mid})
                edges.append({"from": mid, "to": sid, "rel": "monitors"})
    return {"nodes": nodes, "edges": edges}


def build_timeseries(rng: random.Random, start: datetime) -> list[dict]:
    """Per-sensor readings with injected anomalies (the timeseries modality)."""
    series = []
    for line in LINES:
        for m in range(1, MACHINES_PER_LINE + 1):
            mid = _machine_id(line, m)
            fault_metric, fault_kind = FAULTS.get(mid, (None, None))
            for metric, baseline, noise_sd, unit in SENSORS:
                points = []
                for i in range(POINTS_PER_SENSOR):
                    ts = start + i * CADENCE
                    value = baseline + rng.gauss(0, noise_sd)
                    if metric == fault_metric and i > POINTS_PER_SENSOR * 0.6:
                        prog = (i - POINTS_PER_SENSOR * 0.6) / (POINTS_PER_SENSOR * 0.4)
                        if fault_kind == "spike" and rng.random() < 0.25:
                            value += (2.5 + prog * 3.0) * noise_sd  # intermittent spikes
                        elif fault_kind == "drift":
                            value += prog * baseline * 0.25          # slow upward drift
                    points.append({"ts": ts.isoformat(), "value": round(value, 3)})
                series.append({
                    "sensor_id": f"{mid}:{metric}", "machine": mid, "metric": metric,
                    "unit": unit, "points": points,
                    "anomalous": metric == fault_metric,
                })
    return series


def main() -> None:
    ap = argparse.ArgumentParser(description="Generate the manufacturing demo dataset.")
    ap.add_argument("--out", default=str(Path(__file__).parent / "data"))
    ap.add_argument("--seed", type=int, default=42)
    args = ap.parse_args()

    rng = random.Random(args.seed)
    start = datetime(2026, 1, 15, 6, 0, tzinfo=timezone.utc)

    assets = build_assets()
    series = build_timeseries(rng, start)

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "assets.json").write_text(json.dumps(assets, indent=2))
    (out / "timeseries.json").write_text(json.dumps(series, indent=2))
    (out / "fault_signatures.json").write_text(json.dumps(FAULT_SIGNATURES, indent=2))
    (out / "maintenance_logs.json").write_text(json.dumps(MAINTENANCE_LOGS, indent=2))

    n_points = sum(len(s["points"]) for s in series)
    n_anom = sum(1 for s in series if s["anomalous"])
    print(f"✅ manufacturing dataset written to {out}")
    print(f"   assets:        {len(assets['nodes'])} nodes / {len(assets['edges'])} edges")
    print(f"   timeseries:    {len(series)} sensors / {n_points} points ({n_anom} anomalous)")
    print(f"   signatures:    {len(FAULT_SIGNATURES)}   maintenance logs: {len(MAINTENANCE_LOGS)}")
    print(f"   faults seeded: {', '.join(f'{k} ({v[0]} {v[1]})' for k, v in FAULTS.items())}")


if __name__ == "__main__":
    main()
