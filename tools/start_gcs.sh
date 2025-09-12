#!/bin/bash
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "Starting fake-gcs-server on port 4443..."
"$SCRIPT_DIR/fake-gcs-server" -data "$PROJECT_ROOT/test-data/gcs" -port 4443
