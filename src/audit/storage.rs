//! Audit Storage Backends
//!
//! Multiple storage backend implementations for audit logs
//! including file-based, database, and cloud storage options.

use super::types::{AuditEvent, AuditEventType};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, info, warn};

/// Trait for audit storage backends
#[async_trait]
pub trait AuditStorage {
    /// Store an audit event
    async fn store_audit_event(&self, event: &AuditEvent) -> Result<()>;

    /// Query audit events with filters
    async fn query_events(
        &self,
        event_type: Option<AuditEventType>,
        user_id: Option<String>,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<AuditEvent>>;

    /// Get audit statistics
    async fn get_audit_statistics(&self, since: DateTime<Utc>) -> Result<AuditStatistics>;

    /// Cleanup old audit logs based on retention policy
    async fn cleanup_old_logs(&self, retention_days: u32) -> Result<usize>;
}

/// Audit statistics for reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditStatistics {
    pub total_events: u64,
    pub events_by_type: std::collections::HashMap<AuditEventType, u64>,
    pub unique_users: u64,
    pub unique_tenants: u64,
    pub success_rate: f64,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
}

/// File-based audit storage implementation
pub struct FileAuditStorage {
    base_directory: String,
    current_file: tokio::sync::RwLock<Option<String>>,
    file_rotation_size_mb: usize,
}

impl FileAuditStorage {
    pub async fn new(directory: String) -> Result<Self> {
        // Ensure directory exists
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|e| anyhow!("Failed to create audit directory {}: {}", directory, e))?;

        info!("✅ File audit storage initialized: {}", directory);

        Ok(Self {
            base_directory: directory,
            current_file: tokio::sync::RwLock::new(None),
            file_rotation_size_mb: 100, // Rotate files at 100MB
        })
    }

    /// Get current audit log file path
    async fn get_current_file_path(&self) -> Result<String> {
        let mut current_file = self.current_file.write().await;

        // Check if we need a new file
        if let Some(ref file_path) = *current_file {
            // Check file size for rotation
            if let Ok(metadata) = tokio::fs::metadata(file_path).await {
                let size_mb = metadata.len() as usize / (1024 * 1024);
                if size_mb < self.file_rotation_size_mb {
                    return Ok(file_path.clone());
                }
            }
        }

        // Create new audit log file
        let now = Utc::now();
        let filename = format!("audit_log_{}.jsonl", now.format("%Y%m%d_%H%M%S"));
        let file_path = Path::new(&self.base_directory).join(filename);

        *current_file = Some(file_path.to_string_lossy().to_string());

        info!("📝 Created new audit log file: {}", file_path.display());
        Ok(file_path.to_string_lossy().to_string())
    }
}

#[async_trait]
impl AuditStorage for FileAuditStorage {
    async fn store_audit_event(&self, event: &AuditEvent) -> Result<()> {
        let file_path = self.get_current_file_path().await?;

        // Serialize event as JSON line
        let event_json = serde_json::to_string(event)?;
        let log_line = format!("{}\n", event_json);

        // Append to audit log file
        tokio::fs::write(&file_path, &log_line)
            .await
            .map_err(|e| anyhow!("Failed to write audit event to {}: {}", file_path, e))?;

        debug!("📝 Stored audit event {} to {}", event.event_id, file_path);
        Ok(())
    }

    async fn query_events(
        &self,
        event_type: Option<AuditEventType>,
        user_id: Option<String>,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<AuditEvent>> {
        let mut events = Vec::new();

        // Read all audit log files in directory
        let mut dir_entries = tokio::fs::read_dir(&self.base_directory).await?;

        while let Some(entry) = dir_entries.next_entry().await? {
            let file_path = entry.path();

            if file_path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                let file_events = self
                    .read_events_from_file(
                        &file_path,
                        event_type.clone(),
                        user_id.clone(),
                        since,
                        until,
                    )
                    .await?;
                events.extend(file_events);

                // Apply limit if specified
                if let Some(limit) = limit {
                    if events.len() >= limit {
                        events.truncate(limit);
                        break;
                    }
                }
            }
        }

        // Sort by timestamp (newest first)
        events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        debug!("🔍 Queried {} audit events matching criteria", events.len());
        Ok(events)
    }

