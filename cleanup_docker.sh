#!/bin/bash
# Docker cleanup script for ProximaDB development

echo "==================================="
echo "Docker Cleanup Script"
echo "==================================="
echo ""

# 1. Stop all running containers
echo "1. Stopping all running containers..."
docker ps -q | xargs -r docker stop
echo "   Done."
echo ""

# 2. Remove all stopped containers
echo "2. Removing all stopped containers..."
docker ps -aq | xargs -r docker rm
echo "   Done."
echo ""

# 3. Show remaining containers
echo "3. Remaining containers:"
docker ps -a
echo ""

# 4. Optional: Remove all unused images
read -p "Do you want to remove unused Docker images? (y/N) " -n 1 -r
echo ""
if [[ $REPLY =~ ^[Yy]$ ]]
then
    echo "4. Removing unused images..."
    docker image prune -a -f
    echo "   Done."
fi
echo ""

# 5. Show Docker disk usage
echo "5. Docker disk usage:"
docker system df
echo ""

echo "==================================="
echo "Cleanup Complete!"
echo "==================================="
echo ""
echo "To start fresh Neo4j and TigerGraph containers:"
echo "  cd clients/python/tests"
echo "  ./setup_graph_databases.sh"
