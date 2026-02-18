#!/usr/bin/env bash
# ProximaDB pre-remove script
# Stops and disables the systemd service before package removal.

set -e

if command -v systemctl >/dev/null 2>&1; then
  systemctl stop proximadb 2>/dev/null || true
  systemctl disable proximadb 2>/dev/null || true
fi
