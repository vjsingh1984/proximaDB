#!/usr/bin/env bash
# Embedding dev->qa gate smoke.
#
# Ingests one document with server-side (native) embedding and asserts the
# engine actually embeds it — failing LOUD if the build lacks the `onnx`
# feature (model_unavailable). That failure is the gate's whole point: `onnx`
# is a non-default cargo feature and the default `:latest` image is the no-onnx
# `runtime` target, so without this check nothing in CI catches "embedding
# broken" or "the `-full` (onnx) image fails to embed".
#
# Usage:  scripts/embedding_smoke.sh [ENGINE_URL]    (default http://localhost:5678)
# Exit:   0 = native embedding works; 1 = engine can't embed (model_unavailable / non-2xx).
set -euo pipefail

URL="${1:-http://localhost:5678}"
URL="${URL%/}"
COLL="embedding_smoke_$$"

echo "→ creating collection $COLL (dim 384 = BGE-small)"
curl -fsS -X POST "$URL/api/v2/collections" \
  -H 'Content-Type: application/json' \
  -d "{\"name\":\"$COLL\",\"dimension\":384}" >/dev/null

echo "→ ingesting a doc with X-Embed-Source: native (server-side embed)"
CODE=$(curl -s -o /tmp/embedding_smoke_resp.json -w "%{http_code}" -X POST \
  "$URL/api/v2/collections/$COLL/documents" \
  -H 'Content-Type: application/json' -H 'X-Embed-Source: native' \
  -d '{"records":[{"id":"s1","text":"spark executor out of memory after autoscaling","metadata":{"tenant_id":"smoke","source":"probe"}}]}')

fail() {
  echo "::error:: $1"
  echo "--- response (HTTP $CODE) ---"
  cat /tmp/embedding_smoke_resp.json 2>/dev/null || true
  echo
  exit 1
}

case "$CODE" in
  200|201|202) ;;
  *) fail "native ingest returned HTTP $CODE (expected 2xx)" ;;
esac

if grep -q "model_unavailable" /tmp/embedding_smoke_resp.json 2>/dev/null; then
  fail "engine returned model_unavailable — the 'onnx' feature is disabled in this build. Rebuild with --features onnx (the runtime-full image), or use a non-BGE route."
fi

echo "✓ native embedding works (HTTP $CODE) — onnx/BGE path is live in this image."
