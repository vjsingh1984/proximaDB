#!/bin/bash

# Script to fix undefined 'key' variables in test files

# Find all test files with undefined key variables
files=$(find tests -name "*.rs" -exec grep -l "\.get(key)" {} \;)

for file in $files; do
    echo "Fixing key variables in $file..."

    # Fix common patterns for getting JSON values
    sed -i '' 's/\.get(key)/\.get("enable_two_stage_search")/g' "$file"
    sed -i '' 's/hints_json\.get("enable_two_stage_search")\.and_then.*hints_json\.get("enable_two_stage_search")/hints_json.get("candidate_multiplier")/g' "$file"
    sed -i '' 's/sorted_latencies\.get("enable_two_stage_search")/sorted_latencies.get(\&(index \/ 2))/g' "$file"
    sed -i '' 's/vector_sets\.get("enable_two_stage_search")/vector_sets.get(\&i)/g' "$file"
    sed -i '' 's/hints_map\.get("enable_two_stage_search")/hints_map.get("quantization_hint")/g' "$file"
    sed -i '' 's/search_hints\.get("enable_two_stage_search")/search_hints.get("candidate_multiplier")/g' "$file"
    sed -i '' 's/results\.get("enable_two_stage_search")/results.get("sparse")/g' "$file"
    sed -i '' 's/metadata\.get("enable_two_stage_search")/metadata.get("active")/g' "$file"

    # More specific fixes
    sed -i '' 's/search_request\.get(&"enable_two_stage_search")/search_request.get("enable_two_stage_search")/g' "$file"

    # Fix p50/p99 calculations that should use indices
    sed -i '' 's/sorted_latencies\.get("candidate_multiplier")/sorted_latencies.get(percentile_50_index)/g' "$file"
    sed -i '' 's/sorted_filter_latencies\.get("candidate_multiplier")/sorted_filter_latencies.get(percentile_50_index)/g' "$file"
done

echo "Key variable fixes completed."