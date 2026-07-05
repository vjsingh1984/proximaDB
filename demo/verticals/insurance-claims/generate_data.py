#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Synthetic insurance dataset for the ProximaDB *insurance claims-fraud* vertical demo.

Deterministic, stdlib-only, no external data — a small auto-insurance world with one
embedded *staged-accident ring* (an organizer recruits participants who all file through a
single crooked clinic) over a sea of legitimate claims. Exercises all four modalities:

  * **graph**       claimants + providers (nodes) + `linked` edges — the ring is a path
  * **timeseries**  per-claimant claimed-amount series; the ring claimants burst
  * **vector**      labelled fraud *typology* signatures for behaviour matching
  * **text/RAG**    SIU (Special Investigations Unit) case notes as a retrieval corpus

Outputs JSON under ``--out`` (default ``./data``). Run this before ``copilot_live.py``.
"""

from __future__ import annotations

import argparse
import json
import random
from datetime import datetime, timedelta, timezone
from pathlib import Path

N_CLAIMANTS = 30
WINDOW_START = datetime(2026, 4, 6, 8, 0, tzinfo=timezone.utc)

# The staged-accident ring: an organizer recruits participants who all file through one
# crooked clinic, which bills a phantom (capper) claimant. This is the path the graph traces.
RING = {
    "organizer": "clmt-007",
    "participants": ["clmt-011", "clmt-012", "clmt-013"],
    "provider": "prov-briarwood",   # the colluding clinic
    "capper": "clmt-020",
}

# Labelled fraud-typology signatures (the vector library). Features are the SCALE-NORMALISED
# shape of the claimed-amount burst — [severity, spike_ratio, spread, concentration] — the same
# vector copilot_live.py derives from the ProximaDB timeseries aggregate (see feature_vector):
#   severity      = peak_bucket / 10000        (how big the biggest window is)
#   spike_ratio   = peak / mean                (how spiky vs the claimant baseline)
#   spread        = burst_fraction * 10        (how many windows are hot)
#   concentration = (peak / total) * 10        (how concentrated in one window)
# Scaling keeps every dimension O(1..10) so cosine discriminates on *shape*, not raw magnitude.
TYPOLOGIES = [
    {"id": "typ-staged-ring", "label": "staged-accident ring", "case_ref": "siu-ring-01",
     "features": [6.0, 5.0, 2.5, 6.5]},    # big, spiky, few hot windows, concentrated burst
    {"id": "typ-provider-collusion", "label": "provider collusion", "case_ref": "siu-collusion-01",
     "features": [3.0, 2.2, 6.0, 2.2]},    # moderate, flat, sustained over many windows
    {"id": "typ-claim-stacking", "label": "claim stacking", "case_ref": "siu-stacking-01",
     "features": [3.5, 4.0, 6.0, 2.6]},    # moderate, spiky, many windows (repeat filer)
    {"id": "typ-nominal", "label": "nominal", "case_ref": None,
     "features": [0.6, 1.4, 0.5, 3.0]},
]

CASE_NOTES = [
    {"id": "siu-ring-01", "typology": "staged-accident ring",
     "text": "Multiple bodily-injury claims from a single staged multi-vehicle collision, all "
             "treated at one clinic within days, organized by a recruiter who signs up participants. "
             "Tight temporal cluster, shared provider, low prior claim history.",
     "resolution": "Denied the cluster; referred the organizer and clinic to SIU + the state fraud bureau; flagged the provider NPI."},
    {"id": "siu-collusion-01", "typology": "provider collusion",
     "text": "A provider systematically upcodes and bills phantom treatments across many unrelated "
             "claimants. Sustained abnormal billing volume rather than a single burst.",
     "resolution": "Audited the provider's billing; suspended direct payment; recovered overpayments."},
    {"id": "siu-stacking-01", "typology": "claim stacking",
     "text": "A single claimant files repeated overlapping claims across policies/incidents to stack "
             "payouts. High personal claim frequency over time.",
     "resolution": "Consolidated the claims; applied anti-stacking policy terms; opened an EUO."},
    {"id": "siu-indicators-01", "typology": "indicators",
     "text": "SIU red-flag guidance: shared provider across unrelated claimants, tight temporal "
             "clustering of injuries, a recruiter linking participants, and low pre-incident history "
             "are classic staged-accident-ring indicators — trace the shared entities.",
     "resolution": "Trace shared-provider and recruiter links; hold payment on the cluster pending SIU review."},
]


def build_parties(rng: random.Random) -> list[dict]:
    ring_ids = {RING["organizer"], RING["capper"], *RING["participants"]}
    parties = []
    for i in range(1, N_CLAIMANTS + 1):
        cid = f"clmt-{i:03d}"
        parties.append({"id": cid, "kind": "claimant", "in_ring": cid in ring_ids})
    # a handful of providers; prov-briarwood is the crooked one
    for name in ["prov-briarwood", "prov-oakhill", "prov-central", "prov-mercy",
                 "prov-lakeside", "prov-summit"]:
        parties.append({"id": name, "kind": "provider", "in_ring": name == RING["provider"]})
    return parties


def build_linked_edges(rng: random.Random) -> list[dict]:
    """`linked` edges. Background sparse claimant-provider ties + the ring path."""
    edges = []
    claimants = [f"clmt-{i:03d}" for i in range(1, N_CLAIMANTS + 1)]
    providers = ["prov-oakhill", "prov-central", "prov-mercy", "prov-lakeside", "prov-summit"]

    # ── background: each non-ring claimant linked to a legit provider ──
    ring_ids = {RING["organizer"], RING["capper"], *RING["participants"]}
    for c in claimants:
        if c in ring_ids:
            continue
        edges.append({"from": c, "to": rng.choice(providers)})

    # ── the ring: organizer -> participants -> crooked provider -> capper ──
    org, parts, prov, capper = (RING["organizer"], RING["participants"],
                                RING["provider"], RING["capper"])
    for p in parts:
        edges.append({"from": org, "to": p})       # organizer recruited each participant
        edges.append({"from": p, "to": prov})      # participant filed with the crooked clinic
    edges.append({"from": prov, "to": capper})     # clinic billed the phantom capper
    return edges


def build_claim_series(rng: random.Random) -> dict[str, list[dict]]:
    """Per-claimant claimed-amount points. Legit low/occasional + the ring burst."""
    claimants = [f"clmt-{i:03d}" for i in range(1, N_CLAIMANTS + 1)]
    series: dict[str, list[dict]] = {c: [] for c in claimants}

    # ── background: sparse, modest claims spread over the window ──
    for c in claimants:
        for _ in range(rng.randint(2, 5)):
            ts = WINDOW_START + timedelta(hours=rng.randint(0, 72))
            series[c].append({"ts": ts.isoformat(), "amount": round(rng.uniform(200, 1500), 2)})

    # ── the ring: a tight burst of large bodily-injury claims in one ~90-min window ──
    burst = WINDOW_START + timedelta(hours=30)
    for claimant in [RING["organizer"], RING["capper"], *RING["participants"]]:
        for _ in range(rng.randint(5, 7)):
            ts = burst + timedelta(minutes=rng.randint(0, 90))
            series[claimant].append({"ts": ts.isoformat(), "amount": round(rng.uniform(9000, 15000), 2)})
    return series


def main() -> None:
    ap = argparse.ArgumentParser(description="Generate the insurance claims-fraud demo dataset.")
    ap.add_argument("--out", default=str(Path(__file__).parent / "data"))
    ap.add_argument("--seed", type=int, default=42)
    args = ap.parse_args()
    rng = random.Random(args.seed)

    parties = build_parties(rng)
    edges = [{"from": e["from"], "to": e["to"], "type": "linked"}
             for e in build_linked_edges(rng)]
    series = build_claim_series(rng)
    ts_out = [{"claimant": c, "points": sorted(pts, key=lambda p: p["ts"])}
              for c, pts in series.items()]

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "parties.json").write_text(json.dumps({"nodes": parties, "edges": edges}, indent=2))
    (out / "claims.json").write_text(json.dumps(ts_out, indent=2))
    (out / "typologies.json").write_text(json.dumps(TYPOLOGIES, indent=2))
    (out / "casenotes.json").write_text(json.dumps(CASE_NOTES, indent=2))

    print(f"✅ insurance claims-fraud dataset written to {out}")
    print(f"   parties: {len(parties)} ({sum(p['in_ring'] for p in parties)} in the ring)")
    print(f"   linked edges: {len(edges)}   ts claimants: {len(ts_out)}")
    print(f"   typologies: {len(TYPOLOGIES)}   case notes: {len(CASE_NOTES)}")
    print(f"   ring: {RING['organizer']} → {RING['participants']} → {RING['provider']} → {RING['capper']}")


if __name__ == "__main__":
    main()
