# ProximaDB Monitoring Stack

Complete observability stack for ProximaDB cloud infrastructure using Prometheus, Grafana, Alertmanager, and friends.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Grafana Dashboards                      │
│         (Overview, Performance, Alerts, Logs)                │
└────────────────────────┬────────────────────────────────────┘
                         │
┌────────────────────────┴────────────────────────────────────┐
│                   Prometheus Server                          │
│            (Metrics Collection & Storage)                    │
└───────┬──────────────────────────────────────────┬──────────┘
        │                                          │
        │                                          │
┌───────┴─────────────┐              ┌─────────────┴──────────┐
│  Service Monitors   │              │   Alertmanager         │
│  - ProximaDB        │              │   - Slack              │
│  - Node Exporter    │              │   - PagerDuty          │
│  - Kube-State-Metrics           │   - Email              │
└─────────────────────┘              └────────────────────────┘
```

## Components

### Prometheus
- **Purpose**: Metrics collection and storage
- **Retention**: 15 days
- **Storage**: 100Gi (gp3-encrypted)
- **Scrape Interval**: 15s
- **Resources**: 500m CPU - 1Gi memory (request), 1 CPU - 2Gi memory (limit)

### Grafana
- **Purpose**: Visualization and dashboards
- **Authentication**: admin / prom-operator (CHANGE IN PRODUCTION)
- **Persistence**: 20Gi (gp3-encrypted)
- **Access**: LoadBalancer (NLB) or Ingress
- **Dashboards**:
  - ProximaDB Overview
  - ProximaDB Performance
  - Kubernetes Cluster
  - Kubernetes Pods
  - Node Exporter

### Alertmanager
- **Purpose**: Alert routing and deduplication
- **Persistence**: 5Gi (gp3-encrypted)
- **Routes**:
  - Critical → Slack + PagerDuty
  - Warning → Slack
  - Default → Slack

### Additional Components
- **Node Exporter**: System-level metrics
- **Kube-State-Metrics**: Kubernetes resource metrics
- **Kubelet**: Kubernetes node metrics
- **Kube Controller Manager**: Control plane metrics
- **Kube Scheduler**: Scheduler metrics
- **Kube Proxy**: Network proxy metrics

## Quick Start

### Prerequisites

1. Kubernetes cluster with sufficient resources
2. kubectl configured
3. Helm 3.x installed

### Installation

```bash
# Add Prometheus Helm repository
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo update

# Create namespace
kubectl create namespace monitoring

# Install monitoring stack
helm install prometheus prometheus-community/kube-prometheus-stack \
  --namespace monitoring \
  --values prometheus-operator-values.yaml \
  --timeout 15m

# Wait for pods to be ready
kubectl wait --for=condition=ready pod \
  -l app.kubernetes.io/instance=prometheus \
  -n monitoring \
  --timeout=600s
```

### Access Grafana

```bash
# Port forward Grafana
kubectl port-forward svc/prometheus-grafana 3000:80 \
  -n monitoring &

# Access Grafana
# URL: http://localhost:3000
# Username: admin
# Password: prom-operator (CHANGE IN PRODUCTION)
```

### Access Prometheus

```bash
# Port forward Prometheus
kubectl port-forward svc/prometheus-operated 9090:9090 \
  -n monitoring &

# Access Prometheus
# URL: http://localhost:9090
```

## Dashboards

### ProximaDB Overview
- **Purpose**: High-level system health
- **Metrics**:
  - Pod health (gauge)
  - Request rate (req/s)
  - Request latency (P50, P95, P99)
  - Error rate (%)
  - Storage usage (bytes)

### ProximaDB Performance
- **Purpose**: Detailed performance analysis
- **Metrics**:
  - Query throughput (ops/s)
  - Query latency (P99)
  - Cache hit rate (%)
  - Connection pool (active/idle)
  - Memory usage (working set, cache)
  - CPU usage (usage vs. requested)

## Alerting

### Critical Alerts

1. **ProximaDBDown**
   - Condition: `up{job="proximadb"} == 0`
   - Duration: 5 minutes
   - Severity: Critical
   - Action: Page on-call engineer

2. **ProximaDBHighCPU**
   - Condition: CPU usage > 80%
   - Duration: 10 minutes
   - Severity: Warning
   - Action: Investigate and scale if needed

3. **ProximaDBHighMemory**
   - Condition: Memory usage > 90%
   - Duration: 10 minutes
   - Severity: Warning
   - Action: Investigate memory leaks, scale if needed

4. **ProximaDBHighLatency**
   - Condition: P99 latency > 1s
   - Duration: 10 minutes
   - Severity: Warning
   - Action: Investigate slow queries, optimize indexes

### Alert Routing

```
Critical (severity: critical)
  ├─ Slack: #proximadb-critical
  └─ PagerDuty: On-call engineer

