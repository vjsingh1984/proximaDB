#!/usr/bin/env python3
# Copyright (C) 2025 ProximaDB
# SPDX-License-Identifier: Apache-2.0
"""
Synthetic web-property dataset for the ProximaDB *internet / web* vertical demo.

Deterministic, stdlib-only, no external data — a small content site + its service
dependency graph, with one embedded incident (a bad deploy on a shared database that
cascades errors up through the dependency chain to a user-facing page). Exercises all
four modalities:

  * **graph**       pages + services (nodes) + `depends_on` edges — the incident is a chain
  * **timeseries**  per-entity 5xx-error-rate series; the failing chain bursts
  * **vector**      labelled web-incident *typology* signatures for root-cause matching
  * **text/RAG**    postmortems + runbooks as a retrieval corpus

Outputs JSON under ``--out`` (default ``./data``). Run this before ``copilot_live.py``.
"""

from __future__ import annotations

import argparse
import json
import random
from datetime import datetime, timedelta, timezone
from pathlib import Path

WINDOW_START = datetime(2026, 3, 2, 8, 0, tzinfo=timezone.utc)

# The dependency chain that fails: a user-facing page resolves through two services to a
# shared database that gets a bad deploy. Errors cascade UP the chain (dependents of the
# broken db). The graph query traces DOWN `depends_on` from the page to the root cause.
INCIDENT = {
    "page": "/pricing",  # the alerted, user-facing page
    "chain": ["/pricing", "checkout-svc", "payments-api", "db-primary"],  # depends_on path
    "cascade": ["/pricing", "/checkout", "checkout-svc", "payments-api", "db-primary"],
}

# Pages (user-facing URLs) and services (backend). ~40 entities total with background noise.
PAGES = ["/", "/pricing", "/checkout", "/docs", "/blog", "/login", "/search",
         "/account", "/support", "/status", "/careers", "/about"]
SERVICES = ["cdn-edge", "checkout-svc", "payments-api", "db-primary", "db-replica",
            "auth-svc", "search-svc", "content-svc", "profile-svc", "ledger-svc"]

# depends_on edges: page -> service(s) -> service(s). The incident chain is explicit.
DEPENDS_ON = [
    ("/", "cdn-edge"),
    ("/pricing", "cdn-edge"), ("/pricing", "checkout-svc"),
    ("/checkout", "checkout-svc"), ("/checkout", "auth-svc"),
    ("checkout-svc", "payments-api"), ("checkout-svc", "auth-svc"),
    ("payments-api", "db-primary"), ("payments-api", "ledger-svc"),
    ("ledger-svc", "db-primary"),
    ("/account", "profile-svc"), ("profile-svc", "db-replica"),
    ("/login", "auth-svc"), ("auth-svc", "db-replica"),
    ("/search", "search-svc"), ("search-svc", "db-replica"),
    ("/docs", "content-svc"), ("/blog", "content-svc"), ("content-svc", "cdn-edge"),
    ("/support", "content-svc"), ("/status", "cdn-edge"),
]

# Labelled web-incident signatures (the vector library). Features are the SCALE-NORMALISED
# shape of the error burst — [severity, spike_ratio, spread, concentration] — the same vector
# copilot_live.py derives from the ProximaDB timeseries aggregate (see feature_vector there):
#   severity      = peak_bucket / 100          (how big)
#   spike_ratio   = peak / mean                 (how spiky vs baseline)
#   spread        = burst_fraction * 10         (how many buckets are hot)
#   concentration = (peak / total) * 10         (how concentrated in one bucket)
# Scaling keeps every dimension O(1..10) so cosine discriminates on *shape*, not raw magnitude —
# a sharp concentrated deploy spike separates cleanly from a spread-out bot surge.
TYPOLOGIES = [
    {"id": "typ-deploy-regression", "label": "deploy regression", "case_ref": "pm-deploy-01",
     "features": [6.0, 5.0, 2.5, 6.5]},   # big, spiky, few hot buckets, highly concentrated
    {"id": "typ-cdn-cache", "label": "CDN cache poisoning", "case_ref": "pm-cdn-01",
     "features": [2.0, 2.0, 6.0, 2.0]},   # moderate, flat, many hot buckets, diffuse
    {"id": "typ-bot-surge", "label": "bot / scraper surge", "case_ref": "pm-bot-01",
     "features": [3.0, 4.0, 6.0, 2.5]},   # moderate, spiky, many hot buckets, diffuse
    {"id": "typ-nominal", "label": "nominal", "case_ref": None,
     "features": [0.5, 1.4, 0.5, 3.0]},
]

