#!/bin/bash

echo "🚀 ProximaDB Server Startup Script"
echo "=================================="
echo ""

# Check Rust installation
echo "🔍 Checking Rust installation..."
if ! command -v cargo &> /dev/null; then
    echo "❌ Cargo not found. Please install Rust."
    exit 1
fi

echo "✅ Rust found: $(rustc --version)"
echo ""

# Create data directories
echo "📁 Creating data directories..."
mkdir -p ./data/{wal,metadata,store}
echo "✅ Data directories created"
echo ""

# Build the server
echo "🔨 Building ProximaDB server..."
echo "   This may take a few minutes on first build..."

# Try to build with cargo
if cargo build --bin proximadb-server --release; then
    echo "✅ Build successful!"
    echo ""
    
    # Start the server
    echo "🚀 Starting ProximaDB server..."
    echo "   Config: ./config.toml"
    echo "   Data: ./data/"
    echo ""
    
    # Run the server
    RUST_LOG=info cargo run --bin proximadb-server --release
else
    echo ""
    echo "❌ Build failed!"
    echo ""
    echo "Common issues:"
    echo "1. Rust version too old - try: rustup update"
    echo "2. Missing dependencies - check Cargo.toml"
    echo "3. Syntax errors in code"
    echo ""
    echo "Try running: cargo check"
    exit 1
fi