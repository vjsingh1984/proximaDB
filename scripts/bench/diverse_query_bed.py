#!/usr/bin/env python3
"""Hot-tier value under REALISTIC (diverse) query load.

The '0-GET hot' claim was measured with identical repeats, which the
query-result cache (vector_hash-keyed) short-circuits above the survivor
cache. Real vector search never repeats a query. This bed streams UNIQUE
queries (every one misses the result cache -> falls through to the survivor
cache) and measures whether GETs/query DECLINES across the stream as
overlapping hot ranges become resident. Convergence toward low GETs = the
hot tier delivers for diverse workloads; flat = the '0-GET' number was a
result-cache artifact.

Usage: diverse_query_bed.py <bin> <cfg> <datadir> <port>
"""
import json, os, signal, socket, subprocess, sys, time, urllib.request
import numpy as np

BIN, CFG, DATADIR, PORT = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
BASE = f"http://127.0.0.1:{PORT}"
SIFT = "/Users/vijaysingh/sift1m/sift_base.fvecs"
QUERY = "/Users/vijaysingh/sift1m/sift_query.fvecs"
LOG = "/tmp/diverse_bed_server.log"
N = 200_000
STREAM = 1200

ENV = dict(os.environ,
           PROXIMADB_INTERNAL_MUX_PORT="25811",
           PROXIMADB_PAX_WRITE_A0_TRAIN="1",
           PROXIMADB_PAX_READ_COARSE_PROBE="1",
           PROXIMADB_L0_COMPACTION_ENABLED="1",
           PROXIMADB_SURVIVOR_CACHE_BUDGET_MB="256",
           RUST_LOG="proximadb=warn")


def read_fvecs(path, count):
    with open(path, "rb") as f:
        d = np.frombuffer(f.read(4), dtype="<i4")[0]
        rec = 4 + 4 * d
        f.seek(0)
        raw = f.read(rec * count)
    a = np.frombuffer(raw, dtype="<u1").reshape(-1, rec)
    return a[:, 4:].copy().view("<f4").reshape(-1, d)


def post(path, body, timeout=300, retries=4):
    last = None
    for a in range(retries):
        req = urllib.request.Request(BASE + path, data=json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            last = e
            if e.code >= 500:
                time.sleep(2 * (a + 1)); continue
            raise
    raise last


def gets():
    with urllib.request.urlopen(BASE + "/metrics/prometheus", timeout=30) as r:
        txt = r.read().decode()
    g = s = h = m = 0.0
    for line in txt.splitlines():
        if line.startswith("proximadb_object_store_gets_total") and " " in line:
            g += float(line.rsplit(" ", 1)[1])
        elif line.startswith("proximadb_survivor_cache_hits ") :
            h = float(line.rsplit(" ", 1)[1])
        elif line.startswith("proximadb_survivor_cache_misses "):
            m = float(line.rsplit(" ", 1)[1])
    return g, h, m


def main():
    for _ in range(120):
        with socket.socket() as sck:
            try: sck.bind(("127.0.0.1", PORT)); break
            except OSError: time.sleep(1)
    else: sys.exit("port never freed")
    lf = open(LOG, "wb")
    p = subprocess.Popen([BIN, "-c", CFG, "-d", DATADIR, "-p", str(PORT)],
                         stdout=lf, stderr=lf, env=ENV, cwd=DATADIR)
    deadline = time.time() + 180
    while time.time() < deadline:
        try:
            urllib.request.urlopen(BASE + "/health", timeout=2); time.sleep(3)
            if p.poll() is not None: sys.exit("foreign trap")
            break
        except Exception:
            if p.poll() is not None: sys.exit(f"died, see {LOG}")
            time.sleep(0.5)

    base = read_fvecs(SIFT, N)
    qs = read_fvecs(QUERY, 10_000)
    cid = post("/api/v2/collections", {"name": "hot", "dimension": 128, "engine": "sst",
               "enable_proxima_record": True, "distance_metric": "euclidean"}).get("collection_id")
    print(f"collection -> {cid}", flush=True)
    t0 = time.time()
    for i in range(0, N, 1000):
        b = base[i:i+1000]
        post(f"/api/v2/collections/{cid}/records/batch",
             {"records": [{"id": f"v{i+j}", "vector": b[j].tolist()} for j in range(len(b))]})
    print(f"ingest {time.time()-t0:.0f}s; settle+train 60s", flush=True)
    time.sleep(60)

    # Stream UNIQUE queries; report GETs/query per 100-window + survivor hit-rate.
    print(f"\nstreaming {STREAM} UNIQUE queries (each misses result cache):", flush=True)
    print(f"{'window':>12} {'GETs/q':>8} {'surv_hit%':>9}")
    g_prev, h_prev, m_prev = gets()
    for w in range(0, STREAM, 100):
        for q in qs[w:w+100]:
            post(f"/api/v2/collections/{cid}/search", {"vector": q.tolist(), "top_k": 10})
        g, h, m = gets()
        dg, dh, dm = g - g_prev, h - h_prev, m - m_prev
        hr = 100.0 * dh / max(dh + dm, 1)
        print(f"{w:>5}-{w+100:<6} {dg/100:>8.1f} {hr:>8.1f}%", flush=True)
        g_prev, h_prev, m_prev = g, h, m

    p.send_signal(signal.SIGTERM)
    try: p.wait(timeout=120)
    except subprocess.TimeoutExpired: p.kill()
    print("done", flush=True)


if __name__ == "__main__":
    main()
