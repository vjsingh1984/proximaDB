#!/bin/bash
# ProximaDB Demo Test Runner
# Automated testing script for all demos - CI/CD ready
#
# Usage:
#   ./demo/run_all_demos.sh              # Run all demos
#   ./demo/run_all_demos.sh --quick      # Run quick demos only
#   ./demo/run_all_demos.sh --verbose    # Show detailed output
#
# Exit codes:
#   0 = All demos passed
#   1 = Some demos failed
#   2 = Environment not ready

set -e  # Exit on error for setup commands

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VERBOSE=false
QUICK_MODE=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --verbose|-v)
            VERBOSE=true
            shift
            ;;
        --quick|-q)
            QUICK_MODE=true
            shift
            ;;
        --help|-h)
            echo "ProximaDB Demo Test Runner"
            echo ""
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --verbose, -v    Show detailed demo output"
            echo "  --quick, -q      Run only quick demos (<10s each)"
            echo "  --help, -h       Show this help message"
            echo ""
            echo "Exit codes:"
            echo "  0 = All demos passed"
            echo "  1 = Some demos failed"
            echo "  2 = Environment not ready"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 2
            ;;
    esac
done

# Print header
echo -e "${BLUE}=====================================================================${NC}"
echo -e "${BLUE}  ProximaDB Demo Test Runner${NC}"
echo -e "${BLUE}=====================================================================${NC}"
echo ""

# Step 1: Check environment
echo -e "${BLUE}🔍 Step 1: Checking environment...${NC}"
cd "$REPO_ROOT"

if python3 demo/check_demo_health.py > /tmp/demo_health_check.log 2>&1; then
    echo -e "${GREEN}   ✅ Environment ready${NC}"
else
    exit_code=$?
    echo -e "${RED}   ❌ Environment not ready (exit code: $exit_code)${NC}"
    echo ""
    echo "Health check output:"
    cat /tmp/demo_health_check.log
    echo ""
    echo -e "${YELLOW}Fix the issues above and try again${NC}"
    exit 2
fi

# Step 2: Setup environment
echo ""
echo -e "${BLUE}🔧 Step 2: Setting up environment...${NC}"
export PYTHONPATH="${REPO_ROOT}/clients/python/src"
export PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python
echo -e "${GREEN}   ✅ PYTHONPATH set: $PYTHONPATH${NC}"

# Step 3: Define demos to test
declare -A DEMOS
declare -A TIMEOUTS
declare -A DESCRIPTIONS

# Quick demos (<10s)
DEMOS[basic_demo]="demo/quickstart/basic_demo.py"
TIMEOUTS[basic_demo]=10
DESCRIPTIONS[basic_demo]="Core vector operations"

DEMOS[feature_showcase]="demo/quickstart/feature_showcase.py"
TIMEOUTS[feature_showcase]=10
DESCRIPTIONS[feature_showcase]="Multi-feature overview"

DEMOS[chunking_demo]="demo/showcases/features/chunking_demo.py"
TIMEOUTS[chunking_demo]=15
DESCRIPTIONS[chunking_demo]="Text chunking strategies"

DEMOS[wal_search]="demo/showcases/features/wal_search.py"
TIMEOUTS[wal_search]=10
DESCRIPTIONS[wal_search]="WAL operations"

# Longer demos (>10s)
if [ "$QUICK_MODE" = false ]; then
    DEMOS[metadata_filtering]="demo/showcases/features/metadata_filtering.py"
    TIMEOUTS[metadata_filtering]=20
    DESCRIPTIONS[metadata_filtering]="Server-side filtering (gRPC)"

    DEMOS[quantization_demo]="demo/showcases/features/quantization_demo.py"
    TIMEOUTS[quantization_demo]=90
    DESCRIPTIONS[quantization_demo]="Vector quantization (10K vectors)"
fi

# Step 4: Run demos
echo ""
echo -e "${BLUE}🚀 Step 3: Running ${#DEMOS[@]} demos...${NC}"
echo ""

