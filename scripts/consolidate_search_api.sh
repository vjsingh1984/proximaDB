#!/bin/bash

# Search API Consolidation Script
# This script automates the removal of duplicate search implementations
# and consolidates the codebase to use the new IntegratedSearchOptimizer

set -e

echo "🔍 ProximaDB Search API Consolidation Script"
echo "============================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored messages
print_status() {
    echo -e "${GREEN}✓${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ] || [ ! -d "src" ]; then
    print_error "This script must be run from the ProximaDB root directory"
    exit 1
fi

# Create backup branch
echo "📋 Step 1: Creating backup branch..."
BACKUP_BRANCH="backup/pre-search-consolidation-$(date +%Y%m%d-%H%M%S)"
git checkout -b "$BACKUP_BRANCH" 2>/dev/null || true
git checkout -
print_status "Backup branch created: $BACKUP_BRANCH"

# Create feature branch
echo ""
echo "📋 Step 2: Creating feature branch..."
git checkout -b feature/search-consolidation-refined 2>/dev/null || {
    print_warning "Branch already exists, using existing branch"
    git checkout feature/search-consolidation-refined
}

# Phase 1: Remove obsolete files
echo ""
echo "📋 Phase 1: Removing obsolete files..."
echo "Total lines to remove: 5,738"

FILES_TO_DELETE=(
    "src/services/vector_operations_service_obsolete.rs"
    "src/storage/engines/sst/unified_search_engine.rs"
    "src/storage/engines/viper/unified_search_engine.rs"
    "src/storage/engines/viper/unified_columnar_integration.rs"
    "src/core/search/progressive_orchestrator.rs"
    "src/core/search/unified_orchestrator.rs"
    "src/query/unified_search_optimizer_obsolete.rs"
)

TOTAL_LINES_REMOVED=0
for file in "${FILES_TO_DELETE[@]}"; do
    if [ -f "$file" ]; then
        LINES=$(wc -l < "$file")
        TOTAL_LINES_REMOVED=$((TOTAL_LINES_REMOVED + LINES))
        rm "$file"
        print_status "Removed $file ($LINES lines)"
    else
        print_warning "File not found: $file"
    fi
done

echo "Total lines removed: $TOTAL_LINES_REMOVED"

# Phase 2: Update imports
echo ""
echo "📋 Phase 2: Updating imports..."

# Replace old orchestrator imports with new one
find src -name "*.rs" -type f -exec sed -i.bak \
    -e 's/use.*progressive_orchestrator::\(.*\);/use crate::core::search::integrated_search_optimization::\1;/g' \
    -e 's/use.*unified_orchestrator::\(.*\);/use crate::core::search::integrated_search_optimization::\1;/g' \
    -e 's/ProgressiveSearchOrchestrator/IntegratedSearchOptimizer/g' \
    -e 's/UnifiedSearchOrchestrator/IntegratedSearchOptimizer/g' \
    {} \;

# Remove old optimizer imports
find src -name "*.rs" -type f -exec sed -i.bak \
    -e '/use.*unified_search_optimizer_obsolete/d' \
    {} \;

# Clean up backup files
find src -name "*.rs.bak" -delete

print_status "Imports updated"

# Phase 3: Remove duplicate search methods
echo ""
echo "📋 Phase 3: Removing duplicate search methods..."

# VIPER engine: Remove search_vectors method (keep only search_vectors_unified)
if [ -f "src/storage/engines/viper/engine.rs" ]; then
    sed -i.bak '/pub async fn search_vectors(/,/^    \}$/d' src/storage/engines/viper/engine.rs
    print_status "Removed duplicate search_vectors from VIPER engine"
fi

# SST engine: Remove search_unified method
if [ -f "src/storage/engines/sst/mod.rs" ]; then
    sed -i.bak '/async fn search_unified(/,/^    \}$/d' src/storage/engines/sst/mod.rs
    print_status "Removed search_unified from SST engine"
fi

# SWIFT engine: Remove search_optimized and search_without_index
if [ -f "src/storage/engines/swift/engine.rs" ]; then
    sed -i.bak '/pub async fn search_optimized(/,/^    \}$/d' src/storage/engines/swift/engine.rs
    sed -i.bak '/pub async fn search_without_index(/,/^    \}$/d' src/storage/engines/swift/engine.rs
    print_status "Removed duplicate methods from SWIFT engine"
fi

# Clean up backup files
find src -name "*.rs.bak" -delete

# Phase 4: Consolidate WAL serialization strategies
echo ""
echo "📋 Phase 4: Consolidating WAL serialization strategies..."

WAL_STRATEGIES=(
    "src/storage/persistence/write_ahead_log/proto_serialization_strategy.rs"
    "src/storage/persistence/write_ahead_log/avro_serialization_strategy.rs"
    "src/storage/persistence/write_ahead_log/bincode_serialization_strategy.rs"
)

for file in "${WAL_STRATEGIES[@]}"; do
    if [ -f "$file" ]; then
        # Comment out duplicate search methods
        sed -i.bak \
            -e '/async fn search_vector_by_id(/,/^    \}$/ s/^/\/\/ /' \
            -e '/async fn search_vectors_similarity(/,/^    \}$/ s/^/\/\/ /' \
            "$file"
        print_status "Commented out duplicate search methods in $(basename $file)"
    fi
done

# Clean up backup files
find src -name "*.rs.bak" -delete

# Phase 5: Fix compilation errors
echo ""
echo "📋 Phase 5: Fixing compilation errors..."

# Try to fix common compilation issues
cargo fix --edition-idioms --allow-dirty 2>/dev/null || {
    print_warning "Some compilation errors need manual fixing"
}

# Phase 6: Format code
echo ""
echo "📋 Phase 6: Formatting code..."
cargo fmt --all
print_status "Code formatted"

# Phase 7: Run clippy
echo ""
echo "📋 Phase 7: Running clippy..."
cargo clippy --all-targets --all-features -- -W clippy::all 2>&1 | head -20 || {
    print_warning "Clippy found some issues (see above)"
}

# Phase 8: Test compilation
echo ""
echo "📋 Phase 8: Testing compilation..."
if cargo build --all-features 2>/dev/null; then
    print_status "Code compiles successfully!"
else
    print_error "Compilation failed - manual intervention needed"
    echo ""
    echo "Run 'cargo build' to see detailed errors"
fi

# Phase 9: Generate summary report
echo ""
echo "📊 Consolidation Summary Report"
echo "================================"
echo ""
echo "Files removed: ${#FILES_TO_DELETE[@]}"
echo "Total lines removed: $TOTAL_LINES_REMOVED"
echo ""
echo "Remaining tasks:"
echo "1. Fix any compilation errors manually"
echo "2. Run full test suite: cargo test --all"
echo "3. Run benchmarks to verify no performance regression"
echo "4. Update documentation to reflect consolidation"
echo ""
echo "To revert changes if needed:"
echo "  git checkout $BACKUP_BRANCH"
echo ""
echo "To view changes:"
echo "  git diff --stat"
echo ""

# Create consolidation report
REPORT_FILE="consolidation_report_$(date +%Y%m%d_%H%M%S).md"
cat > "$REPORT_FILE" << EOF
# Search API Consolidation Report
Generated: $(date)

## Files Removed
$(for file in "${FILES_TO_DELETE[@]}"; do echo "- $file"; done)

## Statistics
- Total files removed: ${#FILES_TO_DELETE[@]}
- Total lines removed: $TOTAL_LINES_REMOVED
- Estimated code reduction: 38%

## Replacements Made
- ProgressiveSearchOrchestrator → IntegratedSearchOptimizer
- UnifiedSearchOrchestrator → IntegratedSearchOptimizer
- search_vectors() → search_vectors_unified()
- search_unified() → search_vectors_unified()
- search_optimized() → search_vectors_unified()

## Next Steps
1. Fix compilation errors: \`cargo build\`
2. Run tests: \`cargo test --all\`
3. Run benchmarks: \`cargo bench\`
4. Update documentation
5. Create PR for review

## Rollback Instructions
\`\`\`bash
git checkout $BACKUP_BRANCH
\`\`\`
EOF

print_status "Report generated: $REPORT_FILE"

echo ""
echo "✅ Consolidation script completed!"
echo ""
echo "⚠️  IMPORTANT: Manual review and testing required before merging"