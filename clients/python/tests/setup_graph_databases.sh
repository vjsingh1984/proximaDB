#!/bin/bash
# Setup script for graph database benchmarking
# Installs and configures Neo4j and TigerGraph for embedded/local testing

set -e

echo "=================================="
echo "Graph Database Benchmark Setup"
echo "=================================="

# Function to check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Function to check if port is in use
port_in_use() {
    lsof -i :$1 >/dev/null 2>&1
}

# =============================================================================
# Neo4j Setup (Docker-based for easy embedded-like experience)
# =============================================================================

setup_neo4j() {
    echo ""
    echo "Setting up Neo4j..."
    echo "-------------------"

    if ! command_exists docker; then
        echo "✗ Docker not found. Install Docker to run Neo4j."
        echo "  Visit: https://docs.docker.com/get-docker/"
        return 1
    fi

    # Check if Neo4j container is already running
    if docker ps | grep -q neo4j-bench; then
        echo "✓ Neo4j container already running"
        return 0
    fi

    # Stop and remove existing container if it exists
    docker stop neo4j-bench 2>/dev/null || true
    docker rm neo4j-bench 2>/dev/null || true

    echo "Starting Neo4j container..."
    docker run -d \
        --name neo4j-bench \
        -p 7474:7474 \
        -p 7687:7687 \
        -e NEO4J_AUTH=neo4j/benchmark \
        -e NEO4J_dbms_memory_heap_max__size=4G \
        -e NEO4J_dbms_memory_pagecache_size=2G \
        neo4j:latest

    echo "Waiting for Neo4j to start (may take 30-60 seconds)..."
    sleep 10

    max_attempts=30
    attempt=0
    while [ $attempt -lt $max_attempts ]; do
        if curl -s http://localhost:7474 > /dev/null 2>&1; then
            echo "✓ Neo4j is ready!"
            echo "  Web UI: http://localhost:7474"
            echo "  Bolt: bolt://localhost:7687"
            echo "  Username: neo4j"
            echo "  Password: benchmark"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 2
        echo -n "."
    done

    echo ""
    echo "✗ Neo4j failed to start within timeout"
    return 1
}

# =============================================================================
# TigerGraph Setup (Docker-based)
# =============================================================================

setup_tigergraph() {
    echo ""
    echo "Setting up TigerGraph..."
    echo "------------------------"

    if ! command_exists docker; then
        echo "✗ Docker not found. Install Docker to run TigerGraph."
        return 1
    fi

    # Check if TigerGraph container is already running
    if docker ps | grep -q tigergraph-bench; then
        echo "✓ TigerGraph container already running"
        return 0
    fi

    # Stop and remove existing container if it exists
    docker stop tigergraph-bench 2>/dev/null || true
    docker rm tigergraph-bench 2>/dev/null || true

    echo "Starting TigerGraph container (this may take a few minutes)..."
    docker run -d \
        --name tigergraph-bench \
        -p 9000:9000 \
        -p 14240:14240 \
        --ulimit nofile=1000000:1000000 \
        -t tigergraph/tigergraph:latest

    echo "Waiting for TigerGraph to initialize (may take 2-3 minutes)..."
    sleep 30

    max_attempts=60
    attempt=0
    while [ $attempt -lt $max_attempts ]; do
        if docker exec tigergraph-bench curl -s http://localhost:9000/api/ping > /dev/null 2>&1; then
            echo "✓ TigerGraph is ready!"
            echo "  REST API: http://localhost:9000"
            echo "  GraphStudio: http://localhost:14240"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 3
        echo -n "."
    done

    echo ""
    echo "✗ TigerGraph failed to start within timeout"
    return 1
}

# =============================================================================
# Python Dependencies
# =============================================================================

install_python_deps() {
    echo ""
    echo "Installing Python dependencies..."
    echo "---------------------------------"

    pip install -q neo4j pyTigerGraph python-igraph networkx numpy rich

    echo "✓ Python dependencies installed"
}

# =============================================================================
# Main
# =============================================================================

main() {
    echo ""
    echo "This script will set up the following databases for benchmarking:"
    echo "  - Neo4j (via Docker)"
    echo "  - TigerGraph (via Docker)"
    echo "  - ProximaDB (already built via maturin)"
    echo "  - NetworkX, igraph (Python libraries)"
    echo ""

    # Install Python dependencies
    install_python_deps

    # Setup Neo4j
    setup_neo4j
    neo4j_status=$?

    # Setup TigerGraph
    setup_tigergraph
    tigergraph_status=$?

    # Summary
    echo ""
    echo "=================================="
    echo "Setup Summary"
    echo "=================================="
    echo ""

    if [ $neo4j_status -eq 0 ]; then
        echo "✓ Neo4j: READY"
    else
        echo "✗ Neo4j: FAILED (optional)"
    fi

    if [ $tigergraph_status -eq 0 ]; then
        echo "✓ TigerGraph: READY"
    else
        echo "✗ TigerGraph: FAILED (optional)"
    fi

    echo "✓ NetworkX: READY"
    echo "✓ igraph: READY"
    echo "✓ ProximaDB: READY"
    echo ""

    echo "To run benchmarks:"
    echo "  cd clients/python/tests"
    echo "  python3 comprehensive_graph_benchmark.py"
    echo ""
    echo "To stop databases:"
    echo "  docker stop neo4j-bench tigergraph-bench"
    echo ""
}

# Run main function
main
