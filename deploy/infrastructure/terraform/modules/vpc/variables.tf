# VPC Module Variables

variable "name" {
  description = "Name of the VPC and resources"
  type        = string
}

variable "environment" {
  description = "Environment (e.g., dev, staging, production)"
  type        = string
}

variable "vpc_cidr" {
  description = "CIDR block for VPC"
  type        = string
  default     = "10.0.0.0/16"
}

variable "availability_zone_count" {
  description = "Number of availability zones to use"
  type        = number
  default     = 3

  validation {
    condition     = var.availability_zone_count >= 2 && var.availability_zone_count <= 4
    error_message = "availability_zone_count must be between 2 and 4."
  }
}

variable "single_nat_gateway" {
  description = "Use single NAT Gateway for all availability zones (cost savings)"
  type        = bool
  default     = true
}

variable "enable_flow_logs" {
  description = "Enable VPC Flow Logs for security monitoring"
  type        = bool
  default     = true
}

variable "flow_log_retention_days" {
  description = "Number of days to retain VPC Flow Logs"
  type        = number
  default     = 7

  validation {
    condition     = contains([1, 3, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1827, 3653], var.flow_log_retention_days)
    error_message = "flow_log_retention_days must be a valid CloudWatch retention period."
  }
}

variable "enable_dhcp_options" {
  description = "Enable custom DHCP options"
  type        = bool
  default     = false
}

variable "dhcp_domain_name_servers" {
  description = "List of DNS servers to configure in DHCP options"
  type        = list(string)
  default     = ["AmazonProvidedDNS"]
}

variable "dhcp_domain_name" {
  description = "DNS domain name to use in DHCP options"
  type        = string
  default     = null
}

variable "dhcp_ntp_servers" {
  description = "List of NTP servers to configure in DHCP options"
  type        = list(string)
  default     = null
}

variable "dhcp_netbios_name_servers" {
  description = "List of NetBIOS name servers"
  type        = list(string)
  default     = null
}

variable "dhcp_netbios_node_type" {
  description = "NetBIOS node type"
  type        = string
  default     = null
}

variable "tags" {
  description = "Common tags to apply to all resources"
  type        = map(string)
  default     = {}
}
