# E1-E3 GitHub Issue Pack (2026-03-31)

Copy-paste-ready issue bodies for `SB-01` through `SB-20` from the sprint board matrix.

Source matrix: [E1_E3_SPRINT_BOARD_MATRIX_2026_03_31.adoc](./E1_E3_SPRINT_BOARD_MATRIX_2026_03_31.adoc)
Automation helper: [`scripts/github/create_e1_e3_issues.py`](/Users/vijaysingh/code/proximaDB/scripts/github/create_e1_e3_issues.py)
Shell wrapper: [`scripts/github/create_e1_e3_issues.sh`](/Users/vijaysingh/code/proximaDB/scripts/github/create_e1_e3_issues.sh)
Assignment helper: [`scripts/github/assign_e1_e3_issues.py`](/Users/vijaysingh/code/proximaDB/scripts/github/assign_e1_e3_issues.py)
Assignment wrapper: [`scripts/github/assign_e1_e3_issues.sh`](/Users/vijaysingh/code/proximaDB/scripts/github/assign_e1_e3_issues.sh)

## Automation

Dry-run all issue creates with derived sprint-board labels:

```bash
python3 scripts/github/create_e1_e3_issues.py --include-board-labels
```

Dry-run label creation plus issue creation:

```bash
python3 scripts/github/create_e1_e3_issues.py --include-board-labels --ensure-labels
```

Create all 20 issues against the current `gh` repo:

```bash
python3 scripts/github/create_e1_e3_issues.py --apply --include-board-labels --ensure-labels --skip-existing
```

Create a subset against an explicit repository:

```bash
python3 scripts/github/create_e1_e3_issues.py --apply --repo OWNER/REPO --select SB-01,SB-02,SB-03 --include-board-labels --ensure-labels --skip-existing
```

Assign the created issues to a milestone:

```bash
python3 scripts/github/assign_e1_e3_issues.py --apply --milestone "E1-E3 Tranche" --ensure-milestone
```

Assign the created issues to a named project:

```bash
python3 scripts/github/assign_e1_e3_issues.py --apply --project-title "Roadmap"
```

Assign a subset to an explicit Project v2 owner and number:

```bash
python3 scripts/github/assign_e1_e3_issues.py --apply --select SB-01,SB-02,SB-03 --project-owner OWNER --project-number 1
```

If project assignment fails for auth scope, refresh GitHub CLI auth with:

```bash
gh auth refresh -s project
```

## SB-01

**Title**: `[SB-01][E1] Define capability registry core types`

**Labels**: `epic`, `subtask`, `p0`, `query-fabric`

**Sprint**: `Sprint 1`

**Depends on**: `None`

**Parallel with**: `SB-04`

```md
## Summary
Define the foundational capability registry types used by E1, E2, and E3.

## Scope
- [ ] Add `CapabilityDescriptor`
- [ ] Add `CapabilityRequirement`
- [ ] Add `CapabilityTier`
- [ ] Add registry skeleton and serialization-friendly types

## Files
- `src/query/capabilities/mod.rs`
- `src/query/capabilities/types.rs`
- `src/query/capabilities/registry.rs`

## Dependencies
- Depends on: None
- Parallel with: SB-04
- Blocks: SB-02, SB-03, SB-05

## Validation
- [ ] Unit tests for type construction and serialization
- [ ] Registry compiles cleanly with no production callers yet

## Done When
- [ ] Core capability types exist in a stable module
- [ ] Types are ready to bridge store and provider capabilities
```

## SB-02

**Title**: `[SB-02][E1] Bridge store and provider capabilities into the registry`

**Labels**: `epic`, `subtask`, `p0`, `query-fabric`

**Sprint**: `Sprint 1`

**Depends on**: `SB-01`

**Parallel with**: `SB-04`

