#!/bin/bash
# Script to monitor ProximaDB server logs for WAL and VIPER operations

LOG_FILE="/tmp/proximadb_server_grpc.log"

echo "🔍 Monitoring ProximaDB Server Logs for WAL and VIPER Operations"
echo "=================================================="
echo "Log file: $LOG_FILE"
echo ""
echo "Watching for:"
echo "  💾 WAL write operations"
echo "  🔄 Flush operations" 
echo "  📁 VIPER storage operations"
echo "  ✅ Successful operations"
echo "  🧠 Collection operations"
echo ""
echo "Press Ctrl+C to stop monitoring"
echo ""

# Check if log file exists
if [ ! -f "$LOG_FILE" ]; then
    echo "❌ Log file not found: $LOG_FILE"
    echo "Make sure the ProximaDB server is running and logging to this file"
    exit 1
fi

# Monitor logs with color coding
tail -f "$LOG_FILE" | grep --line-buffered -E '💾|🔄|📁|✅|🧠|WAL|VIPER|Flush|flush|wal.*write|write.*wal|batch.*insert|UUID|uuid' | \
while read line; do
    # Add timestamp for clarity
    timestamp=$(date "+%H:%M:%S")
    echo "[$timestamp] $line"
done