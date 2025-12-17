# ProximaDB Module Outputs

output "name" {
  description = "Resource name prefix"
  value       = var.name
}

output "environment" {
  description = "Deployment environment"
  value       = var.environment
}

output "container_image" {
  description = "Container image"
  value       = var.container_image
}

output "storage_config" {
  description = "Storage configuration"
  value = {
    size_gb = var.storage_size_gb
    type    = var.storage_type
    engine  = var.storage_engine
  }
}

output "replica_count" {
  description = "Number of replicas"
  value       = var.replica_count
}

output "monitoring_enabled" {
  description = "Whether monitoring is enabled"
  value       = var.enable_monitoring
}

output "backup_config" {
  description = "Backup configuration"
  value = {
    enabled        = var.enable_backups
    retention_days = var.backup_retention_days
  }
}
