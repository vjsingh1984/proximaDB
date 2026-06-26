#!/usr/bin/env bash
# OSS docs decontamination lint.
#
# ProximaDB OSS ships MECHANISM + NEUTRAL meters only. Commercial POLICY
# ($ rates, ARR/revenue targets, tier authority, GTM) lives in anvaiops — see
# docs/12-design/OSS_ENTERPRISE_BOUNDARY_2026_06_17.adoc. This lint blocks
# accidental re-introduction of commercial $-content into the public docs.
#
# Phase 1 (this script): $-per-unit pricing + $-denominated ARR/TAM/revenue
# targets. Phase 2 (once the MEDIUM commercial-flavored docs are cleaned):
# broaden to GTM/competitive-positioning phrases. Archived docs
# (docs/_archive/, docs/_internal/archive/) are excluded here — their
# commercial content is relocated separately (tracked follow-up).
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

# $-per-unit pricing ($0.50/hour, $10,000/node, $0.02/TB, …) and
# $-denominated targets ($50M ARR, $4.3B TAM, $10K, …).
PATTERN='\$[0-9]+(\.[0-9]+)?[[:space:]]*(per[[:space:]]*)?(TB|GB|MB|hour|hr|node|month|mo|credit|DBU|query|index|replica)|\$[0-9]+(\.[0-9]+)?[[:space:]]*[KMB]\b'

# Scope: docs/ (AsciiDoc + Markdown). Exclude BOTH archive roots (deferred).
mapfile -t HITS < <(grep -rniE "$PATTERN" --include='*.adoc' --include='*.md' "$ROOT/docs" 2>/dev/null \
  | grep -vE '/docs/(_archive|_internal/archive)/' || true)

if [ "${#HITS[@]}" -gt 0 ]; then
  echo "::error::Commercial \$-content (pricing / ARR / TAM targets) found in OSS docs." >&2
  echo "ProximaDB OSS ships mechanism + neutral meters only; \$ rates/revenue targets belong in anvaiops." >&2
  echo "See docs/12-design/OSS_ENTERPRISE_BOUNDARY_2026_06_17.adoc (the boundary definition)." >&2
  echo "--- hits ---" >&2
  printf '%s\n' "${HITS[@]}" >&2
  exit 1
fi
echo "ok: no commercial \$-content (pricing / ARR / TAM) in OSS docs"
