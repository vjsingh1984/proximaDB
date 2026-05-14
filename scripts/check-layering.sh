#!/usr/bin/env bash
# Canonical workspace layering validation wrapper.
#
# Keep the policy table in scripts/check_workspace_boundaries.py and the
# architecture rationale in roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$ROOT_DIR"
python3 scripts/check_workspace_boundaries.py --strict
