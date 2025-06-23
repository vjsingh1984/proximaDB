#!/bin/bash

echo "🚀 ProximaDB Vector Operations Test Suite"
echo "=========================================="
echo ""

# Check if ProximaDB server is running
echo "🔍 Checking ProximaDB server connection..."
if ! curl -s localhost:5678/health > /dev/null 2>&1; then
    echo "❌ ProximaDB server is not running on localhost:5678"
    echo "   Please start the server with: cargo run --bin proximadb-server"
    exit 1
fi

echo "✅ ProximaDB server is running"
echo ""

# Install Python dependencies
echo "📦 Installing Python dependencies..."
pip install -q transformers torch sentence-transformers lorem numpy psutil

echo ""
echo "🧪 Running Vector Operations Tests..."
echo "====================================="

# Quick SDK verification
echo ""
echo "0️⃣ Running gRPC SDK Verification..."
echo "   Tests: Basic SDK functionality check"
python test_grpc_sdk_verification.py

# Run comprehensive gRPC operations test
echo ""
echo "1️⃣ Running Comprehensive gRPC Operations Test..."
echo "   Tests: Insert, Update, Delete, Search (ID, Filters, Similarity)"
python test_comprehensive_grpc_operations.py

echo ""
echo "2️⃣ Running Vector Coordinator Integration Test..."
echo "   Tests: gRPC → UnifiedAvroService → VectorCoordinator → VIPER → WAL"
python test_vector_coordinator_integration.py

echo ""
echo "3️⃣ Running Pipeline Data Flow Test..."
echo "   Tests: Data flow verification through each component"
python test_pipeline_data_flow.py

echo ""
echo "4️⃣ Running Zero-Copy Performance Test..."
echo "   Tests: WAL performance, memory efficiency, throughput"
python test_zero_copy_performance.py

echo ""
echo "5️⃣ Running Original BERT Operations Test..."
echo "   Tests: Basic vector operations with BERT embeddings"
python test_vector_operations_bert.py

echo ""
echo "✨ All tests completed!"
echo ""
echo "📊 Check the following for detailed results:"
echo "   - Console output above"
echo "   - ProximaDB server logs"
echo "   - Data files in ./data/ directory"
echo ""
echo "🎯 Pipeline tested: gRPC → UnifiedAvroService → VectorCoordinator → WAL"
echo ""
echo "📈 Performance Characteristics:"
echo "   - Zero-copy Avro for inserts"
echo "   - Trust-but-verify WAL writes"
echo "   - VIPER storage engine with Parquet"
echo "   - HNSW indexing for similarity search"