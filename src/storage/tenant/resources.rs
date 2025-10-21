//! Tenant resource tracking and management - simple implementation

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Simple tenant resource tracker
pub struct TenantResourceTracker {
    tenant_id: String,
    limits: ResourceLimits,
    usage: TenantResourceUsage,
    last_updated: AtomicU64, // Unix timestamp
}

/// Resource limits (copied from context.rs to avoid circular deps)
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    pub max_memory_mb: u64,
    pub max_storage_mb: u64,
    pub max_operations_per_minute: u64,
    pub max_concurrent_users: u32,
    pub max_collections: u32,
    pub max_domains: u32,
}

/// Current resource usage tracking
pub struct TenantResourceUsage {
    memory_used_mb: AtomicU64,
    storage_used_mb: AtomicU64,
    operations_current_minute: AtomicU64,
    concurrent_users: AtomicU32,
    total_collections: AtomicU32,
    total_domains: AtomicU32,
}

impl TenantResourceTracker {
    /// Create new resource tracker
    pub fn new(tenant_id: &str, limits: &ResourceLimits) -> Self {
        Self {
            tenant_id: tenant_id.to_string(),
            limits: limits.clone(),
            usage: TenantResourceUsage {
                memory_used_mb: AtomicU64::new(0),
                storage_used_mb: AtomicU64::new(0),
                operations_current_minute: AtomicU64::new(0),
                concurrent_users: AtomicU32::new(0),
                total_collections: AtomicU32::new(0),
                total_domains: AtomicU32::new(0),
            },
            last_updated: AtomicU64::new(Utc::now().timestamp() as u64),
        }
    }

    /// Check if operation is within resource limits
    pub fn check_operation_allowed(&self) -> Result<()> {
        let current_ops = self.usage.operations_current_minute.load(Ordering::Relaxed);
        if current_ops >= self.limits.max_operations_per_minute {
            return Err(anyhow!(
                "Tenant {} exceeded operations limit",
                self.tenant_id
            ));
        }

        let current_users = self.usage.concurrent_users.load(Ordering::Relaxed);
        if current_users >= self.limits.max_concurrent_users {
            return Err(anyhow!(
                "Tenant {} exceeded concurrent user limit",
                self.tenant_id
            ));
        }

        Ok(())
    }

    /// Record operation for tracking
    pub fn record_operation(&self) {
        self.usage
            .operations_current_minute
            .fetch_add(1, Ordering::Relaxed);
        self.last_updated
            .store(Utc::now().timestamp() as u64, Ordering::Relaxed);
    }

    /// Add concurrent user
    pub fn add_concurrent_user(&self) -> Result<()> {
        let current = self.usage.concurrent_users.fetch_add(1, Ordering::Relaxed);
        if current >= self.limits.max_concurrent_users {
            self.usage.concurrent_users.fetch_sub(1, Ordering::Relaxed);
            return Err(anyhow!("Concurrent user limit exceeded"));
        }
        Ok(())
    }

    /// Remove concurrent user
    pub fn remove_concurrent_user(&self) {
        self.usage.concurrent_users.fetch_sub(1, Ordering::Relaxed);
    }

    /// Add collection
    pub fn add_collection(&self) -> Result<()> {
        let current = self.usage.total_collections.fetch_add(1, Ordering::Relaxed);
        if current >= self.limits.max_collections {
            self.usage.total_collections.fetch_sub(1, Ordering::Relaxed);
            return Err(anyhow!("Collection limit exceeded"));
        }
        Ok(())
    }

    /// Add domain
    pub fn add_domain(&self) -> Result<()> {
        let current = self.usage.total_domains.fetch_add(1, Ordering::Relaxed);
        if current >= self.limits.max_domains {
            self.usage.total_domains.fetch_sub(1, Ordering::Relaxed);
            return Err(anyhow!("Domain limit exceeded"));
        }
        Ok(())
    }

    /// Get current usage snapshot
    pub fn get_current_usage(&self) -> TenantResourceUsageSnapshot {
        TenantResourceUsageSnapshot {
            memory_used_mb: self.usage.memory_used_mb.load(Ordering::Relaxed),
            storage_used_mb: self.usage.storage_used_mb.load(Ordering::Relaxed),
            operations_current_minute: self.usage.operations_current_minute.load(Ordering::Relaxed),
            concurrent_users: self.usage.concurrent_users.load(Ordering::Relaxed),
            total_collections: self.usage.total_collections.load(Ordering::Relaxed),
            total_domains: self.usage.total_domains.load(Ordering::Relaxed),
            last_updated: DateTime::from_timestamp(
                self.last_updated.load(Ordering::Relaxed) as i64,
                0,
            )
            .unwrap_or(Utc::now()),
        }
    }

    /// Reset minute-based counters (called by background task)
    pub fn reset_minute_counters(&self) {
        self.usage
            .operations_current_minute
            .store(0, Ordering::Relaxed);
        self.last_updated
            .store(Utc::now().timestamp() as u64, Ordering::Relaxed);
    }
}

/// Resource usage snapshot
#[derive(Debug, Clone)]
pub struct TenantResourceUsageSnapshot {
    pub memory_used_mb: u64,
    pub storage_used_mb: u64,
    pub operations_current_minute: u64,
    pub concurrent_users: u32,
    pub total_collections: u32,
    pub total_domains: u32,
    pub last_updated: DateTime<Utc>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_mb: 4096,
            max_storage_mb: 102400,
            max_operations_per_minute: 10000,
            max_concurrent_users: 100,
            max_collections: 50,
            max_domains: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();

        assert_eq!(limits.max_memory_mb, 4096);
        assert_eq!(limits.max_storage_mb, 102400);
        assert_eq!(limits.max_operations_per_minute, 10000);
        assert_eq!(limits.max_concurrent_users, 100);
        assert_eq!(limits.max_collections, 50);
        assert_eq!(limits.max_domains, 10);
    }

    #[test]
    fn test_resource_tracker_operation_limits() {
        let limits = ResourceLimits {
            max_operations_per_minute: 2,
            ..Default::default()
        };

        let tracker = TenantResourceTracker::new("test_tenant", &limits);

        // First two operations should succeed
        assert!(tracker.check_operation_allowed().is_ok());
        tracker.record_operation();

        assert!(tracker.check_operation_allowed().is_ok());
        tracker.record_operation();

        // Third operation should be rate limited
        assert!(tracker.check_operation_allowed().is_err());
    }

    #[test]
    fn test_concurrent_user_tracking() {
        let limits = ResourceLimits {
            max_concurrent_users: 2,
            ..Default::default()
        };

        let tracker = TenantResourceTracker::new("test_tenant", &limits);

        // Add users up to limit
        assert!(tracker.add_concurrent_user().is_ok());
        assert!(tracker.add_concurrent_user().is_ok());

        // Exceeding limit should fail
        assert!(tracker.add_concurrent_user().is_err());

        // Removing user should allow new one
        tracker.remove_concurrent_user();
        assert!(tracker.add_concurrent_user().is_ok());
    }
}
