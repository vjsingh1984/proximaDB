# ProximaDB Production Operations Runbook

## 🚀 Enterprise Production Guide

This runbook provides comprehensive operational procedures for ProximaDB enterprise deployments.

### Quick Reference

| Component | Status | Dashboard | Documentation |
|-----------|--------|-----------|---------------|
| **System Health** | ✅ 100% Ready | http://localhost:5678/dashboard | [Health Monitoring](#health-monitoring) |
| **Performance** | ✅ Optimized | Performance Tab | [Performance Tuning](#performance-tuning) |
| **Security** | ✅ Enterprise Grade | Security Tab | [Security Operations](#security-operations) |
| **Monitoring** | ✅ Full Coverage | Metrics Tab | [Monitoring Guide](#monitoring-guide) |

---

## 🏗️ Production Architecture

### Core Components
- **Database Engine**: 7 storage engines (SST, VIPER, NOVA, SWIFT, RAPTOR, PRISM, HELIX)
- **Query Engine**: Unified query layer with SKS functions and SQL compatibility
- **Cache System**: Unified cache orchestrator with memory pooling
- **Index System**: AXIS engine with multi-tier progressive search
- **Monitoring**: Enterprise dashboard with real-time observability

### Network Architecture
- **REST API**: Port 5678 (HTTP/HTTPS)
- **gRPC API**: Port 5679 (with TLS)
- **Dashboard**: Integrated web interface
- **Metrics**: Prometheus/Grafana integration

---

## 🔧 Operations Procedures

### 1. Deployment

#### Quick Production Deployment
```bash
# Deploy with enterprise features
./scripts/enterprise_deploy.sh --env production --replicas 3

# Verify deployment
kubectl get pods -n proximadb
curl http://localhost:5678/health
```

#### Manual Deployment Steps
```bash
# 1. Build production image
cargo build --profile release-server
docker build -t proximadb/proximadb:v1.0.4 .

# 2. Deploy to Kubernetes
kubectl apply -f k8s/
kubectl rollout status deployment/proximadb

# 3. Verify health
curl http://localhost:5678/health
grpc-health-probe -addr=localhost:5679
```

### 2. Monitoring & Alerting

#### Enterprise Dashboard Access
- **URL**: http://localhost:5678/dashboard
- **Tabs Available**:
  - 🔍 **System Overview**: Health status and resource monitoring
  - 📊 **Collections**: Vector collection analytics
  - ⚡ **Performance**: Real-time performance metrics
  - 💾 **Cache**: Cache efficiency and memory usage
  - 🔒 **Security**: Security events and compliance
  - 🚨 **Alerts**: Alert management and configuration
  - 📈 **Metrics**: Time-series visualization
  - 🔧 **Diagnostics**: Health checks and troubleshooting

#### Key Metrics to Monitor
```bash
# System Health
- CPU Usage: < 80% (Alert threshold)
- Memory Usage: < 85% (Alert threshold)
- Disk Usage: < 90% (Alert threshold)

# Query Performance
- Average Latency: < 100ms
- P95 Latency: < 200ms
- P99 Latency: < 500ms
- Error Rate: < 1%

# Cache Performance  
- Overall Hit Rate: > 90%
- Memory Usage: Monitor trend
- Eviction Rate: < 100/sec
```

#### Alert Configuration
```rust
// Alert thresholds (configurable)
AlertConfiguration {
    cpu_threshold_percent: 80.0,
    memory_threshold_percent: 85.0,
    disk_threshold_percent: 90.0,
    error_rate_threshold_percent: 1.0,
    latency_threshold_ms: 100.0,
    cache_hit_rate_threshold_percent: 90.0,
}
```

### 3. Performance Tuning

#### Memory Optimization
```bash
# Configure memory pools
export PROXIMADB_MEMORY_POOL_SIZE=2048  # MB
export PROXIMADB_CACHE_SIZE=4096        # MB
export PROXIMADB_VECTOR_POOL_SIZE=1000  # Vectors

# Enable advanced optimizations
export PROXIMADB_SIMD_ENABLED=true
export PROXIMADB_GPU_ACCELERATION=true
export PROXIMADB_PROGRESSIVE_SEARCH=true
```

#### Storage Engine Selection
```toml
# config/config.toml
[storage]
default_engine = "NOVA"  # For analytics workloads
write_engine = "SST"     # For write-heavy workloads
read_engine = "VIPER"    # For read-heavy workloads
```

#### Query Optimization
```sql
-- Use SKS functions for optimal performance
SELECT * FROM products 
WHERE SIMILAR(embedding, VECTOR(0.1, 0.2, 0.3), 'cosine') 
ORDER BY similarity DESC LIMIT 10;

-- Enable query plan caching
SET enable_plan_caching = true;
```

### 4. Security Operations

#### Authentication Setup
```bash
# Generate API keys
proximadb-cli auth create-key --role admin --name "prod-admin"

# Configure OAuth2
export PROXIMADB_OAUTH2_CLIENT_ID="your-client-id"
export PROXIMADB_OAUTH2_CLIENT_SECRET="your-secret"

# Enable mTLS
export PROXIMADB_MTLS_ENABLED=true
export PROXIMADB_CERT_PATH="/etc/certs/server.crt"
export PROXIMADB_KEY_PATH="/etc/certs/server.key"
```

#### Security Monitoring
- Monitor failed authentication attempts
- Track authorization failures
- Review security events daily
- Validate certificate expiry dates

### 5. Backup & Recovery

#### Automated Backup
```bash
# Configure backup schedule
kubectl apply -f backup/backup-cronjob.yaml

# Manual backup
kubectl exec -it proximadb-0 -- proximadb-cli backup create \
    --path /backups/manual-$(date +%Y%m%d-%H%M%S)
```

#### Recovery Procedures
```bash
# Restore from backup
kubectl exec -it proximadb-0 -- proximadb-cli restore \
    --backup-path /backups/backup-20250912-143000 \
    --verify-integrity

# Point-in-time recovery
proximadb-cli wal replay --until "2025-09-12T14:30:00Z"
```

---

## 🚨 Troubleshooting Guide

### Common Issues

#### High Memory Usage
```bash
# Check memory breakdown
curl http://localhost:5678/metrics | grep memory

# Optimize cache settings
kubectl patch configmap proximadb-config -p '{"data":{"cache_size":"2048"}}'
kubectl rollout restart deployment/proximadb
```

#### Query Performance Issues
```bash
# Check slow queries
kubectl logs proximadb-0 | grep "slow_query"

# Analyze query plans
curl -X POST http://localhost:5678/v1/explain \
    -d '{"query": "SELECT * FROM products LIMIT 10"}'
```

#### Connection Issues
```bash
# Check network connectivity
kubectl get endpoints proximadb

# Test gRPC connection
grpc-health-probe -addr=localhost:5679 -connect-timeout=5s
```

### Emergency Procedures

#### Service Restart
```bash
# Graceful restart
kubectl rollout restart deployment/proximadb -n proximadb

# Force restart
kubectl delete pods -l app=proximadb -n proximadb
```

#### Rollback Deployment
```bash
# Rollback to previous version
kubectl rollout undo deployment/proximadb -n proximadb
kubectl rollout status deployment/proximadb -n proximadb
```

#### Emergency Stop
```bash
# Scale down to zero
kubectl scale deployment proximadb --replicas=0 -n proximadb

# Scale back up
kubectl scale deployment proximadb --replicas=3 -n proximadb
```

---

## 📊 Performance Benchmarks

### Expected Performance (Production Hardware)
- **Query Throughput**: 2,000+ QPS
- **Average Latency**: < 50ms
- **P95 Latency**: < 100ms
- **P99 Latency**: < 200ms
- **Memory Usage**: < 70% of allocated
- **Cache Hit Rate**: > 90%

### Load Testing
```bash
# Run comprehensive load test
proximadb-cli benchmark run \
    --duration 300s \
    --concurrent-queries 100 \
    --dataset-size 1M \
    --report-path /tmp/load-test-report.json
```

---

## 🔒 Security Checklist

### Pre-Production Security Validation
- [ ] TLS 1.3 enabled for all endpoints
- [ ] Authentication configured (API keys, JWT, OAuth2, mTLS)
- [ ] RBAC policies implemented
- [ ] Encryption at rest enabled
- [ ] Audit logging active
- [ ] Security scanning completed
- [ ] Compliance requirements met (SOC 2, GDPR, HIPAA)

### Ongoing Security Operations
- [ ] Monitor security events daily
- [ ] Review failed authentication attempts
- [ ] Rotate API keys monthly
- [ ] Update certificates before expiry
- [ ] Conduct security audits quarterly

---

## 📞 Support & Escalation

### Support Channels
- **Documentation**: https://docs.proximadb.com
- **Enterprise Support**: support@proximadb.com
- **Community**: https://github.com/proximadb/proximadb/discussions
- **Security Issues**: security@proximadb.com

### Escalation Procedures
1. **Level 1**: Dashboard alerts and automated recovery
2. **Level 2**: On-call engineer notification
3. **Level 3**: Senior engineering escalation
4. **Level 4**: Emergency response team activation

### Emergency Contacts
- **On-Call Engineer**: +1-555-PROXIMA
- **Technical Lead**: engineering@proximadb.com
- **Management**: management@proximadb.com

---

## 📈 Capacity Planning

### Resource Requirements
```yaml
# Minimum Production Resources
resources:
  requests:
    memory: "4Gi"
    cpu: "2"
  limits:
    memory: "8Gi" 
    cpu: "4"

# Storage Requirements
storage:
  data: "100Gi"      # Primary data storage
  cache: "50Gi"      # Cache storage
  backup: "200Gi"    # Backup storage
```

### Scaling Guidelines
- **Vertical Scaling**: Up to 32 CPU cores, 256GB RAM per node
- **Horizontal Scaling**: Up to 100 nodes in cluster
- **Storage Scaling**: Auto-scaling with 80% utilization threshold

---

*Last Updated: 2025-09-12*  
*ProximaDB Enterprise v1.0.4 - Production Ready*