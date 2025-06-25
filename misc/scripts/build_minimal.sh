#!/bin/bash

echo "🔨 Building minimal ProximaDB for testing..."

# Temporarily rename files that use Arrow/Parquet
echo "📁 Temporarily disabling Arrow/Parquet dependent files..."

# Find and rename files that import arrow/parquet
find src/ -name "*.rs" -exec grep -l "use arrow\|use parquet" {} \; | while read file; do
    if [[ -f "$file" ]]; then
        echo "  Disabling: $file"
        mv "$file" "$file.disabled"
        # Create a minimal stub
        filename=$(basename "$file" .rs)
        cat > "$file" << EOF
// Temporarily disabled for ARM64 build compatibility
// This file has been disabled due to Arrow/Parquet dependencies
// that conflict with chrono on ARM64 Ubuntu Docker

#[allow(dead_code)]
pub struct Placeholder;

impl Default for Placeholder {
    fn default() -> Self {
        Self
    }
}
EOF
    fi
done

echo "✅ Arrow/Parquet files disabled"

# Try to build
echo "🔨 Attempting minimal build..."
cargo build --bin proximadb-server

build_result=$?

# Restore files
echo "🔄 Restoring disabled files..."
find src/ -name "*.rs.disabled" | while read file; do
    original="${file%.disabled}"
    echo "  Restoring: $original"
    mv "$file" "$original"
done

if [ $build_result -eq 0 ]; then
    echo "✅ Minimal build successful!"
    ls -la target/debug/proximadb-server
else
    echo "❌ Build failed even with minimal configuration"
fi

exit $build_result