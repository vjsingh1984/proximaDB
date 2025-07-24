#!/bin/bash
echo "Building and running VIPER compaction test..."

# Build with explicit features
cargo test --release viper::tests::compaction_tests::test_basic_compaction -- --nocapture 2>&1 | tee /tmp/compact_test_output.log

# Check if we see our new code marker
echo -e "\n\nChecking for new code marker..."
if grep -q "NEW CODE VERSION" /tmp/compact_test_output.log; then
    echo "✅ Test is using NEW code!"
else
    echo "❌ Test is using OLD code!"
fi

# Check what happened with records
echo -e "\n\nChecking record processing..."
grep -E "Final records count|should_keep|Building RecordBatch" /tmp/compact_test_output.log | head -20