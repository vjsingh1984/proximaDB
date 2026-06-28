---
name: td-adr-convention
description: File or migrate ProximaDB Technical Debt (TD) entries and Architecture Decision Records (ADRs) per the collision-free per-file convention (PRs #441–443). Use whenever filing a new TD/ADR, migrating a legacy TD to per-file, fixing a `td-adr-unique` guard failure, or choosing a TD/ADR id. Enforces topic-scoped ids, id-in-heading, and the uniqueness guard.
---

# File / migrate TD & ADR entries (collision-free per-file convention)

**Source of truth:** `docs/12-design/HOW_TO_FILE_TD_AND_ADR.adoc`. This skill
operationalizes it. Goal: concurrent sessions filing TDs/ADRs never textually
conflict and never claim a duplicate id.

## Layout
- **ADRs:** `docs/12-design/adr/ADR-<NNN>-<slug>.adoc` (index it in `adr/README.adoc`)
- **TDs (new):** `docs/10-quality/td/TD-<id>-<slug>.adoc`
- **TDs (legacy ~100):** `docs/10-quality/TECHNICAL_DEBT.adoc` — **do NOT migrate ad hoc**

## File a NEW TD
1. `eval "$(scripts/worktree.sh new docs/td-<topic>)"` — branch off `origin/develop`.
2. Pick the id:
   - **Preferred — topic-scoped** `TD-<TOPIC>-N` (e.g. `TD-CAT-1`, `TD-SC-2`,
     `TD-CG3`) for any multi-step effort. One initiative owns its prefix ⇒ parallel
     sessions on different initiatives never collide.
   - **Standalone one-off** → global `TD-NNN`: scan `docs/10-quality/td/` **and** the
     legacy `TECHNICAL_DEBT.adoc` rows for the highest N, +1.
3. Create `docs/10-quality/td/TD-<id>-<slug>.adoc`:
   ```
   = TD-<id>: <short title>
   :status: Open | In Progress | Partial | Resolved | Won't Do
   :dim: D1..D5
   :pillar: OE | SEC | REL | PERF | COST | SUS

   Problem → next action; key file location; deps.
   ```
   The id in the `= TD-<id>:` **title heading** is authoritative (the guard reads it
   there, not the filename — the slug's hyphens make the filename ambiguous).
4. Backstop: `python3 scripts/check_td_adr_unique.py` (and the `td-adr-unique` CI
   job). Must pass — no duplicate id across the legacy table + per-file dir.
5. Verify locally, commit, PR. If a rebase lands a colliding id, renumber yours
   (never the merged one) and re-check within 48h (rebase mandate).

## File a NEW ADR
1. `docs/12-design/adr/ADR-<NNN>-<slug>.adoc` — scan `adr/` for the highest N, +1.
2. First heading `= ADR-<NNN>: <title>`.
3. **Index it** in `docs/12-design/adr/README.adoc` (the guard warns on unindexed ADRs).
4. Run the guard; PR.

## Migrate a LEGACY TD → per-file  (DELIBERATE, BATCHED, post-merge only)
The legacy table is referenced **~900×** across code/docs and conflicts with every
open PR touching `TECHNICAL_DEBT.adoc`. Do **not** migrate incidentally.

When the TD-touching PR queue is clear, migrate as **one batched PR**:
- For each legacy row: create `td/TD-<id>-<slug>.adoc` with the row's content, then
  replace the row with a one-line pointer (e.g. `| TD-163 | → see td/TD-163-… |`).
- **Keep the id identical** so the ~900 `TD-<id>` references across the codebase
  still resolve.
- The guard treats both the legacy row's `| TD-<id> |` and the per-file `= TD-<id>:` **as declarations**. During migration, leave exactly ONE declaration per id: the
  per-file heading. Reduce the legacy row to a pointer that is NOT a `= TD-<id>:`
  heading and does not re-declare the id, so the guard sees one.

## Fix a `td-adr-unique` failure
It fires on a duplicate ADR number or TD id (across legacy table + per-file dir).
Renumber **your** new entry (never the already-merged one): pick the next-free id
per the rules above, update filename + title heading, re-run the guard.

## Why (don't subvert it)
Sequential numbers in a shared space have no atomic allocator — `ADR-031/032/033`
and `TD-167` were each claimed twice in one week, surfacing only at merge. One file
per entry + topic-scoped ids + the guard removes both failure modes. If you're
tempted to hand-edit the monolith or skip the guard, re-read
`docs/12-design/HOW_TO_FILE_TD_AND_ADR.adoc` first.
