#!/bin/bash

# Script to update all core::VectorRecord references to proto::proximadb_v1::VectorRecord
# This is safer than sed as we can review each change

echo "Updating VectorRecord references from core to proto..."

# List of files to update
files=(
    "src/storage/persistence/write_ahead_log/compaction_types.rs"
    "src/storage/persistence/write_ahead_log/enhanced_flush_result.rs"
    "src/storage/persistence/write_ahead_log/parallel_recovery.rs"
    "src/storage/persistence/write_ahead_log/bincode_serialization_strategy.rs"
    "src/storage/persistence/write_ahead_log/proto_serialization_strategy.rs"
    "src/storage/persistence/write_ahead_log/compaction_axis_integration.rs"
    "src/storage/persistence/write_ahead_log/avro_serialization_strategy.rs"
    "src/storage/persistence/write_ahead_log/serialization/bincode.rs"
    "src/storage/persistence/write_ahead_log/serialization/mod.rs"
    "src/storage/persistence/write_ahead_log/serialization/proto.rs"
    "src/storage/persistence/write_ahead_log/serialization/avro.rs"
)

for file in "${files[@]}"; do
    if [ -f "$file" ]; then
        echo "Processing $file..."
        # Check if file contains the old import
        if grep -q "use crate::core::VectorRecord;" "$file"; then
            echo "  Found core::VectorRecord import, updating..."
            # Create backup
            cp "$file" "$file.bak"
            # Replace the import
            sed -i '' 's/use crate::core::VectorRecord;/use crate::proto::proximadb_v1::VectorRecord;/g' "$file"
            echo "  Updated!"
        else
            echo "  No core::VectorRecord import found"
        fi
    else
        echo "File not found: $file"
    fi
done

echo "Update complete!"