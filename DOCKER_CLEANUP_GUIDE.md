# Docker Container Cleanup Guide

## Why So Many Containers?

Docker containers accumulate from:
1. **Development testing** - Each test run may create containers
2. **Failed startups** - Containers that didn't start properly
3. **Old benchmark runs** - Previous Neo4j/TigerGraph attempts
4. **ProximaDB tests** - Integration tests that spawn containers

## Quick Cleanup (Choose One)

### Option 1: Remove ALL Containers (Recommended)
```bash
# Stop everything
docker stop $(docker ps -aq)

# Remove everything
docker rm $(docker ps -aq)

# Verify
docker ps -a
```

### Option 2: Use the Cleanup Script
```bash
./remove_old_containers.sh
```

### Option 3: Remove Only Stopped Containers
```bash
docker container prune -f
```

### Option 4: Interactive Removal
```bash
# See what you have
docker ps -a

# Remove specific containers by name
docker rm container_name_1 container_name_2

# Or remove by ID
docker rm abc123 def456
```

## After Cleanup

Once containers are removed, start fresh:

```bash
cd clients/python/tests
./setup_graph_databases.sh
```

This will create ONLY:
- `neo4j-bench` (ports 7474, 7687)
- `tigergraph-bench` (ports 9000, 14240)

## Prevention

To avoid accumulation in the future:

1. **Clean up after tests**:
   ```bash
   docker stop neo4j-bench tigergraph-bench
   docker rm neo4j-bench tigergraph-bench
   ```

2. **Use Docker Desktop cleanup**:
   - Open Docker Desktop
   - Go to Settings → Resources → Advanced
   - Click "Clean / Purge Data"

3. **Regular maintenance**:
   ```bash
   # Weekly cleanup
   docker system prune -a -f
   ```

## Troubleshooting

### If Docker commands hang:
```bash
# Restart Docker Desktop
# Or from terminal:
killall Docker && open -a Docker

# Wait 30 seconds, then try again
docker ps
```

### If you see "permission denied":
```bash
sudo docker rm $(sudo docker ps -aq)
```

### If containers won't stop:
```bash
# Force kill
docker rm -f $(docker ps -aq)
```

## Current Status Check

Run this to see your current situation:
```bash
echo "Total containers: $(docker ps -aq | wc -l)"
echo "Running: $(docker ps -q | wc -l)"
echo "Stopped: $(docker ps -aq -f status=exited | wc -l)"
```

## Docker Disk Space

Check how much space Docker is using:
```bash
docker system df
```

If it's using too much:
```bash
# Remove everything unused
docker system prune -a --volumes -f
```

⚠️ **WARNING**: This removes ALL stopped containers, unused images, and volumes!
