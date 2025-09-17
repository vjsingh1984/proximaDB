#!/bin/bash

# Script to fix common import issues in test files

# Find all test files with incorrect imports
files=$(find tests -name "*.rs" -exec grep -l "proximadb::storage::engines::sst\|proximadb::core::search::SearchResult\|proximadb::storage::engines::viper\|proximadb::proto::proximadb" {} \;)

for file in $files; do
    echo "Fixing $file..."

    # Fix proto imports
    sed -i '' 's/proximadb::proto::proximadb\b/proximadb::proto::proximadb_v1/g' "$file"

    # Fix SearchResult imports
    sed -i '' 's/proximadb::core::search::SearchResult/proximadb::proto::proximadb_v1::SearchResult/g' "$file"

    # Fix SST engine imports
    sed -i '' 's/proximadb::storage::engines::sst/proximadb::storage::engines::impls::sst/g' "$file"

    # Fix VIPER engine imports
    sed -i '' 's/proximadb::storage::engines::viper/proximadb::storage::engines::impls::viper/g' "$file"
done

echo "Import fixes completed."