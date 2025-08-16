# PRISM Ultimate Cost/Performance Optimization

## Revised Strategy: Local-First with Cloud Backup

With **zero egress costs** within AWS region, we can optimize more aggressively for local storage while using S3 purely for durability.

## New Optimized Tier Strategy

### Level 4: Binary Navigation (Memory)
```yaml
Storage: Memory only
Cost: $0.00
Performance: Instant access
Recovery: 100ms rebuild from L3
Durability: Not needed (fast rebuild)
```

### Level 3: INT8 Tree (NVMe SSD)
```yaml
Storage: Local NVMe or EBS GP3
Cost: ~$100/month for 1TB
Performance: <1ms access, 16K IOPS
Recovery: Direct load (source of truth)
Durability: EBS snapshots (hourly/daily)
Backup: Weekly compressed upload to S3 ($0.50/month)
```

### Level 2: PQ Index (Local Disk) ⭐ OPTIMIZED
```yaml
Storage: Local SSD or EBS ST1  
Cost: ~$45/month for 5TB
Performance: 2ms access, high IOPS
Recovery: Restore from S3 backup
Durability: RAID 1 + daily S3 backup
Backup Cost: $2/month (4GB compressed daily)
API Calls: 30 per month (vs 10,000+ for cloud-first)
```

### Level 1: SuperBlocks (Local Disk) ⭐ NEW OPTIMIZATION
```yaml
Storage: Local Disk (EBS ST1 or instance storage)
Cost: ~$20/month for 2TB  
Performance: 5ms access vs 10ms S3 Express
Recovery: Restore from S3 backup
Durability: Daily backup to S3 Standard
Backup Cost: $3/month (compressed)
API Calls: 30 per month vs 1,000+ for cloud-first

Why This Works:
- SuperBlocks are accessed frequently during search
- 5ms local vs 10ms S3 Express = 2x faster
- $20 local vs $50 S3 Express = 60% cheaper
- Only pay S3 for backup, not active storage
```

### Level 0: DataBlocks (S3 Standard for Cold Data)
```yaml
Storage: S3 Standard (infrequently accessed)
Cost: ~$70/month for 300GB
Performance: 50ms access (acceptable for cold data)
Access Pattern: <1% of queries (cache misses only)
API Optimization: 256MB blocks, S3 Select for filtering
```

## Cost Comparison Analysis

### Old Strategy (Cloud-First):
```yaml
Monthly Costs:
- L4 Memory: $0
- L3 NVMe: $100  
- L2 S3 Standard: $50 + API costs
- L1 S3 Express: $50 + API costs
- L0 S3 Standard: $70 + API costs
Total: ~$270/month + significant API costs

API Calls (1000 queries/day, 5% cache miss):
- L2 access: 50 queries × 10 blocks = 500 calls/day = 15,000/month
- L1 access: 50 queries × 4 blocks = 200 calls/day = 6,000/month  
- L0 access: 50 queries × 16 blocks = 800 calls/day = 24,000/month
Total API calls: 45,000/month × $0.0004 = $18/month
```

### New Strategy (Local-First):
```yaml
Monthly Costs:
- L4 Memory: $0
- L3 NVMe: $100
- L2 Local + Backup: $45 + $2 = $47
- L1 Local + Backup: $20 + $3 = $23  
- L0 S3 Standard: $70 + minimal API
Total: ~$240/month (12% savings)

API Calls (1000 queries/day, 5% cache miss):
- L2 access: 0 API calls (local disk)
- L1 access: 0 API calls (local disk)
- L0 access: 50 queries × 16 blocks = 800 calls/day = 24,000/month
- Backup uploads: 60 calls/month (daily backups)
Total API calls: 24,060/month × $0.0004 = $10/month (44% reduction)
```

## Performance Analysis

### Latency Improvements:
```yaml
L1 SuperBlock Access:
- S3 Express: 10ms
- Local Disk: 5ms  
- Improvement: 2x faster

L2 PQ Access:
- S3 Standard: 50ms
- Local SSD: 2ms
- Improvement: 25x faster

Search Path Optimization:
- Memory (L4): 0.1ms
- Local NVMe (L3): 1ms  
- Local SSD (L2): 2ms
- Local Disk (L1): 5ms
- S3 (L0): 50ms (rare)

Total Search Time:
- Cache hit (95%): 0.1ms (pure memory)
- Warm data (4%): 8ms (L1-L3 local)
- Cold data (1%): 58ms (includes S3)
- Average: 0.1×0.95 + 8×0.04 + 58×0.01 = 1.3ms
```

