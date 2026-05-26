#!/usr/bin/env bash
# Drift guard: assert config/tier-config.json matches its committed SHA256
# fingerprint. Run in CI before any cargo step that depends on the embedded
# tier defaults.
#
# When this fails, the baked-in tier config has been modified without
# updating the fingerprint. Resolve locally by recomputing the SHA:
#
#   sha256sum config/tier-config.json > config/tier-config.json.sha256
#   git add config/tier-config.json config/tier-config.json.sha256
#
# For AnvaiOps customers whose deployments use the commercial image with
# AnvaiOps pricing baked in at the overlay layer, this check covers the
# OSS-default baseline only (the AnvaiOps overlay is built separately
# in the anvaiops repo's build-commercial-image.yml).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${SCRIPT_DIR}/.." && pwd)"

CONFIG="${REPO}/config/tier-config.json"
FINGERPRINT="${REPO}/config/tier-config.json.sha256"

if [[ ! -f "${CONFIG}" ]]; then
    echo "✗ ${CONFIG} not found — proximaDB cannot build without it" >&2
    exit 1
fi

if [[ ! -f "${FINGERPRINT}" ]]; then
    echo "✗ ${FINGERPRINT} not found" >&2
    exit 1
fi

EXPECTED=$(awk '{print $1}' "${FINGERPRINT}")
ACTUAL=$(shasum -a 256 "${CONFIG}" | awk '{print $1}')

if [[ "${EXPECTED}" != "${ACTUAL}" ]]; then
    echo "✗ config/tier-config.json SHA256 mismatch — fingerprint outdated" >&2
    echo "  expected: ${EXPECTED}" >&2
    echo "  actual:   ${ACTUAL}" >&2
    echo "" >&2
    echo "  Resolve: sha256sum config/tier-config.json > config/tier-config.json.sha256" >&2
    exit 1
fi

echo "✓ config/tier-config.json fingerprint matches (${ACTUAL:0:12}…)"
