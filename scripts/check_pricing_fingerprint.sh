#!/usr/bin/env bash
# Cross-repo drift guard: assert config/pricing.json matches its committed
# SHA256 fingerprint. Run in CI before any cargo step that depends on tier
# defaults.
#
# When this fails, the pricing config in this repo has drifted from the
# canonical anvaiops/pricing/tiers.json. Resolve by running
# `scripts/sync_pricing_to_proximadb.sh` from the anvaiops repo, which copies
# the canonical file + rewrites the fingerprint here.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"

CONFIG="${REPO}/config/pricing.json"
FINGERPRINT="${REPO}/config/pricing.json.sha256"

if [[ ! -f "${CONFIG}" ]]; then
    echo "✗ ${CONFIG} not found — proximaDB cannot build without it" >&2
    exit 1
fi

if [[ ! -f "${FINGERPRINT}" ]]; then
    echo "✗ ${FINGERPRINT} not found — sync from anvaiops first" >&2
    exit 1
fi

EXPECTED=$(cat "${FINGERPRINT}" | tr -d '[:space:]')
ACTUAL=$(shasum -a 256 "${CONFIG}" | awk '{print $1}')

if [[ "${EXPECTED}" != "${ACTUAL}" ]]; then
    echo "✗ config/pricing.json SHA256 mismatch — drift from anvaiops canonical" >&2
    echo "  expected: ${EXPECTED}" >&2
    echo "  actual:   ${ACTUAL}" >&2
    echo "" >&2
    echo "  Resolve: in anvaiops, run scripts/sync_pricing_to_proximadb.sh" >&2
    exit 1
fi

echo "✓ config/pricing.json fingerprint matches (${ACTUAL:0:12}…)"
