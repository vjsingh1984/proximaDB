# EKS Module Variables

variable "cluster_name" {
  description = "Name of the EKS cluster"
  type        = string
}

variable "environment" {
  description = "Environment (e.g., dev, staging, production)"
  type        = string
}

variable "vpc_id" {
  description = "VPC ID where the cluster should be created"
  type        = string
}

variable "subnet_ids" {
  description = "List of subnet IDs where the cluster and nodes should be created"
  type        = list(string)
}

variable "kubernetes_version" {
  description = "Kubernetes version to use for the cluster"
  type        = string
  default     = "1.29"

  validation {
    condition     = can(regex("^1\\.(2[6-9]|[3-9][0-9])$", var.kubernetes_version))
    error_message = "Kubernetes version must be between 1.26 and 1.99."
  }
}

variable "cluster_service_ipv4_cidr" {
  description = "CIDR block to use for Kubernetes service network"
  type        = string
  default     = null
}

variable "endpoint_private_access" {
  description = "Enable private access to the Kubernetes API server"
  type        = bool
  default     = true
}

variable "endpoint_public_access" {
  description = "Enable public access to the Kubernetes API server"
  type        = bool
  default     = false
}

variable "public_access_cidrs" {
  description = "List of CIDR blocks that can access the Kubernetes API server publicly"
  type        = list(string)
  default     = ["0.0.0.0/0"]
}

variable "cluster_enabled_log_types" {
  description = "List of the desired control plane logging to enable"
  type        = list(string)
  default     = ["api", "audit", "authenticator", "controllerManager", "scheduler"]
}

variable "create_cloudwatch_log_group" {
  description = "Determines whether to create a CloudWatch Log Group for the cluster"
  type        = bool
  default     = true
}

variable "cloudwatch_log_retention_days" {
  description = "Number of days to retain CloudWatch logs"
  type        = number
  default     = 7

  validation {
    condition     = contains([1, 3, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1827, 3653], var.cloudwatch_log_retention_days)
    error_message = "cloudwatch_log_retention_days must be a valid CloudWatch retention period."
  }
}

variable "encryption_key_arn" {
  description = "ARN of the KMS key to use for envelope encryption of Kubernetes secrets"
  type        = string
  default     = null
}

variable "encryption_resources" {
  description = "List of Kubernetes resources to encrypt"
  type        = list(string)
  default     = ["secrets"]
}

variable "create_oidc_provider" {
  description = "Determines whether to create an IAM OpenID Connect provider for the cluster"
  type        = bool
  default     = true
}

variable "oidc_thumbprint_list" {
  description = "List of server certificate thumbprints for the OIDC provider"
  type        = list(string)
  default     = null
}

variable "managed_node_groups" {
  description = "Map of managed node group definitions to create"
  type = map(object({
    desired_size                   = number
    max_size                       = number
    min_size                       = number
    instance_types                 = list(string)
    capacity_type                  = string
    disk_size                      = number
    disk_type                      = string
    custom_ami_id                  = string
    platform                       = string
    ssh_key_name                   = string
    pre_bootstrap_user_data        = string
    post_bootstrap_user_data       = string
    kubelet_extra_args             = string
    max_unavailable_percentage     = number
    max_unavailable_count           = number
    labels                         = map(string)
    taints                         = list(object({
      key    = string
      value  = string
      effect = string
    }))
    tags                           = map(string)
  }))

  default = {
    main = {
      desired_size                 = 2
      max_size                     = 4
      min_size                     = 2
      instance_types               = ["t3.medium"]
      capacity_type                = "ON_DEMAND"
      disk_size                    = 50
      disk_type                    = "gp3"
      custom_ami_id                = null
      platform                     = "linux"
      ssh_key_name                 = null
      pre_bootstrap_user_data      = null
      post_bootstrap_user_data     = null
      kubelet_extra_args          = null
      max_unavailable_percentage    = 33
      max_unavailable_count        = null
      labels                       = {}
      taints                       = []
      tags                         = {}
    }
  }
}

variable "worker_additional_iam_policies" {
  description = "List of additional IAM policy ARNs to attach to worker nodes"
  type        = list(string)
  default     = []
}

variable "worker_security_group_ids" {
  description = "List of additional security group IDs to attach to worker nodes"
  type        = list(string)
  default     = []
}

variable "cluster_create_wait_duration" {
  description = "Duration to wait after cluster creation before applying resources"
  type        = string
  default     = "3m"
}

variable "create_coredns_configmap" {
  description = "Determines whether to create a CoreDNS ConfigMap"
  type        = bool
  default     = true
}

variable "create_kube_proxy_configmap" {
  description = "Determines whether to create a kube-proxy ConfigMap"
  type        = bool
  default     = true
}

variable "tags" {
  description = "Common tags to apply to all resources"
  type        = map(string)
  default     = {}
}
