#!/bin/bash

# ProximaDB Systemd Service Installation Script for Docker/Ubuntu
# This script sets up ProximaDB as a systemd service

set -euo pipefail

PROXIMADB_USER="proximadb"
PROXIMADB_GROUP="proximadb"
PROXIMADB_HOME="/opt/proximadb"
PROXIMADB_DATA_DIR="/opt/proximadb/data"
PROXIMADB_CONFIG_DIR="/opt/proximadb/config"
PROXIMADB_LOG_DIR="/var/log/proximadb"
PROXIMADB_BIN_DIR="/opt/proximadb/bin"

echo "🚀 ProximaDB Systemd Service Installation"
echo "========================================"

# Check if running as root
if [[ $EUID -ne 0 ]]; then
   echo "❌ This script must be run as root (use sudo)"
   exit 1
fi

# Function to print status
print_status() {
    echo "✅ $1"
}

print_error() {
    echo "❌ $1"
}

# Create system user and group
echo "📋 Creating system user and group..."
if ! id "$PROXIMADB_USER" &>/dev/null; then
    useradd --system --no-create-home --shell /bin/false \
        --home-dir "$PROXIMADB_HOME" \
        --comment "ProximaDB Vector Database" \
        "$PROXIMADB_USER"
    print_status "Created user: $PROXIMADB_USER"
else
    print_status "User already exists: $PROXIMADB_USER"
fi

# Create directories
echo "📁 Creating directories..."
mkdir -p "$PROXIMADB_HOME" "$PROXIMADB_DATA_DIR" "$PROXIMADB_CONFIG_DIR" "$PROXIMADB_LOG_DIR" "$PROXIMADB_BIN_DIR"
mkdir -p "$PROXIMADB_DATA_DIR"/{wal,metadata,store}

# Create subdirectories for data organization
mkdir -p "$PROXIMADB_DATA_DIR"/wal
mkdir -p "$PROXIMADB_DATA_DIR"/metadata/collections
mkdir -p "$PROXIMADB_DATA_DIR"/store

print_status "Created directory structure"

# Copy binary (assuming it exists in target/release)
echo "📦 Installing ProximaDB binary..."
if [[ -f "/workspace/target/release/proximadb-server" ]]; then
    cp "/workspace/target/release/proximadb-server" "$PROXIMADB_BIN_DIR/"
    chmod +x "$PROXIMADB_BIN_DIR/proximadb-server"
    print_status "Installed binary to $PROXIMADB_BIN_DIR/proximadb-server"
elif [[ -f "/workspace/target/debug/proximadb-server" ]]; then
    cp "/workspace/target/debug/proximadb-server" "$PROXIMADB_BIN_DIR/"
    chmod +x "$PROXIMADB_BIN_DIR/proximadb-server"
    print_status "Installed debug binary to $PROXIMADB_BIN_DIR/proximadb-server"
else
    print_error "ProximaDB binary not found. Please build first with: cargo build --release"
    echo "Expected locations:"
    echo "  - /workspace/target/release/proximadb-server"
    echo "  - /workspace/target/debug/proximadb-server"
    exit 1
fi

# Copy configuration
echo "⚙️  Installing configuration..."
if [[ -f "/workspace/config.toml" ]]; then
    cp "/workspace/config.toml" "$PROXIMADB_CONFIG_DIR/proximadb.toml"
    
    # Update paths in config for production deployment
    sed -i "s|/Users/vijaysingh/code/proximaDB/data|$PROXIMADB_DATA_DIR|g" "$PROXIMADB_CONFIG_DIR/proximadb.toml"
    sed -i "s|file:///workspace/data|file://$PROXIMADB_DATA_DIR|g" "$PROXIMADB_CONFIG_DIR/proximadb.toml"
    
    print_status "Installed configuration to $PROXIMADB_CONFIG_DIR/proximadb.toml"
else
    print_error "Configuration file not found at /workspace/config.toml"
    exit 1
fi

