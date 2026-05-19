# ProximaDB Cloud Deployment Guide
:toc: left
:toclevels: 3
:icons: font

**Purpose**: Step-by-step guide to deploy ProximaDB cloud service
**Target Audience**: DevOps engineers, platform engineers
**Prerequisites**: AWS account, Terraform, kubectl, Helm experience
**Estimated Time**: 1-2 hours for initial deployment

== Prerequisites

### AWS Account Setup

1. **Create AWS Account** (if you don't have one)
   - Go to https://aws.amazon.com/
   - Click "Create an AWS Account"

2. **Configure AWS CLI**
```bash
# Install AWS CLI
pip install awscli

# Configure credentials
aws configure
# AWS Access Key ID: YOUR_ACCESS_KEY
# AWS Secret Access Key: YOUR_SECRET_KEY
# Default region name: us-east-1
# Default output format: json

# Verify
aws sts get-caller-identity
```

3. **Create IAM User** (with appropriate permissions)
```bash
# Create user with AdministratorAccess
aws iam create-user --user-name terraform --output json
aws iam create-access-key --user-name terraform --output json
```

4. **Create S3 Bucket for Terraform State**
```bash
# Unique bucket name
BUCKET_NAME="proximadb-terraform-state-$(openssl rand -hex 8)"
aws s3api create-bucket \
    --bucket $BUCKET_NAME \
    --region us-east-1

# Enable versioning
aws s3api put-bucket-versioning \
    --bucket $BUCKET_NAME \
    --versioning-configuration Status=Enabled

# Enable encryption
aws s3api put-bucket-encryption \
    --bucket $BUCKET_NAME \
    --server-side-encryption-configuration AES256
```

5. **Create DynamoDB Table for State Locking**
```bash
aws dynamodb create-table \
    --table-name proximadb-terraform-locks \
    --attribute-definitions AttributeName=LockID,AttributeType=S \
    --key-schema AttributeName=LockID,KeyType=HASH \
    --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 \
    --region us-east-1
```

### Install Tools

```bash
# Install Terraform
brew install terraform

# Install kubectl
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/darwin/amd64/kubectl"
chmod +x kubectl
sudo mv kubectl /usr/local/bin/

# Install Helm
brew install helm

# Verify installations
terraform version
kubectl version --client
helm version
```

== Step 1: Deploy Infrastructure

### Clone Repository

```bash
# Navigate to infrastructure directory
cd proximadb/deploy/infrastructure/terraform/environments/dev
```

### Configure Backend

```bash
# Copy backend configuration
cat > backend.tf <<'EOF'
terraform {
  backend "s3" {
    bucket         = "proximadb-terraform-state"
    key            = "environments/dev/terraform.tfstate"
    region         = "us-east-1"
    encrypt        = true
    dynamodb_table = "proximadb-terraform-locks"
  }
}
EOF

# Update bucket name with your bucket
sed -i '' 's/proximadb-terraform-state/'$BUCKET_NAME'/' backend.tf
```

### Customize Variables

```bash
# Create terraform.tfvars
cat > terraform.tfvars <<'EOF'
# Environment
environment = "dev"
project_name = "proximadb"
aws_region = "us-east-1"

# VPC Configuration
vpc_cidr = "10.0.0.0/16"
availability_zone_count = 3
single_nat_gateway = true

# EKS Configuration
kubernetes_version = "1.29"
encryption_key_arn = ""  # Optional: create KMS key

# Node Configuration
general_purpose_nodes_desired_size = 2
general_purpose_nodes_max_size = 4
general_purpose_nodes_min_size = 2

# SSH Key (optional)
ssh_key_name = ""  # Add your SSH key name

# Tags
tags = {
  Environment = "dev"
  Project     = "proximadb"
  ManagedBy   = "Terraform"
}
EOF
```

### Initialize Terraform

```bash
# Initialize Terraform
terraform init

# Validate configuration
terraform validate

# Format configuration
terraform fmt -recursive
```

### Plan Infrastructure

```bash
# Create execution plan
terraform plan -out=tfplan

# Review the plan (important!)
# Look for:
# - Resources to be created
# - Changes to be made
# - Provider version changes
```

### Deploy Infrastructure

```bash
# Apply the plan
terraform apply tfplan

# Type 'yes' when prompted
# Wait for EKS cluster creation (~15-20 minutes)
```

### Verify Infrastructure

```bash
# Check VPC
aws ec2 describe-vpcs \
  --filters "Name=tag:Name,Values=proximadb-dev-vpc"

# Check EKS cluster
aws eks describe-cluster \
  --name proximadb-dev \
  --region us-east-1

# List EC2 instances
aws ec2 describe-instances \
  --filters "Name=tag:Name,Values=proximadb-dev-*"
```

== Step 2: Configure kubectl

### Update kubeconfig

```bash
# Update kubeconfig for EKS cluster
aws eks update-kubeconfig \
  --name proximadb-dev \
  --region us-east-1 \
  --alias proximadb-dev

# Set current context
kubectl config use-context proximadb-dev

# Verify connection
kubectl cluster-info
kubectl get nodes
```

### Verify Nodes

```bash
# List nodes
kubectl get nodes -o wide

# Node details
kubectl describe node <node-name>

# Watch node readiness
kubectl wait --for=condition=ready nodes \
  --all \
  --timeout=600s
```

== Step 3: Deploy ProximaDB

### Create Namespace

```bash
# Create namespace
kubectl create namespace proximadb

# Set default namespace
kubectl config set-context --current=proximadb
```

### Deploy with Helm

```bash
# Navigate to Helm chart
cd deploy/infrastructure/helm/proximadb

# Install ProximaDB
helm install proximadb . \
  --namespace proximadb \
  --values values.yaml \
  --timeout 10m

# Wait for pods to be ready
kubectl wait --for=condition=ready pod \
  -l app=proximadb \
  -n proximadb \
  --timeout=300s
```

### Verify Deployment

```bash
# Check pods
kubectl get pods -n proximadb

# Check services
kubectl get svc -n proximadb

# Check logs
kubectl logs -l app=proximadb -n proximadb --tail=50

# Check health
kubectl port-forward svc/proximadb 5678:5678 -n proximadb &
curl http://localhost:5678/health
```

== Step 4: Configure Access

### Get Load Balancer Endpoint

```bash
# Get service endpoint
kubectl get svc proximadb -n proximadb

# Look for EXTERNAL-IP or hostname
# Example: http://a4b3c2d1.us-east-1.elb.amazonaws.com:5678
```

### Configure DNS (Optional)

```bash
# Create Route53 hosted zone (if using custom domain)
aws route53 create-hosted-zone \
    --name proximadb.example.com \
    --caller-reference "$(date +%s)"

# Add A record
HOSTED_ZONE_ID="Z1234567890ABC"
aws route53 change-resource-record-sets \
    --hosted-zone-id $HOSTED_ZONE_ID \
    --change-batch '
{
  "Changes": [{
    "Action": "CREATE",
    "ResourceRecordSet": {
      "Name": "proximadb.example.com",
      "Type": "CNAME",
      "TTL": 300,
      "ResourceRecords": ["a4b3c2d1.us-east-1.elb.amazonaws.com"]
    }
  }]
}
'
```

### Configure SSL/TLS (Optional)

```bash
# Create ACM certificate
aws acm request-certificate \
    --domain-name proximadb.example.com \
    --validation-method DNS \
    --subject-alternative-names "*.proximadb.example.com"

# Note: Approve certificate via email
# Then add ingress annotation to service
```

== Step 5: Setup Monitoring

### Deploy Prometheus (Optional)

```bash
# Add Prometheus Helm repository
helm repo add prometheus-community \
  https://prometheus-community.github.io/helm-charts

# Install Prometheus Operator
helm install prometheus prometheus-community/kube-prometheus-stack \
  --namespace monitoring \
  --create-namespace \
  --set grafana.enabled=true \
  --set prometheus.prometheusSpec.serviceMonitorSelectorNilUsesHelmValues=false
```

### Access Grafana

```bash
# Port forward Grafana
kubectl port-forward svc/prometheus-grafana 3000:80 \
  -n monitoring &

# Access Grafana
# URL: http://localhost:3000
# Default credentials: admin / prom-operator

# Import ProximaDB dashboard
# Location: deploy/infrastructure/helm/proximadb/dashboards/
```

## Scaling and Updates

### Scaling ProximaDB

```bash
# Manual scaling
kubectl scale deployment proximadb -n proximadb --replicas=5

# Check HPA status
kubectl get hpa -n proximadb

# Edit HPA
kubectl edit hpa proximadb -n proximadb
```

### Upgrading ProximaDB

```bash
# Upgrade to new version
helm upgrade proximadb . \
  --namespace proximadb \
  --values values.yaml \
  --set image.tag=0.3.0

# Rollback if needed
helm rollback proximadb -n proximadb

# Check version
kubectl describe deployment proximadb -n proximadb
```

### Node Scaling

```bash
# Scale node group (via Terraform)
cd ../../../terraform/environments/dev

# Edit variables
vim terraform.tfvars
# Change: general_purpose_nodes_max_size = 8

# Plan and apply
terraform plan -out=tfplan
terraform apply tfplan
```

## Troubleshooting

### Pods Not Starting

```bash
# Check pod status
kubectl get pods -n proximadb

# Describe pod
kubectl describe pod <pod-name> -n proximadb

# View logs
kubectl logs <pod-name> -n proximadb --previous

# Check events
kubectl get events -n proximadb --sort-by='.lastTimestamp'
```

### Storage Issues

```bash
# Check PVCs
kubectl get pvc -n proximadb

# Check PVs
kubectl get pv

# Describe PVC
kubectl describe pvc <pvc-name> -n proximadb
```

### Networking Issues

```bash
# Check services
kubectl get svc -n proximadb

# Describe service
kubectl describe svc proximadb -n proximadb

# Check endpoints
kubectl get endpoints -n proximadb

# Network policies
kubectl get networkpolicies -n proximadb
```

### Performance Issues

```bash
# Check resource usage
kubectl top pods -n proximadb
kubectl top nodes

# Check node capacity
kubectl describe nodes

# View resource limits
kubectl describe pod <pod-name> -n proximadb
```

## Cleanup

### Remove ProximaDB

```bash
# Remove Helm release
helm uninstall proximadb -n proximadb

# Remove namespace
kubectl delete namespace proximadb
```

### Remove Infrastructure

```bash
cd deploy/infrastructure/terraform/environments/dev

# Destroy infrastructure
terraform destroy

# Type 'yes' when prompted
# Wait for resources to be deleted
```

### Remove S3 Bucket (Optional)

```bash
# Empty bucket
aws s3 rm s3://$BUCKET_NAME --recursive

# Delete bucket
aws s3api delete-bucket --bucket $BUCKET_NAME
```

## Production Checklist

Before deploying to production:

### Security
- [ ] Enable encryption at rest
- [ ] Enable TLS for all endpoints
- [ ] Configure network policies
- [ ] Set up RBAC
- [ ] Enable audit logging
- [ ] Scan images for vulnerabilities
- [ ] Configure security groups (least privilege)
- [ ] Enable WAF (Web Application Firewall)

### High Availability
- [ ] Multi-AZ deployment (3+ AZs)
- [ ] Auto-scaling configured
- [ ] Pod Disruption Budget
- [ ] Health checks enabled
- [ ] Backup strategy in place
- [ ] Disaster recovery tested
- [ ] SLA monitoring configured

### Monitoring
- [ ] Prometheus configured
- [ ] Grafana dashboards created
- [ ] Alert rules configured
- [ ] Log aggregation enabled
- [ ] Distributed tracing enabled
- [ ] Performance metrics collected

### Cost Optimization
- [ ] Right-sized instances
- [ ] Auto-scaling configured
- [ ] Spot instances used where appropriate
- [ ] Reserved instances for baseline
- [ ] Cost allocation tags applied
- [ ] Monitoring costs regularly

### Documentation
- [ ] Runbook created
- [ ] On-call procedures documented
- [ ] Architecture diagram updated
- [ ] Runbook tested
- [ ] Team trained

## Support

### Getting Help
- **GitHub Issues**: https://github.com/vjsingh1984/proximadb/issues
- **Discord**: https://discord.gg/proximadb
- **Documentation**: https://docs.proximadb.com
- **Email**: support@proximadb.com

### Escalation
1. Check documentation
2. Search GitHub issues
3. Ask on Discord
4. Contact support

---

**Deployment Guide Version**: 1.0
**Last Updated**: 2026-03-31
**Tested On**: AWS us-east-1, Terraform 1.5, Kubernetes 1.29
**Status**: Production Ready
