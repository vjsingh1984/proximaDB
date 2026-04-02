// Alert history tracking
//
// Records all alert lifecycle events (fired, resolved, acknowledged)
// for audit trail and operational analytics.

use super::AlertSeverity;
use serde::{Deserialize, Serialize};

/// A single alert history entry recording a lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertHistoryEntry {
    /// Alert name
    pub alert_name: String,
    /// Associated rule ID
    pub rule_id: u64,
    /// Alert severity at time of firing
    pub severity: HistorySeverity,
    /// When the alert was fired (nanoseconds since epoch)
    pub fired_at_ns: i64,
    /// When the alert was resolved (None if still active)
    pub resolved_at_ns: Option<i64>,
    /// Duration in nanoseconds (computed from fired_at to resolved_at)
    pub duration_ns: Option<i64>,
    /// Whether the alert was acknowledged
    pub acknowledged: bool,
    /// Source that triggered the alert
    pub source: String,
    /// Metric value that triggered the alert
    pub value: f64,
}

/// Serializable severity for history persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistorySeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl From<AlertSeverity> for HistorySeverity {
    fn from(s: AlertSeverity) -> Self {
        match s {
            AlertSeverity::Low => HistorySeverity::Low,
            AlertSeverity::Medium => HistorySeverity::Medium,
            AlertSeverity::High => HistorySeverity::High,
            AlertSeverity::Critical => HistorySeverity::Critical,
        }
    }
}

impl From<HistorySeverity> for AlertSeverity {
    fn from(s: HistorySeverity) -> Self {
        match s {
            HistorySeverity::Low => AlertSeverity::Low,
            HistorySeverity::Medium => AlertSeverity::Medium,
            HistorySeverity::High => AlertSeverity::High,
            HistorySeverity::Critical => AlertSeverity::Critical,
        }
    }
}

/// Filter for querying alert history.
#[derive(Debug, Clone, Default)]
pub struct HistoryFilter {
    pub start_time_ns: Option<i64>,
    pub end_time_ns: Option<i64>,
    pub severity: Option<HistorySeverity>,
    pub rule_id: Option<u64>,
    pub limit: usize,
}

impl HistoryFilter {
    pub fn new() -> Self {
        Self {
            limit: 100,
            ..Default::default()
        }
    }

    /// Check if a history entry matches this filter.
    pub fn matches(&self, entry: &AlertHistoryEntry) -> bool {
        if let Some(start) = self.start_time_ns
            && entry.fired_at_ns < start
        {
            return false;
        }
        if let Some(end) = self.end_time_ns
            && entry.fired_at_ns > end
        {
            return false;
        }
        if let Some(ref sev) = self.severity
            && entry.severity != *sev
        {
            return false;
        }
        if let Some(rid) = self.rule_id
            && entry.rule_id != rid
        {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_filter_matches_all() {
        let entry = AlertHistoryEntry {
            alert_name: "HighCPU".to_string(),
            rule_id: 1,
            severity: HistorySeverity::High,
            fired_at_ns: 1000,
            resolved_at_ns: Some(2000),
            duration_ns: Some(1000),
            acknowledged: true,
            source: "server-1".to_string(),
            value: 95.0,
        };

        let filter = HistoryFilter::new();
        assert!(filter.matches(&entry));
    }

    #[test]
    fn test_history_filter_time_range() {
        let entry = AlertHistoryEntry {
            alert_name: "HighCPU".to_string(),
            rule_id: 1,
            severity: HistorySeverity::High,
            fired_at_ns: 1000,
            resolved_at_ns: None,
            duration_ns: None,
            acknowledged: false,
            source: "server-1".to_string(),
            value: 95.0,
        };

        let filter = HistoryFilter {
            start_time_ns: Some(500),
            end_time_ns: Some(1500),
            ..HistoryFilter::new()
        };
        assert!(filter.matches(&entry));

        let filter_before = HistoryFilter {
            start_time_ns: Some(2000),
            ..HistoryFilter::new()
        };
        assert!(!filter_before.matches(&entry));
    }

    #[test]
    fn test_history_filter_severity() {
        let entry = AlertHistoryEntry {
            alert_name: "HighCPU".to_string(),
            rule_id: 1,
            severity: HistorySeverity::Critical,
            fired_at_ns: 1000,
            resolved_at_ns: None,
            duration_ns: None,
            acknowledged: false,
            source: "server-1".to_string(),
            value: 99.0,
        };

        let filter = HistoryFilter {
            severity: Some(HistorySeverity::Critical),
            ..HistoryFilter::new()
        };
        assert!(filter.matches(&entry));

        let filter_wrong = HistoryFilter {
            severity: Some(HistorySeverity::Low),
            ..HistoryFilter::new()
        };
        assert!(!filter_wrong.matches(&entry));
    }

    #[test]
    fn test_severity_roundtrip() {
        let severities = [
            AlertSeverity::Low,
            AlertSeverity::Medium,
            AlertSeverity::High,
            AlertSeverity::Critical,
        ];
        for s in &severities {
            let h: HistorySeverity = (*s).into();
            let back: AlertSeverity = h.into();
            assert_eq!(*s, back);
        }
    }
}
