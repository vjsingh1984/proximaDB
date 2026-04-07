# E1-E3 Sprint Board Import (Markdown)

Source issue pack: [E1_E3_GITHUB_ISSUE_PACK_2026_03_31.md](./E1_E3_GITHUB_ISSUE_PACK_2026_03_31.md)  
Source matrix: [E1_E3_SPRINT_BOARD_MATRIX_2026_03_31.adoc](./E1_E3_SPRINT_BOARD_MATRIX_2026_03_31.adoc)

| Board ID | Epic | Title | Lane | Estimate | Sprint | Depends On | Parallel With | Labels | Status | Issue Pack Ref |
|---|---|---|---|---:|---|---|---|---|---|---|
| SB-01 | E1 | `[SB-01][E1] Define capability registry core types` | Query Fabric | 3d | Sprint 1 | None | SB-04 | `epic, subtask, p0, query-fabric` | Todo | `#sb-01` |
| SB-02 | E1 | `[SB-02][E1] Bridge store and provider capabilities into the registry` | Query Fabric | 3d | Sprint 1 | SB-01 | SB-04 | `epic, subtask, p0, query-fabric` | Todo | `#sb-02` |
| SB-03 | E1 | `[SB-03][E1] Attach capability requirements to plan nodes` | Query Fabric | 4d | Sprint 1 | SB-02 | SB-04 | `epic, subtask, p0, query-fabric, planner` | Todo | `#sb-03` |
| SB-04 | E1 | `[SB-04][E1] Define protocol-level capability error mapping` | API and Protocol | 2d | Sprint 1 | SB-01 | SB-02, SB-03 | `epic, subtask, p0, api-parity` | Todo | `#sb-04` |
| SB-05 | E1 | `[SB-05][E1] Add canonical plan validation to public entrypoints` | API and Protocol | 4d | Sprint 2 | SB-03, SB-04 | SB-06 | `epic, subtask, p0, api-parity, query-fabric` | Todo | `#sb-05` |
| SB-06 | E1 | `[SB-06][E1] Add capability contract tests and snapshot generation` | Quality and Release | 3d | Sprint 2 | SB-02, SB-04 | SB-05, SB-07 | `epic, subtask, p0, quality, release-gating` | Todo | `#sb-06` |
| SB-07 | E1 | `[SB-07][E1] Generate supported surface and CI gate design` | Quality and Release | 3d | Sprint 4 | SB-06 | SB-14 | `epic, subtask, p0, docs-generated, release-gating` | Todo | `#sb-07` |
| SB-08 | E2 | `[SB-08][E2] Define normalized filter and candidate-set contracts` | Vector and Indexing | 3d | Sprint 2 | SB-02 | SB-10 | `epic, subtask, p0, vector, indexing` | Todo | `#sb-08` |
| SB-09 | E2 | `[SB-09][E2] Wire canonical vector paths to build filtered HybridQuery` | Vector and Indexing | 3d | Sprint 2 | SB-08, SB-05 | SB-10 | `epic, subtask, p0, vector, api-parity` | Todo | `#sb-09` |
| SB-10 | E2 | `[SB-10][E2] Implement candidate handling in AXIS manager` | Vector and Indexing | 5d | Sprint 3 | SB-08 | SB-09, SB-11 | `epic, subtask, p0, vector, indexing, performance` | Todo | `#sb-10` |
| SB-11 | E2 | `[SB-11][E2] Apply backend-specific filtered contracts in HNSW and IVF` | Vector and Indexing | 4d | Sprint 3 | SB-10 | SB-12 | `epic, subtask, p0, vector, indexing` | Todo | `#sb-11` |
| SB-12 | E2 | `[SB-12][E2] Add filtered ANN differential and graph-first regressions` | Quality and Release | 4d | Sprint 3 | SB-09, SB-11 | SB-13 | `epic, subtask, p0, quality, vector` | Todo | `#sb-12` |
| SB-13 | E2 | `[SB-13][E2] Add filtered ANN benchmark harness` | Quality and Release | 2d | Sprint 4 | SB-11 | SB-07 | `epic, subtask, p0, benchmark, vector` | Todo | `#sb-13` |
| SB-14 | E3 | `[SB-14][E3] Define MultiModelPlan v1 contract` | Query Fabric | 4d | Sprint 2 | SB-03, SB-08 | SB-15, SB-16 | `epic, subtask, p0, planner, query-fabric` | Todo | `#sb-14` |
| SB-15 | E3 | `[SB-15][E3] Replace placeholder UQL lowering with MultiModelPlan v1` | Query Fabric | 4d | Sprint 3 | SB-14 | SB-16 | `epic, subtask, p0, planner, uql` | Todo | `#sb-15` |
| SB-16 | E3 | `[SB-16][E3] Route federated SQL and facade requests into shared plan contract` | Query Fabric | 4d | Sprint 3 | SB-14, SB-05 | SB-15, SB-17 | `epic, subtask, p0, planner, api-parity` | Todo | `#sb-16` |
| SB-17 | E3 | `[SB-17][E3] Unify explain schema across REST, gRPC, SQL, and unified APIs` | API and Protocol | 3d | Sprint 3 | SB-14, SB-16 | SB-18 | `epic, subtask, p0, api-parity, explain` | Todo | `#sb-17` |
| SB-18 | E3 | `[SB-18][E3] Remove degraded execute_parallel production entrypoints` | Query Fabric | 3d | Sprint 4 | SB-15, SB-16, SB-17 | SB-19 | `epic, subtask, p0, query-fabric, cleanup` | Todo | `#sb-18` |
| SB-19 | E3 | `[SB-19][E3] Close PlanNodeType::Scan honesty gap` | Query Fabric | 4d | Sprint 4 | SB-18, SB-05 | SB-20 | `epic, subtask, p0, planner, scan` | Todo | `#sb-19` |
| SB-20 | E3 | `[SB-20][E3] Add SQL/UQL/REST/gRPC plan-parity test suite` | Quality and Release | 4d | Sprint 4 | SB-17, SB-18, SB-19 | SB-07 | `epic, subtask, p0, quality, api-parity, planner` | Todo | `#sb-20` |

