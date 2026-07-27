#!/usr/bin/env python3
"""TD-CACHE-3 sale-gate: two-tenant fairness at cloud (Azurite) latency.

Tenant A (enterprise, pinned floor) vs tenant B (free, churner).
Budget sized so A's SQ8 working set ~= its pinned floor; B's churn
overwhelms the shared pool. PASS = A's hit-rate/latency hold during churn.

Usage: two_tenant_bed.py <bin> <cfg> <datadir> <port>
"""
import json, os, signal, socket, subprocess, sys, time, urllib.request
import numpy as np

BIN, CFG, DATADIR, PORT = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
BASE = f"http://127.0.0.1:{PORT}"
SIFT = "/Users/vijaysingh/sift1m/sift_base.fvecs"
QUERY = "/Users/vijaysingh/sift1m/sift_query.fvecs"
LOG = "/tmp/tt_bench_server.log"
NA, NB = 100_000, 150_000

ENV = dict(os.environ,
           PROXIMADB_AZURE_EMULATOR="1",
           AZURE_STORAGE_ACCOUNT="devstoreaccount1",
           AZURE_STORAGE_ACCOUNT_NAME="devstoreaccount1",
           AZURE_STORAGE_USE_EMULATOR="true",
           AZURITE_BLOB_STORAGE_URL="http://127.0.0.1:11000",
           PROXIMADB_TENANT_HEADER_TRUST="open",
           PROXIMADB_CACHE_TIERS_PATH="/tmp/tt-cache-tiers.json",
           PROXIMADB_SURVIVOR_PIN_RESERVE_FRAC="0.4",
           PROXIMADB_SURVIVOR_CACHE_BUDGET_MB="32",
           PROXIMADB_INTERNAL_MUX_PORT="25711",
           PROXIMADB_L0_COMPACTION_ENABLED="1",
           PROXIMADB_PAX_WRITE_A0_TRAIN="1",
           RUST_LOG="proximadb=info")


def read_fvecs(path, count, offset=0):
    with open(path, "rb") as f:
        d = np.frombuffer(f.read(4), dtype="<i4")[0]
        rec = 4 + 4 * d
        f.seek(rec * offset)
        raw = f.read(rec * count)
    a = np.frombuffer(raw, dtype="<u1").reshape(-1, rec)
    return a[:, 4:].copy().view("<f4").reshape(-1, d)


def post(path, body, tenant, tier, timeout=300, retries=5):
    last = None
    for a in range(retries):
        req = urllib.request.Request(
            BASE + path, data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json",
                     "X-Tenant-ID": tenant, "X-Tenant-Tier": tier})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            last = e
            if e.code >= 500:
                time.sleep(2 * (a + 1))
                continue
            raise
    raise last


def scrape():
    with urllib.request.urlopen(BASE + "/metrics/prometheus", timeout=30) as r:
        txt = r.read().decode()
    out = {}
    for line in txt.splitlines():
        if not line.startswith("#") and " " in line:
            n, _, v = line.rpartition(" ")
            try:
                out[n] = float(v)
            except ValueError:
                pass
    return out


def tenant_gauges(m, tenant):
    g = {}
    for k, v in m.items():
        if f'tenant_id="{tenant}"' in k and "survivor" in k:
            name = k.split("{")[0].replace("proximadb_survivor_cache_tenant_", "")
            g[name] = v
    return g


def qslice(qs, off, n, tenant, tier, cid, conc=1):
    lats, miss0 = [], None
    for q in qs[off:off + n]:
        t = time.time()
        post(f"/api/v2/collections/{cid}/search", {"vector": q.tolist(), "top_k": 10},
             tenant, tier)
        lats.append((time.time() - t) * 1000)
    lats.sort()
    return {"n": n, "mean": sum(lats) / n, "p95": lats[int(n * 0.95)]}


