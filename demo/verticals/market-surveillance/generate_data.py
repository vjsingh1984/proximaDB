#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Synthetic markets dataset for the ProximaDB *market-surveillance & trading* vertical demo.

Deterministic, stdlib-only, no external data — a small equities-trading world with one
embedded *coordinated manipulation ring* (a principal account coordinates linked accounts
through a shell entity to spoof and wash-trade a symbol) over a sea of normal order flow.
Exercises all four modalities:

  * **graph**       accounts + shell entities (nodes) + `coordinated` edges — the ring is a path
  * **timeseries**  per-account order-message-rate series; the ring accounts burst (spoofing)
  * **vector**      labelled manipulation *typology* signatures for behaviour matching
  * **text/RAG**    surveillance case notes + MAR/MiFID guidance as a retrieval corpus

Outputs JSON under ``--out`` (default ``./data``). Run this before ``copilot_live.py``.
"""

from __future__ import annotations

import argparse
import json
import random
from datetime import datetime, timedelta, timezone
from pathlib import Path

N_ACCOUNTS = 30
WINDOW_START = datetime(2026, 5, 4, 13, 30, tzinfo=timezone.utc)  # a trading session

# The manipulation ring: a principal coordinates linked accounts through a shell entity to
# spoof (place-and-cancel) and wash-trade the symbol. This is the path the graph traces.
RING = {
    "principal": "trd-007",
    "linked": ["trd-011", "trd-012", "trd-013"],
    "shell": "ent-cayman",        # the beneficial-owner shell tying them together
    "nominee": "trd-020",
}

# Labelled manipulation-typology signatures (the vector library). Features are the
# SCALE-NORMALISED shape of the order-message burst — [severity, spike_ratio, spread,
# concentration] — the same vector copilot_live.py derives from the ProximaDB timeseries
# aggregate (see feature_vector). Scaling keeps every dimension O(1..10) so cosine
# discriminates on *shape*, not raw message counts.
TYPOLOGIES = [
    {"id": "typ-spoofing", "label": "spoofing / layering", "case_ref": "mar-spoof-01",
     "features": [6.0, 5.0, 2.5, 6.5]},     # big, spiky, few hot windows, concentrated place-and-cancel
    {"id": "typ-wash", "label": "wash trading", "case_ref": "mar-wash-01",
     "features": [3.0, 2.2, 6.0, 2.2]},     # moderate, flat, sustained matched trades
    {"id": "typ-momentum", "label": "momentum ignition", "case_ref": "mar-momentum-01",
     "features": [3.5, 4.0, 6.0, 2.6]},     # moderate, spiky, many windows (repeated ignition)
    {"id": "typ-nominal", "label": "nominal", "case_ref": None,
     "features": [0.6, 1.4, 0.5, 3.0]},
]

CASE_NOTES = [
    {"id": "mar-spoof-01", "typology": "spoofing / layering",
     "text": "A concentrated burst of large non-bona-fide orders placed on one side of the book and "
             "cancelled before execution to move the price, benefiting real orders on the other side. "
             "Very high order-to-trade ratio in a tight window; coordinated across linked accounts.",
     "resolution": "Filed a MAR Article 12 STOR; froze the principal account; escalated the linked cluster and the beneficial-owner shell."},
    {"id": "mar-wash-01", "typology": "wash trading",
     "text": "Matched buy/sell orders between accounts under common control creating fake volume with no "
             "change in beneficial ownership. Sustained self-crossing rather than a single burst.",
     "resolution": "Suspended the accounts; mapped common beneficial ownership; reported to the venue and regulator."},
    {"id": "mar-momentum-01", "typology": "momentum ignition",
     "text": "Aggressive orders fired to ignite a rapid price move and attract algorithmic followers, then "
             "the instigator reverses into the induced liquidity. Repeated ignition bursts.",
     "resolution": "Flagged the instigating account; correlated with the reversal fills; opened a manipulation case."},
    {"id": "mar-indicators-01", "typology": "indicators",
     "text": "Surveillance red-flag guidance (MAR / MiFID II): abnormal order-to-trade ratio, tight temporal "
             "clustering of place-and-cancel, coordinated timing across accounts sharing a beneficial owner, "
             "and price impact around the burst are classic spoofing/layering indicators — trace common control.",
     "resolution": "Trace beneficial-owner and coordination links; hold and review the cluster; file a STOR if corroborated."},
]


def build_accounts(rng: random.Random) -> list[dict]:
    ring_ids = {RING["principal"], RING["nominee"], *RING["linked"]}
    accounts = []
    for i in range(1, N_ACCOUNTS + 1):
        aid = f"trd-{i:03d}"
        accounts.append({"id": aid, "kind": "account", "in_ring": aid in ring_ids})
    # shell / beneficial-owner entities; ent-cayman is the ring's
    for name in ["ent-cayman", "ent-onshore", "ent-family", "ent-fund"]:
        accounts.append({"id": name, "kind": "entity", "in_ring": name == RING["shell"]})
    return accounts


def build_coordinated_edges(rng: random.Random) -> list[dict]:
    """`coordinated` edges. Background account-entity ties + the ring path."""
    edges = []
    accounts = [f"trd-{i:03d}" for i in range(1, N_ACCOUNTS + 1)]
    entities = ["ent-onshore", "ent-family", "ent-fund"]
    ring_ids = {RING["principal"], RING["nominee"], *RING["linked"]}

    # ── background: each non-ring account tied to a benign entity ──
    for a in accounts:
        if a in ring_ids:
            continue
        edges.append({"from": a, "to": rng.choice(entities)})

    # ── the ring: principal -> linked accounts -> shell -> nominee ──
    principal, linked, shell, nominee = (RING["principal"], RING["linked"],
                                         RING["shell"], RING["nominee"])
    for a in linked:
        edges.append({"from": principal, "to": a})   # principal coordinates each linked account
        edges.append({"from": a, "to": shell})       # linked accounts controlled via the shell
    edges.append({"from": shell, "to": nominee})     # shell fronts the nominee account
    return edges


def build_order_series(rng: random.Random) -> dict[str, list[dict]]:
    """Per-account order-message-count points. Normal low flow + the ring spoofing burst."""
    accounts = [f"trd-{i:03d}" for i in range(1, N_ACCOUNTS + 1)]
    series: dict[str, list[dict]] = {a: [] for a in accounts}

    # ── background: modest order-message flow spread across the session ──
    for a in accounts:
        for _ in range(rng.randint(3, 6)):
            ts = WINDOW_START + timedelta(minutes=rng.randint(0, 240))
            series[a].append({"ts": ts.isoformat(), "messages": round(rng.uniform(2, 20), 1)})

    # ── the ring: a tight burst of place-and-cancel messages in one ~15-min window ──
    burst = WINDOW_START + timedelta(hours=1)
    for account in [RING["principal"], RING["nominee"], *RING["linked"]]:
        for _ in range(rng.randint(5, 7)):
            ts = burst + timedelta(minutes=rng.randint(0, 15))
            series[account].append({"ts": ts.isoformat(), "messages": round(rng.uniform(400, 700), 1)})
    return series


def main() -> None:
    ap = argparse.ArgumentParser(description="Generate the market-surveillance demo dataset.")
    ap.add_argument("--out", default=str(Path(__file__).parent / "data"))
    ap.add_argument("--seed", type=int, default=42)
    args = ap.parse_args()
    rng = random.Random(args.seed)

    accounts = build_accounts(rng)
    edges = [{"from": e["from"], "to": e["to"], "type": "coordinated"}
             for e in build_coordinated_edges(rng)]
    series = build_order_series(rng)
    ts_out = [{"account": a, "points": sorted(pts, key=lambda p: p["ts"])}
              for a, pts in series.items()]

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "accounts.json").write_text(json.dumps({"nodes": accounts, "edges": edges}, indent=2))
    (out / "orders.json").write_text(json.dumps(ts_out, indent=2))
    (out / "typologies.json").write_text(json.dumps(TYPOLOGIES, indent=2))
    (out / "casenotes.json").write_text(json.dumps(CASE_NOTES, indent=2))

    print(f"✅ market-surveillance dataset written to {out}")
    print(f"   accounts: {len(accounts)} ({sum(a['in_ring'] for a in accounts)} in the ring)")
    print(f"   coordinated edges: {len(edges)}   ts accounts: {len(ts_out)}")
    print(f"   typologies: {len(TYPOLOGIES)}   case notes: {len(CASE_NOTES)}")
    print(f"   ring: {RING['principal']} → {RING['linked']} → {RING['shell']} → {RING['nominee']}")


if __name__ == "__main__":
    main()
