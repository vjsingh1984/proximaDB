#!/bin/bash
# Remove old Docker containers - keeps only recent Neo4j/TigerGraph for benchmarks

echo "==================================="
echo "Removing Old Docker Containers"
echo "==================================="
echo ""

# Show what we have before cleanup
echo "BEFORE cleanup:"
echo "---------------"
docker ps -a --format "table {{.Names}}\t{{.Status}}\t{{.CreatedAt}}" 2>&1 | head -20
echo ""
echo "Total containers: $(docker ps -aq | wc -l)"
echo ""

# Stop all containers (except if you want to keep some running)
echo "Stopping all containers..."
docker stop $(docker ps -aq) 2>/dev/null
echo "Done."
echo ""

# Remove all containers
echo "Removing all containers..."
docker rm $(docker ps -aq) 2>/dev/null
echo "Done."
echo ""

# Alternative: Remove only containers older than 24 hours
# Uncomment if you want to keep recent containers:
# echo "Removing containers older than 24 hours..."
# docker container prune --filter "until=24h" -f

# Show what's left
echo "AFTER cleanup:"
echo "--------------"
docker ps -a
echo ""
echo "Total containers: $(docker ps -aq | wc -l)"
echo ""

# Clean up dangling images too
echo "Cleaning up dangling images..."
docker image prune -f
echo ""

# Show final disk usage
echo "Final Docker disk usage:"
docker system df
echo ""

echo "==================================="
echo "Cleanup Complete!"
echo "==================================="
echo ""
echo "You can now start fresh containers:"
echo "  cd clients/python/tests"
echo "  ./setup_graph_databases.sh"
echo ""
echo "Or start specific containers:"
echo "  docker run -d --name neo4j-bench -p 7474:7474 -p 7687:7687 \\"
echo "    -e NEO4J_AUTH=neo4j/benchmark neo4j:latest"
