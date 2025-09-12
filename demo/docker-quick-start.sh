#!/bin/bash
# ProximaDB Docker Quick Start Script
# Deploys the complete ProximaDB stack with Web UI

set -e

echo "🚀 ProximaDB Docker Quick Start"
echo "=============================="

# Check Docker availability
if ! command -v docker &> /dev/null; then
    echo "❌ Docker is not installed. Please install Docker first."
    exit 1
fi

if ! command -v docker compose &> /dev/null; then
    echo "❌ Docker Compose is not available. Please install Docker Compose."
    exit 1
fi

# Check port availability
echo "🔍 Checking port availability..."
for port in 5678 5679 8080; do
    if ss -tulpn | grep -q ":$port "; then
        echo "⚠️  Port $port is already in use. Please stop the service using this port."
        ss -tulpn | grep ":$port "
        read -p "Continue anyway? (y/N): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            exit 1
        fi
    fi
done

echo "✅ Ports are available"

# Build and start services
echo "🏗️  Building and starting ProximaDB services..."
docker compose down -v 2>/dev/null || true
docker compose build --pull
docker compose up -d proximadb-unified

# Wait for services to be healthy
echo "⏳ Waiting for services to be ready..."
timeout=60
counter=0

while [ $counter -lt $timeout ]; do
    if docker compose ps | grep -q "healthy"; then
        break
    fi
    echo -n "."
    sleep 2
    counter=$((counter + 2))
done

echo ""

# Check service status
echo "📊 Service Status:"
docker compose ps

# Test connectivity
echo ""
echo "🧪 Testing connectivity..."

# Test ProximaDB health
if curl -sf http://localhost:5678/health > /dev/null 2>&1; then
    echo "✅ ProximaDB REST API: http://localhost:5678"
else
    echo "❌ ProximaDB REST API not responding"
fi

# Test Web UI
if curl -sf -I http://localhost:8080 > /dev/null 2>&1; then
    echo "✅ Web UI: http://localhost:8080"
else
    echo "❌ Web UI not responding"
fi

echo ""
echo "🎉 ProximaDB Docker deployment complete!"
echo ""
echo "🌐 Access the Web UI: http://localhost:8080"
echo "📡 ProximaDB REST API: http://localhost:5678"
echo "🔧 gRPC endpoint: localhost:5679"
echo ""
echo "🎬 Next steps:"
echo "1. Open http://localhost:8080 in your browser"
echo "2. Navigate to the 'Demo Runner' tab"
echo "3. Run the 'E-commerce Demo' to get started"
echo "4. Explore other tabs for vector search and SQL queries"
echo ""
echo "🔍 Debug commands:"
echo "  docker compose logs proximadb-unified    # View all logs"
echo "  docker compose ps               # Check service status"
echo "  docker compose down             # Stop all services"
echo ""
echo "📖 Full documentation: demo/README.adoc"