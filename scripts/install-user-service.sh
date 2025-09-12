#!/bin/bash
set -euo pipefail

# ProximaDB User Service Installation Script
# This script installs ProximaDB as a systemd user service

USER=${1:-$(whoami)}
echo "🚀 Installing ProximaDB user service for user: $USER"

# Check if running as the target user
if [ "$(whoami)" != "$USER" ]; then
    echo "❌ Error: Please run this script as user '$USER' or use sudo -u $USER"
    exit 1
fi

# Create systemd user directory
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"
mkdir -p "$SYSTEMD_USER_DIR"

# Copy service file
echo "📁 Creating systemd user service..."
cp proximadb.service "$SYSTEMD_USER_DIR/"

# Build ProximaDB in release mode
echo "🔨 Building ProximaDB in release mode..."
cargo build --release --bin proximadb-server

# Create data directories
echo "📁 Creating data directories..."
sudo mkdir -p /data/proximadb/1/{wal,store,metadata}
sudo mkdir -p /data/proximadb/2/{wal,store}

# Set permissions
echo "🔐 Setting directory permissions..."
sudo chown -R $USER:$USER /data/proximadb
sudo chmod -R 755 /data/proximadb

# Reload systemd user daemon
echo "🔄 Reloading systemd user daemon..."
systemctl --user daemon-reload

# Enable the service
echo "✅ Enabling ProximaDB user service..."
systemctl --user enable proximadb.service

echo ""
echo "🎉 ProximaDB user service installed successfully!"
echo ""
echo "📋 Service Management Commands:"
echo "   Start:   systemctl --user start proximadb"
echo "   Stop:    systemctl --user stop proximadb" 
echo "   Status:  systemctl --user status proximadb"
echo "   Logs:    journalctl --user -u proximadb -f"
echo "   Restart: systemctl --user restart proximadb"
echo ""
echo "🌐 Service Endpoints:"
echo "   gRPC:    localhost:5679"
echo "   REST:    localhost:5678"
echo ""
echo "📁 Data Layout:"
echo "   Metadata: /data/proximadb/1/metadata/"
echo "   WAL:      /data/proximadb/{1,2}/wal/{collection_uuid}/"
echo "   Storage:  /data/proximadb/{1,2}/store/{collection_uuid}/"
echo ""
echo "💡 To start ProximaDB now: systemctl --user start proximadb"