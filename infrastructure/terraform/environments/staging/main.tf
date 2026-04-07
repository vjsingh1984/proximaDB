/**
 * ProximaDB Staging Environment Configuration
 *
 * Purpose: Pre-production testing environment
 * Characteristics: Multi-AZ, right-sized for load testing, production-like configuration
 */

terraform {
  required_version = ">= 1.5"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      Environment = "staging"
      Project     = var.project_name
      ManagedBy   = "Terraform"
    }
  }
}

# Locals for environment-specific configuration
locals {
  environment = "staging"

  # Multi-AZ configuration for high availability
  availability_zones = slice(data.aws_availability_zones.available.names, 0, 3)

  # CIDR blocks
  vpc_cidr             = var.vpc_cidr
  public_subnet_cidrs  = ["10.0.1.0/24", "10.0.2.0/24", "10.0.3.0/24"]
  private_subnet_cidrs = ["10.0.4.0/24", "10.0.5.0/24", "10.0.6.0/24"]
  database_subnet_cidrs = ["10.0.7.0/24", "10.0.8.0/24", "10.0.9.0/24"]
}

# Data sources
data "aws_availability_zones" "available" {
  state = "available"
}

# VPC Module
module "vpc" {
  source = "../../modules/vpc"

  name                  = "${var.project_name}-${local.environment}"
  environment           = local.environment
  vpc_cidr              = local.vpc_cidr
  availability_zone_count = 3
  single_nat_gateway    = false  # Multi-AZ needs multiple NAT gateways

  enable_flow_logs      = true
  flow_logs_retention_days = 30

  tags = var.tags
}

# EKS Module
module "eks" {
  source = "../../modules/eks"

  cluster_name    = "${var.project_name}-${local.environment}"
  environment     = local.environment

  kubernetes_version = var.kubernetes_version

  vpc_id          = module.vpc.vpc_id
  subnet_ids      = module.vpc.private_subnet_ids

  # Staging requires more capacity than dev
  managed_node_groups = {
    general_purpose = {
      desired_size = var.general_purpose_nodes_desired_size
      max_size     = var.general_purpose_nodes_max_size
      min_size     = var.general_purpose_nodes_min_size

      instance_types = ["t3.large"]
      capacity_type  = "ON_DEMAND"

      # Multi-AZ spread
      subnet_ids = module.vpc.private_subnet_ids

      # Enhanced monitoring
      enable_monitoring = true

      # Disk size
      disk_size = 100
    }

    compute_optimized = {
      desired_size = var.compute_optimized_nodes_desired_size
      max_size     = var.compute_optimized_nodes_max_size
      min_size     = var.compute_optimized_nodes_min_size

      instance_types = ["c5.xlarge"]
      capacity_type  = "SPOT"  # Use spot for cost optimization

      # Multi-AZ spread
      subnet_ids = module.vpc.private_subnet_ids

      # Enhanced monitoring
      enable_monitoring = true

      # Disk size
      disk_size = 100
    }
  }

  # Encryption
  encryption_key_arn = var.encryption_key_arn

  # Logging
  enable_control_plane_logging = true
  log_retention_days = 30

  tags = var.tags
}

# RDS Subnet Group
resource "aws_db_subnet_group" "proximadb" {
  name       = "${var.project_name}-${local.environment}"
  subnet_ids = module.vpc.database_subnet_ids

  tags = {
    Name        = "${var.project_name}-${local.environment}"
    Environment = local.environment
  }
}

# ElastiCache Subnet Group
resource "aws_elasticache_subnet_group" "proximadb" {
  name       = "${var.project_name}-${local.environment}"
  subnet_ids = module.vpc.database_subnet_ids

  tags = {
    Name        = "${var.project_name}-${local.environment}"
    Environment = local.environment
  }
}

# S3 Bucket for WAL Archival
resource "aws_s3_bucket" "wal_archival" {
  bucket = "${var.project_name}-wal-archive-${local.environment}"

  tags = {
    Name        = "${var.project_name}-wal-archive-${local.environment}"
    Environment = local.environment
  }
}

resource "aws_s3_bucket_versioning" "wal_archival" {
  bucket = aws_s3_bucket.wal_archival.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "wal_archival" {
  bucket = aws_s3_bucket.wal_archival.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "wal_archival" {
  bucket = aws_s3_bucket.wal_archival.id

  rule {
    id     = "wal-retention"
    status = "Enabled"

    expiration {
      days = 30
    }

    noncurrent_version_expiration {
      noncurrent_days = 7
    }
  }
}

# Security Group for RDS
resource "aws_security_group" "rds" {
  name_prefix = "${var.project_name}-${local.environment}-rds-"
  description = "Security group for RDS"
  vpc_id      = module.vpc.vpc_id

  tags = {
    Name        = "${var.project_name}-${local.environment}-rds"
    Environment = local.environment
  }
}

resource "aws_security_group_rule" "rds_ingress" {
  description              = "Allow EKS nodes to connect to RDS"
  from_port                = 5432
  to_port                  = 5432
  protocol                 = "tcp"
  security_group_id        = aws_security_group.rds.id
  source_security_group_id = module.eks.node_security_group_id
}

# Security Group for ElastiCache
resource "aws_security_group" "elasticache" {
  name_prefix = "${var.project_name}-${local.environment}-elasticache-"
  description = "Security group for ElastiCache"
  vpc_id      = module.vpc.vpc_id

  tags = {
    Name        = "${var.project_name}-${local.environment}-elasticache"
    Environment = local.environment
  }
}

resource "aws_security_group_rule" "elasticache_ingress" {
  description              = "Allow EKS nodes to connect to ElastiCache"
  from_port                = 6379
  to_port                  = 6379
  protocol                 = "tcp"
  security_group_id        = aws_security_group.elasticache.id
  source_security_group_id = module.eks.node_security_group_id
}

# Outputs
output "vpc_id" {
  description = "VPC ID"
  value       = module.vpc.vpc_id
}

output "eks_cluster_id" {
  description = "EKS cluster ID"
  value       = module.eks.cluster_id
}

output "eks_cluster_endpoint" {
  description = "EKS cluster endpoint"
  value       = module.eks.cluster_endpoint
}

output "eks_cluster_security_group_id" {
  description = "EKS cluster security group ID"
  value       = module.eks.cluster_security_group_id
}

output "eks_node_security_group_id" {
  description = "EKS node security group ID"
  value       = module.eks.node_security_group_id
}

output "eks_cluster_certificate_authority_data" {
  description = "EKS cluster certificate authority data"
  value       = module.eks.cluster_certificate_authority_data
}

output "private_subnet_ids" {
  description = "Private subnet IDs"
  value       = module.vpc.private_subnet_ids
}

output "wal_archive_bucket_name" {
  description = "S3 bucket name for WAL archival"
  value       = aws_s3_bucket.wal_archival.id
}

output "rds_subnet_group_name" {
  description = "RDS subnet group name"
  value       = aws_db_subnet_group.proximadb.name
}

output "elasticache_subnet_group_name" {
  description = "ElastiCache subnet group name"
  value       = aws_elasticache_subnet_group.proximadb.name
}

output "rds_security_group_id" {
  description = "RDS security group ID"
  value       = aws_security_group.rds.id
}

output "elasticache_security_group_id" {
  description = "ElastiCache security group ID"
  value       = aws_security_group.elasticache.id
}
