#!/bin/sh
# ProximaDB container entrypoint.
#
# Before launching the server, refreshes the pricing/tier config from
# the AnvaiOps canonical R2 artifact (published by the anvaiops
# repo's `publish-pricing-r2.yml` GitHub Actions workflow). This
# replaces the prior hand-synced `scripts/sync_pricing_to_proximadb.sh`
# workflow: every pod restart now picks up the latest pricing without
# needing a coordinated PR across both repos.
#
# Fallback: if the fetch fails (offline build, R2 outage, env override),
# the container falls through to whatever `/config/pricing.json` was
# baked in at image build time. So:
#   - Air-gapped deployments work as before (use the baked file).
#   - Online deployments get fresh pricing on every restart.
#   - Network failures during boot don't crash the pod.
#
# Customer environments can disable the fetch entirely by setting
# ANVAIOPS_PRICING_URL='' (empty) via a config map. The baked-in file
# always wins if the fetched body is malformed (non-JSON or empty).

set -eu

PRICING_URL="${ANVAIOPS_PRICING_URL:-https://pricing.anvaiops.com/tiers.json}"
PRICING_PATH="${ANVAIOPS_PRICING_PATH:-/config/pricing.json}"
FETCH_TIMEOUT_SECS="${ANVAIOPS_PRICING_FETCH_TIMEOUT:-10}"
FETCH_RETRIES="${ANVAIOPS_PRICING_FETCH_RETRIES:-2}"

refresh_pricing() {
    if [ -z "${PRICING_URL}" ]; then
        echo "[entrypoint] ANVAIOPS_PRICING_URL is empty; using baked-in ${PRICING_PATH}"
        return 0
    fi

    echo "[entrypoint] fetching pricing from ${PRICING_URL}..."

    tmpfile="$(mktemp)"

    if ! curl --fail --silent --show-error --location \
              --max-time "${FETCH_TIMEOUT_SECS}" \
              --retry "${FETCH_RETRIES}" --retry-delay 2 \
              "${PRICING_URL}" -o "${tmpfile}"; then
        echo "[entrypoint] WARN: pricing fetch from ${PRICING_URL} failed; using baked-in ${PRICING_PATH}" >&2
        rm -f "${tmpfile}"
        return 0
    fi

    # Sanity check: refuse to install a non-JSON / empty body. A bad
    # response is worse than a stale one — the baked-in file is at
    # least the version this image was built against.
    if [ ! -s "${tmpfile}" ]; then
        echo "[entrypoint] WARN: pricing fetch returned empty body; using baked-in ${PRICING_PATH}" >&2
        rm -f "${tmpfile}"
        return 0
    fi
    if ! head -c 1 "${tmpfile}" | grep -q '{'; then
        echo "[entrypoint] WARN: pricing fetch returned non-JSON body; using baked-in ${PRICING_PATH}" >&2
        rm -f "${tmpfile}"
        return 0
    fi

    # Atomic replace: write tmp first, then rename. Prevents the
    # server reading a half-written file if it tries to load pricing
    # mid-update on a hot reload.
    mv "${tmpfile}" "${PRICING_PATH}"
    chmod 0644 "${PRICING_PATH}"
    echo "[entrypoint] installed fresh pricing at ${PRICING_PATH} ($(wc -c < "${PRICING_PATH}") bytes)"
}

refresh_pricing

# Hand off to the server. `exec` so the server becomes PID 1 (signal
# handling, OOM, healthchecks all work correctly).
exec /usr/local/bin/proximadb-server "$@"
