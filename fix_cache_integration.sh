#!/bin/bash
# Fix cache integration in all storage engines to use VectorCache instead of QueryCache

echo "Fixing cache integration in all storage engines..."

# List of engines to fix
engines=("viper" "nova" "swift" "raptor" "helix")

for engine in "${engines[@]}"; do
    engine_file="src/storage/engines/impls/$engine/engine.rs"

    # For helix, the file is mod.rs
    if [ "$engine" = "helix" ]; then
        engine_file="src/storage/engines/impls/$engine/mod.rs"
    fi

    echo "Fixing $engine engine in $engine_file..."

    # Replace get_query_cache with get_vector_cache
    sed -i '' 's/get_query_cache()/get_vector_cache()/g' "$engine_file"

    # Replace query_cache variable name with vector_cache
    sed -i '' 's/if let Some(query_cache)/if let Some(vector_cache)/g' "$engine_file"
    sed -i '' 's/query_cache\.get/vector_cache.get/g' "$engine_file"
    sed -i '' 's/query_cache\.put/vector_cache.put/g' "$engine_file"

    # Fix CacheType from Query to VectorData
    sed -i '' 's/CacheType::Query/CacheType::VectorData/g' "$engine_file"

    echo "✓ Fixed $engine engine"
done

echo "All engines fixed!"