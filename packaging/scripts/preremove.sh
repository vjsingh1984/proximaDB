#!/bin/bash
# Pre-remove script for ProximaDB package

set -e

# Stop and disable the service if it's running
if command -v systemctl >/dev/null 2>&1; then
    if systemctl is-active --quiet proximadb.service; then
        echo "Stopping proximadb service..."
        systemctl stop proximadb.service
    fi

    if systemctl is-enabled --quiet proximadb.service; then
        echo "Disabling proximadb service..."
        systemctl disable proximadb.service
    fi
fi

echo "ProximaDB service stopped and disabled"
