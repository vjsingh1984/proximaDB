#!/bin/bash
# Docker diagnostic script

echo "==================================="
echo "Docker Diagnostic Report"
echo "==================================="
echo ""

echo "1. Docker version:"
docker version --format '{{.Server.Version}}' 2>&1 | head -1
echo ""

echo "2. Docker daemon status:"
docker info 2>&1 | grep -E "Server Version|Containers|Running|Paused|Stopped" | head -10
echo ""

echo "3. Count of containers by status:"
echo "   Total containers: $(docker ps -aq 2>/dev/null | wc -l)"
echo "   Running: $(docker ps -q 2>/dev/null | wc -l)"
echo "   Stopped: $(docker ps -aq -f status=exited 2>/dev/null | wc -l)"
echo ""

echo "4. Containers using most resources:"
docker stats --no-stream --format "table {{.Container}}\t{{.CPUPerc}}\t{{.MemUsage}}" 2>&1 | head -10
echo ""

echo "5. Neo4j/TigerGraph containers:"
docker ps -a --filter "name=neo4j" --filter "name=tigergraph" --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}" 2>&1
echo ""

echo "6. Docker disk usage:"
docker system df 2>&1
echo ""

echo "==================================="
echo "Diagnostic Complete"
echo "==================================="
echo ""
echo "If you see many stopped containers, run:"
echo "  ./cleanup_docker.sh"
