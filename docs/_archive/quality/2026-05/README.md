# May 2026 Quality Archive

This directory contains older quality or architecture-analysis documents that are no longer active
trackers.

| Document | Reason archived | Active replacement |
|---|---|---|
| `MULTIMODEL_ARCHITECTURE_ANALYSIS.adoc` | February 2025 architecture review with outdated engine maturity and roadmap assumptions. | `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc`, `docs/12-design/DATA_AI_PLATFORM_ARCHITECTURE_ANCHOR_2026_05_12.adoc`, and `docs/10-quality/TECHNICAL_DEBT.adoc` |
| `TECHNICAL_DEBT.md` | Older Markdown copy of the technical debt register. | `docs/10-quality/TECHNICAL_DEBT.adoc` |
| `PANIC_PRONE_CODE_AUDIT.md` | Implementation/companion plan for TD-007 (panic-prone code elimination). TD-007 is **Resolved** in `docs/10-quality/TECHNICAL_DEBT.adoc:125-132` (counting script verified 0 unwrap()/expect() in production code; CI guardrails in place). Archived 2026-05-26. | `docs/10-quality/TECHNICAL_DEBT.adoc` TD-007 row |
| `TEST_INLINING_PROGRESS.md` | Completed work log for the standalone-test-files → inline `#[cfg(test)]` migration (23 files / 6,500 lines / 190 tests migrated, completed 2026-04-07). Now tracked as TD-069 (Resolved) in the live register. Archived 2026-05-26. | `docs/10-quality/TECHNICAL_DEBT.adoc` TD-069 row |

Keep live quality status in `docs/10-quality/TECHNICAL_DEBT.adoc` and supported-surface docs.
