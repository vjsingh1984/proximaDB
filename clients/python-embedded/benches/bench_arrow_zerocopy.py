#!/usr/bin/env python3
"""Co-design Pillar C microbench: Arrow IPC path vs zero-copy Arrow C Data Interface.

Measures end-to-end embedded insert latency for the same data via:
  (A) insert_arrow_ipc   — Python serializes the Table to Arrow IPC bytes, Rust
                           deserializes them (the legacy path / baseline);
  (B) insert_arrow_batches — pyarrow RecordBatches cross the FFI boundary zero-copy
                           via the Arrow C Data Interface (no IPC serialize/deserialize).

Both paths share the same downstream Arrow->ProximaRecord conversion + insert, so the
delta isolates the boundary cost the zero-copy change removes. Also reports the IPC
payload size (bytes copied at the boundary for path A; ~0 for path B).

Run after `maturin develop` in the project venv:
    python clients/python-embedded/benches/bench_arrow_zerocopy.py
"""

import statistics
import tempfile
import time

import numpy as np
import pyarrow as pa

import proximadb_embedded as pdb
from proximadb_embedded import _arrow_source_to_ipc_bytes

N = 5000
DIM = 128
REPEATS = 9
WARMUP = 2


def build_table(n: int, dim: int) -> pa.Table:
    vecs = np.random.rand(n, dim).astype(np.float32)
    return pa.table(
        {
            "id": [f"v{i}" for i in range(n)],
            "vector": pa.array(list(vecs), type=pa.list_(pa.float32())),
            "tenant_id": ["bench"] * n,
        }
    )


def time_path(make_db, do_insert) -> float:
    samples = []
    for i in range(REPEATS + WARMUP):
        with tempfile.TemporaryDirectory() as tmp:
            db = make_db(tmp)
            db.create_collection("bench", dimension=DIM)
            t0 = time.perf_counter()
            do_insert(db)
            elapsed = (time.perf_counter() - t0) * 1000.0
            if i >= WARMUP:  # discard warmup iterations
                samples.append(elapsed)
    return statistics.median(samples)


def main() -> None:
    table = build_table(N, DIM)
    ipc_bytes = _arrow_source_to_ipc_bytes(table)
    batches = table.to_batches()

    ipc_p50 = time_path(
        lambda tmp: pdb.ProximaDB(data_dirs=tmp),
        lambda db: db.insert_arrow_ipc("bench", ipc_bytes, "insert", None),
    )
    zc_p50 = time_path(
        lambda tmp: pdb.ProximaDB(data_dirs=tmp),
        lambda db: db.insert_arrow_batches("bench", batches, "insert", None),
    )

    speedup = ipc_p50 / zc_p50 if zc_p50 else float("nan")
    print(f"records={N} dim={DIM} repeats={REPEATS}")
    print(f"IPC payload bytes copied at boundary (path A): {len(ipc_bytes):,}")
    print(f"(A) insert_arrow_ipc       p50 = {ipc_p50:8.2f} ms")
    print(f"(B) insert_arrow_batches   p50 = {zc_p50:8.2f} ms   (zero-copy C Data Interface)")
    print(f"speedup (A/B)              = {speedup:6.2f}x   boundary bytes copied B ~= 0")


if __name__ == "__main__":
    main()
