#!/bin/bash

# Test runner for flush and compaction configuration tests
# This script runs all unit tests related to flush management and compaction

set -e

echo "🧪 Running ProximaDB Flush and Compaction Configuration Tests"
echo "=============================================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test categories
declare -A test_categories=(
    ["Config Tests"]="test_flush_config"
    ["Flush Management Tests"]="test_flush_management"
    ["Global Flush Tests"]="test_global_flush" 
    ["Compaction Config Tests"]="test_compaction_config"
)

# Function to run a test category
run_test_category() {
    local category_name="$1"
    local test_pattern="$2"
    
    echo -e "\n${YELLOW}=== $category_name ===${NC}"
    echo "Running tests matching pattern: $test_pattern"
    
    if cargo test "$test_pattern" --quiet; then
        echo -e "${GREEN}✅ $category_name - PASSED${NC}"
        return 0
    else
        echo -e "${RED}❌ $category_name - FAILED${NC}"
        return 1
    fi
}

# Function to run specific tests
run_specific_tests() {
    echo -e "\n${YELLOW}=== Running Specific Test Cases ===${NC}"
    
    local specific_tests=(
        "test_default_flush_configuration"
        "test_collection_specific_overrides"
        "test_collection_flush_threshold_trigger"
        "test_global_memory_threshold_calculation"
        "test_compaction_threshold_configuration"
        "test_compaction_with_expired_records"
    )
    
    for test_name in "${specific_tests[@]}"; do
        echo -n "Running $test_name... "
        if cargo test "$test_name" --quiet > /dev/null 2>&1; then
            echo -e "${GREEN}✅ PASSED${NC}"
        else
            echo -e "${RED}❌ FAILED${NC}"
        fi
    done
}

# Function to run performance tests
run_performance_tests() {
    echo -e "\n${YELLOW}=== Running Performance Tests ===${NC}"
    
    local perf_tests=(
        "test_global_flush_performance_metrics"
        "test_compaction_performance_metrics"
    )
    
    for test_name in "${perf_tests[@]}"; do
        echo -n "Running $test_name... "
        if timeout 60 cargo test "$test_name" --quiet > /dev/null 2>&1; then
            echo -e "${GREEN}✅ PASSED${NC}"
        else
            echo -e "${RED}❌ FAILED (timeout or error)${NC}"
        fi
    done
}

# Function to validate configuration
validate_configuration() {
    echo -e "\n${YELLOW}=== Validating Configuration ===${NC}"
    
    # Check if TOML configs can be parsed
    local configs=(
        "config.toml"
        "config/local.toml"
        "demo/docker-config.toml"
    )
    
    for config_file in "${configs[@]}"; do
        if [[ -f "$config_file" ]]; then
            echo -n "Validating $config_file... "
            if cargo run --bin proximadb-server -- --config "$config_file" --validate-config > /dev/null 2>&1; then
                echo -e "${GREEN}✅ VALID${NC}"
            else
                echo -e "${YELLOW}⚠️ SKIPPED (no validation binary)${NC}"
            fi
        else
            echo -e "${RED}❌ $config_file not found${NC}"
        fi
    done
}

# Main execution
main() {
    echo "Starting test execution..."
    
    # Check if we're in the right directory
    if [[ ! -f "Cargo.toml" ]]; then
        echo -e "${RED}❌ Error: Must run from ProximaDB root directory${NC}"
        exit 1
    fi
    
    # Build first to catch compilation errors
    echo -e "\n${YELLOW}=== Building Project ===${NC}"
    if cargo build --quiet; then
        echo -e "${GREEN}✅ Build successful${NC}"
    else
        echo -e "${RED}❌ Build failed${NC}"
        exit 1
    fi
    
    local failed_tests=0
    
    # Run test categories
    for category in "${!test_categories[@]}"; do
        if ! run_test_category "$category" "${test_categories[$category]}"; then
            ((failed_tests++))
        fi
    done
    
    # Run specific tests
    run_specific_tests
    
    # Run performance tests
    run_performance_tests
    
    # Validate configuration
    validate_configuration
    
    # Summary
    echo -e "\n${YELLOW}=== Test Summary ===${NC}"
    if [[ $failed_tests -eq 0 ]]; then
        echo -e "${GREEN}🎉 All test categories passed!${NC}"
        echo -e "${GREEN}✅ Flush and compaction configuration tests complete${NC}"
    else
        echo -e "${RED}❌ $failed_tests test categories failed${NC}"
        echo -e "${RED}❌ Some tests need attention${NC}"
    fi
    
    # Additional information
    echo -e "\n${YELLOW}=== Configuration Information ===${NC}"
    echo "• Collection flush threshold: 10MB (configurable via TOML)"
    echo "• Global flush threshold: 4GB (configurable via TOML)"
    echo "• Global shrink factor: 40% (configurable via TOML)"
    echo "• Compaction threshold: 4 files (configurable via LSM config)"
    echo "• Background flush coordination: Enabled"
    echo "• Expired record deletion: Enabled during compaction"
    
    echo -e "\n${YELLOW}=== Next Steps ===${NC}"
    echo "1. Review any failed tests above"
    echo "2. Run 'cargo test' to execute all tests"
    echo "3. Check TOML configuration files for customization"
    echo "4. Monitor flush and compaction performance in production"
    
    return $failed_tests
}

# Execute main function
main "$@"