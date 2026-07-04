#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Synthetic AML dataset for the ProximaDB *fraud & anti-money-laundering* vertical demo.

Deterministic, stdlib-only, no external data — a small bank-transaction world with one
embedded laundering ring (placement → layering mules → consolidation → cash-out) over a
sea of normal activity. Exercises all four modalities:

  * **graph**       accounts (nodes) + `sent_to` transfers (edges) — the ring is a path
  * **timeseries**  per-account transacted-amount series; the ring accounts burst
  * **vector**      labelled AML *typology* signatures for behaviour matching
  * **text/RAG**    case notes + sanctions/typology guidance as a retrieval corpus

Outputs JSON under ``--out`` (default ``./data``). Run this before ``copilot_live.py``.
"""

from __future__ import annotations

import argparse
import json
import random
from datetime import datetime, timedelta, timezone
from pathlib import Path

N_ACCOUNTS = 40
WINDOW_START = datetime(2026, 2, 1, 8, 0, tzinfo=timezone.utc)
HIGH_RISK_COUNTRIES = ["Shell-Isle", "Nowheria"]

# The laundering ring: cash structured into `placement`, layered through `mules`,
# consolidated, then cashed out. This is the path the graph query must trace.
RING = {
    "placement": "acct-007",
    "mules": ["acct-011", "acct-012", "acct-013"],
    "consolidation": "acct-020",
    "cashout": "acct-030",
}

# Labelled AML typology signatures (the vector library). Features are the SCALE-NORMALISED
# shape of the transacted-volume burst — [severity, spike_ratio, spread, concentration] — the
# same vector copilot_live.py derives from the ProximaDB timeseries aggregate (see feature_vector).
# Scaling keeps every dimension O(1..10) so cosine discriminates on burst *shape*, not raw dollars:
# structuring's concentrated sub-threshold cash-in burst separates cleanly from spread-out layering.
# This is the shared pattern used by every built vertical (internet/insurance/markets/D&B).
TYPOLOGIES = [
    {"id": "typ-structuring", "label": "structuring / smurfing", "case_ref": "case-struct-01",
     "features": [6.0, 5.0, 2.5, 6.5]},     # concentrated sub-threshold cash-in burst
    {"id": "typ-layering", "label": "rapid layering", "case_ref": "case-layer-01",
     "features": [3.0, 2.2, 6.0, 2.2]},     # spread across intermediaries, sustained
    {"id": "typ-mule", "label": "money mule", "case_ref": "case-mule-01",
     "features": [3.5, 4.0, 6.0, 2.6]},     # moderate, spiky, many windows (forwarding)
    {"id": "typ-nominal", "label": "nominal", "case_ref": None,
     "features": [0.6, 1.4, 0.5, 3.0]},
]

CASE_NOTES = [
    {"id": "case-struct-01", "typology": "structuring / smurfing",
     "text": "Multiple sub-threshold cash deposits (just under the 10,000 reporting limit) into a "
             "single account within a short window, immediately moved on. Indicative of structuring.",
     "resolution": "Filed a SAR; froze the placement account; escalated the fan-out beneficiaries for KYC review."},
    {"id": "case-layer-01", "typology": "rapid layering",
     "text": "Funds split across several intermediary accounts and moved rapidly in and out to obscure origin. "
             "High fan-out, low holding time.",
     "resolution": "Traced the layering chain to a consolidation account; SAR filed on the full ring."},
    {"id": "case-mule-01", "typology": "money mule",
     "text": "Personal account receiving funds it forwards on within hours for a fee. Classic money-mule behaviour.",
     "resolution": "Account closed; beneficiary network mapped for the wider investigation."},
    {"id": "sanctions-01", "typology": "sanctions",
     "text": "Sanctions/PEP screening guidance: counterparties in high-risk jurisdictions (Shell-Isle, Nowheria) "
             "require enhanced due diligence and transaction holds.",
     "resolution": "Hold and manual review for any counterparty resolving to a high-risk jurisdiction."},
]


def build_accounts(rng: random.Random) -> list[dict]:
    ring_ids = {RING["placement"], RING["consolidation"], RING["cashout"], *RING["mules"]}
    accounts = []
    for i in range(1, N_ACCOUNTS + 1):
        aid = f"acct-{i:03d}"
        kind = "business" if rng.random() < 0.3 else "personal"
        country = rng.choice(HIGH_RISK_COUNTRIES) if rng.random() < 0.1 else "Domestica"
        accounts.append({"id": aid, "kind": kind, "country": country,
                         "in_ring": aid in ring_ids})
    return accounts


def build_transactions(rng: random.Random) -> list[dict]:
    """Return raw transfers {from, to, amount, ts}. Background noise + the ring burst."""
    txns = []
    accts = [f"acct-{i:03d}" for i in range(1, N_ACCOUNTS + 1)]

    # ── background: sparse, small, time-spread transfers (low per-bucket volume) ──
    for a in accts:
        for _ in range(rng.randint(1, 3)):
            b = rng.choice([x for x in accts if x != a])
            ts = WINDOW_START + timedelta(minutes=rng.randint(0, 360))
            txns.append({"from": a, "to": b, "amount": round(rng.uniform(50, 700), 2),
                         "ts": ts.isoformat()})

    # ── the ring: a 90-minute burst starting at hour 3 ──────────────────────────
    burst = WINDOW_START + timedelta(hours=3)
    p, mules, cons, cash = (RING["placement"], RING["mules"],
                            RING["consolidation"], RING["cashout"])
    # 1. structuring: ~16 sub-threshold cash-ins into the placement account, in a TIGHT
    #    window (rapid smurfing) so the placement account's signature is a concentrated burst.
    for k in range(16):
        ts = burst + timedelta(minutes=rng.randint(0, 25))
        txns.append({"from": f"cash-in-{k:02d}", "to": p,
                     "amount": round(rng.uniform(820, 980), 2), "ts": ts.isoformat()})
    # 2. layering: placement -> each mule, several rapid transfers
    for m in mules:
        for _ in range(rng.randint(3, 5)):
            ts = burst + timedelta(minutes=rng.randint(30, 100))
            txns.append({"from": p, "to": m, "amount": round(rng.uniform(2000, 3200), 2),
                         "ts": ts.isoformat()})
    # 3. consolidation: mules -> consolidation account
    for m in mules:
        ts = burst + timedelta(minutes=rng.randint(80, 130))
        txns.append({"from": m, "to": cons, "amount": round(rng.uniform(3500, 4500), 2),
                     "ts": ts.isoformat()})
    # 4. cash-out: consolidation -> cash-out -> external
    ts = burst + timedelta(minutes=rng.randint(120, 150))
    txns.append({"from": cons, "to": cash, "amount": 11800.0, "ts": ts.isoformat()})
    txns.append({"from": cash, "to": "external-payout",
                 "amount": 11500.0, "ts": (ts + timedelta(minutes=20)).isoformat()})
    return txns


def main() -> None:
    ap = argparse.ArgumentParser(description="Generate the fraud/AML demo dataset.")
    ap.add_argument("--out", default=str(Path(__file__).parent / "data"))
    ap.add_argument("--seed", type=int, default=42)
    args = ap.parse_args()
    rng = random.Random(args.seed)

    accounts = build_accounts(rng)
    txns = build_transactions(rng)

    # graph edges: aggregate transfers into sent_to (from -> to) with total + count
    agg: dict[tuple[str, str], dict] = {}
    for t in txns:
        key = (t["from"], t["to"])
        e = agg.setdefault(key, {"from": t["from"], "to": t["to"], "total": 0.0, "count": 0})
        e["total"] = round(e["total"] + t["amount"], 2)
        e["count"] += 1
    edges = list(agg.values())

    # per-account time series: transacted amount points (as sender OR receiver)
    series: dict[str, list[dict]] = {}
    for t in txns:
        for acct in (t["from"], t["to"]):
            if acct.startswith("acct-"):
                series.setdefault(acct, []).append({"ts": t["ts"], "amount": t["amount"]})
    ts_out = [{"account": a, "points": sorted(pts, key=lambda p: p["ts"])}
              for a, pts in series.items()]

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "accounts.json").write_text(json.dumps({"nodes": accounts, "edges": edges}, indent=2))
    (out / "transactions.json").write_text(json.dumps(ts_out, indent=2))
    (out / "typologies.json").write_text(json.dumps(TYPOLOGIES, indent=2))
    (out / "casenotes.json").write_text(json.dumps(CASE_NOTES, indent=2))

    print(f"✅ fraud/AML dataset written to {out}")
    print(f"   accounts:     {len(accounts)} ({sum(a['in_ring'] for a in accounts)} in the ring)")
    print(f"   transactions: {len(txns)}  ->  sent_to edges: {len(edges)}")
    print(f"   ts accounts:  {len(ts_out)}   typologies: {len(TYPOLOGIES)}   case notes: {len(CASE_NOTES)}")
    print(f"   ring: {RING['placement']} → {RING['mules']} → {RING['consolidation']} → {RING['cashout']}")


if __name__ == "__main__":
    main()
