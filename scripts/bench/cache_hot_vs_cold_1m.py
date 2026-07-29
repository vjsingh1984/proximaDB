#!/usr/bin/env python3
"""Cache hot-vs-cold measurement (Phase 2) — SIFT 1M, post-compaction.

Measures the cache effectiveness delta between a COLD first-access (caches empty
after server restart) and a HOT repeat (result cache warm). Scrapes Prometheus
cache metrics (survivor hits/misses, segment invariants hits/misses) to capture
the per-sweep cache-hit delta.

Prerequisites:
  - proximadb-server running on :5678 (release binary from develop).
  - SIFT1M ingested + compaction settled.
  - Restart the server immediately before running (to clear in-memory caches).
  - Set PROXIMADB_L0_STOP_TRIGGER=0 on restart if l0_count >= stop_trigger
    (D2 restart-recovery bug, fixed in #1294).

Usage:
  # 1. Restart server (clears caches):
  #    pkill proximadb-server; sleep 3; <boot with same data_dir>
  # 2. Immediately run:
  python3 scripts/bench/cache_hot_vs_cold_1m.py
"""
import json, os, struct, time, urllib.request

SERVER = os.environ.get("PROXIMA_VERIFY_SERVER", "http://127.0.0.1:5678")
CID    = os.environ.get("PROXIMA_COLLECTION_ID", "1")
NQUERY = 200
TOPK   = 10

def post(p, b, t=120):
    r = urllib.request.urlopen(urllib.request.Request(
        SERVER + p, data=json.dumps(b).encode(),
        headers={"Content-Type": "application/json"}, method="POST"), timeout=t)
    return json.loads(r.read())

def read_fvecs(path, lim):
    with open(path, "rb") as f:
        raw = f.read()
    o = []; i = 0
    while len(o) < lim:
        if i + 4 > len(raw): break
        d = struct.unpack("<i", raw[i:i+4])[0]; i += 4
        o.append(list(struct.unpack("<%df" % d, raw[i:i+4*d]))); i += 4 * d
    return o

def scrape_metrics():
    """Scrape cache hit/miss counters from /metrics/prometheus."""
    try:
        text = urllib.request.urlopen(
            SERVER + "/metrics/prometheus", timeout=10).read().decode()
    except Exception:
        return {}
    keys = [
        "proximadb_survivor_cache_hits",
        "proximadb_survivor_cache_misses",
        "proximadb_segment_invariants_cache_hits_total",
        "proximadb_segment_invariants_cache_misses_total",
    ]
    out = {}
    for line in text.splitlines():
        if line.startswith("#") or not line.strip():
            continue
        for key in keys:
            if line.startswith(key + " ") or line.startswith(key + "{"):
                parts = line.split()
                if len(parts) >= 2:
                    try:
                        out[key] = float(parts[-1])
                    except ValueError:
                        pass
    return out

def run_sweep(queries, label):
    """Run a sweep of queries, return latency stats."""
    lats = []
    for q in queries:
        ts = time.time()
        post(f"/api/v2/collections/{CID}/search",
             {"vector": q, "top_k": TOPK}, t=120)
        lats.append((time.time() - ts) * 1000)
    lats.sort()
    return {
        "p50": lats[len(lats) // 2],
        "p95": lats[int(len(lats) * 0.95)],
        "mean": sum(lats) / len(lats),
    }

def fmt_delta(m_before, m_after, key):
    d = m_after.get(key, 0) - m_before.get(key, 0)
    return f"{d:+,.0f}"

def main():
    base_path = os.environ.get(
        "SIFT_QUERY", "/Users/vijaysingh/sift1m/sift_query.fvecs")
    queries = read_fvecs(base_path, NQUERY)
    print(f"loaded {len(queries)} queries", flush=True)

    m0 = scrape_metrics()
    print(f"\nbaseline: {m0}", flush=True)

    # COLD sweep
    print(f"\nCOLD sweep ({NQUERY} queries, caches empty)...", flush=True)
    cold = run_sweep(queries, "COLD")
    m1 = scrape_metrics()
    print(f"COLD: p50={cold['p50']:.1f}ms p95={cold['p95']:.1f}ms", flush=True)
    for k in ["proximadb_survivor_cache_hits", "proximadb_survivor_cache_misses",
              "proximadb_segment_invariants_cache_hits_total",
              "proximadb_segment_invariants_cache_misses_total"]:
        print(f"  Δ {k}: {fmt_delta(m0, m1, k)}", flush=True)

    # HOT sweep (same queries)
    print(f"\nHOT sweep (repeat {NQUERY} queries)...", flush=True)
    hot = run_sweep(queries, "HOT")
    m2 = scrape_metrics()
    print(f"HOT:  p50={hot['p50']:.1f}ms p95={hot['p95']:.1f}ms", flush=True)
    for k in ["proximadb_survivor_cache_hits", "proximadb_survivor_cache_misses",
              "proximadb_segment_invariants_cache_hits_total",
              "proximadb_segment_invariants_cache_misses_total"]:
        print(f"  Δ {k}: {fmt_delta(m1, m2, k)}", flush=True)

    # Summary
    print(f"\n{'='*60}")
    print(f"RESULT: cold p50={cold['p50']:.1f}ms vs hot p50={hot['p50']:.1f}ms"
          f" = {cold['p50']/hot['p50']:.1f}x speedup" if hot['p50'] > 0 else "")
    sh = m1.get("proximadb_survivor_cache_hits", 0) - m0.get("proximadb_survivor_cache_hits", 0)
    sm = m1.get("proximadb_survivor_cache_misses", 0) - m0.get("proximadb_survivor_cache_misses", 0)
    if sh + sm > 0:
        print(f"COLD survivor hit%: {sh/(sh+sm)*100:.1f}%")
    ih = (m1.get("proximadb_segment_invariants_cache_hits_total", 0)
          - m0.get("proximadb_segment_invariants_cache_hits_total", 0))
    im = (m1.get("proximadb_segment_invariants_cache_misses_total", 0)
          - m0.get("proximadb_segment_invariants_cache_misses_total", 0))
    if ih + im > 0:
        print(f"COLD invariants hit%: {ih/(ih+im)*100:.1f}%")

if __name__ == "__main__":
    main()
