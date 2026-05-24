#!/bin/sh
# ProximaDB container entrypoint.
#
# Before launching the server, optionally refreshes the tier
# configuration from an operator-supplied URL. The fetched JSON is
# atomic-replaced into /config/tier-config.json on success; failures
# fall back to whatever was baked into the image at build time. So:
#
#   - Air-gapped deployments work as before (use the baked file).
#   - Online deployments get fresh tier config on every restart.
#   - Network failures during boot don't crash the pod.
#
# To disable the remote fetch entirely (use baked file only), set
# PROXIMADB_TIER_CONFIG_URL=''. To use a custom URL (your own R2 / S3 /
# CDN / config server), set it to that URL. The schema the URL must
# serve is documented in config/TIER_CONFIG.md.
#
# Backward compatibility: the legacy ANVAIOPS_PRICING_URL +
# /config/pricing.json variables are still honored if the new
# PROXIMADB_TIER_CONFIG_* variables are unset. This keeps existing
# AnvaiOps deployments working during the migration window. The
# legacy code path is scheduled for removal in the next major version.

set -eu

# ─── New canonical variables (operator-neutral) ─────────────────────────────
TIER_CONFIG_URL="${PROXIMADB_TIER_CONFIG_URL-}"
TIER_CONFIG_PATH="${PROXIMADB_TIER_CONFIG_PATH:-/config/tier-config.json}"
FETCH_TIMEOUT_SECS="${PROXIMADB_TIER_CONFIG_FETCH_TIMEOUT:-10}"
FETCH_RETRIES="${PROXIMADB_TIER_CONFIG_FETCH_RETRIES:-2}"

# ─── Legacy variables (deprecated; honored for backward compatibility) ──────
LEGACY_URL="${ANVAIOPS_PRICING_URL-}"
LEGACY_PATH="${ANVAIOPS_PRICING_PATH:-/config/pricing.json}"

# If the new variable is unset AND the legacy variable is set, use the
# legacy values + write a deprecation warning so operators see it in the
# logs and migrate at their own pace.
if [ -z "${TIER_CONFIG_URL+x}" ] && [ -n "${LEGACY_URL}" ]; then
    echo "[entrypoint] WARN: ANVAIOPS_PRICING_URL is deprecated; set PROXIMADB_TIER_CONFIG_URL instead." >&2
    echo "[entrypoint] WARN: see config/TIER_CONFIG.md for the new schema; legacy path will be removed in the next major release." >&2
    TIER_CONFIG_URL="${LEGACY_URL}"
    TIER_CONFIG_PATH="${LEGACY_PATH}"
fi

refresh_tier_config() {
    if [ -z "${TIER_CONFIG_URL}" ]; then
        echo "[entrypoint] No tier config URL set; using baked-in ${TIER_CONFIG_PATH}"
        return 0
    fi

    echo "[entrypoint] fetching tier config from ${TIER_CONFIG_URL}..."

    tmpfile="$(mktemp)"

    if ! curl --fail --silent --show-error --location \
              --max-time "${FETCH_TIMEOUT_SECS}" \
              --retry "${FETCH_RETRIES}" --retry-delay 2 \
              -H "User-Agent: proximadb-entrypoint/1.0" \
              "${TIER_CONFIG_URL}" -o "${tmpfile}"; then
        echo "[entrypoint] WARN: tier config fetch from ${TIER_CONFIG_URL} failed; using baked-in ${TIER_CONFIG_PATH}" >&2
        rm -f "${tmpfile}"
        return 0
    fi

    # Sanity check: refuse to install a non-JSON / empty body. A bad
    # response is worse than a stale one — the baked-in file is at
    # least the version this image was built against.
    if [ ! -s "${tmpfile}" ]; then
        echo "[entrypoint] WARN: tier config fetch returned empty body; using baked-in ${TIER_CONFIG_PATH}" >&2
        rm -f "${tmpfile}"
        return 0
    fi
    if ! head -c 1 "${tmpfile}" | grep -q '{'; then
        echo "[entrypoint] WARN: tier config fetch returned non-JSON body; using baked-in ${TIER_CONFIG_PATH}" >&2
        rm -f "${tmpfile}"
        return 0
    fi

    # Atomic replace: write tmp first, then rename. Prevents the
    # server reading a half-written file if it tries to load tier
    # config mid-update on a hot reload.
    mv "${tmpfile}" "${TIER_CONFIG_PATH}"
    chmod 0644 "${TIER_CONFIG_PATH}"
    echo "[entrypoint] installed fresh tier config at ${TIER_CONFIG_PATH} ($(wc -c < "${TIER_CONFIG_PATH}") bytes)"
}

refresh_tier_config

# Hand off to the server. `exec` so the server becomes PID 1 (signal
# handling, OOM, healthchecks all work correctly).
exec /usr/local/bin/proximadb-server "$@"