# Set proper ownership
echo "🔐 Setting permissions..."
chown -R "$PROXIMADB_USER:$PROXIMADB_GROUP" "$PROXIMADB_HOME"
chown -R "$PROXIMADB_USER:$PROXIMADB_GROUP" "$PROXIMADB_LOG_DIR"

# Set proper permissions
chmod -R 755 "$PROXIMADB_HOME"
chmod -R 750 "$PROXIMADB_DATA_DIR"
chmod -R 750 "$PROXIMADB_LOG_DIR"
chmod 644 "$PROXIMADB_CONFIG_DIR/proximadb.toml"

print_status "Set ownership and permissions"

# Install systemd service
echo "🔧 Installing systemd service..."
if [[ -f "/workspace/proximadb-docker.service" ]]; then
    cp "/workspace/proximadb-docker.service" "/etc/systemd/system/proximadb.service"
    systemctl daemon-reload
    print_status "Installed systemd service"
else
    print_error "Service file not found at /workspace/proximadb-docker.service"
    exit 1
fi

# Enable service
echo "🚀 Enabling ProximaDB service..."
systemctl enable proximadb.service
print_status "Service enabled"

# Create logrotate configuration
echo "📝 Setting up log rotation..."
cat > /etc/logrotate.d/proximadb << 'EOF'
/var/log/proximadb/*.log {
    daily
    rotate 30
    compress
    delaycompress
    missingok
    notifempty
    create 644 proximadb proximadb
    postrotate
        systemctl reload proximadb.service > /dev/null 2>&1 || true
    endscript
}
EOF
print_status "Configured log rotation"

# Create health check script
echo "🏥 Installing health check script..."
cat > "$PROXIMADB_BIN_DIR/health-check.sh" << 'EOF'
#!/bin/bash
# ProximaDB Health Check Script

HEALTH_URL="http://localhost:5678/health"
TIMEOUT=10

if curl -f -s --max-time $TIMEOUT "$HEALTH_URL" > /dev/null 2>&1; then
    echo "ProximaDB is healthy"
    exit 0
else
    echo "ProximaDB health check failed"
    exit 1
fi
EOF

chmod +x "$PROXIMADB_BIN_DIR/health-check.sh"
chown "$PROXIMADB_USER:$PROXIMADB_GROUP" "$PROXIMADB_BIN_DIR/health-check.sh"
print_status "Installed health check script"

echo ""
echo "🎉 ProximaDB Installation Complete!"
echo "=================================="
echo ""
echo "📋 Installation Summary:"
echo "  User/Group: $PROXIMADB_USER:$PROXIMADB_GROUP"
echo "  Home: $PROXIMADB_HOME"
echo "  Binary: $PROXIMADB_BIN_DIR/proximadb-server"
echo "  Config: $PROXIMADB_CONFIG_DIR/proximadb.toml"
echo "  Data: $PROXIMADB_DATA_DIR"
echo "  Logs: $PROXIMADB_LOG_DIR"
echo ""
echo "🚀 Next Steps:"
echo "  1. Start service:    sudo systemctl start proximadb"
echo "  2. Check status:     sudo systemctl status proximadb"
echo "  3. View logs:        sudo journalctl -u proximadb -f"
echo "  4. Health check:     $PROXIMADB_BIN_DIR/health-check.sh"
echo "  5. Test connection:  curl http://localhost:5678/health"
echo ""
echo "🔧 Service Management:"
echo "  Start:    sudo systemctl start proximadb"
echo "  Stop:     sudo systemctl stop proximadb"
echo "  Restart:  sudo systemctl restart proximadb"
echo "  Status:   sudo systemctl status proximadb"
echo "  Logs:     sudo journalctl -u proximadb -f"
echo ""
echo "📊 Monitoring:"
echo "  Health:   curl http://localhost:5678/health"
echo "  Metrics:  curl http://localhost:5678/metrics"
echo "  Collections: curl http://localhost:5678/collections"