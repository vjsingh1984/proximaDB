#!/bin/bash
# Recovery Test with Fresh Data
# Tests write/read format compatibility after cleanup

echo "==================================================================="
echo "WAL RECOVERY TEST WITH FRESH DATA"
echo "==================================================================="
echo ""

# Ensure we're in project root
cd "$(dirname "$0")"

# 1. Verify cleanup
echo "1️⃣  Verifying cleanup..."
if [ -d "/tmp/proximadb" ]; then
    echo "   ⚠️  /tmp/proximadb exists, removing..."
    rm -rf /tmp/proximadb
fi
echo "   ✅ /tmp/proximadb clean"
echo ""

# 2. Verify build is current
echo "2️⃣  Verifying release build..."
if [ ! -f "target/release/proximadb-server" ]; then
    echo "   ❌ Release binary not found"
    echo "   Building..."
    cargo build --release
fi
echo "   ✅ Release binary ready"
echo ""

# 3. Run recovery test
echo "3️⃣  Running WAL persistence test..."
echo "   This will:"
echo "   - Start server"
echo "   - Create collection with 20 vectors"
echo "   - Restart server"
echo "   - Check if vectors recovered"
echo ""

cd clients/python
export PYTHONPATH=src

python3 tests/server_lifecycle/test_wal_persistence_detailed.py 2>&1 | tee /tmp/recovery_test.log

# 4. Extract results
echo ""
echo "==================================================================="
echo "TEST RESULTS"
echo "==================================================================="
echo ""

# Show recovery status
grep -A 5 "Recovery Status" /tmp/recovery_test.log || echo "❌ Test didn't complete"

# Show any errors
echo ""
echo "Errors encountered:"
grep "❌" /tmp/recovery_test.log | head -10 || echo "None"

# Check for success
if grep -q "SUCCESS: WAL persistence working correctly" /tmp/recovery_test.log; then
    echo ""
    echo "==================================================================="
    echo "✅ RECOVERY TEST PASSED!"
    echo "==================================================================="
    exit 0
else
    echo ""
    echo "==================================================================="
    echo "❌ RECOVERY TEST FAILED"
    echo "==================================================================="
    echo "Full log available at: /tmp/recovery_test.log"
    exit 1
fi
