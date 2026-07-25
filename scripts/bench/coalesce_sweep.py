#!/usr/bin/env python3
"""Coalescing-policy sweep on the existing 1M bed (local SSD): does per-backend
tuning of (max_gap, max_range) move bytes/query, read-ops/query, or latency?

Fresh server per profile (clears RAM caches); 50 NOVEL cold queries per
profile from disjoint slices; scrapes the object-store counters (on file://
these count ranged disk reads).

Usage: coalesce_sweep.py <bin> <cfg> <datadir> <port> <cid>
"""
import json, os, signal, socket, subprocess, sys, time, urllib.request
import numpy as np

BIN, CFG, DATADIR, PORT, CID = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5]
BASE = f"http://127.0.0.1:{PORT}"
QUERY = "/Users/vijaysingh/sift1m/sift_query.fvecs"
LOG = "/tmp/coalesce_sweep.log"

PROFILES = [
    ("LOCAL-default(256K/1M)", None, None),
    ("ultra-tight(4K/64K)", 4 * 1024, 64 * 1024),
    ("tight(16K/256K)", 16 * 1024, 256 * 1024),
    ("cloud(1M/4M)", 1024 * 1024, 4 * 1024 * 1024),
    ("mega(4M/8M)", 4 * 1024 * 1024, 8 * 1024 * 1024),
]


def read_fvecs(path, count):
    with open(path, "rb") as f:
        d = np.frombuffer(f.read(4), dtype="<i4")[0]
        rec = 4 + 4 * d
        f.seek(0)
        raw = f.read(rec * count)
    a = np.frombuffer(raw, dtype="<u1").reshape(-1, rec)
    return a[:, 4:].copy().view("<f4").reshape(-1, d)


def scrape():
    with urllib.request.urlopen(BASE + "/metrics/prometheus", timeout=30) as r:
        txt = r.read().decode()
    out = {}
    for line in txt.splitlines():
        if line.startswith("#") or " " not in line:
            continue
        name, _, val = line.rpartition(" ")
        try:
            out[name] = float(val)
        except ValueError:
            pass
    return out


def main():
    qs = read_fvecs(QUERY, 10_000)
    off = 3000  # disjoint from every earlier phase
    print(f"{'profile':<24} {'reads/q':>8} {'MB/q':>8} {'mean ms':>8} {'p95 ms':>8}")
    for name, gap, rng in PROFILES:
        env = dict(os.environ,
                   PROXIMADB_PAX_COALESCED_RABITQ="1",
                   PROXIMADB_PAX_VECTOR_QUANT="rabitq",
                   PROXIMADB_COUNT_FS_IO="1",
                   PROXIMADB_INTERNAL_MUX_PORT="15876",
                   RUST_LOG="proximadb=warn")
        if gap:
            env["PROXIMADB_PAX_COALESCE_GAP"] = str(gap)
            env["PROXIMADB_PAX_COALESCE_RANGE"] = str(rng)
        # wait port free
        for _ in range(60):
            with socket.socket() as sck:
                try:
                    sck.bind(("127.0.0.1", PORT))
                    break
                except OSError:
                    time.sleep(1)
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

        before = scrape()
        lats = []
        for q in qs[off:off + 50]:
            body = json.dumps({"vector": q.tolist(), "top_k": 10}).encode()
            req = urllib.request.Request(f"{BASE}/api/v2/collections/{CID}/search",
                                         data=body, headers={"Content-Type": "application/json"})
            t0 = time.time()
            with urllib.request.urlopen(req, timeout=120) as r:
                r.read()
            lats.append((time.time() - t0) * 1000)
        after = scrape()
        off += 50

        def delta(sub):
            return sum(v - before.get(k, 0) for k, v in after.items() if sub in k)
        reads = delta("object_store_gets_total")
        mb = delta("object_store_bytes_read_total") / 1e6
        lats.sort()
        print(f"{name:<24} {reads/50:>8.1f} {mb/50:>8.1f} "
              f"{sum(lats)/len(lats):>8.1f} {lats[int(len(lats)*0.95)]:>8.1f}", flush=True)
        p.send_signal(signal.SIGTERM)
        try:
            p.wait(timeout=30)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait(timeout=10)


if __name__ == "__main__":
    main()
