#!/bin/bash
# Script to set up local cloud storage emulators for testing ProximaDB

set -e

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
TOOLS_DIR="$PROJECT_ROOT/tools"

mkdir -p "$TOOLS_DIR"
cd "$TOOLS_DIR"

echo "Setting up cloud storage emulators..."

# 1. MinIO (S3-compatible)
echo "Installing MinIO..."
if [ ! -f "$TOOLS_DIR/minio" ]; then
    wget https://dl.min.io/server/minio/release/linux-amd64/minio
    chmod +x minio
fi

# 2. Azurite (Azure Storage emulator)
echo "Installing Azurite..."
if ! command -v azurite &> /dev/null; then
    npm install -g azurite || echo "Azurite installation failed - npm required"
fi

# 3. fake-gcs-server (Google Cloud Storage emulator)
echo "Installing fake-gcs-server..."
if [ ! -f "$TOOLS_DIR/fake-gcs-server" ]; then
    # Download fake-gcs-server binary
    wget https://github.com/fsouza/fake-gcs-server/releases/download/v1.47.8/fake-gcs-server_1.47.8_Linux_amd64.tar.gz
    tar -xzf fake-gcs-server_1.47.8_Linux_amd64.tar.gz
    rm fake-gcs-server_1.47.8_Linux_amd64.tar.gz
fi

# Create data directories
mkdir -p "$PROJECT_ROOT/test-data/minio"
mkdir -p "$PROJECT_ROOT/test-data/azurite"
mkdir -p "$PROJECT_ROOT/test-data/gcs"

# Create start scripts
cat > "$TOOLS_DIR/start_minio.sh" << 'EOF'
#!/bin/bash
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

export MINIO_ROOT_USER=minioadmin
export MINIO_ROOT_PASSWORD=minioadmin

echo "Starting MinIO on port 9000..."
"$SCRIPT_DIR/minio" server "$PROJECT_ROOT/test-data/minio" --console-address ":9001"
EOF

cat > "$TOOLS_DIR/start_azurite.sh" << 'EOF'
#!/bin/bash
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "Starting Azurite on ports 10000-10002..."
azurite --location "$PROJECT_ROOT/test-data/azurite" --silent
EOF

cat > "$TOOLS_DIR/start_gcs.sh" << 'EOF'
#!/bin/bash
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "Starting fake-gcs-server on port 4443..."
"$SCRIPT_DIR/fake-gcs-server" -data "$PROJECT_ROOT/test-data/gcs" -port 4443
EOF

chmod +x "$TOOLS_DIR"/*.sh

echo ""
echo "Cloud emulators installed successfully!"
echo ""
echo "To start the emulators:"
echo "  MinIO (S3):     $TOOLS_DIR/start_minio.sh"
echo "  Azurite (Azure): $TOOLS_DIR/start_azurite.sh"
echo "  GCS:            $TOOLS_DIR/start_gcs.sh"
echo ""
echo "Connection strings:"
echo "  S3:    s3://localhost:9000 (Access: minioadmin, Secret: minioadmin)"
echo "  Azure: DefaultEndpointsProtocol=http;AccountName=devstoreaccount1;AccountKey=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==;BlobEndpoint=http://127.0.0.1:10000/devstoreaccount1;"
echo "  GCS:   http://localhost:4443"