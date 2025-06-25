#!/bin/bash

echo "🔧 Fixing ProximaDB Build Issues"
echo "================================="
echo ""

echo "1️⃣ Installing protobuf compiler..."
if command -v brew &> /dev/null; then
    echo "   Using Homebrew to install protobuf..."
    brew install protobuf
else
    echo "   Please install protobuf manually:"
    echo "   brew install protobuf"
    echo "   or visit: https://grpc.io/docs/protoc-installation/"
fi

echo ""
echo "2️⃣ Checking protoc version..."
if command -v protoc &> /dev/null; then
    protoc_version=$(protoc --version)
    echo "   Installed: $protoc_version"
else
    echo "   ❌ protoc not found in PATH"
fi

echo ""
echo "3️⃣ Fixing arrow version conflict..."
echo "   The arrow/parquet version 53.0 has compatibility issues."
echo "   Downgrading to a stable version..."

# Create a backup of Cargo.toml
cp Cargo.toml Cargo.toml.backup

# Fix arrow versions in Cargo.toml
sed -i '' 's/parquet = "53.0"/parquet = "52.0"/' Cargo.toml
sed -i '' 's/arrow = "53.0"/arrow = "52.0"/' Cargo.toml

echo "   ✅ Updated arrow/parquet to version 52.0"

echo ""
echo "4️⃣ Cleaning build cache..."
cargo clean

echo ""
echo "5️⃣ Attempting build with fixes..."
echo "   This may take several minutes..."

if cargo build --bin proximadb-server > build_fixed.log 2>&1; then
    echo "✅ Build successful!"
    echo ""
    echo "Binary location: $(pwd)/target/debug/proximadb-server"
    ls -la target/debug/proximadb-server
else
    echo "❌ Build still failing. Check build_fixed.log for details:"
    tail -20 build_fixed.log
fi