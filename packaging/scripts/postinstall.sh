#!/usr/bin/env bash
# ProximaDB post-install script
# Creates system user, data directories, and enables the systemd service.

set -e

# Create system user if it doesn't exist
if ! id -u proximadb >/dev/null 2>&1; then
  useradd --system --no-create-home --shell /usr/sbin/nologin proximadb
fi

# Create data and log directories
mkdir -p /var/lib/proximadb
mkdir -p /var/log/proximadb
chown proximadb:proximadb /var/lib/proximadb
chown proximadb:proximadb /var/log/proximadb

# Create config directory if it doesn't exist
mkdir -p /etc/proximadb

# Reload systemd and enable the service
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
  systemctl enable proximadb
fi
