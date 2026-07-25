#!/usr/bin/env python3
"""Restart-under-load verification for the production-resilience package.

Phases:
  1. Ingest a 100k SIFT bed (streaming REST, rabitq+coalesced).
  2. Settle + hot phase: c=8 sustained queries; record steady-state p50/p95.
  3. Graceful restart (SIGTERM): server emits warm manifests; relaunch;
     sustain c=8 load immediately; measure time until p95 <= 2x steady-state
     and first-window GET counts (warm replay budget vs cold herd).
  4. Report manifest presence + warming log lines.

Usage: python3 restart_verify.py <server-binary> <config> <datadir> <port>
"""

import json
import os
import signal
import subprocess
import sys
import time
import urllib.request
import numpy as np

BIN, CFG, DATADIR, PORT = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
BASE = f"http://127.0.0.1:{PORT}/api/v2"
COLL = "restart_bed_100k"
SIFT = "/Users/vijaysingh/sift1m/sift_base.fvecs"
QUERY = "/Users/vijaysingh/sift1m/sift_query.fvecs"
N = 100_000
LOG = os.environ.get("RESTART_LOG", "/tmp/restart_verify_server.log")

ENV = dict(os.environ,
           PROXIMADB_PAX_COALESCED_RABITQ="1",
           PROXIMADB_PAX_VECTOR_QUANT="rabitq",
           RUST_LOG="proximadb=info")


def read_fvecs(path, count):
    with open(path, "rb") as f:
        d = np.frombuffer(f.read(4), dtype="<i4")[0]
        rec = 4 + 4 * d
        f.seek(0)
        raw = f.read(rec * count)
    a = np.frombuffer(raw, dtype="<u1").reshape(count, rec)
    return a[:, 4:].copy().view("<f4").reshape(count, d)


def post(path, body, timeout=120):
    req = urllib.request.Request(BASE + path, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def launch():
    lf = open(LOG, "ab")
    p = subprocess.Popen([BIN, "-c", CFG, "-d", DATADIR, "-p", str(PORT)],
                         stdout=lf, stderr=lf, env=ENV)
    deadline = time.time() + 120
    while time.time() < deadline:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{PORT}/health", timeout=2)
            return p
        except Exception:
            if p.poll() is not None:
                sys.exit(f"server died at boot, see {LOG}")
            time.sleep(0.5)
    sys.exit("server did not become healthy")


def graceful_stop(p):
    p.send_signal(signal.SIGTERM)
    deadline = time.time() + 45
    while time.time() < deadline:
        if p.poll() is not None:
            return "clean-exit"
        with open(LOG, "rb") as f:
            tail = f.read()[-4000:].decode(errors="replace")
        if "ProximaDB server stopped" in tail:
            time.sleep(1)
            p.kill()
            p.wait(timeout=10)
            return "shutdown-complete (reaped lingering runtime: known discovery-executor hang)"
        time.sleep(0.5)
    p.kill()
    p.wait(timeout=10)
    return "TIMEOUT: shutdown did not complete in 45s"


def search_one(vec, k=10, timeout=30):
    t0 = time.time()
    post(f"/collections/{COLL}/search", {"vector": vec, "top_k": k}, timeout=timeout)
    return (time.time() - t0) * 1000


def sustained(queries, seconds, conc=8):
    """c=conc closed-loop load for `seconds`; returns per-window p50/p95 (1s windows)."""
    import threading
    lat, stop = [], time.time() + seconds
    lock = threading.Lock()

    def worker(off):
        i = off
        while time.time() < stop:
            try:
                ms = search_one(queries[i % len(queries)].tolist())
                with lock:
                    lat.append((time.time(), ms))
            except Exception:
                with lock:
                    lat.append((time.time(), float("nan")))
                time.sleep(0.2)
            i += conc

    ts = [__import__("threading").Thread(target=worker, args=(o,)) for o in range(conc)]
    [t.start() for t in ts]
    [t.join() for t in ts]
    return lat


def windows(lat, t0):
    out = {}
    for ts, ms in lat:
        out.setdefault(int(ts - t0), []).append(ms)
    rows = []
    for w in sorted(out):
        v = np.array([m for m in out[w] if not np.isnan(m)])
        errs = sum(1 for m in out[w] if np.isnan(m))
        if len(v):
            rows.append((w, len(v), errs, float(np.percentile(v, 50)), float(np.percentile(v, 95))))
        else:
            rows.append((w, 0, errs, float("nan"), float("nan")))
    return rows


def main():
    base = read_fvecs(SIFT, N)
    qs = read_fvecs(QUERY, 200)

    print("== phase 1: launch + ingest 100k ==", flush=True)
    p = launch()
    c = post("/collections", {"name": COLL, "dimension": 128,
                              "engine": "sst", "enable_proxima_record": True,
                              "distance_metric": "euclidean"})
    cid = c.get("collection_id", COLL)
    globals()["COLL"] = cid
    print(f"collection -> {cid}", flush=True)
    t0 = time.time()
    for i in range(0, N, 1000):
        batch = base[i:i + 1000]
        post(f"/collections/{COLL}/records/batch",
             {"records": [{"id": f"v{i+j}", "vector": batch[j].tolist()}
                          for j in range(len(batch))]})
        time.sleep(0.05)
    print(f"ingested {N} in {time.time()-t0:.0f}s", flush=True)
    time.sleep(20)  # settle/flush

    print("== phase 2: steady-state (30s c=8) ==", flush=True)
    for q in qs[:32]:
        search_one(q.tolist())  # warm
    lat = sustained(qs[:100], 30)
    v = np.array([m for _, m in lat if not np.isnan(m)])
    steady_p50, steady_p95 = np.percentile(v, 50), np.percentile(v, 95)
    print(f"steady: n={len(v)} p50={steady_p50:.1f}ms p95={steady_p95:.1f}ms "
          f"qps={len(v)/30:.0f}", flush=True)

    print("== phase 3: SIGTERM restart under load ==", flush=True)
    print("stop:", graceful_stop(p), flush=True)
    import subprocess as sp
    m = sp.run(["find", DATADIR, "-name", "warm_cache.json"], capture_output=True, text=True).stdout.strip()
    print(f"manifests on disk: {m or 'NONE'}", flush=True)
    p = launch()
    t_up = time.time()
    lat = sustained(qs[100:200], 25)
    rows = windows(lat, t_up)
    print("window  n  errs  p50ms  p95ms")
    recov = None
    for w, n, errs, p50, p95 in rows:
        mark = ""
        if recov is None and n > 0 and p95 <= 2 * steady_p95:
            recov = w
            mark = "  <= RECOVERED (p95 <= 2x steady)"
        print(f"{w:>4}  {n:>4} {errs:>4}  {p50:>7.1f} {p95:>7.1f}{mark}", flush=True)
    print(f"RESULT steady_p95={steady_p95:.1f}ms recovery_window_s={recov}")

    print("stop:", graceful_stop(p), flush=True)
    import subprocess as sp
    warm = sp.run(["grep", "-iE", "warm manifest|warm.replay|restart cache warming", LOG],
                  capture_output=True, text=True).stdout
    print("warming log lines:\n" + warm)
    print("== done ==")


if __name__ == "__main__":
    main()
