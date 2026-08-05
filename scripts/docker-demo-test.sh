#!/usr/bin/env bash
# Build the image and prove the exact customer-facing MVP trust corridor.
set -Eeuo pipefail

CONTAINER_NAME="${PROXIMADB_MVP_CONTAINER:-proximadb-mvp-smoke}"
IMAGE_NAME="${PROXIMADB_MVP_IMAGE:-proximadb:mvp-smoke}"
REST_PORT="${PROXIMADB_MVP_REST_PORT:-5678}"

cleanup() {
  docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
docker info >/dev/null

echo "Building $IMAGE_NAME"
docker build -t "$IMAGE_NAME" .
cleanup

echo "Starting $CONTAINER_NAME on REST port $REST_PORT"
docker run -d \
  --name "$CONTAINER_NAME" \
  -p "$REST_PORT:5678" \
  "$IMAGE_NAME" >/dev/null

for attempt in $(seq 1 60); do
  if curl --fail --silent "http://127.0.0.1:$REST_PORT/health" >/dev/null; then
    break
  fi
  if [ "$attempt" -eq 60 ]; then
    docker logs "$CONTAINER_NAME" >&2
    echo "ProximaDB did not become healthy" >&2
    exit 1
  fi
  sleep 1
done

python3 scripts/mvp_smoke.py \
  --base-url "http://127.0.0.1:$REST_PORT" \
  --output artifacts/mvp/docker-smoke.json

echo "MVP Docker smoke passed; report: artifacts/mvp/docker-smoke.json"
