# ProximaDB Cloud Infrastructure

This directory contains the infrastructure as code (IaC) for deploying ProximaDB as a managed cloud service on AWS.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        Load Balancer                        │
│                   (AWS ALB / NLB / Gateway)                   │
└────────────────────────┬────────────────────────────────────┘
                         │
         ┌───────────────┴───────────────┐
         │      EKS Control Plane      │
         │      (Kubernetes 1.29+)      │
         └───────────────┬───────────────┘
                         │
    ┌────────────────────┼────────────────────┐
    │                    │                    │
┌───┴────┐         ┌────┴────┐         ┌───┴────┐
│  Pods  │         │  Pods   │         │  Pods  │
│ (API)  │         │(Query) │         │ (Data) │
└───┬────┘         └────┬────┘         └───┬────┘
    │                  │                   │
    │              ┌───┴───────┐          │
    │              │   S3      │          │
    └──────────────┤ (WAL Archive)          │
                   └─────────────────┘

    ┌──────────────────────────────┐
    │   Amazon RDS (PostgreSQL)    │
    │      (Metadata Service)      │
    └──────────────────────────────┘

    ┌──────────────────────────────┐
    │   ElastiCache (Redis)        │
    │         (Caching)            │
    └──────────────────────────────┘
```

## Directory Structure

```
deploy/infrastructure/
├── terraform/
│   ├── modules/
│   │   ├── vpc/              # VPC and networking
│   │   ├── eks/              # EKS Kubernetes cluster
│   │   ├── proximadb/        # ProximaDB deployment
│   │   └── monitoring/       # CloudWatch, Prometheus, Grafana
│   └── environments/
│       ├── dev/              # Development environment
│       ├── staging/          # Staging environment
│       └── production/       # Production environment
└── helm/
    └── proximadb/             # Helm chart for ProximaDB
        ├── Chart.yaml
        ├── values.yaml
        └── templates/
```

## Quick Start

### Prerequisites

1. **AWS Account** with appropriate IAM permissions
2. **Terraform** >= 1.0
3. **kubectl** configured to manage EKS
4. **Helm** >= 3.0
5. **AWS CLI** configured

### Step 1: Configure Environment

```bash
cd deploy/infrastructure/terraform/environments/dev

# Copy and edit variables
cp terraform.tfvars.example terraform.tfvars
# Edit terraform.tfvars with your values
```

### Step 2: Initialize Terraform

```bash
terraform init
terraform validate
```

### Step 3: Plan Infrastructure

```bash
terraform plan -out=tfplan
```

### Step 4: Deploy Infrastructure

```bash
terraform apply tfplan
```

### Step 5: Configure kubectl

```bash
aws eks update-kubeconfig \
  --name proximadb-dev \
  --region us-east-1

# Verify connection
kubectl get nodes
```

### Step 6: Deploy ProximaDB

```bash
# Add Helm repository
helm repo add proximadb https://charts.proximadb.com
helm repo update

# Install ProximaDB
helm install proximadb deploy/infrastructure/helm/proximadb \
  --namespace proximadb \
  --create-namespace \
  --values dev-values.yaml

# Wait for pods to be ready
kubectl wait --for=condition=ready pod -l app=proximadb -n proximadb --timeout=300s
```

### Step 7: Verify Deployment

```bash
# Check pods
kubectl get pods -n proximadb

# Get service endpoint
kubectl get svc proximadb -n proximadb

# Port forward to test
kubectl port-forward svc/proximadb 5678:5678 -n proximadb

# Test health endpoint
curl http://localhost:5678/health
```

## Terraform Modules

### VPC Module

**Location**: `terraform/modules/vpc/`

**Creates**:
- VPC with public and private subnets
- NAT Gateways for internet access
- Route tables
- Security groups
- VPC Flow Logs (for security monitoring)

**Variables**:
- `name`: Resource name prefix
- `environment`: Environment (dev, staging, production)
- `vpc_cidr`: CIDR block (default: 10.0.0.0/16)
- `availability_zone_count`: Number of AZs (2-4)
- `single_nat_gateway`: Use single NAT (cost savings)
- `enable_flow_logs`: Enable VPC monitoring

**Usage**:
```hcl
module "vpc" {
  source = "../../modules/vpc"
  name = "proximadb-dev"
  environment = "dev"
  vpc_cidr = "10.0.0.0/16"
}
```

### EKS Module

**Location**: `terraform/modules/eks/`

**Creates**:
- Amazon EKS cluster
- Managed node groups
- IAM roles and policies
- Security groups
- CloudWatch log groups
- OIDC provider (for IRSA)

**Variables**:
- `cluster_name`: Name of the EKS cluster
- `kubernetes_version`: Kubernetes version (default: 1.29)
- `vpc_id`: VPC ID for cluster
- `subnet_ids`: Subnet IDs for nodes
- `managed_node_groups`: Node group configurations

**Node Groups**:
- `general_purpose`: General purpose nodes (t3.medium)
- `compute_optimized`: Compute optimized nodes (c5.xlarge, SPOT)

**Usage**:
```hcl
module "eks" {
  source = "../../modules/eks"
  cluster_name = "proximadb-dev"
  vpc_id = module.vpc.vpc_id
  subnet_ids = module.vpc.private_subnet_ids