    async fn get_audit_statistics(&self, since: DateTime<Utc>) -> Result<AuditStatistics> {
        let events = self
            .query_events(None, None, Some(since), None, None)
            .await?;

        let mut events_by_type = std::collections::HashMap::new();
        let mut unique_users = std::collections::HashSet::new();
        let mut unique_tenants = std::collections::HashSet::new();
        let mut success_count = 0;

        for event in &events {
            // Count by event type
            *events_by_type.entry(event.event_type.clone()).or_insert(0) += 1;

            // Track unique users and tenants
            if let Some(ref user_id) = event.user_id {
                unique_users.insert(user_id.clone());
            }
            if let Some(ref tenant_id) = event.tenant_id {
                unique_tenants.insert(tenant_id.clone());
            }

            // Count successes
            if matches!(event.result, super::types::AuditResult::Success) {
                success_count += 1;
            }
        }

        let success_rate = if !events.is_empty() {
            (success_count as f64) / (events.len() as f64) * 100.0
        } else {
            0.0
        };

        Ok(AuditStatistics {
            total_events: events.len() as u64,
            events_by_type,
            unique_users: unique_users.len() as u64,
            unique_tenants: unique_tenants.len() as u64,
            success_rate,
            period_start: since,
            period_end: Utc::now(),
        })
    }

    async fn cleanup_old_logs(&self, retention_days: u32) -> Result<usize> {
        let cutoff_date = Utc::now() - chrono::Duration::days(retention_days as i64);
        let mut deleted_files = 0;

        let mut dir_entries = tokio::fs::read_dir(&self.base_directory).await?;

        while let Some(entry) = dir_entries.next_entry().await? {
            let file_path = entry.path();

            if let Ok(metadata) = entry.metadata().await {
                if let Ok(modified) = metadata.modified() {
                    let modified_dt: DateTime<Utc> = modified.into();

                    if modified_dt < cutoff_date {
                        if let Err(e) = tokio::fs::remove_file(&file_path).await {
                            warn!(
                                "Failed to delete old audit log {}: {}",
                                file_path.display(),
                                e
                            );
                        } else {
                            deleted_files += 1;
                            info!("🗑️ Deleted old audit log: {}", file_path.display());
                        }
                    }
                }
            }
        }

        info!(
            "🧹 Cleanup complete: deleted {} old audit log files",
            deleted_files
        );
        Ok(deleted_files)
    }
}

impl FileAuditStorage {
    /// Read events from a specific file with filtering
    async fn read_events_from_file(
        &self,
        file_path: &Path,
        event_type: Option<AuditEventType>,
        user_id: Option<String>,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    ) -> Result<Vec<AuditEvent>> {
        let mut events = Vec::new();

        let content = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| anyhow!("Failed to read audit file {}: {}", file_path.display(), e))?;

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<AuditEvent>(line) {
                Ok(event) => {
                    // Apply filters
                    if let Some(ref filter_type) = event_type {
                        if &event.event_type != filter_type {
                            continue;
                        }
                    }

                    if let Some(ref filter_user) = user_id {
                        if event.user_id.as_ref() != Some(filter_user) {
                            continue;
                        }
                    }

                    if let Some(since_time) = since {
                        if event.timestamp < since_time {
                            continue;
                        }
                    }

                    if let Some(until_time) = until {
                        if event.timestamp > until_time {
                            continue;
                        }
                    }

                    events.push(event);
                }
                Err(e) => {
                    warn!(
                        "Failed to parse audit event from {}: {}",
                        file_path.display(),
                        e
                    );
                }
            }
        }

        Ok(events)
    }
}

/// Database-based audit storage implementation (placeholder)
pub struct DatabaseAuditStorage {
    connection_string: String,
}

impl DatabaseAuditStorage {
    pub async fn new(connection_string: String) -> Result<Self> {
        // Placeholder for database initialization
        // Real implementation would:
        // - Connect to PostgreSQL/MySQL
        // - Create audit tables if they don't exist
        // - Set up indexes for efficient querying

        info!(
            "✅ Database audit storage initialized: {}",
            connection_string
        );

        Ok(Self { connection_string })
    }
}

#[async_trait]
impl AuditStorage for DatabaseAuditStorage {
    async fn store_audit_event(&self, _event: &AuditEvent) -> Result<()> {
        // Placeholder for database storage
        // Real implementation would:
        // - Insert into audit_events table
        // - Handle database connection pooling
        // - Implement transaction safety
        Ok(())
    }

    async fn query_events(
        &self,
        _event_type: Option<AuditEventType>,
        _user_id: Option<String>,
        _since: Option<DateTime<Utc>>,
        _until: Option<DateTime<Utc>>,
        _limit: Option<usize>,
    ) -> Result<Vec<AuditEvent>> {
        // Placeholder for database querying
        Ok(vec![])
    }

    async fn get_audit_statistics(&self, _since: DateTime<Utc>) -> Result<AuditStatistics> {
        // Placeholder for database statistics
        Ok(AuditStatistics {
            total_events: 0,
            events_by_type: std::collections::HashMap::new(),
            unique_users: 0,
            unique_tenants: 0,
            success_rate: 0.0,
            period_start: Utc::now(),
            period_end: Utc::now(),
        })
    }

    async fn cleanup_old_logs(&self, _retention_days: u32) -> Result<usize> {
        // Placeholder for database cleanup
        Ok(0)
    }
}