PASSED=0
FAILED=0
SKIPPED=0
declare -a FAILED_DEMOS

for demo_name in "${!DEMOS[@]}"; do
    demo_path="${DEMOS[$demo_name]}"
    timeout_val="${TIMEOUTS[$demo_name]}"
    description="${DESCRIPTIONS[$demo_name]}"

    # Print demo info
    printf "%-25s %-35s " "$demo_name" "$description"

    # Check if demo file exists
    if [ ! -f "$demo_path" ]; then
        echo -e "${YELLOW}SKIP (not found)${NC}"
        ((SKIPPED++))
        continue
    fi

    # Run demo with timeout
    log_file="/tmp/demo_${demo_name}_$(date +%s).log"

    if [ "$VERBOSE" = true ]; then
        echo ""
        echo -e "${BLUE}--- Running: python3 $demo_path ---${NC}"
        if timeout "$timeout_val" python3 "$demo_path" 2>&1 | tee "$log_file"; then
            echo -e "${GREEN}✅ PASS${NC}"
            ((PASSED++))
        else
            exit_code=$?
            if [ $exit_code -eq 124 ]; then
                echo -e "${RED}❌ FAIL (timeout after ${timeout_val}s)${NC}"
            else
                echo -e "${RED}❌ FAIL (exit code: $exit_code)${NC}"
            fi
            ((FAILED++))
            FAILED_DEMOS+=("$demo_name")
        fi
        echo ""
    else
        if timeout "$timeout_val" python3 "$demo_path" > "$log_file" 2>&1; then
            echo -e "${GREEN}✅ PASS${NC}"
            ((PASSED++))
            rm -f "$log_file"  # Clean up successful runs
        else
            exit_code=$?
            if [ $exit_code -eq 124 ]; then
                echo -e "${RED}❌ FAIL (timeout)${NC}"
            else
                echo -e "${RED}❌ FAIL${NC}"
            fi
            ((FAILED++))
            FAILED_DEMOS+=("$demo_name")
            echo -e "   ${YELLOW}Log: $log_file${NC}"
        fi
    fi
done

# Step 5: Summary
echo ""
echo -e "${BLUE}=====================================================================${NC}"
echo -e "${BLUE}  Test Summary${NC}"
echo -e "${BLUE}=====================================================================${NC}"
echo ""

TOTAL=$((PASSED + FAILED + SKIPPED))
echo "Total demos: $TOTAL"
echo -e "${GREEN}Passed: $PASSED${NC}"
if [ $FAILED -gt 0 ]; then
    echo -e "${RED}Failed: $FAILED${NC}"
fi
if [ $SKIPPED -gt 0 ]; then
    echo -e "${YELLOW}Skipped: $SKIPPED${NC}"
fi
echo ""

# Success rate
if [ $TOTAL -gt 0 ]; then
    SUCCESS_RATE=$((PASSED * 100 / TOTAL))
    if [ $SUCCESS_RATE -eq 100 ]; then
        echo -e "${GREEN}Success rate: ${SUCCESS_RATE}% 🎉${NC}"
    elif [ $SUCCESS_RATE -ge 80 ]; then
        echo -e "${YELLOW}Success rate: ${SUCCESS_RATE}%${NC}"
    else
        echo -e "${RED}Success rate: ${SUCCESS_RATE}%${NC}"
    fi
fi

# Failed demos details
if [ $FAILED -gt 0 ]; then
    echo ""
    echo -e "${RED}Failed demos:${NC}"
    for demo in "${FAILED_DEMOS[@]}"; do
        echo "  - $demo"
    done
    echo ""
    echo -e "${YELLOW}Check log files in /tmp/demo_*.log for details${NC}"
fi

echo ""
echo -e "${BLUE}=====================================================================${NC}"

# Exit with appropriate code
if [ $FAILED -gt 0 ]; then
    exit 1
else
    exit 0
fi
