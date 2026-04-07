# Development Environment Variables

variable "project_name" {
  description = "Project name used for resource naming"
  type        = string
  default     = "proximadb"
}

variable "environment" {
  description = "Environment name"
  type        = string
  default     = "dev"
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

variable "availability_zone_count" {
  description = "Number of availability zones"
  type        = number
  default     = 3
}

variable "single_nat_gateway" {
  description = "Use single NAT Gateway (cost savings for dev)"
  type        = bool
  default     = true
}

variable "kubernetes_version" {
  description = "Kubernetes version"
  type        = string
  default     = "1.29"
}

variable "encryption_key_arn" {
  description = "KMS key ARN for envelope encryption"
  type        = string
  default     = null
}

variable "ssh_key_name" {
  description = "SSH key name for EC2 instances"
  type        = string
  default     = null
}

variable "worker_additional_iam_policies" {
  description = "Additional IAM policies for worker nodes"
  type        = list(string)
  default     = [
    "arn:aws:iam::aws:policy/AmazonS3FullAccess",
    "arn:aws:iam::aws:policy/CloudWatchLogsFullAccess",
    "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
  ]
}

# Node group configurations
variable "general_purpose_nodes_desired_size" {
  description = "Desired number of general purpose nodes"
  type        = number
  default     = 2
}

variable "general_purpose_nodes_max_size" {
  description = "Maximum number of general purpose nodes"
  type        = number
  default     = 4
}

variable "general_purpose_nodes_min_size" {
  description = "Minimum number of general purpose nodes"
  type        = number
  default     = 2
}

variable "compute_optimized_nodes_desired_size" {
  description = "Desired number of compute optimized nodes"
  type        = number
  default     = 0
}

variable "compute_optimized_nodes_max_size" {
  description = "Maximum number of compute optimized nodes"
  type        = number
  default     = 2
}

variable "compute_optimized_nodes_min_size" {
  description = "Minimum number of compute optimized nodes"
  type        = number
  default     = 0
}

variable "tags" {
  description = "Common tags"
  type        = map(string)
  default = {
    CostCenter = "engineering"
    Team       = "platform"
  }
}
