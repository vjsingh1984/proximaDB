//! Shared audit storage contracts.
//!
//! Concrete file/database/cloud implementations remain in runtime crates. This
//! module defines the async contract and reporting DTOs that callers can depend
//! on without pulling in root audit services.

use crate::audit::{AuditEvent, AuditEventType};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Trait for audit storage backends.
#[async_trait]
pub trait AuditStorage {
    /// Store an audit event.
    async fn store_audit_event(&self, event: &AuditEvent) -> Result<()>;

    /// Query audit events with filters.
    async fn query_events(
        &self,
        event_type: Option<AuditEventType>,
        user_id: Option<String>,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<AuditEvent>>;

    /// Get audit statistics.
    async fn get_audit_statistics(&self, since: DateTime<Utc>) -> Result<AuditStatistics>;

    /// Cleanup old audit logs based on retention policy.
    async fn cleanup_old_logs(&self, retention_days: u32) -> Result<usize>;
}

/// Audit statistics for reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditStatistics {
    /// Total number of audit events in the reporting period.
    pub total_events: u64,
    /// Breakdown of event counts grouped by event type.
    pub events_by_type: std::collections::HashMap<AuditEventType, u64>,
    /// Number of distinct users who generated at least one event in the period.
    pub unique_users: u64,
    /// Number of distinct tenants that had activity in the period.
    pub unique_tenants: u64,
    /// Percentage of events that resulted in `AuditResult::Success` (0.0 - 100.0).
    pub success_rate: f64,
    /// Start of the statistics window (UTC).
    pub period_start: DateTime<Utc>,
    /// End of the statistics window (UTC).
    pub period_end: DateTime<Utc>,
}
