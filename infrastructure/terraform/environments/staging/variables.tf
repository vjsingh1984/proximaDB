/**
 * ProximaDB Staging Environment Variables
 */

variable "project_name" {
  description = "Project name used for resource naming"
  type        = string
  default     = "proximadb"
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
}

variable "encryption_key_arn" {
  description = "KMS key ARN for encryption (optional)"
  type        = string
  default     = ""
}

# General Purpose Node Group

variable "general_purpose_nodes_desired_size" {
  description = "Desired size of general purpose node group"
  type        = number
  default     = 4
}

variable "general_purpose_nodes_max_size" {
  description = "Maximum size of general purpose node group"
  type        = number
  default     = 8
}

variable "general_purpose_nodes_min_size" {
  description = "Minimum size of general purpose node group"
  type        = number
  default     = 4
}

# Compute Optimized Node Group

variable "compute_optimized_nodes_desired_size" {
  description = "Desired size of compute optimized node group"
  type        = number
  default     = 2
}

variable "compute_optimized_nodes_max_size" {
  description = "Maximum size of compute optimized node group"
  type        = number
  default     = 4
}

variable "compute_optimized_nodes_min_size" {
  description = "Minimum size of compute optimized node group"
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
    Environment = "staging"
    Project     = "proximadb"
    ManagedBy   = "Terraform"
  }
}
