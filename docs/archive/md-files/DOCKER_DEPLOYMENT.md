# ProximaDB Docker Deployment with Systemd

This guide shows how to deploy ProximaDB as a systemd service in a Docker container or Ubuntu environment.

## Quick Start

1. **Build ProximaDB** (if not already built):
```bash
# Remove RocksDB dependency (already done)
cargo build --release --bin proximadb-server
```

2. **Install as systemd service**:
```bash
sudo ./install-proximadb-service.sh
```

3. **Start the service**:
```bash
sudo systemctl start proximadb
sudo systemctl status proximadb
```

## Files Created

### System Service Files
- **Service Definition**: `/etc/systemd/system/proximadb.service`
- **Log Rotation**: `/etc/logrotate.d/proximadb`

### Application Files
- **Binary**: `/opt/proximadb/bin/proximadb-server`
- **Configuration**: `/opt/proximadb/config/proximadb.toml`
- **Data Directory**: `/opt/proximadb/data/`
- **Logs**: `/var/log/proximadb/`
- **Health Check**: `/opt/proximadb/bin/health-check.sh`

### Directory Structure
```
/opt/proximadb/
├── bin/
│   ├── proximadb-server           # Main binary
│   └── health-check.sh           # Health monitoring script
├── config/
│   └── proximadb.toml           # Configuration file
└── data/                        # Data storage
    ├── wal/                    # Write-ahead logs
    ├── metadata/               # Collection metadata
    │   └── collections/        # Collection definitions
    └── store/                  # Vector data (VIPER)

/var/log/proximadb/             # Application logs
```

## Service Management

### Basic Operations
```bash
# Start service
sudo systemctl start proximadb

# Stop service
sudo systemctl stop proximadb

# Restart service
sudo systemctl restart proximadb

# Check status
sudo systemctl status proximadb

# Enable auto-start on boot
sudo systemctl enable proximadb

# Disable auto-start
sudo systemctl disable proximadb
```

### Monitoring and Logs
```bash
# Follow logs in real-time
sudo journalctl -u proximadb -f

# View recent logs
sudo journalctl -u proximadb --since="1 hour ago"

# View logs from specific date
sudo journalctl -u proximadb --since="2024-01-15 10:00:00"

# Check service health
/opt/proximadb/bin/health-check.sh

# Test HTTP endpoints
curl http://localhost:5678/health
curl http://localhost:5678/metrics
curl http://localhost:5678/collections
```

## Configuration

### Service Configuration (`/etc/systemd/system/proximadb.service`)

Key settings for production deployment:

```ini
[Service]
Type=exec
User=proximadb
Group=proximadb
WorkingDirectory=/opt/proximadb
ExecStart=/opt/proximadb/bin/proximadb-server --config /opt/proximadb/config/proximadb.toml

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/proximadb/data /var/log/proximadb

# Resource limits
LimitNOFILE=1048576
LimitMEMLOCK=infinity
MemoryMax=80%

# Environment
Environment=RUST_LOG=info,proximadb=debug
Environment=PROXIMADB_CONFIG=/opt/proximadb/config/proximadb.toml
Environment=PROXIMADB_DATA_DIR=/opt/proximadb/data
```

### Application Configuration (`/opt/proximadb/config/proximadb.toml`)

Automatically updated to use production paths:
```toml
[server]
bind_address = "0.0.0.0:5678"
workers = 4

[storage]
wal_url = "file:///opt/proximadb/data/wal"
collections_url = "file:///opt/proximadb/data/metadata"
metadata_url = "file:///opt/proximadb/data/metadata"
```

## Docker Integration

### Dockerfile Example
```dockerfile
FROM ubuntu:22.04

# Install systemd and dependencies
RUN apt-get update && apt-get install -y \
    systemd \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy ProximaDB files
COPY target/release/proximadb-server /opt/proximadb/bin/
COPY config.toml /opt/proximadb/config/proximadb.toml
COPY proximadb-docker.service /etc/systemd/system/proximadb.service
COPY install-proximadb-service.sh /tmp/

# Install and configure
RUN /tmp/install-proximadb-service.sh

# Enable service
RUN systemctl enable proximadb

EXPOSE 5678

# Run systemd as PID 1
CMD ["/lib/systemd/systemd"]
```

