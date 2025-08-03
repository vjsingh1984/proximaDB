#!/bin/bash

# Fix all test files to use typed metadata

# List of test files with metadata issues
test_files=(
    "tests/unit/storage/sst_core_tests.rs"
    "tests/unit/storage/sst_atomic_operations_test.rs"
    "tests/unit/storage/sst_flush_test.rs"
    "tests/integration/isolated_sst_engine_test.rs"
    "tests/integration/unified_search_integration.rs"
    "tests/unit/search/multi_tier_deduplication_tests.rs"
    "tests/integration/sst_search_integration_test.rs"
    "tests/integration/sst_collection_test.rs"
    "tests/rust/unit_tests.rs"
    "tests/unit/write_buffer_recovery_stress_tests.rs"
)

# Common replacements for string values
for file in "${test_files[@]}"; do
    if [ -f "$file" ]; then
        echo "Fixing $file..."
        
        # Replace simple string metadata values
        sed -i 's/value: "\([^"]*\)"\.to_string()/value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("\1".to_string()))/g' "$file"
        
        # Replace category/type style metadata
        sed -i 's/value: \([a-zA-Z_][a-zA-Z0-9_]*\)\[\([^]]*\)\]\.to_string()/value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(\1[\2].to_string()))/g' "$file"
        
        # Replace numeric to_string() metadata
        sed -i 's/value: \([a-zA-Z0-9_()/ ]*\)\.to_string()/value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(\1.to_string()))/g' "$file"
        
        # Replace format! macro metadata
        sed -i 's/value: format!(\([^)]*\))/value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(format!(\1)))/g' "$file"
    fi
done

echo "Done fixing test files!"