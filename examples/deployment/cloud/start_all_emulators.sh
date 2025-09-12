#!/bin/bash
# Start all cloud storage emulators for testing

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
TOOLS_DIR="$PROJECT_ROOT/tools"

# Function to check if a port is in use
check_port() {
    local port=$1
    if lsof -i:$port >/dev/null 2>&1; then
        echo "Port $port is already in use"
        return 1
    fi
    return 0
}

# Start MinIO
echo "Starting MinIO..."
if check_port 9000; then
    export MINIO_ROOT_USER=minioadmin
    export MINIO_ROOT_PASSWORD=minioadmin
    nohup "$TOOLS_DIR/minio" server "$PROJECT_ROOT/test-data/minio" --console-address ":9001" > "$PROJECT_ROOT/logs/minio.log" 2>&1 &
    echo "MinIO started (PID: $!)"
    
    # Wait for MinIO to start
    sleep 2
    
    # Create test bucket
    export AWS_ACCESS_KEY_ID=minioadmin
    export AWS_SECRET_ACCESS_KEY=minioadmin
    aws --endpoint-url http://localhost:9000 s3 mb s3://proximadb-test 2>/dev/null || true
else
    echo "MinIO already running on port 9000"
fi

# Start fake-gcs-server
echo "Starting fake-gcs-server..."
if check_port 4443; then
    nohup "$TOOLS_DIR/fake-gcs-server" -data "$PROJECT_ROOT/test-data/gcs" -port 4443 > "$PROJECT_ROOT/logs/gcs.log" 2>&1 &
    echo "fake-gcs-server started (PID: $!)"
else
    echo "fake-gcs-server already running on port 4443"
fi

# Start Azurite (if installed)
if command -v azurite &> /dev/null; then
    echo "Starting Azurite..."
    if check_port 10000; then
        nohup azurite --location "$PROJECT_ROOT/test-data/azurite" --silent > "$PROJECT_ROOT/logs/azurite.log" 2>&1 &
        echo "Azurite started (PID: $!)"
    else
        echo "Azurite already running on port 10000"
    fi
else
    echo "Azurite not installed - skipping"
fi

echo ""
echo "All emulators started. Check logs in $PROJECT_ROOT/logs/"
echo ""
echo "To stop all emulators, run: $SCRIPT_DIR/stop_all_emulators.sh"