### Instance Storage Option (Even Better):
```yaml
For instances with NVMe instance storage:
- r5d.4xlarge: 2×300GB NVMe included
- Cost: Same instance cost, no EBS charges
- Performance: 3GB/s, 300K IOPS
- Use for L1+L2: 600GB total
- Backup to S3: Essential (ephemeral storage)

Cost Savings:
- L1 Local: $0 (instance storage)
- L2 Local: $0 (instance storage)  
- Backup: $5/month
- Total: $105/month (vs $240) = 56% savings!
```

## Optimized Architecture

### High-Performance Configuration:
```yaml
Instance: r5d.4xlarge (128GB RAM, 2×300GB NVMe)
Storage Layout:
  L4: 100GB RAM (binary navigation)
  L3: 100GB EBS GP3 (INT8 tree, source of truth)
  L2: 300GB NVMe instance storage (PQ index)
  L1: 300GB NVMe instance storage (SuperBlocks)
  L0: S3 Standard (DataBlocks, cold data)

Monthly Cost: ~$400 instance + $100 EBS + $70 S3 = $570
Performance: Sub-millisecond for 99% of queries
API Calls: <25,000/month
```

### Cost-Optimized Configuration:
```yaml
Instance: r5.2xlarge (64GB RAM)
Storage Layout:
  L4: 60GB RAM (binary navigation)
  L3: 100GB EBS GP3 (INT8 tree)
  L2: 200GB EBS ST1 (PQ index)  
  L1: 100GB EBS ST1 (SuperBlocks)
  L0: S3 Standard (DataBlocks)

Monthly Cost: ~$200 instance + $100 EBS GP3 + $30 EBS ST1 + $70 S3 = $400
Performance: <10ms for 99% of queries
API Calls: <25,000/month
```

## Backup Strategy Optimization

### Smart Backup Scheduling:
```yaml
L3 (Critical - Source of Truth):
  - Frequency: Every 4 hours
  - Retention: 7 days
  - Size: 100GB compressed to ~30GB
  - Cost: 6 × 30 × $0.002 = $0.36/month

L2 (Important - Performance):
  - Frequency: Daily
  - Retention: 3 days  
  - Size: 200GB compressed to ~50GB
  - Cost: 3 × 50 × $0.002 = $0.30/month

L1 (Moderate - Can Rebuild):
  - Frequency: Daily
  - Retention: 2 days
  - Size: 100GB compressed to ~25GB
  - Cost: 2 × 25 × $0.002 = $0.10/month

Total Backup Cost: <$1/month (negligible!)
```

### Recovery Time Objectives:
```yaml
Instance Failure Scenarios:

L4 Loss (Memory):
  - Recovery: 100ms (rebuild from L3)
  - Data Loss: None

L3 Loss (NVMe):  
  - Recovery: 5 minutes (restore from 4-hour backup)
  - Data Loss: <4 hours of updates

L2 Loss (Local):
  - Recovery: 30 minutes (restore from daily backup)  
  - Data Loss: <24 hours (rebuilt from L0+L3)

L1 Loss (Local):
  - Recovery: 20 minutes (restore from daily backup)
  - Data Loss: None (rebuilt from L0)

Total RTO: <35 minutes for complete instance loss
Total RPO: <4 hours for worst case
```

## Final Recommendations

### Tier 1: Cost-Optimized (Target: $400/month)
```yaml
- Use EBS ST1 for L1/L2 (sequential optimized)
- Daily backups to S3
- Target: 99% queries <10ms, 1% <60ms
- API budget: <$15/month
```

### Tier 2: Performance-Optimized (Target: $570/month)  
```yaml
- Use NVMe instance storage for L1/L2
- 4-hour backups to S3
- Target: 99% queries <2ms, 1% <60ms  
- API budget: <$15/month
```

### Tier 3: Balanced (Target: $485/month)
```yaml
- Use EBS GP3 for L2, ST1 for L1
- 12-hour backups to S3
- Target: 95% queries <5ms, 5% <60ms
- API budget: <$15/month
```

This local-first approach with S3 backup gives us the best of both worlds: **local performance** with **cloud durability** at **minimum cost**!