Warning (severity: warning)
  └─ Slack: #proximadb-warnings

Default
  └─ Slack: #proximadb-alerts
```

## Metrics Reference

### ProximaDB Application Metrics

**Request Metrics**:
- `proximadb_request_duration_seconds_count`: Total request count
- `proximadb_request_duration_seconds_sum`: Total request duration
- `proximadb_request_duration_seconds_bucket`: Histogram bucket

**Query Metrics**:
- `proximadb_vector_search_duration_seconds`: Vector search latency
- `proximadb_hybrid_search_duration_seconds`: Hybrid search latency
- `proximadb_graph_query_duration_seconds`: Graph query latency

**Storage Metrics**:
- `proximadb_storage_bytes_used`: Storage used
- `proximadb_storage_bytes_total`: Storage total
- `proximadb_storage_bytes_available`: Storage available

**Cache Metrics**:
- `proximadb_cache_hits_total`: Cache hits
- `proximadb_cache_misses_total`: Cache misses

**Connection Metrics**:
- `proximadb_connections_active`: Active connections
- `proximadb_connections_idle`: Idle connections
- `proximadb_connections_total`: Total connections

## Configuration

### Prometheus

Edit `prometheus-operator-values.yaml`:

```yaml
prometheus:
  prometheusSpec:
    retention: 15d  # Retention period
    resources:
      limits:
        cpu: 1000m
        memory: 2Gi
      requests:
        cpu: 500m
        memory: 1Gi
```

### Grafana

```yaml
grafana:
  adminPassword: YOUR_SECURE_PASSWORD  # CHANGE THIS
  persistence:
    enabled: true
    size: 20Gi
```

### Alertmanager

```yaml
alertmanager:
  config:
    global:
      slack_api_url: 'YOUR_SLACK_WEBHOOK_URL'
      pagerduty_url: 'YOUR_PAGERDUTY_URL'
```

## Maintenance

### Upgrading

```bash
# Upgrade monitoring stack
helm upgrade prometheus prometheus-community/kube-prometheus-stack \
  --namespace monitoring \
  --values prometheus-operator-values.yaml \
  --timeout 15m
```

### Backup

```bash
# Backup Grafana dashboards
kubectl get configmap prometheus-grafana-config-dashboards \
  -n monitoring \
  -o yaml > grafana-dashboards-backup.yaml

# Backup Prometheus configuration
kubectl get secret prometheus-prometheus \
  -n monitoring \
  -o yaml > prometheus-config-backup.yaml
```

### Scaling

```bash
# Edit Prometheus resources
kubectl edit statefulset prometheus-prometheus -n monitoring

# Edit Grafana resources
kubectl edit deployment prometheus-grafana -n monitoring
```

## Troubleshooting

### Prometheus Not Scraping

```bash
# Check ServiceMonitor
kubectl get servicemonitor -n monitoring

# Check Prometheus targets
kubectl port-forward svc/prometheus-operated 9090:9090 -n monitoring &
# Visit: http://localhost:9090/targets
```

### Grafana Not Starting

```bash
# Check logs
kubectl logs deployment/prometheus-grafana -n monitoring

# Check PVC
kubectl get pvc -n monitoring

# Check resources
kubectl describe pod -l app.kubernetes.io/name=grafana -n monitoring
```

### Alerts Not Firing

```bash
# Check Alertmanager logs
kubectl logs prometheus-alertmanager-0 -n monitoring

# Check Prometheus alerts
kubectl port-forward svc/prometheus-operated 9090:9090 -n monitoring &
# Visit: http://localhost:9090/alerts
```

## Best Practices

1. **Secure Credentials**: Change default Grafana password immediately
2. **Resource Limits**: Set appropriate resource limits based on workload
3. **Retention**: Adjust retention based on storage capacity and compliance
4. **Alert Thresholds**: Tune alert thresholds to reduce alert fatigue
5. **Dashboard Customization**: Create custom dashboards for specific use cases
6. **Regular Testing**: Test alerts regularly to ensure they work correctly
7. **Monitoring Monitoring**: Set up monitoring for the monitoring stack

## Additional Resources

- [Prometheus Documentation](https://prometheus.io/docs/)
- [Grafana Documentation](https://grafana.com/docs/)
- [Alertmanager Documentation](https://prometheus.io/docs/alerting/latest/alertmanager/)
- [Kube-Prometheus-Stack](https://github.com/prometheus-community/helm-charts/tree/main/charts/kube-prometheus-stack)

## Support

- **GitHub Issues**: https://github.com/vjsingh1984/proximadb/issues
- **Discord**: https://discord.gg/proximadb
- **Documentation**: https://docs.proximadb.com

---

**Monitoring Stack Version**: 1.0
**Last Updated**: 2026-03-31
**Status**: Production Ready
