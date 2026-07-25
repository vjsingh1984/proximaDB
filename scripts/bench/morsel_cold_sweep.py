#!/usr/bin/env python3
"""TD-SEARCH-2 S2 gate: cold concurrency sweep A/B (morsel degree 1 vs adaptive).

Phase 1 (once): ingest 1M SIFT, settle, snapshot the bed.
Phase 2 (per mode): launch server with PROXIMADB_SEARCH_MORSEL_DEGREE set,
run cold sweeps at c=1/4/8/16 using NEVER-REPEATED query slices, plus a
recall@10 check on a disjoint slice.

Usage: morsel_sweep.py <bin> <cfg> <datadir> <port> <mode:ingest|degree1|adaptive>
"""

import json
import os
import signal
import subprocess
import sys
import time
import urllib.request
import numpy as np

BIN, CFG, DATADIR, PORT, MODE = (
    sys.argv[1],
    sys.argv[2],
    sys.argv[3],
    int(sys.argv[4]),
    sys.argv[5],
)
BASE = f"http://127.0.0.1:{PORT}/api/v2"
SIFT = "/Users/vijaysingh/sift1m/sift_base.fvecs"
QUERY = "/Users/vijaysingh/sift1m/sift_query.fvecs"
N = 1_000_000
LOG = f"/tmp/morsel_{MODE}.log"

ENV = dict(
    os.environ,
    PROXIMADB_PAX_COALESCED_RABITQ="1",
    PROXIMADB_PAX_VECTOR_QUANT="rabitq",
    RUST_LOG="proximadb=info",
)
if MODE == "degree1":
    ENV["PROXIMADB_SEARCH_MORSEL_DEGREE"] = "1"
# adaptive: unset (default)


def read_fvecs(path, count, offset=0):
    with open(path, "rb") as f:
        d = np.frombuffer(f.read(4), dtype="<i4")[0]
        rec = 4 + 4 * d
        f.seek(rec * offset)
        raw = f.read(rec * count)
    a = np.frombuffer(raw, dtype="<u1").reshape(-1, rec)
    return a[:, 4:].copy().view("<f4").reshape(-1, d)


def post(path, body, timeout=300):
    req = urllib.request.Request(
        BASE + path,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def launch():
    lf = open(LOG, "ab")
    p = subprocess.Popen(
        [BIN, "-c", CFG, "-d", DATADIR, "-p", str(PORT)], stdout=lf, stderr=lf, env=ENV
    )
    deadline = time.time() + 180
    while time.time() < deadline:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{PORT}/health", timeout=2)
            time.sleep(3)  # let recovery/arming settle
            return p
        except Exception:
            if p.poll() is not None:
                sys.exit(f"server died at boot, see {LOG}")
            time.sleep(0.5)
    sys.exit("server not healthy")


def stop(p):
    p.send_signal(signal.SIGTERM)
    try:
        p.wait(timeout=30)
    except subprocess.TimeoutExpired:
        p.kill()
        p.wait(timeout=10)


def search(vec, k=10):
    t0 = time.time()
    r = post("/collections/1/search", {"vector": vec, "top_k": k})
    ms = (time.time() - t0) * 1000
    hits = [h.get("id") for h in r.get("results", r.get("hits", []))]
    return ms, hits


def sweep(queries, conc):
    import threading

    lat, errs = [], [0]
    lock = threading.Lock()
    idx = [0]

    def worker():
        while True:
            with lock:
                if idx[0] >= len(queries):
                    return
                q = queries[idx[0]]
                idx[0] += 1
            try:
                ms, _ = search(q.tolist())
                with lock:
                    lat.append(ms)
            except Exception:
                with lock:
                    errs[0] += 1

    t0 = time.time()
    ts = [__import__("threading").Thread(target=worker) for _ in range(conc)]
    [t.start() for t in ts]
    [t.join() for t in ts]
    wall = time.time() - t0
    a = np.array(lat)
    return {
        "n": len(a),
        "errs": errs[0],
        "qps": len(a) / wall,
        "mean": float(a.mean()) if len(a) else 0,
        "p95": float(np.percentile(a, 95)) if len(a) else 0,
    }


def main():
    if MODE == "ingest":
        p = launch()
        base = read_fvecs(SIFT, N)
        post(
            "/collections",
            {
                "name": "morsel_bed_1m",
                "dimension": 128,
                "engine": "sst",
                "enable_proxima_record": True,
                "distance_metric": "euclidean",
            },
        )
        t0 = time.time()
        for i in range(0, N, 1000):
            b = base[i : i + 1000]
            post(
                "/collections/1/records/batch",
                {
                    "records": [
                        {"id": f"v{i+j}", "vector": b[j].tolist()} for j in range(len(b))
                    ]
                },
            )
            time.sleep(0.02)
            if (i + 1000) % 200_000 == 0:
                print(f"  ingested {i+1000} ({time.time()-t0:.0f}s)", flush=True)
        print(f"ingest done {time.time()-t0:.0f}s; settling 30s", flush=True)
        time.sleep(30)
        stop(p)
        segs = subprocess.run(
            ["find", DATADIR, "-name", "*.pax"], capture_output=True, text=True
        ).stdout.strip()
        print(f"segments:\n{segs}")
        return

    # sweep mode: cold queries — each concurrency level gets a FRESH slice.
    p = launch()
    qs = read_fvecs(QUERY, 10_000)
    off = 0
    print(f"== mode {MODE} ==", flush=True)
    for conc, count in [(1, 60), (4, 120), (8, 200), (16, 320)]:
        r = sweep(qs[off : off + count], conc)
        off += count
        print(
            f"c={conc:>2}  n={r['n']:>4} errs={r['errs']}  qps={r['qps']:.1f}  "
            f"mean={r['mean']:.0f}ms  p95={r['p95']:.0f}ms",
            flush=True,
        )

    # recall@10 on a disjoint slice vs brute-force GT over the base
    base = read_fvecs(SIFT, N)
    rq = qs[off : off + 50]
    got = 0
    for q in rq:
        _, ids = search(q.tolist())
        d = ((base - q) ** 2).sum(axis=1)
        gt = {f"v{i}" for i in np.argpartition(d, 10)[:10]}
        got += len(gt & set(ids[:10]))
    print(f"recall@10 = {got / (len(rq) * 10):.4f}", flush=True)
    stop(p)
    print("== done ==")


if __name__ == "__main__":
    main()
