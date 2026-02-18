// Alerting engine for observability data
//
// Provides:
// - Rule-based alerting
// - Threshold and anomaly detection
// - Notification channels (webhook, Slack, PagerDuty)
// - Alert aggregation and deduplication

pub mod engine;
pub mod notifications;
pub mod rules;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, info};

use self::engine::AlertEngine;
use self::notifications::NotificationManager;
use self::rules::{AlertRule, AlertRuleId};

/// Alerting service
pub struct AlertingService {
    /// Alert engine
    engine: Arc<AlertEngine>,
    /// Notification manager
    notifications: Arc<NotificationManager>,
    /// Active alerts
    active_alerts: RwLock<HashMap<String, ActiveAlert>>,
}

impl AlertingService {
    /// Create a new alerting service
    pub fn new() -> Self {
        Self {
            engine: Arc::new(AlertEngine::new()),
            notifications: Arc::new(NotificationManager::new()),
            active_alerts: RwLock::new(HashMap::new()),
        }
    }

    /// Register an alert rule
    pub async fn register_rule(&self, rule: AlertRule) -> Result<AlertRuleId> {
        self.engine.register_rule(rule).await
    }

    /// Unregister an alert rule
    pub async fn unregister_rule(&self, rule_id: &AlertRuleId) -> Result<()> {
        self.engine.unregister_rule(rule_id).await
    }

    /// Fire an alert
    pub async fn fire_alert(&self, alert: Alert) -> Result<()> {
        let alert_key = alert.key();

        // Check for duplicate/aggregation
        {
            let alerts = self.active_alerts.read().await;
            if let Some(_existing) = alerts.get(&alert_key) {
                // Update existing alert
                debug!("Alert already active: {}", alert_key);
                return Ok(());
            }
        }

        // Create active alert
        let active = ActiveAlert {
            alert: alert.clone(),
            fired_at: chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            acknowledged: false,
            acknowledged_by: None,
            acknowledged_at: None,
        };

        {
            let mut alerts = self.active_alerts.write().await;
            alerts.insert(alert_key.clone(), active);
        }

        // Send notifications
        self.notifications.send(&alert).await?;

        info!("Alert fired: {} - {}", alert.name, alert.message);

        Ok(())
    }

    /// Resolve an alert
    pub async fn resolve_alert(&self, alert_key: &str) -> Result<()> {
        let mut alerts = self.active_alerts.write().await;
        if let Some(active) = alerts.remove(alert_key) {
            info!("Alert resolved: {}", active.alert.name);
        }
        Ok(())
    }

    /// Acknowledge an alert
    pub async fn acknowledge_alert(&self, alert_key: &str, user: &str) -> Result<()> {
        let mut alerts = self.active_alerts.write().await;
        if let Some(active) = alerts.get_mut(alert_key) {
            active.acknowledged = true;
            active.acknowledged_by = Some(user.to_string());
            active.acknowledged_at = Some(chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
            info!("Alert acknowledged by {}: {}", user, active.alert.name);
        }
        Ok(())
    }

    /// Get all active alerts
    #[allow(dead_code)]
    pub async fn get_active_alerts(&self) -> Vec<ActiveAlert> {
        self.active_alerts.read().await.values().cloned().collect()
    }

    /// Get alert by key
    pub async fn get_alert(&self, alert_key: &str) -> Option<ActiveAlert> {
        self.active_alerts.read().await.get(alert_key).cloned()
    }
}

impl Default for AlertingService {
    fn default() -> Self {
        Self::new()
    }
}

/// Alert definition
///
/// Represents an alert event that can be fired by the alerting system.
/// Contains information about what triggered the alert, its severity,
/// and associated metadata.
#[derive(Debug, Clone)]
pub struct Alert {
    /// Alert name
    pub name: String,
    /// Alert message
    pub message: String,
    /// Severity level
    pub severity: AlertSeverity,
    /// Source (service, host, etc.)
    pub source: String,
    /// Associated rule ID
    pub rule_id: Option<AlertRuleId>,
    /// Labels for grouping
    pub labels: HashMap<String, String>,
    /// Annotations for context
    pub annotations: HashMap<String, String>,
    /// Value that triggered the alert
    pub value: Option<f64>,
    /// Threshold that was exceeded
    pub threshold: Option<f64>,
}

impl Alert {
    /// Generate alert key for deduplication
    ///
    /// Creates a unique key for this alert based on its name and source.
    /// Used to prevent duplicate alerts from being fired.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}:{}", self.name, self.source)
    }
}

/// Alert severity levels
///
/// Defines the severity levels for alerts, from informational to critical.
/// Each level indicates the urgency of attention required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    /// Low severity (informational)
    Low,
    /// Medium severity (warning)
    Medium,
    /// High severity (error)
    High,
    /// Critical severity (requires immediate attention)
    Critical,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertSeverity::Low => write!(f, "low"),
            AlertSeverity::Medium => write!(f, "medium"),
            AlertSeverity::High => write!(f, "high"),
            AlertSeverity::Critical => write!(f, "critical"),
        }
    }
}

/// Active alert state
///
/// Represents an alert that is currently active in the system.
/// Tracks the alert details along with acknowledgment state.
#[derive(Debug, Clone)]
pub struct ActiveAlert {
    /// Alert details
    pub alert: Alert,
    /// When the alert was fired
    pub fired_at: i64,
    /// Whether the alert has been acknowledged
    pub acknowledged: bool,
    /// Who acknowledged the alert
    pub acknowledged_by: Option<String>,
    /// When the alert was acknowledged
    pub acknowledged_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fire_alert() {
        let service = AlertingService::new();

        let alert = Alert {
            name: "HighCPU".to_string(),
            message: "CPU usage exceeded 90%".to_string(),
            severity: AlertSeverity::High,
            source: "server-1".to_string(),
            rule_id: None,
            labels: HashMap::new(),
            annotations: HashMap::new(),
            value: Some(95.0),
            threshold: Some(90.0),
        };

        service.fire_alert(alert).await.unwrap();

        let active = service.get_active_alerts().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].alert.name, "HighCPU");
    }

    #[tokio::test]
    async fn test_acknowledge_alert() {
        let service = AlertingService::new();

        let alert = Alert {
            name: "HighCPU".to_string(),
            message: "CPU usage exceeded 90%".to_string(),
            severity: AlertSeverity::High,
            source: "server-1".to_string(),
            rule_id: None,
            labels: HashMap::new(),
            annotations: HashMap::new(),
            value: Some(95.0),
            threshold: Some(90.0),
        };

        service.fire_alert(alert.clone()).await.unwrap();
        service
            .acknowledge_alert(&alert.key(), "admin")
            .await
            .unwrap();

        let active = service.get_alert(&alert.key()).await.unwrap();
        assert!(active.acknowledged);
        assert_eq!(active.acknowledged_by, Some("admin".to_string()));
    }
}
