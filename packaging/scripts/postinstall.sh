#!/bin/bash
# Post-install script for ProximaDB package

set -e

# Create proximadb system user if it doesn't exist
if ! id proximadb &>/dev/null; then
    useradd --system --user-group --home-dir /var/lib/proximadb --shell /sbin/nologin proximadb
    echo "Created system user 'proximadb'"
fi

# Create data directory
if [ ! -d /var/lib/proximadb ]; then
    mkdir -p /var/lib/proximadb
    chown proximadb:proximadb /var/lib/proximadb
    chmod 750 /var/lib/proximadb
    echo "Created data directory /var/lib/proximadb"
fi

# Create log directory
if [ ! -d /var/log/proximadb ]; then
    mkdir -p /var/log/proximadb
    chown proximadb:proximadb /var/log/proximadb
    chmod 750 /var/log/proximadb
    echo "Created log directory /var/log/proximadb"
fi

# Reload systemd and enable service
if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
    systemctl enable proximadb.service
    echo "Enabled proximadb systemd service"
fi

echo "ProximaDB installation complete!"
echo "Start the service with: systemctl start proximadb"
