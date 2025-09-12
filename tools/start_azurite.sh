#!/bin/bash
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "Starting Azurite on ports 10000-10002..."
azurite --location "$PROJECT_ROOT/test-data/azurite" --silent
