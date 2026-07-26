#!/usr/bin/env python3
"""nprobe tuning sweep: for each PROXIMADB_PAX_READ_COARSE_NPROBE value,
fresh server on the all-trained 1M bed -> recall@10 (100 q, full GT) +
GETs/q + bytes/q + latency (60 novel cold q). Goal: smallest nprobe with
recall >= 0.984.

Usage: nprobe_sweep.py <bin> <cfg> <datadir> <port> <cid> <nprobe1,nprobe2,...>
"""
import json, os, signal, socket, subprocess, sys, time, urllib.request
import numpy as np

BIN, CFG, DATADIR, PORT, CID = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5]
NPROBES = [int(x) for x in sys.argv[6].split(",")]
BASE = f"http://127.0.0.1:{PORT}"
SIFT = "/Users/vijaysingh/sift1m/sift_base.fvecs"
QUERY = "/Users/vijaysingh/sift1m/sift_query.fvecs"
LOG = "/tmp/nprobe_sweep.log"


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


def main():
    base = read_fvecs(SIFT, 1_000_000)
    qs = read_fvecs(QUERY, 10_000)
    print(f"{'nprobe':>6} {'GETs/q':>7} {'MB/q':>6} {'mean':>7} {'p95':>7} {'recall@10':>9} {'cells/q':>7}")
    off = 7000
    for nprobe in NPROBES:
        env = dict(os.environ,
                   PROXIMADB_INTERNAL_MUX_PORT="25611",
                   PROXIMADB_PAX_WRITE_A0_TRAIN="1",
                   PROXIMADB_PAX_READ_COARSE_PROBE="1",
                   PROXIMADB_PAX_READ_COARSE_NPROBE=str(nprobe),
                   PROXIMADB_L0_COMPACTION_ENABLED="1",
                   RUST_LOG="proximadb=warn")
        for _ in range(120):
            with socket.socket() as sck:
                try:
                    sck.bind(("127.0.0.1", PORT))
                    break
                except OSError:
                    time.sleep(1)
        else:
            sys.exit(f"port {PORT} never freed")
        lf = open(LOG, "ab")
        p = subprocess.Popen([BIN, "-c", CFG, "-d", DATADIR, "-p", str(PORT)],
                             stdout=lf, stderr=lf, env=env, cwd=DATADIR)
        deadline = time.time() + 120
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

        # cold GETs/latency on novel slice
        before = scrape()
        lats = []
        for q in qs[off:off + 60]:
            t = time.time()
            post(f"/api/v2/collections/{CID}/search", {"vector": q.tolist(), "top_k": 10})
            lats.append((time.time() - t) * 1000)
        after = scrape()
        gets = sum(v - before.get(k, 0) for k, v in after.items() if "object_store_gets_total" in k) / 60
        mb = sum(v - before.get(k, 0) for k, v in after.items() if "object_store_bytes_read_total" in k) / 60e6
        cells = sum(v - before.get(k, 0) for k, v in after.items() if "ivf_cells_probed" in k) / 60
        off += 60
        # recall on fixed slice (same for every nprobe -> comparable)
        got = 0
        for q in qs[9000:9100]:
            r = post(f"/api/v2/collections/{CID}/search", {"vector": q.tolist(), "top_k": 10})
            ids = [h.get("id") for h in r.get("results", r.get("hits", []))][:10]
            d = ((base - q) ** 2).sum(axis=1)
            gt = {f"v{i}" for i in np.argpartition(d, 10)[:10]}
            got += len(gt & set(ids))
        recall = got / 1000
        lats.sort()
        print(f"{nprobe:>6} {gets:>7.1f} {mb:>6.1f} {sum(lats)/60:>6.0f}ms {lats[57]:>6.0f}ms {recall:>9.4f} {cells:>7.1f}", flush=True)
        p.send_signal(signal.SIGTERM)
        try:
            p.wait(timeout=150)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait(timeout=10)


if __name__ == "__main__":
    main()
