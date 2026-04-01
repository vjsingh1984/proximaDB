# ProximaDB Operations Runbook

**Purpose**: Standard operating procedures for ProximaDB cloud infrastructure
**Audience**: DevOps engineers, SREs, on-call engineers
**Last Updated**: 2026-03-31
**Version**: 1.0

## Table of Contents

1. [Alert Response Procedures](#alert-response-procedures)
2. [Common Operational Tasks](#common-operational-tasks)
3. [Troubleshooting Guides](#troubleshooting-guides)
4. [Maintenance Procedures](#maintenance-procedures)
5. [Emergency Procedures](#emergency-procedures)
6. [Performance Tuning](#performance-tuning)

---

## Alert Response Procedures

### CRITICAL: ProximaDBDown

**Condition**: ProximaDB instance is down for > 5 minutes

**Severity**: Critical (P1)

**Impact**: Complete service outage

**Response Time**: < 5 minutes

**Steps**:

1. **Verify Alert**
   ```bash
   # Check Prometheus alerts
   kubectl port-forward svc/prometheus-operated 9090:9090 -n monitoring &
   curl http://localhost:9090/api/v1/alerts | jq .

   # Check ProximaDB pods
   kubectl get pods -n proximadb
   ```

2. **Check Pod Status**
   ```bash
   # Describe problematic pod
   kubectl describe pod <pod-name> -n proximadb

   # Check pod logs
   kubectl logs <pod-name> -n proximadb --previous
   kubectl logs <pod-name> -n proximadb --tail=100 -f
   ```

3. **Check Node Health**
   ```bash
   # Check node status
   kubectl get nodes -o wide

   # Describe problematic node
   kubectl describe node <node-name>
   ```

4. **Restart Services**
   ```bash
   # Restart deployment
   kubectl rollout restart deployment proximadb -n proximadb

   # Wait for rollout
   kubectl rollout status deployment proximadb -n proximadb
   ```

5. **Verify Recovery**
   ```bash
   # Check health endpoint
   kubectl port-forward svc/proximadb 5678:5678 -n proximadb &
   curl http://localhost:5678/health

   # Check pod status
   kubectl get pods -n proximadb
   ```

6. **Post-Incident**
   - Document root cause
   - Create incident report
   - Update runbook if needed
   - Schedule post-mortem

### WARNING: ProximaDBHighCPU

**Condition**: CPU usage > 80% for > 10 minutes

**Severity**: Warning (P2)

**Impact**: Performance degradation

**Response Time**: < 15 minutes

**Steps**:

1. **Verify Alert**
   ```bash
   # Check CPU usage
   kubectl top pods -n proximadb
   kubectl top nodes
   ```

2. **Identify Cause**
   ```bash
   # Check current queries
   kubectl logs -l app=proximadb -n proximadb --tail=100 | grep "SLOW QUERY"

   # Check connections
   kubectl exec -it <pod-name> -n proximadb -- curl http://localhost:5678/metrics
   ```

3. **Scale Up (if needed)**
   ```bash
   # Manual scaling
   kubectl scale deployment proximadb -n proximadb --replicas=5

   # Or let HPA handle it
   kubectl get hpa -n proximadb
   ```

4. **Long-term Solutions**
   - Optimize queries
   - Add indexes
   - Scale node groups
   - Review resource limits

### WARNING: ProximaDBHighMemory

**Condition**: Memory usage > 90% for > 10 minutes

**Severity**: Warning (P2)

**Impact**: Potential OOM kills

**Response Time**: < 15 minutes

**Steps**:

1. **Verify Alert**
   ```bash
   # Check memory usage
   kubectl top pods -n proximadb
   ```

2. **Identify Memory Leaks**
   ```bash
   # Check pod metrics
   kubectl exec -it <pod-name> -n proximadb -- curl http://localhost:9090/metrics

   # Look for memory growth patterns
   ```

3. **Scale Up**
   ```bash
   # Edit deployment with higher limits
   kubectl edit deployment proximadb -n proximadb

   # Or add more replicas
   kubectl scale deployment proximadb -n proximadb --replicas=5
   ```

4. **Long-term Solutions**
   - Investigate memory leaks
   - Adjust cache sizes
   - Scale node groups

### WARNING: ProximaDBHighLatency

**Condition**: P99 latency > 1s for > 10 minutes

**Severity**: Warning (P2)

**Impact**: Poor user experience

**Response Time**: < 15 minutes

**Steps**:

1. **Verify Alert**
   ```bash
   # Check latency metrics
   kubectl port-forward svc/prometheus-operated 9090:9090 -n monitoring &
   curl 'http://localhost:9090/api/v1/query?query=histogram_quantile(0.99,rate(proximadb_request_duration_seconds_bucket[5m]))'
   ```

2. **Identify Slow Queries**
   ```bash
   # Check query logs
   kubectl logs -l app=proximadb -n proximadb --tail=100 | grep "latency"

   # Check database locks
   kubectl exec -it <pod-name> -n proximadb -- curl http://localhost:5678/admin/stats
   ```

3. **Optimize**
   - Add missing indexes
   - Optimize slow queries
   - Scale infrastructure
   - Enable caching

---

## Common Operational Tasks

### Deploy New Version

```bash
# Upgrade Helm chart
helm upgrade proximadb infrastructure/helm/proximadb \
  --namespace proximadb \
  --values infrastructure/terraform/environments/dev/helm-values.yaml \
  --set image.tag=0.3.0 \
  --wait \
  --timeout 10m

# Verify deployment
kubectl rollout status deployment proximadb -n proximadb
kubectl get pods -n proximadb
```

### Rollback Deployment

```bash
# Rollback to previous version
helm rollback proximadb -n proximadb

# Or rollback to specific revision
helm rollback proximadb 2 -n proximadb

# Verify rollback
kubectl get pods -n proximadb
```

### Scale Applications

```bash
# Manual scaling
kubectl scale deployment proximadb -n proximadb --replicas=5

# Check HPA status
kubectl get hpa -n proximadb

# Edit HPA
kubectl edit hpa proximadb -n proximadb
```

### Check Logs

```bash
# Recent logs
kubectl logs -l app=proximadb -n proximadb --tail=100

# Follow logs
kubectl logs -l app=proximadb -n proximadb --tail=100 -f

# Logs from previous container (if restarted)
kubectl logs <pod-name> -n proximadb --previous

# All pods logs
kubectl logs -l app=proximadb -n proximadb --all-containers=true
```

### Port Forwarding

```bash
# ProximaDB
kubectl port-forward svc/proximadb 5678:5678 -n proximadb

# Grafana
kubectl port-forward svc/prometheus-grafana 3000:80 -n monitoring

# Prometheus
kubectl port-forward svc/prometheus-operated 9090:9090 -n monitoring
```

### Access Database

```bash
# Exec into pod
kubectl exec -it <pod-name> -n proximadb -- /bin/bash

# Run psql (if PostgreSQL)
kubectl exec -it <pod-name> -n proximadb -- psql -U postgres
```

---

## Troubleshooting Guides

### Pods Not Starting

**Symptoms**: Pods stuck in Pending, CrashLoopBackOff, or Error state

**Diagnosis**:

```bash
# Check pod status
kubectl get pods -n proximadb

# Describe pod
kubectl describe pod <pod-name> -n proximadb

# Check events
kubectl get events -n proximadb --sort-by='.lastTimestamp'
```

**Common Causes**:

1. **Image Pull Errors**
   - Check image name and tag
   - Verify image pull secrets
   - Check registry access

2. **Resource Constraints**
   ```bash
   # Check node resources
   kubectl top nodes

   # Check pod resource requests
   kubectl describe pod <pod-name> -n proximadb | grep -A 5 "Requests"
   ```

3. **Configuration Errors**
   ```bash
   # Check pod logs
   kubectl logs <pod-name> -n proximadb

   # Check ConfigMaps
   kubectl get configmap -n proximadb

   # Check Secrets
   kubectl get secrets -n proximadb
   ```

### High Memory Usage

**Symptoms**: OOMKilled, memory usage > 90%

**Diagnosis**:

```bash
# Check memory usage
kubectl top pods -n proximadb

# Check pod limits
kubectl describe pod <pod-name> -n proximadb | grep -A 10 "Limits"
```

**Solutions**:

1. **Increase Memory Limits**
   ```yaml
   resources:
     limits:
       memory: 8Gi
     requests:
       memory: 2Gi
   ```

2. **Reduce Cache Size**
   ```yaml
   config:
     caching:
       cache_size_mb: 256  # Reduce from 512
   ```

3. **Scale Horizontally**
   ```bash
   kubectl scale deployment proximadb -n proximadb --replicas=5
   ```

### Network Connectivity Issues

**Symptoms**: Timeouts, connection refused, DNS failures

**Diagnosis**:

```bash
# Check service endpoints
kubectl get endpoints -n proximadb

# Check network policies
kubectl get networkpolicies -n proximadb

# Test DNS
kubectl exec -it <pod-name> -n proximadb -- nslookup proximadb

# Test connectivity
kubectl exec -it <pod-name> -n proximadb -- curl http://proximadb:5678/health
```

**Solutions**:

1. **Check Service Configuration**
   ```bash
   kubectl describe svc proximadb -n proximadb
   ```

2. **Verify Network Policies**
   ```bash
   kubectl describe networkpolicy <policy-name> -n proximadb
   ```

3. **Check Security Groups** (AWS)
   ```bash
   aws ec2 describe-security-groups --filters "Name=group-name,Values=*proximadb*"
   ```

### Storage Issues

**Symptoms**: PVC pending, disk full, write failures

**Diagnosis**:

```bash
# Check PVCs
kubectl get pvc -n proximadb

# Check PVs
kubectl get pv

# Describe PVC
kubectl describe pvc <pvc-name> -n proximadb

# Check disk usage
kubectl exec -it <pod-name> -n proximadb -- df -h
```

**Solutions**:

1. **Expand PVC**
   ```bash
   # Edit PVC with larger size
   kubectl edit pvc <pvc-name> -n proximadb
   ```

2. **Clean Up Old Data**
   ```bash
   kubectl exec -it <pod-name> -n proximadb -- find /data -name "*.old" -delete
   ```

3. **Add More Storage**
   ```yaml
   persistence:
     size: 200Gi  # Increase from 100Gi
   ```

---

## Maintenance Procedures

### Weekly Maintenance

**Tasks**:
1. Review logs for errors
2. Check disk usage
3. Review alerts
4. Verify backups
4. Check resource utilization

**Commands**:
```bash
# Check logs
kubectl logs -l app=proximadb -n proximadb --since=168h | grep ERROR

# Check disk usage
kubectl exec -it <pod-name> -n proximadb -- df -h

# Check backups
aws s3 ls s3://proximadb-wal-archive-dev/

# Check resources
kubectl top pods -n proximadb
kubectl top nodes
```

### Monthly Maintenance

**Tasks**:
1. Security updates
2. Performance review
3. Capacity planning
4. Cost optimization
5. Documentation updates

**Commands**:
```bash
# Check for updates
helm repo update
helm search repo proximadb

# Review performance
kubectl port-forward svc/prometheus-grafana 3000:80 -n monitoring &
# Visit Grafana dashboards

# Review costs
aws ce get-cost-and-usage --time-namespace Monthly
```

### Upgrade Procedures

**Pre-Upgrade**:
1. **Backup Data**
   ```bash
   # Create Velero backup
   velero backup create proximadb-pre-upgrade -n proximadb
   ```

2. **Snapshot Volumes** (if using AWS)
   ```bash
   # Create EBS snapshot
   aws ec2 create-snapshot --volume-id <volume-id>
   ```

3. **Test Upgrade in Staging**
   ```bash
   helm upgrade proximadb . --namespace proximadb-staging --values staging-values.yaml
   ```

**Upgrade**:
1. **Apply Upgrade**
   ```bash
   helm upgrade proximadb . --namespace proximadb --values prod-values.yaml
   ```

2. **Verify**
   ```bash
   kubectl rollout status deployment proximadb -n proximadb
   kubectl get pods -n proximadb
   curl http://proximadb.example.com/health
   ```

3. **Monitor**
   - Check Grafana dashboards
   - Review logs
   - Verify alerts

**Post-Upgrade**:
1. **Run Smoke Tests**
2. **Monitor Performance**
3. **Update Documentation**
4. **Create Post-Upgrade Report**

---

## Emergency Procedures

### Complete Service Outage

**Symptoms**: All services down, no connectivity

**Steps**:

1. **Assess Scope**
   ```bash
   # Check cluster status
   kubectl cluster-info

   # Check nodes
   kubectl get nodes

   # Check AWS status
   aws health describe-events --region us-east-1
   ```

2. **Check Control Plane**
   ```bash
   # Check EKS status
   aws eks describe-cluster --name proximadb-production --region us-east-1
   ```

3. **Restart Services**
   ```bash
   # Restart all deployments
   kubectl rollout restart deployment/proximadb -n proximadb
   ```

4. **Escalate**
   - Page on-call engineer
   - Notify stakeholders
   - Create incident

### Data Corruption

**Symptoms**: Data inconsistencies, query failures

**Steps**:

1. **Stop Writes**
   ```bash
   # Scale to zero
   kubectl scale deployment proximadb -n proximadb --replicas=0
   ```

2. **Assess Damage**
   ```bash
   # Check WAL integrity
   kubectl exec -it <pod-name> -n proximadb -- cat /data/proximadb/wal/manifest.jsonl
   ```

3. **Restore from Backup**
   ```bash
   # Restore from Velero
   velero restore create proximadb-restore --from-backup proximadb-latest
   ```

4. **Verify**
   - Run integrity checks
   - Validate queries
   - Monitor logs

### Security Incident

**Symptoms**: Unauthorized access, suspicious activity

**Steps**:

1. **Contain**
   ```bash
   # Revoke access
   aws eks revoke-access --cluster proximadb-production

   # Rotate credentials
   aws iam rotate-access-key --user-name <user-name>
   ```

2. **Investigate**
   - Review CloudTrail logs
   - Check VPC Flow Logs
   - Analyze access patterns

3. **Remediate**
   - Patch vulnerabilities
   - Update security groups
   - Rotate secrets

4. **Document**
   - Create incident report
   - Update policies
   - Schedule post-mortem

---

## Performance Tuning

### Database Optimization

**Query Performance**:
```sql
-- Add indexes
CREATE INDEX idx_vectors_collection_id ON vectors(collection_id);

-- Analyze query performance
EXPLAIN ANALYZE SELECT * FROM vectors WHERE collection_id = 1;
```

**Caching**:
```yaml
config:
  caching:
    enabled: true
    cache_size_mb: 1024  # Increase cache
    cache_ttl_seconds: 600  # Increase TTL
```

### Infrastructure Scaling

**Vertical Scaling**:
```yaml
resources:
  limits:
    cpu: 4000m  # Increase CPU
    memory: 8Gi  # Increase memory
  requests:
    cpu: 1000m
    memory: 2Gi
```

**Horizontal Scaling**:
```yaml
autoscaling:
  enabled: true
  minReplicas: 5  # Increase minimum
  maxReplicas: 20  # Increase maximum
  targetCPUUtilizationPercentage: 70
  targetMemoryUtilizationPercentage: 80
```

**Node Scaling**:
```hcl
# Edit Terraform variables
general_purpose_nodes_max_size = 12  # Increase from 8
```

### Network Optimization

**Connection Pooling**:
```yaml
config:
  server:
    max_connections: 10000  # Increase max connections
    connection_timeout: 300
```

**Load Balancing**:
```yaml
service:
  annotations:
    service.beta.kubernetes.io/aws-load-balancer-type: "nlb"
    service.beta.kubernetes.io/aws-load-balancer-cross-zone-load-balancing-enabled: "true"
```

---

## On-Call Procedures

### Rotation

**Primary On-Call**:
- Response time: < 15 minutes (P1), < 1 hour (P2)
- Escalation: Secondary → Manager → Director

**Secondary On-Call**:
- Backup for primary
- Response time: < 30 minutes (P1), < 2 hours (P2)

### Handoff

**Daily Handoff Checklist**:
1. Review active incidents
2. Check system status
3. Review pending changes
4. Discuss known issues
5. Update runbook

### Escalation Matrix

| Severity | Response Time | Escalation Path |
|----------|---------------|-----------------|
| P1 (Critical) | 15 min | Primary → Secondary → Manager |
| P2 (Warning) | 1 hour | Primary → Secondary |
| P3 (Info) | 1 day | Primary |

### Communication Channels

- **Slack**: #proximadb-alerts (alerts), #proximadb-oncall (coordination)
- **PagerDuty**: On-call scheduling and escalation
- **Email**: oncall@proximadb.com

---

**Runbook Version**: 1.0
**Last Updated**: 2026-03-31
**Next Review**: 2026-04-30

For questions or updates, contact: sre@proximadb.com
