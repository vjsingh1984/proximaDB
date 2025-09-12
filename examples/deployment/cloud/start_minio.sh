#!/bin/bash
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

export MINIO_ROOT_USER=minioadmin
export MINIO_ROOT_PASSWORD=minioadmin

echo "Starting MinIO on port 9000..."
"$SCRIPT_DIR/minio" server "$PROJECT_ROOT/test-data/minio" --console-address ":9001"