```md
## Summary
Bridge existing store and columnar-provider capability models into the new shared registry.

## Scope
- [ ] Add adapter or conversion logic from `StoreCapabilities`
- [ ] Add adapter or conversion logic from `ColumnarCapabilities`
- [ ] Preserve capability fidelity for pushdown, streaming, indexes, and transactions

## Files
- `src/storage/multimodel/traits.rs`
- `src/query/columnar/provider.rs`
- `src/query/capabilities/registry.rs`

## Dependencies
- Depends on: SB-01
- Parallel with: SB-04
- Blocks: SB-03, SB-06, SB-08

## Validation
- [ ] Unit tests for store/provider to registry conversion
- [ ] Snapshot-friendly registry output for representative model combinations

## Done When
- [ ] Store and provider capabilities can be registered without ad hoc side tables
- [ ] Registry exposes enough information for planner validation
```

## SB-03

**Title**: `[SB-03][E1] Attach capability requirements to plan nodes`

**Labels**: `epic`, `subtask`, `p0`, `query-fabric`, `planner`

**Sprint**: `Sprint 1`

**Depends on**: `SB-02`

**Parallel with**: `SB-04`

```md
## Summary
Annotate federated and unified plans with explicit capability requirements.

## Scope
- [ ] Attach capability requirements to federated plan nodes
- [ ] Attach capability requirements to unified plan flows
- [ ] Preserve capability metadata for later explain output

## Files
- `src/query/federated/optimizer/mod.rs`
- `src/query/unified/mod.rs`
- `src/query/capabilities/types.rs`

## Dependencies
- Depends on: SB-02
- Parallel with: SB-04
- Blocks: SB-05, SB-14

## Validation
- [ ] Unit tests for plan nodes carrying required capabilities
- [ ] Explain metadata or debug output exposes plan requirements

## Done When
- [ ] Canonical plans carry explicit capability requirements
- [ ] Planning no longer relies on implicit assumptions only
```

## SB-04

**Title**: `[SB-04][E1] Define protocol-level capability error mapping`

**Labels**: `epic`, `subtask`, `p0`, `api-parity`

**Sprint**: `Sprint 1`

**Depends on**: `SB-01`

**Parallel with**: `SB-02`, `SB-03`

```md
## Summary
Create a consistent protocol-facing capability error contract for REST, gRPC, and adapter layers.

## Scope
- [ ] Introduce `ExecutionCapabilityError`
- [ ] Map capability errors through adapter responses
- [ ] Define REST and gRPC parity for unsupported-plan responses

## Files
- `src/query/facade/adapter.rs`
- `src/network/rest/v1/unified_query.rs`
- `src/network/rest/v1/handlers.rs`
- `src/query/capabilities/validator.rs`

## Dependencies
- Depends on: SB-01
- Parallel with: SB-02, SB-03
- Blocks: SB-05, SB-06

## Validation
- [ ] REST and adapter tests return structured capability failures
- [ ] Error payload shape is documented and stable

## Done When
- [ ] Capability failures map consistently across protocols
- [ ] Later validation work can reuse the same error surface
```

## SB-05

**Title**: `[SB-05][E1] Add canonical plan validation to public entrypoints`

**Labels**: `epic`, `subtask`, `p0`, `api-parity`, `query-fabric`

**Sprint**: `Sprint 2`

**Depends on**: `SB-03`, `SB-04`

**Parallel with**: `SB-06`

```md
## Summary
Run capability validation before execution on all canonical public entrypoints.

## Scope
- [ ] Validate REST unified query entrypoints
- [ ] Validate gRPC and facade-driven entrypoints
- [ ] Validate SQL, UQL, and unified query entrypoints
- [ ] Fail unsupported plan shapes before execution

## Files
- `src/query/federated/mod.rs`
- `src/query/unified/mod.rs`
- `src/query/facade/mod.rs`
- protocol handler modules

## Dependencies
- Depends on: SB-03, SB-04
- Parallel with: SB-06
- Blocks: SB-09, SB-16, SB-19

## Validation
- [ ] Integration tests show unsupported plans fail before execution
- [ ] No public path degrades to empty success when capability checks fail

## Done When
- [ ] Capability validation is active on canonical public entrypoints
- [ ] Planner-level rejection happens before executor dispatch
```

## SB-06

**Title**: `[SB-06][E1] Add capability contract tests and snapshot generation`

**Labels**: `epic`, `subtask`, `p0`, `quality`, `release-gating`

**Sprint**: `Sprint 2`