POSTMORTEMS = [
    {"id": "pm-deploy-01", "typology": "deploy regression",
     "text": "A backend deploy introduced a regression that spiked 5xx errors, cascading up "
             "the service dependency chain to user-facing pages within minutes of rollout. "
             "Sudden sharp error spike correlated with a release marker.",
     "resolution": "Rolled back the offending deploy; added a canary + error-budget gate to the pipeline."},
    {"id": "pm-cdn-01", "typology": "CDN cache poisoning",
     "text": "A malformed cache key poisoned the CDN edge, serving errors for a sustained window "
             "at moderate rate across many pages until the cache was purged.",
     "resolution": "Purged the edge cache; pinned the cache-key normalization; added a cache-hit-ratio alert."},
    {"id": "pm-bot-01", "typology": "bot / scraper surge",
     "text": "A scraper botnet drove a spiky surge of requests to search and listing pages, "
             "exhausting a connection pool and returning 5xx to real users.",
     "resolution": "Rate-limited by ASN; moved search behind a queue; added bot-score WAF rules."},
    {"id": "runbook-oncall-01", "typology": "oncall",
     "text": "Incident on-call runbook: correlate the error spike to the deploy timeline, trace the "
             "service dependency graph from the alerting page down to the root service, check the "
             "release marker, and roll back before mitigating downstream.",
     "resolution": "Follow trace-to-root then roll-back-first; page the owning team for the root service."},
]


def build_nodes(rng: random.Random) -> list[dict]:
    chain = set(INCIDENT["cascade"])
    nodes = []
    for p in PAGES:
        nodes.append({"id": p, "kind": "page", "tier": "edge", "in_incident": p in chain})
    for s in SERVICES:
        nodes.append({"id": s, "kind": "service",
                      "tier": "data" if s.startswith("db-") else "app",
                      "in_incident": s in chain})
    return nodes


def build_error_series(rng: random.Random) -> dict[str, list[dict]]:
    """Per-entity 5xx-error-count points. Background low noise + the incident burst."""
    entities = PAGES + SERVICES
    series: dict[str, list[dict]] = {e: [] for e in entities}

    # ── background: low, time-spread 5xx counts for every entity ──
    for e in entities:
        for _ in range(rng.randint(6, 10)):
            ts = WINDOW_START + timedelta(minutes=rng.randint(0, 300))
            series[e].append({"ts": ts.isoformat(), "errors": round(rng.uniform(1, 9), 1)})

    # ── the incident: bad deploy on db-primary at hour 3, cascading up the chain ──
    deploy = WINDOW_START + timedelta(hours=3)
    for entity in INCIDENT["cascade"]:
        # sharp error spike concentrated in a ~40-min window after the deploy
        for _ in range(rng.randint(8, 12)):
            ts = deploy + timedelta(minutes=rng.randint(0, 40))
            series[entity].append({"ts": ts.isoformat(), "errors": round(rng.uniform(70, 110), 1)})
    return series


def main() -> None:
    ap = argparse.ArgumentParser(description="Generate the internet/web demo dataset.")
    ap.add_argument("--out", default=str(Path(__file__).parent / "data"))
    ap.add_argument("--seed", type=int, default=42)
    args = ap.parse_args()
    rng = random.Random(args.seed)

    nodes = build_nodes(rng)
    edges = [{"from": a, "to": b, "type": "depends_on"} for (a, b) in DEPENDS_ON]
    series = build_error_series(rng)
    ts_out = [{"entity": e, "points": sorted(pts, key=lambda p: p["ts"])}
              for e, pts in series.items()]

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "topology.json").write_text(json.dumps({"nodes": nodes, "edges": edges}, indent=2))
    (out / "errorstreams.json").write_text(json.dumps(ts_out, indent=2))
    (out / "typologies.json").write_text(json.dumps(TYPOLOGIES, indent=2))
    (out / "postmortems.json").write_text(json.dumps(POSTMORTEMS, indent=2))

    print(f"✅ internet/web dataset written to {out}")
    print(f"   entities:   {len(nodes)} ({sum(n['in_incident'] for n in nodes)} in the incident chain)")
    print(f"   depends_on: {len(edges)} edges   ts entities: {len(ts_out)}")
    print(f"   typologies: {len(TYPOLOGIES)}   postmortems: {len(POSTMORTEMS)}")
    print(f"   incident: {' → '.join(INCIDENT['chain'])} (bad deploy on db-primary)")


if __name__ == "__main__":
    main()
