#!/bin/bash
# Helper script to identify and suggest fixes for unwrap() calls
# Usage: ./scripts/fix_unwraps.sh <module_path>

MODULE_PATH=${1:-src/storage/engines/impls/tst}

echo "=== Scanning $MODULE_PATH for unwrap() calls ==="
echo ""

# Count unwrap calls
UNWRAP_COUNT=$(find "$MODULE_PATH" -name "*.rs" ! -path "*/tests/*" ! -name "*_test.rs" -exec grep -h "\.unwrap()" {} \; | wc -l | tr -d ' ')

echo "Found $UNWRAP_COUNT unwrap() calls in production code"
echo ""

# Show top files with most unwrap calls
echo "=== Files with most unwrap() calls ==="
find "$MODULE_PATH" -name "*.rs" ! -path "*/tests/*" ! -name "*_test.rs" -exec sh -c 'echo "$(grep -h "\.unwrap()" "$1" 2>/dev/null | wc -l | tr -d " ") $1"' {} \; | sort -rn | head -10
echo ""

# Show sample unwrap calls
echo "=== Sample unwrap() calls ==="
find "$MODULE_PATH" -name "*.rs" ! -path "*/tests/*" ! -name "*_test.rs" -exec grep -Hn "\.unwrap()" {} \; | head -10
echo ""

echo "=== Suggested fix pattern ==="
echo "Before: collection.get(&id).unwrap()"
echo "After:  collection.get(&id)"
echo "           .ok_or_else(|| Error::CollectionNotFound(id))?"
echo ""
echo "Before: result.unwrap()"
echo "After:  result.map_err(|e| Error::context(format!(\"operation failed: {}\", e)))?"
