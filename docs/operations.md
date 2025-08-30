# ProximaDB Operations Guide

## Deployment

### Docker (Recommended)
```bash
docker run -d \
  --name proximadb \
  -p 5678:5678 \
  -p 5679:5679 \
  -v proximadb_data:/data \
  proximadb/proximadb:latest
```

### Binary Installation
```bash
# Download latest release
wget https://github.com/proximadb/releases/latest/proximadb-linux-amd64.tar.gz
tar -xzf proximadb-linux-amd64.tar.gz

# Run server
./proximadb-server --config config.toml
```

### Configuration
```toml
# config.toml
[server]
http_port = 5678
grpc_port = 5679
data_dir = "/data"

[storage]
default_engine = "sst"
wal_dir = "/data/wal"
metadata_backend = "rocksdb"

[cache]
total_memory_mb = 4096

[monitoring]
metrics_enabled = true
dashboard_enabled = true
```

## Monitoring

### Health Checks
```bash
# REST endpoint
curl http://localhost:5678/health

# Response
{"status": "healthy", "version": "1.0.0"}
```

### Metrics
Prometheus metrics available at `/metrics`:

- `proximadb_vectors_total` - Total vectors stored
- `proximadb_queries_per_second` - Query throughput
- `proximadb_latency_ms` - Query latency percentiles
- `proximadb_memory_bytes` - Memory usage

### Dashboard
Web UI available at `http://localhost:8080`

## Performance Tuning

### Memory Settings
```toml
[cache]
vector_cache_mb = 2048
query_cache_mb = 512
metadata_cache_mb = 256

[memtable]
max_size_mb = 256
flush_interval_sec = 300
```

### Storage Optimization
```toml
[storage.compaction]
strategy = "leveled"
threads = 4
min_threshold = 4
max_threshold = 10
```

### Hardware Optimization
```toml
[hardware]
enable_simd = true
enable_gpu = true
gpu_min_batch_size = 1000
```

## Backup & Recovery

### Backup
```bash
# Online backup
proximadb-admin backup \
  --source http://localhost:5678 \
  --destination s3://backups/proximadb/

# Filesystem backup
tar -czf backup.tar.gz /data/
```

### Recovery
```bash
# Restore from backup
proximadb-admin restore \
  --source s3://backups/proximadb/latest \
  --target /data/

# Verify integrity
proximadb-admin verify --data-dir /data/
```

## Scaling

### Vertical Scaling
- Increase CPU cores for parallel queries
- Add RAM for larger caches
- Use NVMe SSDs for storage

### Horizontal Scaling
- Shard by collection name
- Use consistent hashing for distribution
- Deploy behind load balancer

## Troubleshooting

### Common Issues

| Issue | Cause | Solution |
|-------|-------|----------|
| High memory usage | Large cache settings | Reduce cache_size_mb |
| Slow queries | Missing indexes | Run ANALYZE COLLECTION |
| Write failures | Disk full | Check disk space |
| Connection refused | Port blocked | Check firewall rules |

### Debug Logging
```bash
# Enable debug logs
RUST_LOG=proximadb=debug ./proximadb-server

# Trace specific module
RUST_LOG=proximadb::storage=trace ./proximadb-server
```

### Performance Analysis
```sql
-- Slow query log
SELECT * FROM system.slow_queries 
WHERE duration_ms > 100
ORDER BY timestamp DESC;

-- Resource usage
SELECT * FROM system.resource_usage
WHERE cpu_percent > 80;
```

## Security

### Authentication
```toml
[auth]
enabled = true
jwt_secret = "your-secret-key"
api_keys = ["key1", "key2"]
```

### TLS Configuration
```toml
[tls]
enabled = true
cert_file = "/certs/server.crt"
key_file = "/certs/server.key"
```

### Network Security
```toml
[network]
allowed_ips = ["10.0.0.0/8", "192.168.0.0/16"]
rate_limit_rps = 1000
```

## Maintenance

### Regular Tasks
- **Daily**: Check health endpoints
- **Weekly**: Review slow query logs
- **Monthly**: Analyze storage usage
- **Quarterly**: Update ProximaDB version

### Optimization Commands
```sql
-- Analyze collection statistics
ANALYZE COLLECTION products;

-- Compact storage
OPTIMIZE COLLECTION products;

-- Rebuild indexes
REINDEX COLLECTION products;
```