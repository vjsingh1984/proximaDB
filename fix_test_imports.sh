#!/bin/bash

# Fix test imports to include the common module

# Find all test files that use common::integration_test_helpers
files=$(grep -r "common::integration_test_helpers" tests/ --include="*.rs" -l 2>/dev/null)

for file in $files; do
    # Skip if already has the module import
    if grep -q "#\[path = " "$file"; then
        echo "Skipping $file (already has module import)"
        continue
    fi

    echo "Fixing $file..."

    # Calculate relative path to common/mod.rs based on file location
    depth=$(echo "$file" | tr '/' '\n' | wc -l)
    if [[ $file == tests/integration/*.rs ]]; then
        path_to_common="../common/mod.rs"
    elif [[ $file == tests/integration/*/*.rs ]]; then
        path_to_common="../../common/mod.rs"
    elif [[ $file == tests/unit/*/*.rs ]]; then
        path_to_common="../../common/mod.rs"
    elif [[ $file == tests/rust/*.rs ]]; then
        path_to_common="../common/mod.rs"
    elif [[ $file == tests/search/*.rs ]]; then
        path_to_common="../common/mod.rs"
    else
        path_to_common="../common/mod.rs"
    fi

    # Create temporary file with the module import at the beginning
    {
        echo "// Import the common test helpers"
        echo "#[path = \"$path_to_common\"]"
        echo "mod common;"
        echo ""
        cat "$file"
    } > "$file.tmp"

    # Replace the original file
    mv "$file.tmp" "$file"
done

echo "Done fixing test imports!"