**Depends on**: `SB-02`, `SB-04`

**Parallel with**: `SB-05`, `SB-07`

```md
## Summary
Add contract coverage for the capability registry and emit a machine-readable capability snapshot artifact.

## Scope
- [ ] Add capability registry contract tests
- [ ] Emit snapshot artifact from tests or utility
- [ ] Make artifact stable and diffable

## Files
- `tests/contracts/capability_registry_contract.rs`
- capability snapshot utility under `src/query/capabilities/*`

## Dependencies
- Depends on: SB-02, SB-04
- Parallel with: SB-05, SB-07
- Blocks: SB-07

## Validation
- [ ] Contract tests pass on representative capability combinations
- [ ] Snapshot artifact is deterministic across runs

## Done When
- [ ] Capability snapshot exists and is test-backed
- [ ] Snapshot can drive support-surface generation
```

## SB-07

**Title**: `[SB-07][E1] Generate supported surface and CI gate design`

**Labels**: `epic`, `subtask`, `p0`, `docs-generated`, `release-gating`

**Sprint**: `Sprint 4`

**Depends on**: `SB-06`

**Parallel with**: `SB-14`

```md
## Summary
Generate the supported-surface document from capability outputs and define CI gates against documentation drift.

## Scope
- [ ] Convert `SUPPORTED_SURFACE` to generated or partially generated output
- [ ] Add CI checks that compare docs against capability artifacts
- [ ] Update roadmap references where needed

## Files
- `docs/SUPPORTED_SURFACE.adoc`
- CI configuration
- roadmap docs under `docs/_internal/roadmap/*`

## Dependencies
- Depends on: SB-06
- Parallel with: SB-14
- Blocked by: capability snapshot readiness

## Validation
- [ ] Generated support matrix matches the artifact
- [ ] CI fails when docs overstate supported capability tiers

## Done When
- [ ] Support-surface generation is wired into the release workflow
- [ ] Documentation drift is mechanically detectable
```

## SB-08

**Title**: `[SB-08][E2] Define normalized filter and candidate-set contracts`

**Labels**: `epic`, `subtask`, `p0`, `vector`, `indexing`

**Sprint**: `Sprint 2`

**Depends on**: `SB-02`

**Parallel with**: `SB-10`

```md
## Summary
Define the normalized filter and candidate-set contracts used by filtered ANN on the canonical path.

## Scope
- [ ] Add `NormalizedFilter`
- [ ] Add `CandidateSet`
- [ ] Add shared `HybridQueryBuilder`

## Files
- `src/core/search/*`
- `src/services/operations/vectors.rs`
- new shared helper module as needed

## Dependencies
- Depends on: SB-02
- Parallel with: SB-10
- Blocks: SB-09, SB-10, SB-14

## Validation
- [ ] Unit tests for lowering `FilterExpression` into normalized filters
- [ ] Unit tests for candidate-set construction basics

## Done When
- [ ] Filter normalization is defined once and reusable
- [ ] Canonical vector path can consume shared filtered-query contracts
```

## SB-09

**Title**: `[SB-09][E2] Wire canonical vector paths to build filtered HybridQuery`

**Labels**: `epic`, `subtask`, `p0`, `vector`, `api-parity`

**Sprint**: `Sprint 2`

**Depends on**: `SB-08`, `SB-05`

**Parallel with**: `SB-10`

```md
## Summary
Ensure canonical vector request paths preserve filters when building `HybridQuery`.

## Scope
- [ ] Route v1 and unified vector search requests through shared `HybridQueryBuilder`
- [ ] Stop emitting empty `metadata_filters` and `id_filters` when filters exist
- [ ] Preserve filter intent through handler and facade layers

## Files
- `src/services/operations/vectors.rs`
- `src/api_handlers/unified_handlers.rs`
- `src/query/facade/strategies/vector.rs`

## Dependencies
- Depends on: SB-08, SB-05
- Parallel with: SB-10
- Blocks: SB-12

## Validation
- [ ] Integration tests show filtered vector requests produce populated `HybridQuery` state
- [ ] No canonical path drops filter intent before AXIS dispatch

## Done When
- [ ] Filter-bearing requests reach vector execution as filtered queries
- [ ] Public-path filter propagation is covered by regression tests
```

## SB-10

**Title**: `[SB-10][E2] Implement candidate handling in AXIS manager`

**Labels**: `epic`, `subtask`, `p0`, `vector`, `indexing`, `performance`

**Sprint**: `Sprint 3`

**Depends on**: `SB-08`

**Parallel with**: `SB-09`, `SB-11`

```md
## Summary
Implement candidate-set handling and metadata-index prefiltering in the live AXIS manager flow.

## Scope
- [ ] Add candidate-set handling in the AXIS manager
- [ ] Reuse metadata pushdown logic where appropriate
- [ ] Expose diagnostics for candidate count and index choice

## Files
- `src/index/axis/management/manager.rs`
- `src/core/search/metadata_filter_pushdown.rs`

## Dependencies
- Depends on: SB-08
- Parallel with: SB-09, SB-11
- Blocks: SB-11

## Validation
- [ ] Unit tests for candidate generation and filtered-query routing
- [ ] AXIS diagnostics expose candidate-set stage behavior

## Done When
- [ ] AXIS performs intentional prefiltering on supported filtered queries
- [ ] Candidate generation is no longer implicit or scattered
```

## SB-11

**Title**: `[SB-11][E2] Apply backend-specific filtered contracts in HNSW and IVF`

**Labels**: `epic`, `subtask`, `p0`, `vector`, `indexing`

**Sprint**: `Sprint 3`

**Depends on**: `SB-10`

**Parallel with**: `SB-12`

```md
## Summary
Apply backend-specific filtered-query behavior in HNSW and IVF execution paths.

## Scope
- [ ] Implement filtered-query or candidate-restricted behavior in HNSW
- [ ] Implement filtered-query or candidate-restricted behavior in IVF
- [ ] Mark unsupported ANN backends honestly through capability gates if needed

## Files
- `src/index/axis/indexes/hnsw_index.rs`
- `src/index/axis/indexes/ivf_unified.rs`
- optionally `src/index/axis/indexes/lsh_index.rs`

## Dependencies
- Depends on: SB-10
- Parallel with: SB-12
- Blocks: SB-13

## Validation
- [ ] Deterministic backend tests for filtered-query behavior
- [ ] Unsupported behavior is rejected rather than silently ignored

## Done When
- [ ] HNSW and IVF honor the filtered-query contract
- [ ] Backend capability reporting matches reality
```

## SB-12

**Title**: `[SB-12][E2] Add filtered ANN differential and graph-first regressions`

**Labels**: `epic`, `subtask`, `p0`, `quality`, `vector`

**Sprint**: `Sprint 3`

**Depends on**: `SB-09`, `SB-11`

**Parallel with**: `SB-13`

```md
## Summary
Add exact-baseline, recall, and graph-first regression coverage for filtered ANN.

## Scope
- [ ] Replace TODO filtered-search tests with assertion-backed coverage
- [ ] Add exact-vs-approximate regression cases
- [ ] Extend graph-first metadata-filter path coverage

## Files
- `src/services/tests/index_first_search_tests.rs`
- `tests/sks_graph_first_integration_test.rs`

## Dependencies
- Depends on: SB-09, SB-11
- Parallel with: SB-13
- Blocks: SB-14

## Validation
- [ ] Exact-baseline tests pass
- [ ] Recall tests exist for selective filters
- [ ] Graph-first path is covered by integration regression tests

## Done When
- [ ] Filtered ANN correctness is test-backed on canonical and graph-first paths
- [ ] TODO-only filtered-search coverage is eliminated
```

## SB-13

**Title**: `[SB-13][E2] Add filtered ANN benchmark harness`

**Labels**: `epic`, `subtask`, `p0`, `benchmark`, `vector`

**Sprint**: `Sprint 4`

**Depends on**: `SB-11`

**Parallel with**: `SB-07`

```md
## Summary
Add benchmark coverage for exact filtered scan, filtered ANN, and post-filter baselines.

## Scope
- [ ] Add filtered ANN benchmark harness
- [ ] Record candidate counts, rerank counts, and recall
- [ ] Capture target performance envelopes for release gating

## Files
- `benches/filtered_ann_bench.rs`

## Dependencies
- Depends on: SB-11
- Parallel with: SB-07
- Blocks: release evidence for E2

## Validation
- [ ] Bench harness runs on representative corpora
- [ ] Output includes correctness and performance context, not latency only

## Done When
- [ ] Filtered ANN claims are benchmark-backed
- [ ] Bench harness is ready for gated or scheduled CI usage
```

## SB-14

**Title**: `[SB-14][E3] Define MultiModelPlan v1 contract`

**Labels**: `epic`, `subtask`, `p0`, `planner`, `query-fabric`

**Sprint**: `Sprint 2`

**Depends on**: `SB-03`, `SB-08`

**Parallel with**: `SB-15`, `SB-16`

```md
## Summary
Define the shared `MultiModelPlan v1` contract and canonical lowering interfaces.

## Scope
- [ ] Define shared logical plan types
- [ ] Define canonical lowering entrypoints
- [ ] Ensure plan contract can carry capability metadata and filtered vector operators

## Files
- new shared plan types under `src/query/*`

## Dependencies
- Depends on: SB-03, SB-08
- Parallel with: SB-15, SB-16
- Blocks: SB-15, SB-16, SB-17

## Validation
- [ ] Unit tests for plan construction
- [ ] Plan contract supports all canonical multi-model operators in scope

## Done When
- [ ] `MultiModelPlan v1` exists as the single production lowering target
- [ ] SQL and UQL can converge on the same logical contract
```

## SB-15

**Title**: `[SB-15][E3] Replace placeholder UQL lowering with MultiModelPlan v1`

**Labels**: `epic`, `subtask`, `p0`, `planner`, `uql`

**Sprint**: `Sprint 3`

**Depends on**: `SB-14`

**Parallel with**: `SB-16`

```md
## Summary
Replace placeholder UQL conversion helpers with lowering into `MultiModelPlan v1`.

## Scope
- [ ] Replace `convert_multimodal_to_query()` production lowering path
- [ ] Remove placeholder vector extraction from production flow
- [ ] Preserve enough syntax detail for explain and execution

## Files
- `src/query/unified/uql.rs`

## Dependencies
- Depends on: SB-14
- Parallel with: SB-16
- Blocks: SB-18

## Validation
- [ ] UQL queries lower into `MultiModelPlan v1`
- [ ] Plan parity tests exist for representative UQL requests

## Done When
- [ ] UQL no longer relies on lossy placeholder lowering in production
- [ ] UQL and SQL can target the same execution contract
```

## SB-16

**Title**: `[SB-16][E3] Route federated SQL and facade requests into shared plan contract`

**Labels**: `epic`, `subtask`, `p0`, `planner`, `api-parity`

**Sprint**: `Sprint 3`

**Depends on**: `SB-14`, `SB-05`

**Parallel with**: `SB-15`, `SB-17`

```md
## Summary
Route federated SQL lowering and facade-produced requests into the shared multi-model plan contract.

## Scope
- [ ] Lower federated SQL into `MultiModelPlan v1`
- [ ] Route facade-produced requests through the same contract
- [ ] Keep capability validation in front of the shared planner

## Files
- `src/query/federated/*`
- `src/query/facade/*`

## Dependencies
- Depends on: SB-14, SB-05
- Parallel with: SB-15, SB-17
- Blocks: SB-17, SB-18

## Validation
- [ ] SQL and facade-driven execution reach the same plan compiler
- [ ] Regression tests cover equivalent plan shapes where expected

## Done When
- [ ] Canonical SQL and facade requests share one plan contract
- [ ] Planner no longer treats canonical SQL as a separate execution world
```

## SB-17

**Title**: `[SB-17][E3] Unify explain schema across REST, gRPC, SQL, and unified APIs`

**Labels**: `epic`, `subtask`, `p0`, `api-parity`, `explain`

**Sprint**: `Sprint 3`

**Depends on**: `SB-14`, `SB-16`

**Parallel with**: `SB-18`

```md
## Summary
Expose one canonical explain schema across all supported multi-model entrypoints.

## Scope
- [ ] Unify explain DTOs and operator metadata
- [ ] Align REST SQL explain and unified explain payloads
- [ ] Preserve capability and fallback information in explain output

## Files
- `src/query/explain.rs`
- `src/network/rest/v1/handlers.rs`
- `src/network/rest/v1/unified_query.rs`

## Dependencies
- Depends on: SB-14, SB-16
- Parallel with: SB-18
- Blocks: SB-20

## Validation
- [ ] Protocol parity tests for explain payloads
- [ ] Explain output is stable for equivalent SQL and UQL requests

## Done When
- [ ] Explain is protocol-independent on the canonical path
- [ ] Operator IDs, capabilities, and fallback reasons are visible everywhere
```

## SB-18

**Title**: `[SB-18][E3] Remove degraded execute_parallel production entrypoints`

**Labels**: `epic`, `subtask`, `p0`, `query-fabric`, `cleanup`

**Sprint**: `Sprint 4`

**Depends on**: `SB-15`, `SB-16`, `SB-17`

**Parallel with**: `SB-19`

```md
## Summary
Remove or privatize degraded `execute_parallel*` production entrypoints and force canonical execution.

## Scope
- [ ] Restrict degraded execution helpers to non-production use if still needed
- [ ] Route canonical production execution through validated plans only
- [ ] Remove public-path ambiguity in unified execution

## Files
- `src/query/unified/mod.rs`
- `src/query/unified/executor.rs`

## Dependencies
- Depends on: SB-15, SB-16, SB-17
- Parallel with: SB-19
- Blocks: SB-20

## Validation
- [ ] No production caller uses degraded `execute_parallel*` entrypoints
- [ ] Canonical execution tests continue to pass through the shared path

## Done When
- [ ] Production execution has one canonical entry
- [ ] Degraded parallel helpers no longer shape supported behavior
```

## SB-19

**Title**: `[SB-19][E3] Close PlanNodeType::Scan honesty gap`

**Labels**: `epic`, `subtask`, `p0`, `planner`, `scan`

**Sprint**: `Sprint 4`

**Depends on**: `SB-18`, `SB-05`

**Parallel with**: `SB-20`

```md
## Summary
Make scan support honest on the canonical path: provider-backed where supported, explicit rejection where unsupported.

## Scope
- [ ] Back supported scan execution through `ColumnarReadProvider`
- [ ] Reject unsupported scan shapes at plan time
- [ ] Align explain and diagnostics with actual scan behavior

## Files
- `src/query/federated/execution/mod.rs`
- `src/query/columnar/*`

## Dependencies
- Depends on: SB-18, SB-05
- Parallel with: SB-20
- Blocks: tranche closure for E3

## Validation
- [ ] Provider-backed scans execute on supported paths
- [ ] Unsupported scans fail capability validation instead of degrading at execution time

## Done When
- [ ] `PlanNodeType::Scan` has an honest canonical support story
- [ ] Scan behavior is visible in explain and release docs
```

## SB-20

**Title**: `[SB-20][E3] Add SQL/UQL/REST/gRPC plan-parity test suite`

**Labels**: `epic`, `subtask`, `p0`, `quality`, `api-parity`, `planner`

**Sprint**: `Sprint 4`

**Depends on**: `SB-17`, `SB-18`, `SB-19`

**Parallel with**: `SB-07`

```md
## Summary
Add parity and plan-contract tests across SQL, UQL, REST, and gRPC entrypoints.

## Scope
- [ ] Add plan-contract tests for SQL and UQL
- [ ] Add explain parity tests across protocols
- [ ] Add execution parity tests for supported multi-model requests

## Files
- `tests/query/*`
- protocol integration tests

## Dependencies
- Depends on: SB-17, SB-18, SB-19
- Parallel with: SB-07
- Blocks: tranche closure for E3

## Validation
- [ ] SQL and UQL lower to equivalent plans where expected
- [ ] REST and gRPC explain payloads match the canonical schema
- [ ] Supported execution paths produce equivalent results across protocol surfaces

## Done When
- [ ] Canonical multi-model plan behavior is parity-tested across supported entrypoints
- [ ] E3 has release-quality regression evidence
```