  managed_node_groups = {
    general_purpose = {
      desired_size = 2
      max_size = 4
      min_size = 2
      instance_types = ["t3.medium"]
    }
  }
}
```

## Helm Chart

### ProximaDB Helm Chart

**Location**: `deploy/infrastructure/helm/proximadb/`

**Features**:
- Deployment or StatefulSet mode
- Horizontal Pod Autoscaler
- Pod Disruption Budget
- Service monitor (Prometheus)
- Prometheus alerts
- Configurable storage (PVC)
- Resource limits and requests
- Probes (liveness, readiness, startup)

**Values**:
```yaml
replicaCount: 3
image:
  tag: "0.2.0"

resources:
  limits:
    cpu: 2000m
    memory: 4Gi
  requests:
    cpu: 500m
    memory: 1Gi

autoscaling:
  enabled: true
  minReplicas: 2
  maxReplicas: 10
  targetCPUUtilizationPercentage: 70

persistence:
  enabled: true
  size: 100Gi
  storageClass: "gp3-encrypted"
```

## Environment Configurations

### Development (dev)

- **Nodes**: 2 general purpose (t3.medium)
- **Replicas**: 2
- **Storage**: 100Gi per node
- **Cost**: ~$200/month

### Staging (staging)

- **Nodes**: 4 general purpose + 2 compute optimized
- **Replicas**: 3
- **Storage**: 200Gi per node
- **Cost**: ~$800/month

### Production (production)

- **Nodes**: 6 general purpose + 4 compute optimized
- **Replicas**: 5
- **Storage**: 500Gi per node
- **Multi-AZ**: Yes
- **Cost**: ~$2,500/month

## Security Features

### Network Security
- VPC isolation (private subnets for nodes)
- Security groups (least privilege)
- Network policies (Kubernetes NetworkPolicy)
- TLS encryption (in transit)

### Data Security
- Encryption at rest (EBS volumes)
- S3 encryption (WAL archival)
- RDS encryption (metadata)
- KMS key management

### Access Control
- IAM roles (least privilege)
- IRSA (IAM Roles for Service Accounts)
- RBAC (Kubernetes RBAC)
- Network policies

## Monitoring and Observability

### Metrics Collection
- Prometheus scraping (/metrics)
- CloudWatch Container Insights
- Node performance metrics

### Logging
- CloudWatch Logs (control plane)
- Fluentd (node logs)
- ELK Stack (aggregated logs)

### Tracing
- AWS X-Ray
- Jaeger (distributed tracing)
- OpenTelemetry

### Dashboards
- Grafana dashboards
- CloudWatch dashboards
- Performance metrics

## Cost Optimization

### Development Environment
- Single NAT Gateway (cost savings)
- On-Demand instances (predictable)
- t3.medium instances (cost-effective)
- 2 AZ deployment (balance)

### Production Environment
- Multi-AZ deployment (high availability)
- Spot instances (compute nodes)
- Auto-scaling (scale with demand)
- Reserved instances (1-3 year terms)

### Cost Breakdown (Monthly)

**Development** (~$200/month):
- EKS control plane: $72
- EC2 instances: $80
- Load balancer: $20
- NAT Gateway: $32
- Data transfer: $10
- Other: $20

**Production** (~$2,500/month):
- EKS control plane: $108
- EC2 instances: $1,200
- Load balancer: $40
- NAT Gateway: $96
- RDS: $200
- ElastiCache: $150
- S3: $50
- Data transfer: $200
- Other: $500

## Maintenance

### Upgrading ProximaDB

```bash
# Update Helm chart values
helm upgrade proximadb deploy/infrastructure/helm/proximadb \
  --namespace proximadb \
  --values dev-values.yaml \
  --set image.tag=0.3.0

# Rollback if needed
helm rollback proximadb -n proximadb
```

### Scaling

```bash
# Manual scaling
kubectl scale deployment proximadb -n proximadb --replicas=5

# Edit HPA
kubectl edit hpa proximadb -n proximadb
```

### Backup and Restore

```bash
# Backup (Velero)
velero backup create proximadb-backup -n proximadb

# Restore
velero restore create proximadb-backup -n proximadb
```

## Troubleshooting

### Common Issues

**Pods not starting**:
```bash
kubectl describe pod <pod-name> -n proximadb
kubectl logs <pod-name> -n proximadb
```

**Storage issues**:
```bash
kubectl get pvc -n proximadb
kubectl get pv
```

**Networking issues**:
```bash
kubectl get svc -n proximadb
kubectl describe ingress <ingress-name> -n proximadb
```

**Performance issues**:
```bash
kubectl top pods -n proximadb
kubectl top nodes
```

### Getting Help

1. Check CloudWatch Logs
2. Review EKS control plane logs
3. Check GitHub issues: https://github.com/vjsingh1984/proximadb/issues
4. Join Discord: https://discord.gg/proximadb

## Cleanup

```bash
# Remove ProximaDB
helm uninstall proximadb -n proximadb

# Remove infrastructure
cd deploy/infrastructure/terraform/environments/dev
terraform destroy
```

## Best Practices

1. **Use separate AWS accounts** for dev/staging/production
2. **Enable versioning** on Terraform state
3. **Use remote state** (S3 + DynamoDB)
4. **Lock state** during CI/CD
5. **Review and approve** Terraform plans
6. **Use Terraform workspaces** for multiple environments
7. **Tag all resources** for cost allocation
8. **Set up monitoring** before deploying
9. **Test disaster recovery** procedures
10. **Regular security updates**

## Contributing

When adding new infrastructure:

1. Update Terraform modules
2. Update Helm charts
3. Update documentation
4. Test in dev environment
5. Submit PR with detailed description

## License

Apache License 2.0 - See LICENSE file for details
