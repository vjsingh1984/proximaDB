#!/bin/bash

# Fix duplicate module definitions in test files

echo "Fixing duplicate module definitions in test files..."

# Find all test files with duplicate module definitions
files=$(grep -r "mod common {" tests/ --include="*.rs" -l 2>/dev/null)

for file in $files; do
    echo "Fixing $file..."

    # Remove the duplicate module definition block
    sed -i '' '/^mod common {$/,/^}$/d' "$file"

    # Clean up extra blank lines
    sed -i '' '/^[[:space:]]*$/N;/\n[[:space:]]*$/d' "$file"
done

echo "Done fixing duplicate module definitions!"