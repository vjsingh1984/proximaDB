#!/bin/bash

echo "🚀 ProximaDB Build and Test Script"
echo "==================================="
echo "Rust version: $(rustc --version)"
echo "Cargo version: $(cargo --version)"
echo ""

# Create data directories
echo "📁 Creating data directories..."
mkdir -p ./data/{wal,metadata,store}
echo "✅ Data directories created at: $(pwd)/data/"
echo ""

# Build the server (this may take 5-10 minutes on first build)
echo "🔨 Building ProximaDB server..."
echo "   This may take several minutes due to many dependencies..."
echo "   You can monitor progress with: tail -f build.log"
echo ""

# Build with progress logging
if cargo build --bin proximadb-server > build.log 2>&1; then
    echo "✅ Build successful!"
    echo ""
    
    # Show binary info
    ls -la target/debug/proximadb-server
    echo ""
    
    # Start the server
    echo "🚀 Starting ProximaDB server..."
    echo "   Config: $(pwd)/config.toml"
    echo "   Data: $(pwd)/data/"
    echo "   Logs: RUST_LOG=info"
    echo ""
    
    # Start server in background
    RUST_LOG=info cargo run --bin proximadb-server > server.log 2>&1 &
    SERVER_PID=$!
    echo "Server PID: $SERVER_PID"
    
    # Wait for server to start
    echo "⏳ Waiting for server to start..."
    for i in {1..30}; do
        if curl -s localhost:5678/health > /dev/null 2>&1; then
            echo "✅ Server is running on localhost:5678!"
            break
        fi
        echo "   Attempt $i/30..."
        sleep 2
    done
    
    # Test server connectivity
    if curl -s localhost:5678/health > /dev/null 2>&1; then
        echo ""
        echo "🧪 Server is ready! You can now run tests:"
        echo ""
        echo "Quick verification:"
        echo "   python test_grpc_sdk_verification.py"
        echo ""
        echo "Full test suite:"
        echo "   ./run_vector_tests.sh"
        echo ""
        echo "Individual tests:"
        echo "   python test_comprehensive_grpc_operations.py"
        echo "   python test_vector_coordinator_integration.py"
        echo "   python test_pipeline_data_flow.py"
        echo "   python test_zero_copy_performance.py"
        echo ""
        echo "To stop server: kill $SERVER_PID"
        echo "To view server logs: tail -f server.log"
        echo "To view build logs: cat build.log"
        
        # Keep script running so server stays alive
        echo ""
        echo "Press Ctrl+C to stop the server and exit..."
        trap "echo ''; echo 'Stopping server...'; kill $SERVER_PID 2>/dev/null; exit 0" INT
        
        # Keep running
        while kill -0 $SERVER_PID 2>/dev/null; do
            sleep 1
        done
        
    else
        echo "❌ Server failed to start. Check server.log for details:"
        tail -20 server.log
        kill $SERVER_PID 2>/dev/null
        exit 1
    fi
    
else
    echo "❌ Build failed! Check build.log for details:"
    tail -20 build.log
    exit 1
fi