/**
 * ProximaDB Production Environment Variables
 *
 * IMPORTANT: Production environment requires:
 * - KMS encryption key ARN (mandatory)
 * - Multi-AZ deployment
 * - Enhanced monitoring and alerting
 * - Backup and disaster recovery
 * - Cost optimization
 */

variable "project_name" {
  description = "Project name used for resource naming"
  type        = string
  default     = "proximadb"

  validation {
    condition     = can(regex("^[a-z0-9-]+$", var.project_name))
    error_message = "Project name must contain only lowercase letters, numbers, and hyphens."
  }
}

variable "aws_region" {
  description = "AWS region for resources"
  type        = string
  default     = "us-east-1"
}

variable "vpc_cidr" {
  description = "CIDR block for VPC"
  type        = string
  default     = "10.0.0.0/16"
}

variable "kubernetes_version" {
  description = "Kubernetes version for EKS cluster"
  type        = string
  default     = "1.29"

  validation {
    condition     = can(regex("^1\\.(2[8-9]|[3-9][0-9])$", var.kubernetes_version))
    error_message = "Kubernetes version must be 1.28 or higher."
  }
}

variable "encryption_key_arn" {
  description = "KMS key ARN for encryption (mandatory for production)"
  type        = string
  default     = ""

  validation {
    condition     = var.encryption_key_arn != "" && can(regex("^arn:aws:kms:", var.encryption_key_arn))
    error_message = "Production environment requires a valid KMS key ARN."
  }
}

# General Purpose Node Group

variable "general_purpose_nodes_desired_size" {
  description = "Desired size of general purpose node group"
  type        = number
  default     = 6

  validation {
    condition     = var.general_purpose_nodes_desired_size >= 3
    error_message = "Desired size must be at least 3 for multi-AZ high availability."
  }
}

variable "general_purpose_nodes_max_size" {
  description = "Maximum size of general purpose node group"
  type        = number
  default     = 12

  validation {
    condition     = var.general_purpose_nodes_max_size >= var.general_purpose_nodes_desired_size
    error_message = "Max size must be greater than or equal to desired size."
  }
}

variable "general_purpose_nodes_min_size" {
  description = "Minimum size of general purpose node group"
  type        = number
  default     = 6

  validation {
    condition     = var.general_purpose_nodes_min_size >= 3
    error_message = "Min size must be at least 3 for multi-AZ high availability."
  }
}

# Compute Optimized Node Group

variable "compute_optimized_nodes_desired_size" {
  description = "Desired size of compute optimized node group"
  type        = number
  default     = 4
}

variable "compute_optimized_nodes_max_size" {
  description = "Maximum size of compute optimized node group"
  type        = number
  default     = 8

  validation {
    condition     = var.compute_optimized_nodes_max_size >= var.compute_optimized_nodes_desired_size
    error_message = "Max size must be greater than or equal to desired size."
  }
}

variable "compute_optimized_nodes_min_size" {
  description = "Minimum size of compute optimized node group"
  type        = number
  default     = 2
}

# Memory Optimized Node Group

variable "memory_optimized_nodes_desired_size" {
  description = "Desired size of memory optimized node group"
  type        = number
  default     = 2
}

variable "memory_optimized_nodes_max_size" {
  description = "Maximum size of memory optimized node group"
  type        = number
  default     = 4

  validation {
    condition     = var.memory_optimized_nodes_max_size >= var.memory_optimized_nodes_desired_size
    error_message = "Max size must be greater than or equal to desired size."
  }
}

variable "memory_optimized_nodes_min_size" {
  description = "Minimum size of memory optimized node group"
  type        = number
  default     = 2
}

variable "ssh_key_name" {
  description = "SSH key name for EC2 instances (optional)"
  type        = string
  default     = ""
}

variable "tags" {
  description = "Additional tags for all resources"
  type        = map(string)
  default = {
    Environment = "production"
    Project     = "proximadb"
    ManagedBy   = "Terraform"
  }
}
