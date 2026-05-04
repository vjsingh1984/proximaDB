#!/bin/bash
# Proto Regeneration Script
#
# This script regenerates Rust code from Protocol Buffer definitions.
# Use this when you modify .proto files and need to regenerate the Rust code.
#
# Usage:
#   ./scripts/regenerate-proto.sh
#
# The regenerated code will be in src/proto/proximadb.v1.rs

set -e

echo "🔄 Regenerating Protocol Buffer code..."

# Check if protoc is installed
if ! command -v protoc &> /dev/null; then
    echo "⚠️  Warning: protoc not found in PATH"
    echo "   Installing protoc is optional - tonic-build will handle compilation"
    echo "   For development, you can install protoc from:"
    echo "   - macOS: brew install protobuf"
    echo "   - Ubuntu: apt-get install protobuf-compiler"
    echo ""
fi

# Clean existing generated code
echo "🧹 Cleaning existing generated code..."
rm -rf target/proto/build
rm -f src/proto/.proto_files

# Force rebuild by touching proto files
echo "📝 Touching proto files to trigger rebuild..."
find proto -name "*.proto" -exec touch {} \;

# Rebuild with cargo
echo "🔨 Rebuilding with cargo (this will regenerate proto code)..."
cargo clean -p proximadb 2>/dev/null || true
cargo build --bin proximadb 2>&1 | grep -i "proto\|compil" || true

echo ""
echo "✅ Proto regeneration complete!"
echo ""
echo "📁 Generated files:"
echo "   - src/proto/proximadb.v1.rs (main proto definitions)"
echo "   - src/proto/proximadb.*.v1.rs (module-specific definitions)"
echo ""
echo "🔍 To verify regeneration worked:"
echo "   grep -c 'CreateGraphWithEngineRequest' src/proto/proximadb.v1.rs"
echo ""
echo "💡 Next steps:"
echo "   1. Check that the generated code compiles: cargo check"
echo "   2. Run tests to ensure nothing broke: cargo test"
echo "   3. Commit the regenerated code if changes look correct"
