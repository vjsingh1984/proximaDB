# ProximaDB gRPC Benchmark Results - Measured BGE-384 Baseline

**Date**: 2026-05-05  
**Build**: `release`  
**Protocol**: gRPC  
**Server**: local `proximadb-server` on `localhost:5679`  
**Collection Engine**: `SST`  
**Embedding Baseline**: `BAAI/bge-small-en-v1.5` (384D)  

## Scope

This report records the measured release-build baseline for ProximaDB vector insert and search using the gRPC path. It supersedes earlier draft numbers in this file that were based on smaller synthetic runs and did not represent the current benchmark harness.

## Method

- Explicit collection create/delete before each run
- Chunked vector ingest
- Search executed only after the collection became query-ready
- Random 384D vectors used to measure storage/search path overhead
- `top_k=10`
- `100` search queries per scale

## Results

| Scale | Insert Batch Size | Insert Duration | Insert Throughput | Insert Avg Latency | Search Duration | Search Throughput | Search Avg Latency |
|---|---:|---:|---:|---:|---:|---:|---:|
| 10K | 5,000 | 172 ms | 58,139.53 ops/sec | 17 us | 43 ms | 2,325.58 ops/sec | 430 us |
| 100K | 10,000 | 1,764 ms | 56,689.34 ops/sec | 17 us | 96 ms | 1,041.67 ops/sec | 960 us |
| 1M | 10,000 | 18,342 ms | 54,519.68 ops/sec | 18 us | 94 ms | 1,063.83 ops/sec | 940 us |

## Observations

- Insert throughput stayed relatively stable from 10K through 1M, dropping from about `58.1k` to `54.5k` ops/sec.
- Search throughput settled around `~1.0k-2.3k` queries/sec for `top_k=10`.
- Search readiness was immediate after ingest in all three runs once the benchmark switched to explicit collection lifecycle management.
- This benchmark is a storage/search-path baseline, not a recall benchmark. It does not yet validate ANN quality against ground truth.

## Limits

- Single-node, single-client benchmark only
- Random synthetic vectors, not real BGE text embeddings
- No concurrent client pressure
- No cross-database comparison in this report

## Recommended Next Steps

1. Run the same 10K/100K/1M matrix through the Python embedded harness with real BGE embeddings for Victor/codingagent-shaped workloads.
2. Add concurrent-client gRPC load to establish saturation behavior.
3. Add recall benchmarking against a fixed BGE-derived corpus so latency and quality are tracked together.
