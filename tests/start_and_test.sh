#!/bin/bash

echo "🚀 ProximaDB Build and Test Script"
echo "==================================="
echo ""

# Ensure we're using the latest Rust
echo "📦 Rust version:"
rustc --version
cargo --version
echo ""

# Create data directories
echo "📁 Creating data directories..."
mkdir -p ./data/{wal,metadata,store}
echo "✅ Data directories created"
echo ""

# Build the server
echo "🔨 Building ProximaDB server..."
echo "   This requires Rust 1.75+ for edition 2024 support"
echo ""

if cargo build --bin proximadb-server; then
    echo "✅ Build successful!"
    echo ""
    
    # Start the server in background
    echo "🚀 Starting ProximaDB server..."
    RUST_LOG=info cargo run --bin proximadb-server &
    SERVER_PID=$!
    
    # Wait for server to start
    echo "⏳ Waiting for server to start..."
    sleep 5
    
    # Check if server is running
    if curl -s localhost:5678/health > /dev/null 2>&1; then
        echo "✅ Server is running!"
        echo ""
        
        # Run tests
        echo "🧪 Running integration tests..."
        echo ""
        
        # Install Python dependencies
        pip install -q transformers torch sentence-transformers lorem numpy psutil
        
        # Run SDK verification test
        echo "📋 Running SDK Verification..."
        python test_grpc_sdk_verification.py
        
        echo ""
        echo "📊 To run more tests:"
        echo "   python test_comprehensive_grpc_operations.py"
        echo "   python test_vector_coordinator_integration.py"
        echo "   python test_pipeline_data_flow.py"
        echo "   python test_zero_copy_performance.py"
        echo ""
        echo "Server PID: $SERVER_PID"
        echo "To stop server: kill $SERVER_PID"
        
    else
        echo "❌ Server failed to start"
        kill $SERVER_PID 2>/dev/null
    fi
    
else
    echo "❌ Build failed!"
    echo ""
    echo "Please ensure you have Rust 1.75+ installed:"
    echo "  rustup update stable"
    echo "  rustup default stable"
    echo ""
    echo "Current version:"
    rustc --version
fi