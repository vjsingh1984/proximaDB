#!/usr/bin/env python3
"""SIFT REST warm-tier sweep (TD-COMPACT-1/TD-FLUSH-3/TD-CACHE evidence
harness): paced streaming 1M ingest -> settle -> recall gate (full-1M GT) ->
cold c=1/8/16 concurrency sweep on never-seen queries -> hot repeat pass ->
prometheus counter scrape. Server must be running on 127.0.0.1:5678 with the
bench config (see BENCHMARK_EVIDENCE.toml entries for exact env/protocol).
Requires: /Users/vijaysingh/sift1m (or set BASE/QUERY/GT paths)."""
from __future__ import annotations
import json, re, struct, subprocess, time, urllib.request
from concurrent.futures import ThreadPoolExecutor
from statistics import mean

SERVER = "http://127.0.0.1:5678"
BASE = "/Users/vijaysingh/sift1m/sift_base.fvecs"
QUERY = "/Users/vijaysingh/sift1m/sift_query.fvecs"
GT = "/Users/vijaysingh/sift1m/sift_groundtruth.ivecs"
BATCH = 1000


def post(path, body, timeout=900):
    req = urllib.request.Request(SERVER + path, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def stream_fvecs(path, start=0):
    with open(path, "rb") as f:
        d = struct.unpack("<i", f.read(4))[0]
        rec = 4 + 4 * d
        f.seek(start * rec)
        idx = start
        while True:
            buf = f.read(rec)
            if len(buf) < rec:
                break
            yield idx, list(struct.unpack_from("<%df" % d, buf, 4))
            idx += 1


def read_queries(start, count):
    out = []
    for idx, vec in stream_fvecs(QUERY, start):
        out.append(vec)
        if len(out) == count:
            break
    return out


def rss_mb():
    out = subprocess.run(["ps", "-axo", "rss,comm"], capture_output=True, text=True).stdout
    for line in out.splitlines():
        if "proximadb-server" in line:
            return int(line.split()[0]) // 1024
    return -1


def main():
    c = None
    for attempt in range(6):
        try:
            c = post("/api/v2/collections", {"name": "sift1m", "dimension": 128,
                                             "engine": "sst", "enable_proxima_record": True,
                                             "distance_metric": "euclidean"})
            break
        except Exception as e:
            print(f"create attempt {attempt}: {e}; retrying in 5s", flush=True)
            time.sleep(5)
    if c is None:
        raise RuntimeError("collection create failed after retries")
    cid = c.get("collection_id", "sift1m")
    print(f"collection -> {cid}", flush=True)

    t0 = time.time()
    batch = []
    for idx, vec in stream_fvecs(BASE):
        batch.append({"id": f"v{idx}", "vector": vec})
        if len(batch) == BATCH:
            for attempt in range(5):
                try:
                    post(f"/api/v2/collections/{cid}/records/batch", {"records": batch})
                    break
                except Exception as e:
                    print(f"  batch @{idx}: {e}; backoff", flush=True)
                    time.sleep(10 * (attempt + 1))
            batch = []
            time.sleep(0.05)
            if (idx + 1) % 200_000 == 0:
                r = rss_mb()
                print(f"  ingested {idx+1} ({time.time()-t0:.0f}s) rss={r}MB", flush=True)
                if r > 30000:
                    time.sleep(30)
    print(f"ingest done {time.time()-t0:.0f}s rss={rss_mb()}MB", flush=True)

    print("settling 180s (compaction)...", flush=True)
    time.sleep(180)
    segs = subprocess.run(["find", "/tmp/pdb-compact-bench/sst", "-name", "*.pax"],
                          capture_output=True, text=True).stdout.strip().splitlines()
    print(f"segments: {len(segs)}", flush=True)

    # RECALL gate (q0-199 vs full-1M GT)
    def read_gt(count):
        out = []
        for _, vec in [(i, v) for i, v in zip(range(count), _gt_iter())]:
            out.append(vec)
        return out
    def _gt_iter():
        with open(GT, "rb") as f:
            while True:
                hdr = f.read(4)
                if len(hdr) < 4:
                    break
                d = struct.unpack("<i", hdr)[0]
                buf = f.read(4 * d)
                yield list(struct.unpack("<%di" % d, buf))
    gt = read_gt(200)
    rq = read_queries(0, 200)
    recalls = []
    for j, vec in enumerate(rq):
        res = post(f"/api/v2/collections/{cid}/search", {"vector": vec, "top_k": 10}, timeout=300)
        got = {h["id"] for h in res.get("results", [])}
        truth = {f"v{x}" for x in gt[j][:10]}
        recalls.append(len(got & truth) / len(truth))
    print(f"RECALL@10 (q0-199) = {sum(recalls)/len(recalls):.4f}", flush=True)

    def one(vec):
        t = time.time()
        post(f"/api/v2/collections/{cid}/search", {"vector": vec, "top_k": 10}, timeout=300)
        return (time.time() - t) * 1000

    # COLD sweep: never-seen queries (indices 1000+)
    qpool = read_queries(1000, 520)
    for conc, n in [(1, 40), (8, 160), (16, 320)]:
        qs = qpool[:n]; qpool = qpool[n:]
        t0 = time.time()
        with ThreadPoolExecutor(conc) as ex:
            lats = sorted(ex.map(one, qs))
        wall = time.time() - t0
        print(f"COLD c={conc:>2} n={n}: QPS={n/wall:.1f} mean={mean(lats):.0f}ms "
              f"p50={lats[len(lats)//2]:.0f} p95={lats[int(len(lats)*0.95)]:.0f}", flush=True)

    # HOT pass: repeat a fixed set twice, measure the second round at c=16
    hot = read_queries(2000, 200)
    for vec in hot:
        one(vec)
    t0 = time.time()
    with ThreadPoolExecutor(16) as ex:
        lats = sorted(ex.map(one, hot))
    wall = time.time() - t0
    print(f"HOT  c=16 n=200: QPS={len(hot)/wall:.0f} mean={mean(lats):.1f}ms "
          f"p50={lats[len(lats)//2]:.1f} p95={lats[int(len(lats)*0.95)]:.1f}", flush=True)

    with urllib.request.urlopen(SERVER + "/metrics/prometheus", timeout=30) as r:
        txt = r.read().decode()
    for name in ["proximadb_queries_total", "proximadb_segment_invariants_cache_hits_total",
                 "proximadb_segment_invariants_cache_misses_total", "proximadb_survivor_cache_hits",
                 "proximadb_survivor_cache_misses", "proximadb_compactions_total"]:
        for line in txt.splitlines():
            if line.startswith(name + " "):
                print("  " + line, flush=True)


if __name__ == "__main__":
    main()
