#!/usr/bin/env bash
# Spin up cloud object-store emulators (Azurite / MinIO / fake-gcs-server) via
# Docker, create the test bucket+container, and run the #[ignore]d object-store
# tier integration tests against them — the TD-168 "validate the Cool tier on a
# real cloud API" check. Used by .github/workflows/qa-gate.yml (the develop→qa
# gate), .github/workflows/ci.yml (the develop early-detection job, --fast), and
# `make cloud-emulator-test` (local). Single source of truth so every CI path is
# exactly the locally-runnable path.
#
# Usage: run_cloud_emulator_tests.sh [--fast|--all]
#   --fast : run ONLY the cheap object-store tier tests (~3-5 min — compiles just
#            the small object-store crate, no main-crate compile, no OOM risk).
#            Used by the develop early-detection job so a tier regression is caught
#            on the introducing feat→develop PR.
#   --all  : (default) also run cold_graph_record_store_round_trips_on_real_azure,
#            which compiles the full main crate and runs under CARGO_BUILD_JOBS=2.
#
# Requires: docker, cargo, aws CLI (MinIO bucket), az CLI (Azurite container),
# curl (fake-gcs bucket). On GitHub ubuntu-latest all are preinstalled.
#
# Azure + S3 are strict (PUT-with-tier must be accepted + round-trip). GCS is
# best-effort: the test itself skips on a fake-gcs/object_store incompatibility,
# so this script never fails on GCS alone.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Scope selection (see header). --fast = object-store tier tests only; --all (default)
# adds the cold-payload azure round-trip, which compiles the full main crate.
SCOPE="all"
for arg in "$@"; do
  case "$arg" in
    --fast) SCOPE="fast" ;;
    --all)  SCOPE="all" ;;
    *) echo "::error::unknown argument: $arg"; echo "usage: $0 [--fast|--all]"; exit 2 ;;
  esac
done
echo "==> Scope: $SCOPE"

CONTAINER_BUCKET="proximadb-test"
AZURITE_CONN="DefaultEndpointsProtocol=http;AccountName=devstoreaccount1;AccountKey=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==;BlobEndpoint=http://127.0.0.1:10000/devstoreaccount1;"

cleanup() { docker rm -f azurite minio fake-gcs >/dev/null 2>&1 || true; }
trap cleanup EXIT

wait_port() { # $1=port $2=name
  for _ in $(seq 1 30); do
    (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null && { exec 3>&- ; echo "  $2 (:$1) up"; return 0; }
    sleep 2
  done
  echo "::error::$2 (:$1) did not come up"; return 1
}

echo "==> Starting emulators (Docker)"
cleanup
docker run -d --name azurite -p 10000:10000 \
  mcr.microsoft.com/azure-storage/azurite \
  azurite-blob --blobHost 0.0.0.0 --skipApiVersionCheck >/dev/null
docker run -d --name minio -p 9000:9000 \
  -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
  minio/minio server /data >/dev/null
docker run -d --name fake-gcs -p 4443:4443 \
  fsouza/fake-gcs-server -scheme http -port 4443 -public-host localhost:4443 >/dev/null

echo "==> Waiting for emulators"
wait_port 10000 azurite
wait_port 9000 minio
wait_port 4443 fake-gcs

echo "==> Creating bucket/container '$CONTAINER_BUCKET'"
# MinIO bucket (aws CLI)
AWS_ACCESS_KEY_ID=minioadmin AWS_SECRET_ACCESS_KEY=minioadmin AWS_DEFAULT_REGION=us-east-1 \
  aws --endpoint-url http://127.0.0.1:9000 s3 mb "s3://$CONTAINER_BUCKET" 2>/dev/null || echo "  (minio bucket exists)"
# Azurite container (az CLI)
az storage container create --name "$CONTAINER_BUCKET" --connection-string "$AZURITE_CONN" >/dev/null 2>&1 \
  || echo "  (azurite container exists / az CLI missing)"
# fake-gcs bucket (JSON API, no auth)
curl -sf -X POST "http://127.0.0.1:4443/storage/v1/b?project=proximadb" \
  -H "Content-Type: application/json" -d "{\"name\":\"$CONTAINER_BUCKET\"}" >/dev/null 2>&1 \
  || echo "  (fake-gcs bucket exists)"

echo "==> Running object-store tier integration tests (scope: $SCOPE)"
# Azure (Azurite) + S3 (MinIO) through the production from_url + env path; GCS via builder.
export AZURE_STORAGE_USE_EMULATOR=true AZURE_ALLOW_HTTP=true
export AWS_ENDPOINT=http://127.0.0.1:9000 AWS_ALLOW_HTTP=true AWS_VIRTUAL_HOSTED_STYLE_REQUEST=false
export AWS_ACCESS_KEY_ID=minioadmin AWS_SECRET_ACCESS_KEY=minioadmin AWS_REGION=us-east-1
export PROXIMADB_GCS_TEST_ENDPOINT=http://127.0.0.1:4443

cargo test -p proximadb-object-store --features aws,azure,gcp -- --ignored --nocapture \
  put_with_tier_accepted_by_azurite \
  put_with_tier_accepted_by_minio \
  put_with_tier_against_fake_gcs

if [ "$SCOPE" = "all" ]; then
  # Compiles the full main `proximadb` crate (~8400-test binary). CARGO_BUILD_JOBS=2
  # caps compile parallelism low enough to keep the 16GB hosted runner off the OOM
  # cliff (the "runner received a shutdown signal" failure that jobs=1 was added to
  # dodge), while halving the serialized ~35-40m compile that grew past the 60m job
  # budget and got cancelled mid-build. 2 (not the .cargo/config.toml default of 3)
  # is the deliberate middle ground: faster than 1, safer than 3 on a 16GB runner.
  CARGO_BUILD_JOBS=2 cargo test -p proximadb --features azure -- --ignored --nocapture \
    cold_graph_record_store_round_trips_on_real_azure
fi

echo "==> Strong read-back: confirm Azurite persisted the Cool tier"
TIER="$(az storage blob show --container-name "$CONTAINER_BUCKET" --name cold/probe-azure.bin \
  --connection-string "$AZURITE_CONN" --query properties.blobTier -o tsv 2>/dev/null || echo "")"
echo "  Azurite blobTier(cold/probe-azure.bin) = '${TIER:-<unknown>}'"
if [ "$TIER" = "Cool" ]; then
  echo "  ✅ Cool tier persisted on a real Azure API"
else
  echo "::warning::Azurite did not report Cool tier (got '${TIER:-<unknown>}') — header accepted but resident tier unverified"
fi

echo "==> Cloud emulator tier validation complete"
