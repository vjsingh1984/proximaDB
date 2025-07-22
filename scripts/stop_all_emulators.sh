#!/bin/bash
# Stop all cloud storage emulators

echo "Stopping cloud storage emulators..."

# Stop MinIO
pkill -f "minio server" && echo "MinIO stopped" || echo "MinIO not running"

# Stop fake-gcs-server
pkill -f "fake-gcs-server" && echo "fake-gcs-server stopped" || echo "fake-gcs-server not running"

# Stop Azurite
pkill -f "azurite" && echo "Azurite stopped" || echo "Azurite not running"

echo "All emulators stopped"