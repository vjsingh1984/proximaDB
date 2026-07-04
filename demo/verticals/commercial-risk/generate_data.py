#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Synthetic commercial-risk dataset for the ProximaDB *D&B / commercial credit & KYB* vertical demo.

Deterministic, stdlib-only, no external data — a small business-population world with one
embedded *related-party (common-control) cluster*: a flagged supplier is quietly controlled,
through a holding shell, alongside a set of related firms up to one ultimate beneficial owner —
so distress in one is concentration risk across all. Over a sea of independent firms. Exercises
all four modalities:

  * **graph**       firms + holding/UBO entities (nodes) + `linked` (common-control) edges — the cluster is a path
  * **timeseries**  per-firm days-beyond-terms (payment-delinquency) series; the cluster firms burst
  * **vector**      labelled risk *typology* signatures for behaviour matching
  * **text/RAG**    credit-policy / risk guidance as a retrieval corpus

Outputs JSON under ``--out`` (default ``./data``). Run this before ``copilot_live.py``.
"""

from __future__ import annotations

import argparse
import json
import random
from datetime import datetime, timedelta, timezone
from pathlib import Path

N_FIRMS = 30
WINDOW_START = datetime(2026, 1, 5, 0, 0, tzinfo=timezone.utc)

# The related-party cluster: a flagged firm is controlled (via a holding shell) alongside
# related firms, up to one ultimate beneficial owner. This is the path the graph traces —
# the exposure to the flagged firm is really exposure to the whole cluster.
CLUSTER = {
    "flagged": "firm-007",
    "related": ["firm-011", "firm-012", "firm-013"],
    "holding": "hold-atlas",   # the common-control shell
    "ubo": "ubo-nominee",
}

# Labelled risk-typology signatures (the vector library). Features are the SCALE-NORMALISED
# shape of the delinquency burst — [severity, spike_ratio, spread, concentration] — the same
# vector copilot_live.py derives from the ProximaDB timeseries aggregate (see feature_vector).
# Scaling keeps every dimension O(1..10) so cosine discriminates on *shape*, not raw day counts.
TYPOLOGIES = [
    {"id": "typ-distress", "label": "financial distress", "case_ref": "cr-distress-01",
     "features": [6.0, 5.0, 2.5, 6.5]},     # sharp deterioration concentrated in a short window
    {"id": "typ-chronic", "label": "chronic late-payer", "case_ref": "cr-chronic-01",
     "features": [3.0, 2.2, 6.0, 2.2]},     # persistently late, sustained across the year
    {"id": "typ-overleverage", "label": "over-leverage", "case_ref": "cr-leverage-01",
     "features": [3.5, 4.0, 6.0, 2.6]},     # recurring stress spikes (debt-service pressure)
    {"id": "typ-nominal", "label": "nominal", "case_ref": None,
     "features": [0.6, 1.4, 0.5, 3.0]},
]

CASE_NOTES = [
    {"id": "cr-distress-01", "typology": "financial distress",
     "text": "A sharp, concentrated deterioration in payment behaviour (days-beyond-terms spiking) "
             "indicates acute financial distress. When the firm sits in a common-control cluster, the "
             "distress is correlated across related parties — the true exposure is the whole group, not one firm.",
     "resolution": "Reduced the credit limit; required a parent guarantee; set a group-level exposure cap across the UBO cluster."},
    {"id": "cr-chronic-01", "typology": "chronic late-payer",
     "text": "Persistently elevated days-beyond-terms without a sharp spike — a chronically slow payer "
             "rather than an acute event. Sustained, low-amplitude delinquency.",
     "resolution": "Tightened terms (shorter net days); added a late-payment surcharge; kept the limit under review."},
    {"id": "cr-leverage-01", "typology": "over-leverage",
     "text": "Recurring delinquency spikes aligned with debt-service dates suggest over-leverage and thin "
             "liquidity headroom. Repeated stress rather than a single event.",
     "resolution": "Requested management accounts; covenant-tested; reduced unsecured exposure pending refinancing."},
    {"id": "cr-indicators-01", "typology": "indicators",
     "text": "KYB / credit red-flag guidance: correlated delinquency across firms sharing a beneficial owner, "
             "a holding shell tying suppliers together, and concentration of exposure behind one UBO are "
             "classic related-party concentration-risk indicators — trace common control before sizing the limit.",
     "resolution": "Trace the ownership/common-control graph; aggregate exposure to the UBO; cap at the group level."},
]


def build_firms(rng: random.Random) -> list[dict]:
    cluster_ids = {CLUSTER["flagged"], CLUSTER["ubo"], CLUSTER["holding"], *CLUSTER["related"]}
    firms = []
    for i in range(1, N_FIRMS + 1):
        fid = f"firm-{i:03d}"
        firms.append({"id": fid, "kind": "firm", "in_cluster": fid in cluster_ids})
    for name in ["hold-atlas", "hold-orion", "hold-vega", "ubo-nominee"]:
        firms.append({"id": name, "kind": "entity",
                      "in_cluster": name in (CLUSTER["holding"], CLUSTER["ubo"])})
    return firms


def build_linked_edges(rng: random.Random) -> list[dict]:
    """`linked` (common-control) edges. Background firm-holding ties + the cluster path."""
    edges = []
    firms = [f"firm-{i:03d}" for i in range(1, N_FIRMS + 1)]
    holdings = ["hold-orion", "hold-vega"]
    cluster_ids = {CLUSTER["flagged"], CLUSTER["ubo"], CLUSTER["holding"], *CLUSTER["related"]}

    # ── background: each non-cluster firm tied to an unrelated holding ──
    for f in firms:
        if f in cluster_ids:
            continue
        edges.append({"from": f, "to": rng.choice(holdings)})

    # ── the cluster: flagged -> related firms -> holding shell -> UBO ──
    flagged, related, holding, ubo = (CLUSTER["flagged"], CLUSTER["related"],
                                      CLUSTER["holding"], CLUSTER["ubo"])
    for f in related:
        edges.append({"from": flagged, "to": f})    # flagged firm tied to each related party
        edges.append({"from": f, "to": holding})    # related parties under the common holding
    edges.append({"from": holding, "to": ubo})      # holding controlled by the UBO
    return edges


def build_dbt_series(rng: random.Random) -> dict[str, list[dict]]:
    """Per-firm days-beyond-terms points. Healthy low DBT + the cluster distress burst."""
    firms = [f"firm-{i:03d}" for i in range(1, N_FIRMS + 1)]
    series: dict[str, list[dict]] = {f: [] for f in firms}

    # ── background: low, occasional days-beyond-terms readings across the year ──
    for f in firms:
        for _ in range(rng.randint(4, 7)):
            ts = WINDOW_START + timedelta(days=rng.randint(0, 300))
            series[f].append({"ts": ts.isoformat(), "dbt": round(rng.uniform(1, 12), 1)})

    # ── the cluster: a sharp, correlated distress burst over a ~10-day window ──
    burst = WINDOW_START + timedelta(days=200)
    for firm in [CLUSTER["flagged"], CLUSTER["ubo"], *CLUSTER["related"]]:
        if firm.startswith("ubo") or firm.startswith("hold"):
            continue  # entities carry no payment series
        for _ in range(rng.randint(5, 7)):
            ts = burst + timedelta(days=rng.randint(0, 10))
            series[firm].append({"ts": ts.isoformat(), "dbt": round(rng.uniform(75, 120), 1)})
    return series


def main() -> None:
    ap = argparse.ArgumentParser(description="Generate the D&B / commercial-risk demo dataset.")
    ap.add_argument("--out", default=str(Path(__file__).parent / "data"))
    ap.add_argument("--seed", type=int, default=42)
    args = ap.parse_args()
    rng = random.Random(args.seed)

    firms = build_firms(rng)
    edges = [{"from": e["from"], "to": e["to"], "type": "linked"}
             for e in build_linked_edges(rng)]
    series = build_dbt_series(rng)
    ts_out = [{"firm": f, "points": sorted(pts, key=lambda p: p["ts"])}
              for f, pts in series.items() if pts]

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "firms.json").write_text(json.dumps({"nodes": firms, "edges": edges}, indent=2))
    (out / "dbt.json").write_text(json.dumps(ts_out, indent=2))
    (out / "typologies.json").write_text(json.dumps(TYPOLOGIES, indent=2))
    (out / "casenotes.json").write_text(json.dumps(CASE_NOTES, indent=2))

    print(f"✅ commercial-risk dataset written to {out}")
    print(f"   firms/entities: {len(firms)} ({sum(f['in_cluster'] for f in firms)} in the cluster)")
    print(f"   linked edges: {len(edges)}   ts firms: {len(ts_out)}")
    print(f"   typologies: {len(TYPOLOGIES)}   case notes: {len(CASE_NOTES)}")
    print(f"   cluster: {CLUSTER['flagged']} → {CLUSTER['related']} → {CLUSTER['holding']} → {CLUSTER['ubo']}")


if __name__ == "__main__":
    main()
