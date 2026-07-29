#!/usr/bin/env python3
"""Adjudicate the 0.988-vs-0.372 recall discrepancy: fresh 1M bed (their toml,
BARE env — no PROXIMADB_* overrides), recall measured pre-restart and
post-restart with the full-GT protocol.

Usage: recall_adjudicate.py <bin> <cfg> <datadir> <port>
"""
import json, os, signal, subprocess, sys, time, urllib.request
import numpy as np

BIN, CFG, DATADIR, PORT = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
BASE = f"http://127.0.0.1:{PORT}/api/v2"
SIFT = "/Users/vijaysingh/sift1m/sift_base.fvecs"
QUERY = "/Users/vijaysingh/sift1m/sift_query.fvecs"
N = 1_000_000
LOG = "/tmp/recall_adjudicate.log"
ENV = dict(os.environ, RUST_LOG="proximadb=info")  # BARE: no PROXIMADB_* overrides

def read_fvecs(path, count, offset=0):
    with open(path, "rb") as f:
        d = np.frombuffer(f.read(4), dtype="<i4")[0]
        rec = 4 + 4 * d
        f.seek(rec * offset)
        raw = f.read(rec * count)
    a = np.frombuffer(raw, dtype="<u1").reshape(-1, rec)
    return a[:, 4:].copy().view("<f4").reshape(-1, d)

def post(path, body, timeout=300, retries=5):
    last = None
    for attempt in range(retries):
        req = urllib.request.Request(BASE + path, data=json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            last = e
            if e.code >= 500:
                # transient (e.g. write backpressure) — back off and retry
                time.sleep(2 * (attempt + 1))
                continue
            raise
    raise last

def launch():
    # Wait for OUR port to be free (a SIGTERM'd predecessor lingers a few
    # seconds); a foreign owner that never releases = hard abort.
    import socket
    for _ in range(120):
        with socket.socket() as sck:
            try:
                sck.bind(("127.0.0.1", PORT))
                break
            except OSError:
                time.sleep(1)
    else:
        sys.exit(f"port {PORT} never freed — foreign owner?")
    lf = open(LOG, "ab")
    # cwd = OUR datadir: some components write cwd-relative state (./data);
    # sharing the launch cwd with other servers cross-contaminates catalogs.
    p = subprocess.Popen([BIN, "-c", CFG, "-d", DATADIR, "-p", str(PORT)],
                         stdout=lf, stderr=lf, env=ENV, cwd=DATADIR)
    deadline = time.time() + 180
    while time.time() < deadline:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{PORT}/health", timeout=2)
            time.sleep(3)
            if p.poll() is not None:
                sys.exit("FOREIGN SERVER TRAP: health answered but MY server "
                         f"process exited (bind conflict?) — see {LOG}")
            return p
        except Exception:
            if p.poll() is not None:
                sys.exit(f"server died, see {LOG}")
            time.sleep(0.5)
    sys.exit("server not healthy")

def stop(p, sig=signal.SIGTERM):
    p.send_signal(sig)
    try:
        p.wait(timeout=45)
    except subprocess.TimeoutExpired:
        p.kill(); p.wait(timeout=10)

def recall(base, qs, label, cid="1"):
    got, n = 0, len(qs)
    for q in qs:
        r = post(f"/collections/{cid}/search", {"vector": q.tolist(), "top_k": 10})
        ids = [h.get("id") for h in r.get("results", r.get("hits", []))][:10]
        d = ((base - q) ** 2).sum(axis=1)
        gt = {f"v{i}" for i in np.argpartition(d, 10)[:10]}
        got += len(gt & set(ids))
    v = got / (n * 10)
    print(f"recall@10 [{label}] = {v:.4f}", flush=True)
    return v

def main():
    base = read_fvecs(SIFT, N)
    qs = read_fvecs(QUERY, 10_000)
    p = launch()
    c = post("/collections", {"name": f"adjud_{int(time.time())}", "dimension": 128,
                              "engine": "sst", "enable_proxima_record": True,
                              "distance_metric": "euclidean"})
    cid = c.get("collection_id") or "1"
    print(f"collection -> {cid}", flush=True)
    t0 = time.time()
    for i in range(0, N, 1000):
        b = base[i:i+1000]
        post(f"/collections/{cid}/records/batch",
             {"records": [{"id": f"v{i+j}", "vector": b[j].tolist()} for j in range(len(b))]})
        time.sleep(0.02)
        if (i + 1000) % 250_000 == 0:
            print(f"  ingested {i+1000} ({time.time()-t0:.0f}s)", flush=True)
    print(f"ingest done {time.time()-t0:.0f}s; settling 30s", flush=True)
    time.sleep(30)
    import subprocess as sp
    segs = sp.run(["find", DATADIR, "-name", "*.pax"], capture_output=True, text=True).stdout
    print(f"segments pre-restart:\n{segs}", flush=True)
    r1 = recall(base, qs[2000:2050], "PRE-restart, live server", cid)
    stop(p)
    p = launch()
    segs = sp.run(["find", DATADIR, "-name", "*.pax"], capture_output=True, text=True).stdout
    print(f"segments post-restart:\n{segs}", flush=True)
    r2 = recall(base, qs[2050:2100], "POST-restart (SIGTERM)", cid)
    stop(p)
    print(f"\nVERDICT: pre={r1:.4f} post={r2:.4f}")

if __name__ == "__main__":
    main()
