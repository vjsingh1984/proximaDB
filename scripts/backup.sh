#!/bin/bash
# ProximaDB Backup Script
#
# Creates incremental backups of ProximaDB data with configurable retention
#
# Usage:
#   ./scripts/backup.sh [--full] [--target local|s3|gcs|azure]
#
# Environment Variables:
#   PROXIMADB_BASE_PATH - Base path for ProximaDB data (default: /tmp/proximadb)
#   BACKUP_TARGET - Target for backups (default: local)
#   BACKUP_RETENTION - Number of backups to retain (default: 7)
#   BACKUP_COMPRESSION - Enable compression (default: true)
#
# Examples:
#   ./scripts/backup.sh                          # Local incremental backup
#   ./scripts/backup.sh --full                  # Local full backup
#   ./scripts/backup.sh --target s3             # S3 backup
#   PROXIMADB_BASE_PATH=/data ./scripts/backup.sh  # Custom data path

set -e

# Default values
PROXIMADB_BASE_PATH="${PROXIMADB_BASE_PATH:-/tmp/proximadb}"
BACKUP_RETENTION="${BACKUP_RETENTION:-7}"
BACKUP_COMPRESSION="${BACKUP_COMPRESSION:-true}"

# Parse arguments
FULL_BACKUP=false
BACKUP_TARGET="local"

while [[ $# -gt 0 ]]; do
    case $1 in
        --full)
            FULL_BACKUP=true
            shift
            ;;
        --target)
            BACKUP_TARGET="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Calculate backup ID timestamp
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_ID="backup_${TIMESTAMP}"

echo "======================================"
echo "ProximaDB Backup"
echo "======================================"
echo "Backup ID: ${BACKUP_ID}"
echo "Base Path: ${PROXIMADB_BASE_PATH}"
echo "Target: ${BACKUP_TARGET}"
echo "Retention: ${BACKUP_RETENTION} backups"
echo "======================================"

# Check if ProximaDB is running
if ! pgrep -f "proximadb-server" > /dev/null; then
    echo "WARNING: ProximaDB server does not appear to be running"
    echo "Continuing with offline backup..."
fi

# Trigger backup via REST API if server is running
if pgrep -f "proximadb-server" > /dev/null; then
    echo "Triggering online backup via REST API..."

    # Use curl to trigger backup
    RESPONSE=$(curl -s -X POST "http://localhost:5678/api/v1/admin/backup" \
        -H "Content-Type: application/json" \
        -d "{
            \"backup_id\": \"${BACKUP_ID}\",
            \"backup_type\": \"$(if [ "$FULL_BACKUP" = true ]; then echo "full"; else echo "incremental"; fi)\",
            \"target\": \"${BACKUP_TARGET}\",
            \"retention\": ${BACKUP_RETENTION},
            \"compression\": ${BACKUP_COMPRESSION}
        }")

    echo "API Response: ${RESPONSE}"

    # Check if backup was successful
    if echo "${RESPONSE}" | grep -q "success\|completed"; then
        echo "✅ Backup completed successfully"
        exit 0
    else
        echo "❌ Backup failed, falling back to filesystem backup..."
    fi
fi

# Fallback: Filesystem backup
BACKUP_DIR="${PROXIMADB_BASE_PATH}/backups/${BACKUP_ID}"
mkdir -p "${BACKUP_DIR}"

echo "Creating filesystem backup at: ${BACKUP_DIR}"

# Create data directory backup
DATA_DIR="${PROXIMADB_BASE_PATH}/d1"
if [ -d "${DATA_DIR}" ]; then
    echo "Backing up collections directory..."
    cp -r "${DATA_DIR}" "${BACKUP_DIR}/"

    # Count files and calculate size
    FILE_COUNT=$(find "${BACKUP_DIR}" -type f | wc -l)
    BACKUP_SIZE=$(du -sh "${BACKUP_DIR}" | cut -f1)

    echo "✅ Filesystem backup completed"
    echo "   Files: ${FILE_COUNT}"
    echo "   Size: ${BACKUP_SIZE}"
else
    echo "WARNING: No data directory found at ${DATA_DIR}"
fi

# Backup WAL segments
WAL_DIR="${PROXIMADB_BASE_PATH}/wal"
if [ -d "${WAL_DIR}" ]; then
    echo "Backing up WAL segments..."
    mkdir -p "${BACKUP_DIR}/wal"
    cp -r "${WAL_DIR}"/* "${BACKUP_DIR}/wal/" 2>/dev/null || echo "No WAL files to backup"
    echo "✅ WAL backup completed"
fi

# Create backup manifest
cat > "${BACKUP_DIR}/manifest.json" <<EOF
{
  "backup_id": "${BACKUP_ID}",
  "timestamp": $(date +%s%N),
  "backup_type": "$(if [ "$FULL_BACKUP" = true ]; then echo "full"; else echo "incremental"; fi)",
  "target": "${BACKUP_TARGET}",
  "base_path": "${PROXIMADB_BASE_PATH}",
  "compression": ${BACKUP_COMPRESSION},
  "retention": ${BACKUP_RETENTION}
}
EOF

echo ""
echo "======================================"
echo "Backup Summary"
echo "======================================"
echo "Backup ID: ${BACKUP_ID}"
echo "Location: ${BACKUP_DIR}"
echo "Timestamp: ${TIMESTAMP}"
echo "Type: $(if [ "$FULL_BACKUP" = true ]; then echo "Full"; else echo "Incremental"; fi)"
echo ""
echo "To restore this backup, run:"
echo "  ./scripts/restore.sh ${BACKUP_ID}"
echo ""
echo "To list all backups, run:"
echo "  ./scripts/backup.sh --list"
echo "======================================"

# Cleanup old backups based on retention policy
BACKUP_BASE="${PROXIMADB_BASE_PATH}/backups"
if [ -d "${BACKUP_BASE_PATH}" ]; then
    echo "Cleaning up old backups (retaining ${BACKUP_RETENTION} most recent)..."

    # List backups by timestamp, remove old ones
    ls -1t "${BACKUP_BASE_PATH}" | tail -n +$((BACKUP_RETENTION + 1)) | while read OLD_BACKUP; do
        if [ -n "${OLD_BACKUP}" ] && [ -d "${BACKUP_BASE_DIR}/${OLD_BACKUP}" ]; then
            echo "Removing old backup: ${OLD_BACKUP}"
            rm -rf "${BACKUP_BASE_DIR}/${OLD_BACKUP}"
        fi
    done
fi

echo "✅ Backup completed successfully!"