def main():
    for _ in range(120):
        with socket.socket() as sck:
            try:
                sck.bind(("127.0.0.1", PORT)); break
            except OSError:
                time.sleep(1)
    else:
        sys.exit("port never freed")
    lf = open(LOG, "wb")
    p = subprocess.Popen([BIN, "-c", CFG, "-d", DATADIR, "-p", str(PORT)],
                         stdout=lf, stderr=lf, env=ENV, cwd=DATADIR)
    deadline = time.time() + 180
    while time.time() < deadline:
        try:
            urllib.request.urlopen(BASE + "/health", timeout=2)
            time.sleep(3)
            if p.poll() is not None:
                sys.exit("foreign server trap")
            break
        except Exception:
            if p.poll() is not None:
                sys.exit(f"server died, see {LOG}")
            time.sleep(0.5)

    base = read_fvecs(SIFT, NA + NB)
    qs = read_fvecs(QUERY, 10_000)

    # Phase 1: ingest per tenant
    ca = post("/api/v2/collections", {"name": "colla", "dimension": 128, "engine": "sst",
              "enable_proxima_record": True, "distance_metric": "euclidean"},
              "tenant-a", "enterprise").get("collection_id")
    cb = post("/api/v2/collections", {"name": "collb", "dimension": 128, "engine": "sst",
              "enable_proxima_record": True, "distance_metric": "euclidean"},
              "tenant-b", "free").get("collection_id")
    print(f"collections: A={ca} B={cb}", flush=True)
    t0 = time.time()
    for i in range(0, NA, 1000):
        b = base[i:i + 1000]
        post(f"/api/v2/collections/{ca}/records/batch",
             {"records": [{"id": f"a{i+j}", "vector": b[j].tolist()} for j in range(len(b))]},
             "tenant-a", "enterprise")
    for i in range(0, NB, 1000):
        b = base[NA + i:NA + i + 1000]
        post(f"/api/v2/collections/{cb}/records/batch",
             {"records": [{"id": f"b{i+j}", "vector": b[j].tolist()} for j in range(len(b))]},
             "tenant-b", "free")
    print(f"ingest done {time.time()-t0:.0f}s; settle 45s (flush+train to azurite)", flush=True)
    time.sleep(45)

    # Phase 2: warm A's hot working set (2 rounds of a fixed slice)
    for _ in range(2):
        qslice(qs, 0, 50, "tenant-a", "enterprise", ca)
    m = scrape()
    print(f"A gauges post-warm: {tenant_gauges(m, 'tenant-a')}", flush=True)

    # Phase 3: A baseline (hot repeat)
    a_base = qslice(qs, 0, 50, "tenant-a", "enterprise", ca)
    m0 = scrape()
    print(f"A baseline: mean={a_base['mean']:.0f}ms p95={a_base['p95']:.0f}ms", flush=True)

    # Phase 4: B churns (novel queries, sequential flood) interleaved with A's repeats
    import threading
    stop = [False]
    def churn():
        off = 1000
        while not stop[0]:
            try:
                qslice(qs, off, 20, "tenant-b", "free", cb)
            except Exception:
                time.sleep(1)
            off += 20
    th = threading.Thread(target=churn); th.start()
    time.sleep(10)  # churn pressure builds
    a_during = qslice(qs, 0, 50, "tenant-a", "enterprise", ca)
    stop[0] = True; th.join()
    m1 = scrape()
    ga0, ga1 = tenant_gauges(m0, "tenant-a"), tenant_gauges(m1, "tenant-a")
    gb1 = tenant_gauges(m1, "tenant-b")
    a_miss_delta = ga1.get("misses", 0) - ga0.get("misses", 0)
    a_hit_delta = ga1.get("hits", 0) - ga0.get("hits", 0)
    print(f"A during churn: mean={a_during['mean']:.0f}ms p95={a_during['p95']:.0f}ms", flush=True)
    print(f"A hit/miss delta during churn: hits+{a_hit_delta:.0f} misses+{a_miss_delta:.0f}", flush=True)
    print(f"A gauges: {ga1}", flush=True)
    print(f"B gauges: {gb1}", flush=True)
    ratio = a_during["p95"] / max(a_base["p95"], 1)
    hold = a_miss_delta <= a_hit_delta * 0.05 and ratio <= 1.5
    print(f"VERDICT: p95 ratio {ratio:.2f}x, miss-rate {'HELD' if hold else 'BROKE'}", flush=True)

    p.send_signal(signal.SIGTERM)
    try:
        p.wait(timeout=120)
    except subprocess.TimeoutExpired:
        p.kill()
    print("done", flush=True)


if __name__ == "__main__":
    main()
