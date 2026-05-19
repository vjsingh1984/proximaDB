# Archived Root-Level Reports

This directory contains historical reports and generated documentation that previously lived in the repository root.

The root directory should stay limited to canonical project entry points and build metadata:

- `README.adoc`, `CHANGELOG.md`, `SUPPORTED_SURFACE.md`
- license and notice files
- build/package files such as `Cargo.toml`, `Cargo.lock`, `Makefile`, Dockerfiles, and toolchain config
- agent/tooling instructions such as `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md`

Use these archive buckets for older material:

| Directory | Contents |
|---|---|
| `benchmarks/` | Historical benchmark reports, validation plans, and competitor comparisons. |
| `diagrams/` | Historical PlantUML diagrams and benchmark CSV outputs. |
| `implementation/` | Implementation audits, deprecated-type notes, documentation restructuring notes, and review reports. |
| `project-status/` | Phase reports, session summaries, roadmap snapshots, and status reports. |
| `release/` | Historical release, deployment, and packaging checklists. |
| `workspace/` | Workspace refactor status reports and migration notes. |

New release-facing documentation should go under the numbered docs tree, such as `docs/01-quick-start/`, `docs/02-guides/`, `docs/04-operations/`, `docs/10-quality/`, or `docs/12-design/`. Use this archive only for retained historical context.
