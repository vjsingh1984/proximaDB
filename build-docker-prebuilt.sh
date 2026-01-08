#!/bin/bash
# Build Docker image with pre-built binary (avoids OOM)

set -e

echo "🔨 Step 1: Building ProximaDB binary locally..."
cargo build --release --bin proximadb-server

echo "✅ Binary built: target/release/proximadb-server"
echo "📊 Binary size:"
ls -lh target/release/proximadb-server

echo ""
echo "🐳 Step 2: Building Docker image with pre-built binary..."
docker build -f Dockerfile.prebuilt -t proximadb:prebuilt .

echo ""
echo "✅ Docker image built successfully!"
echo "📦 Image info:"
docker images proximadb:prebuilt

echo ""
echo "🚀 Run with:"
echo "   docker run -p 5678:5678 -p 5679:5679 proximadb:prebuilt"
