# ProximaDB Consolidated Embedded Benchmark

## Vector Storage Engines

| Engine | Insert (ms) | Throughput (/s) | Search (ms) | p95 Search |
| --- | ---: | ---: | ---: | ---: |
| SST | 122.97 | 8,132 | 0.512 | 0.697 |
| HELIX | 122.87 | 8,138 | 0.524 | 0.695 |
| VIPER | 120.57 | 8,294 | 0.529 | 0.699 |
| NOVA | 122.22 | 8,182 | 0.533 | 0.713 |
| SWIFT | 122.57 | 8,158 | 0.521 | 0.662 |
| RAPTOR | 122.77 | 8,145 | 0.512 | 0.583 |

## Graph Database (ORION)

| Operation | Time (ms) | Throughput (/s) | p95 (ms) |
| --- | ---: | ---: | ---: |
| insert_nodes | 28.21 | 35,446 | - |
| insert_edges | 15.38 | 315,233 | - |
| 1hop_traversal | 0.06 | 17,796 | 0.058 |
| insert_nodes | 99.67 | 100,328 | - |
| insert_edges | 163.55 | 298,072 | - |
| 1hop_traversal | 1.15 | 868 | 1.229 |
| insert_nodes | 0.67 | 1,490,127 | - |
| insert_edges | 2.88 | 1,695,593 | - |
| 1hop_traversal | 0.00 | - | 0.000 |
| insert_nodes | 6.87 | 1,455,587 | - |
| insert_edges | 39.86 | 1,224,152 | - |
| 1hop_traversal | 0.00 | - | 0.001 |

## Semantic Knowledge Store (SKS)

| Operation | Time (ms) | Throughput (/s) | p95 (ms) |
| --- | ---: | ---: | ---: |
| ingest | 144.65 | 6,913 | - |
| hybrid_query | 0.56 | - | 0.583 |
| ingest | 239.46 | 20,880 | - |
| hybrid_query | 4.73 | - | 4.900 |