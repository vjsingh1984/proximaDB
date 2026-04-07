# ProximaDB Cloud Service - Development Environment
# Root Terraform configuration

terraform {
  required_version = ">= 1.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  backend "s3" {
    bucket         = "proximadb-terraform-state"
    key            = "environments/dev/terraform.tfstate"
    region         = "us-east-1"
    encrypt        = true
    dynamodb_table = "proximadb-terraform-locks"
  }
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      Project     = "ProximaDB"
      Environment = var.environment
      ManagedBy   = "Terraform"
    }
  }
}

# VPC Module
module "vpc" {
  source = "../../modules/vpc"

  name                    = "${var.project_name}-${var.environment}"
  environment             = var.environment
  vpc_cidr                = var.vpc_cidr
  availability_zone_count = var.availability_zone_count
  single_nat_gateway     = var.single_nat_gateway
  enable_flow_logs       = true
  flow_log_retention_days = 7
  tags                    = var.tags
}

# EKS Cluster Module
module "eks" {
  source = "../../modules/eks"

  cluster_name                    = "${var.project_name}-${var.environment}"
  environment                     = var.environment
  vpc_id                          = module.vpc.vpc_id
  subnet_ids                      = module.vpc.private_subnet_ids
  kubernetes_version              = var.kubernetes_version
  endpoint_private_access         = true
  endpoint_public_access          = false
  cluster_enabled_log_types       = ["api", "audit", "authenticator", "controllerManager", "scheduler"]
  create_cloudwatch_log_group     = true
  cloudwatch_log_retention_days   = 7
  encryption_key_arn              = var.encryption_key_arn
  encryption_resources           = ["secrets"]
  create_oidc_provider            = true
  worker_security_group_ids       = [module.vpc.vpc_id]

  managed_node_groups = {
    general_purpose = {
      desired_size                = var.general_purpose_nodes_desired_size
      max_size                    = var.general_purpose_nodes_max_size
      min_size                    = var.general_purpose_nodes_min_size
      instance_types              = ["t3.medium"]
      capacity_type               = "ON_DEMAND"
      disk_size                   = 50
      disk_type                   = "gp3"
      platform                    = "linux"
      ssh_key_name                = var.ssh_key_name
      max_unavailable_percentage  = 33
      labels                      = {
        role = "general-purpose"
      }
      taints                      = []
      tags                        = {}
    }

    compute_optimized = {
      desired_size                = var.compute_optimized_nodes_desired_size
      max_size                    = var.compute_optimized_nodes_max_size
      min_size                    = var.compute_optimized_nodes_min_size
      instance_types              = ["c5.xlarge"]
      capacity_type               = "SPOT"
      disk_size                   = 100
      disk_type                   = "gp3"
      platform                    = "linux"
      max_unavailable_percentage  = 50
      labels                      = {
        role = "compute-optimized"
      }
      taints                      = [
        {
          key    = "workload"
          value  = "compute"
          effect = "NO_SCHEDULE"
        }
      ]
      tags                        = {}
    }
  }

  worker_additional_iam_policies = var.worker_additional_iam_policies

  cluster_create_wait_duration = "3m"
  create_coredns_configmap      = true
  create_kube_proxy_configmap   = true

  tags = var.tags
}

# RDS for PostgreSQL (Metadata Service)
resource "aws_db_subnet_group" "metadata" {
  name       = "${var.project_name}-metadata-subnet-group"
  subnet_ids = module.vpc.database_subnet_ids

  tags = merge(
    var.tags,
    {
      Name        = "${var.project_name}-metadata-subnet-group"
      Environment = var.environment
    }
  )
}

resource "aws_security_group" "metadata" {
  name        = "${var.project_name}-metadata-sg"
  description = "Security group for PostgreSQL metadata database"
  vpc_id      = module.vpc.vpc_id

  tags = merge(
    var.tags,
    {
      Name        = "${var.project_name}-metadata-sg"
      Environment = var.environment
    }
  )
}

resource "aws_security_group_rule" "metadata_ingress" {
  description       = "Allow EKS nodes to connect to metadata database"
  from_port         = 5432
  to_port           = 5432
  protocol          = "tcp"
  source_security_group_id = module.eks.cluster_security_group_id
  security_group_id = aws_security_group.metadata.id
  type              = "ingress"
}

# S3 Bucket for WAL Archival
resource "aws_s3_bucket" "wal_archive" {
  bucket = "${var.project_name}-wal-archive-${var.environment}"

  tags = merge(
    var.tags,
    {
      Name        = "${var.project_name}-wal-archive"
      Environment = var.environment
    }
  )
}

resource "aws_s3_bucket_versioning" "wal_archive" {
  bucket = aws_s3_bucket.wal_archive.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "wal_archive" {
  bucket = aws_s3_bucket.wal_archive.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "wal_archive" {
  bucket = aws_s3_bucket.wal_archive.id

  rule {
    id     = "wal-archive-lifecycle"
    status = "Enabled"

    noncurrent_version_transition {
      noncurrent_days = 30
      storage_class   = "STANDARD_IA"
    }

    noncurrent_version_transition {
      noncurrent_days = 90
      storage_class   = "GLACIER"
    }

    noncurrent_version_expiration {
      noncurrent_days = 365
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

# Elasticache (Redis) for Caching
resource "aws_subnet_group" "elasticache" {
  name       = "${var.project_name}-elasticache-subnet-group"
  subnet_ids = module.vpc.private_subnet_ids

  tags = merge(
    var.tags,
    {
      Name        = "${var.project_name}-elasticache-subnet-group"
      Environment = var.environment
    }
  )
}

resource "aws_security_group" "elasticache" {
  name        = "${var.project_name}-elasticache-sg"
  description = "Security group for ElastiCache Redis"
  vpc_id      = module.vpc.vpc_id

  tags = merge(
    var.tags,
    {
      Name        = "${var.project_name}-elasticache-sg"
      Environment = var.environment
    }
  )
}

resource "aws_security_group_rule" "elasticache_ingress" {
  description       = "Allow EKS nodes to connect to Redis"
  from_port         = 6379
  to_port           = 6379
  protocol          = "tcp"
  source_security_group_id = module.eks.cluster_security_group_id
  security_group_id = aws_security_group.elasticache.id
  type              = "ingress"
}

# Outputs
output "vpc_id" {
  description = "VPC ID"
  value       = module.vpc.vpc_id
}

output "eks_cluster_endpoint" {
  description = "EKS cluster endpoint"
  value       = module.eks.cluster_endpoint
}

output "eks_cluster_certificate_authority_data" {
  description = "EKS cluster certificate authority data"
  value       = module.eks.cluster_certificate_authority_data
  sensitive   = true
}

output "eks_cluster_name" {
  description = "EKS cluster name"
  value       = module.eks.cluster_name
}

output "configure_kubectl" {
  description = "Configure kubectl for the cluster"
  value       = module.eks.configure_kubectl
}