### Docker Compose Example
```yaml
version: '3.8'
services:
  proximadb:
    build: .
    privileged: true
    ports:
      - "5678:5678"
    volumes:
      - proximadb_data:/opt/proximadb/data
      - proximadb_logs:/var/log/proximadb
    environment:
      - RUST_LOG=info
    healthcheck:
      test: ["CMD", "/opt/proximadb/bin/health-check.sh"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s

volumes:
  proximadb_data:
  proximadb_logs:
```

## Security Features

The systemd service includes comprehensive security hardening:

- **Process Isolation**: Runs as dedicated `proximadb` user
- **Filesystem Protection**: Read-only access except for data and logs
- **System Call Filtering**: Restricted to essential system calls
- **Memory Protection**: Prevents dangerous memory operations
- **Network Restrictions**: Limited to necessary address families
- **Resource Limits**: Controlled CPU, memory, and file descriptor usage

## Performance Tuning

### Resource Limits
- **File Descriptors**: 1,048,576 (for high connection counts)
- **Memory Lock**: Unlimited (for vector data caching)
- **Memory Max**: 80% of system memory
- **CPU Accounting**: Enabled for monitoring

### Logging
- **Automatic Rotation**: Daily rotation, 30-day retention
- **Structured Logging**: JSON format with correlation IDs
- **Performance Logging**: Request/response times and metrics

## Troubleshooting

### Common Issues

1. **Service won't start**:
```bash
sudo journalctl -u proximadb --no-pager
sudo systemctl status proximadb -l
```

2. **Permission errors**:
```bash
sudo chown -R proximadb:proximadb /opt/proximadb
sudo chown -R proximadb:proximadb /var/log/proximadb
```

3. **Configuration issues**:
```bash
# Test configuration
sudo -u proximadb /opt/proximadb/bin/proximadb-server --config /opt/proximadb/config/proximadb.toml --dry-run
```

4. **Network connectivity**:
```bash
# Check if port is listening
sudo netstat -tlnp | grep 5678
sudo ss -tlnp | grep 5678
```

### Log Analysis
```bash
# Find errors in logs
sudo journalctl -u proximadb | grep -i error

# Monitor performance
sudo journalctl -u proximadb | grep -i "request_duration\|memory_usage"

# Check startup sequence
sudo journalctl -u proximadb --since="10 minutes ago" | grep -i "starting\|ready\|listening"
```

## Health Monitoring

### Endpoints
- **Health Check**: `GET /health` - Basic health status
- **Metrics**: `GET /metrics` - Prometheus-compatible metrics
- **Collections**: `GET /collections` - List active collections

### Custom Health Checks
The included health check script can be extended:
```bash
#!/bin/bash
# Extended health check with collection validation

# Basic connectivity
curl -f -s --max-time 10 "http://localhost:5678/health" || exit 1

# Check if collections endpoint responds
curl -f -s --max-time 10 "http://localhost:5678/collections" || exit 1

# Check metrics endpoint
curl -f -s --max-time 10 "http://localhost:5678/metrics" | grep -q "proximadb_" || exit 1

echo "All health checks passed"
```

## Backup and Recovery

### Data Backup
```bash
# Backup data directory
sudo tar -czf proximadb-backup-$(date +%Y%m%d).tar.gz /opt/proximadb/data

# Backup configuration
sudo cp /opt/proximadb/config/proximadb.toml /opt/proximadb/config/proximadb.toml.backup
```

### Service Recovery
```bash
# Stop service gracefully
sudo systemctl stop proximadb

# Restore data
sudo tar -xzf proximadb-backup-YYYYMMDD.tar.gz -C /

# Fix permissions
sudo chown -R proximadb:proximadb /opt/proximadb/data

# Start service
sudo systemctl start proximadb
```