// Alert persistence layer
//
// Provides durable storage for alert rules, active alert state, and history.
// The default implementation uses JSON files with atomic writes (write-to-temp + rename).

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::history::{AlertHistoryEntry, HistoryFilter};
use super::rules::AlertRule;
use super::rules::RuleCondition;
use super::{ActiveAlert, Alert, AlertSeverity};

/// Trait for durable alert state persistence.
#[async_trait]
pub trait AlertPersistence: Send + Sync {
    /// Persist a rule definition.
    async fn save_rule(&self, rule_id: u64, rule: &AlertRule) -> Result<()>;
    /// Load all persisted rules.
    async fn load_rules(&self) -> Result<Vec<(u64, AlertRule)>>;
    /// Remove a persisted rule.
    async fn delete_rule(&self, rule_id: u64) -> Result<()>;
    /// Persist the current active alert state.
    async fn save_alert_state(&self, key: &str, alert: &ActiveAlert) -> Result<()>;
    /// Load all active alert states.
    async fn load_alert_states(&self) -> Result<Vec<(String, ActiveAlert)>>;
    /// Append a history entry.
    async fn append_history(&self, entry: &AlertHistoryEntry) -> Result<()>;
    /// Query history with filters.
    async fn query_history(&self, filter: &HistoryFilter) -> Result<Vec<AlertHistoryEntry>>;
}

// ── Serializable wrappers (AlertRule/ActiveAlert don't derive Serde) ──

#[derive(Serialize, Deserialize)]
struct SerializableRule {
    id: u64,
    name: String,
    metric_name: String,
    condition: SerializableCondition,
    duration_ns: i64,
    severity: String,
    labels: std::collections::HashMap<String, String>,
    annotations: std::collections::HashMap<String, String>,
    message_template: String,
}

#[derive(Serialize, Deserialize)]
enum SerializableCondition {
    Above(f64),
    Below(f64),
    Equal(f64),
    NotEqual(f64),
    AboveOrEqual(f64),
    BelowOrEqual(f64),
    Between(f64, f64),
    Outside(f64, f64),
    RateOfChange(f64),
    Composite {
        operator: String,
        conditions: Vec<SerializableCondition>,
    },
}

impl From<&RuleCondition> for SerializableCondition {
    fn from(c: &RuleCondition) -> Self {
        match c {
            RuleCondition::Above(v) => SerializableCondition::Above(*v),
            RuleCondition::Below(v) => SerializableCondition::Below(*v),
            RuleCondition::Equal(v) => SerializableCondition::Equal(*v),
            RuleCondition::NotEqual(v) => SerializableCondition::NotEqual(*v),
            RuleCondition::AboveOrEqual(v) => SerializableCondition::AboveOrEqual(*v),
            RuleCondition::BelowOrEqual(v) => SerializableCondition::BelowOrEqual(*v),
            RuleCondition::Between(a, b) => SerializableCondition::Between(*a, *b),
            RuleCondition::Outside(a, b) => SerializableCondition::Outside(*a, *b),
            RuleCondition::RateOfChange(v) => SerializableCondition::RateOfChange(*v),
            RuleCondition::Composite {
                operator,
                conditions,
            } => {
                let op_str = match operator {
                    super::rules::LogicalOp::And => "and",
                    super::rules::LogicalOp::Or => "or",
                    super::rules::LogicalOp::Not => "not",
                };
                SerializableCondition::Composite {
                    operator: op_str.to_string(),
                    conditions: conditions.iter().map(|c| c.into()).collect(),
                }
            }
        }
    }
}

impl TryFrom<SerializableCondition> for RuleCondition {
    type Error = anyhow::Error;
    fn try_from(c: SerializableCondition) -> Result<Self> {
        Ok(match c {
            SerializableCondition::Above(v) => RuleCondition::Above(v),
            SerializableCondition::Below(v) => RuleCondition::Below(v),
            SerializableCondition::Equal(v) => RuleCondition::Equal(v),
            SerializableCondition::NotEqual(v) => RuleCondition::NotEqual(v),
            SerializableCondition::AboveOrEqual(v) => RuleCondition::AboveOrEqual(v),
            SerializableCondition::BelowOrEqual(v) => RuleCondition::BelowOrEqual(v),
            SerializableCondition::Between(a, b) => RuleCondition::Between(a, b),
            SerializableCondition::Outside(a, b) => RuleCondition::Outside(a, b),
            SerializableCondition::RateOfChange(v) => RuleCondition::RateOfChange(v),
            SerializableCondition::Composite {
                operator,
                conditions,
            } => {
                let op = match operator.as_str() {
                    "and" => super::rules::LogicalOp::And,
                    "or" => super::rules::LogicalOp::Or,
                    "not" => super::rules::LogicalOp::Not,
                    _ => return Err(anyhow!("unknown logical operator: {}", operator)),
                };
                let conds: Result<Vec<_>> = conditions
                    .into_iter()
                    .map(RuleCondition::try_from)
                    .collect();
                RuleCondition::Composite {
                    operator: op,
                    conditions: conds?,
                }
            }
        })
    }
}

fn severity_to_str(s: &AlertSeverity) -> &'static str {
    match s {
        AlertSeverity::Low => "low",
        AlertSeverity::Medium => "medium",
        AlertSeverity::High => "high",
        AlertSeverity::Critical => "critical",
    }
}

fn severity_from_str(s: &str) -> AlertSeverity {
    match s {
        "medium" => AlertSeverity::Medium,
        "high" => AlertSeverity::High,
        "critical" => AlertSeverity::Critical,
        _ => AlertSeverity::Low,
    }
}

impl SerializableRule {
    fn from_rule(id: u64, rule: &AlertRule) -> Self {
        Self {
            id,
            name: rule.name.clone(),
            metric_name: rule.metric_name.clone(),
            condition: (&rule.condition).into(),
            duration_ns: rule.duration_ns,
            severity: severity_to_str(&rule.severity).to_string(),
            labels: rule.labels.clone(),
            annotations: rule.annotations.clone(),
            message_template: rule.message_template.clone(),
        }
    }

    fn into_rule(self) -> Result<(u64, AlertRule)> {
        Ok((
            self.id,
            AlertRule {
                name: self.name,
                metric_name: self.metric_name,
                condition: self.condition.try_into()?,
                duration_ns: self.duration_ns,
                severity: severity_from_str(&self.severity),
                labels: self.labels,
                annotations: self.annotations,
                message_template: self.message_template,
            },
        ))
    }
}

/// File-based alert persistence using JSON with atomic writes.
pub struct FileAlertPersistence {
    base_path: PathBuf,
}

impl FileAlertPersistence {
    /// Create a new file-based alert persistence layer.
    ///
    /// # Arguments
    /// * `base_path` - Directory where alert data will be persisted
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    fn rules_path(&self) -> PathBuf {
        self.base_path.join("alert_rules.json")
    }

    fn states_path(&self) -> PathBuf {
        self.base_path.join("alert_states.json")
    }

    fn history_path(&self) -> PathBuf {
        self.base_path.join("alert_history.jsonl")
    }

    /// Atomic write: serialize to temp file, then rename.
    async fn atomic_write(&self, path: &Path, data: &[u8]) -> Result<()> {
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, data).await?;
        tokio::fs::rename(&tmp, path).await?;
        Ok(())
    }
}

#[async_trait]
impl AlertPersistence for FileAlertPersistence {
    async fn save_rule(&self, rule_id: u64, rule: &AlertRule) -> Result<()> {
        tokio::fs::create_dir_all(&self.base_path).await?;
        let mut rules = self.load_rules_internal().await.unwrap_or_default();
        rules.retain(|r| r.id != rule_id);
        rules.push(SerializableRule::from_rule(rule_id, rule));
        let data = serde_json::to_vec_pretty(&rules)?;
        self.atomic_write(&self.rules_path(), &data).await
    }

    async fn load_rules(&self) -> Result<Vec<(u64, AlertRule)>> {
        let rules = self.load_rules_internal().await?;
        rules.into_iter().map(|r| r.into_rule()).collect()
    }

    async fn delete_rule(&self, rule_id: u64) -> Result<()> {
        let mut rules = self.load_rules_internal().await.unwrap_or_default();
        rules.retain(|r| r.id != rule_id);
        let data = serde_json::to_vec_pretty(&rules)?;
        self.atomic_write(&self.rules_path(), &data).await
    }

    async fn save_alert_state(&self, key: &str, alert: &ActiveAlert) -> Result<()> {
        tokio::fs::create_dir_all(&self.base_path).await?;
        let mut states = self.load_states_internal().await.unwrap_or_default();
        states.retain(|s: &SerializableAlertState| s.key != key);
        states.push(SerializableAlertState::from_active(key, alert));
        let data = serde_json::to_vec_pretty(&states)?;
        self.atomic_write(&self.states_path(), &data).await
    }

    async fn load_alert_states(&self) -> Result<Vec<(String, ActiveAlert)>> {
        let states = self.load_states_internal().await?;
        Ok(states.into_iter().map(|s| s.into_active()).collect())
    }

    async fn append_history(&self, entry: &AlertHistoryEntry) -> Result<()> {
        tokio::fs::create_dir_all(&self.base_path).await?;
        let mut line = serde_json::to_string(entry)?;
        line.push('\n');
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_path())
            .await?
            .write_all(line.as_bytes())
            .await?;
        Ok(())
    }

    async fn query_history(&self, filter: &HistoryFilter) -> Result<Vec<AlertHistoryEntry>> {
        let path = self.history_path();
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = tokio::fs::read_to_string(&path).await?;
        let mut results = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: AlertHistoryEntry = serde_json::from_str(line)?;
            if filter.matches(&entry) {
                results.push(entry);
                if results.len() >= filter.limit {
                    break;
                }
            }
        }
        Ok(results)
    }
}

use tokio::io::AsyncWriteExt;

impl FileAlertPersistence {
    async fn load_rules_internal(&self) -> Result<Vec<SerializableRule>> {
        let path = self.rules_path();
        if !path.exists() {
            return Ok(vec![]);
        }
        let data = tokio::fs::read(&path).await?;
        Ok(serde_json::from_slice(&data)?)
    }

    async fn load_states_internal(&self) -> Result<Vec<SerializableAlertState>> {
        let path = self.states_path();
        if !path.exists() {
            return Ok(vec![]);
        }
        let data = tokio::fs::read(&path).await?;
        Ok(serde_json::from_slice(&data)?)
    }
}

#[derive(Serialize, Deserialize)]
struct SerializableAlertState {
    key: String,
    name: String,
    message: String,
    severity: String,
    source: String,
    fired_at: i64,
    acknowledged: bool,
    acknowledged_by: Option<String>,
    acknowledged_at: Option<i64>,
    value: Option<f64>,
}

impl SerializableAlertState {
    fn from_active(key: &str, a: &ActiveAlert) -> Self {
        Self {
            key: key.to_string(),
            name: a.alert.name.clone(),
            message: a.alert.message.clone(),
            severity: severity_to_str(&a.alert.severity).to_string(),
            source: a.alert.source.clone(),
            fired_at: a.fired_at,
            acknowledged: a.acknowledged,
            acknowledged_by: a.acknowledged_by.clone(),
            acknowledged_at: a.acknowledged_at,
            value: a.alert.value,
        }
    }

    fn into_active(self) -> (String, ActiveAlert) {
        (
            self.key,
            ActiveAlert {
                alert: Alert {
                    name: self.name,
                    message: self.message,
                    severity: severity_from_str(&self.severity),
                    source: self.source,
                    rule_id: None,
                    labels: std::collections::HashMap::new(),
                    annotations: std::collections::HashMap::new(),
                    value: self.value,
                    threshold: None,
                },
                fired_at: self.fired_at,
                acknowledged: self.acknowledged,
                acknowledged_by: self.acknowledged_by,
                acknowledged_at: self.acknowledged_at,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::history::HistorySeverity;
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_rule_persistence_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let persistence = FileAlertPersistence::new(tmp.path());

        let rule = AlertRule {
            name: "HighCPU".to_string(),
            metric_name: "cpu_usage".to_string(),
            condition: RuleCondition::Above(90.0),
            duration_ns: 60_000_000_000,
            severity: AlertSeverity::High,
            labels: std::collections::HashMap::new(),
            annotations: std::collections::HashMap::new(),
            message_template: "CPU above 90%".to_string(),
        };

        persistence.save_rule(1, &rule).await.unwrap();

        let loaded = persistence.load_rules().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, 1);
        assert_eq!(loaded[0].1.name, "HighCPU");
        assert_eq!(loaded[0].1.metric_name, "cpu_usage");
    }

    #[tokio::test]
    async fn test_alert_state_persistence() {
        let tmp = TempDir::new().unwrap();
        let persistence = FileAlertPersistence::new(tmp.path());

        let active = ActiveAlert {
            alert: Alert {
                name: "HighMem".to_string(),
                message: "Memory high".to_string(),
                severity: AlertSeverity::Critical,
                source: "host-1".to_string(),
                rule_id: None,
                labels: std::collections::HashMap::new(),
                annotations: std::collections::HashMap::new(),
                value: Some(98.0),
                threshold: Some(95.0),
            },
            fired_at: 1234567890,
            acknowledged: true,
            acknowledged_by: Some("admin".to_string()),
            acknowledged_at: Some(1234567900),
        };

        persistence
            .save_alert_state("HighMem:host-1", &active)
            .await
            .unwrap();

        let loaded = persistence.load_alert_states().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, "HighMem:host-1");
        assert!(loaded[0].1.acknowledged);
        assert_eq!(loaded[0].1.acknowledged_by, Some("admin".to_string()));
    }

    #[tokio::test]
    async fn test_history_append_and_query() {
        let tmp = TempDir::new().unwrap();
        let persistence = FileAlertPersistence::new(tmp.path());

        let entry1 = AlertHistoryEntry {
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
        let entry2 = AlertHistoryEntry {
            alert_name: "LowDisk".to_string(),
            rule_id: 2,
            severity: HistorySeverity::Critical,
            fired_at_ns: 3000,
            resolved_at_ns: None,
            duration_ns: None,
            acknowledged: false,
            source: "server-2".to_string(),
            value: 5.0,
        };

        persistence.append_history(&entry1).await.unwrap();
        persistence.append_history(&entry2).await.unwrap();

        // Query all
        let all = persistence
            .query_history(&HistoryFilter::new())
            .await
            .unwrap();
        assert_eq!(all.len(), 2);

        // Query by severity
        let critical_only = persistence
            .query_history(&HistoryFilter {
                severity: Some(HistorySeverity::Critical),
                ..HistoryFilter::new()
            })
            .await
            .unwrap();
        assert_eq!(critical_only.len(), 1);
        assert_eq!(critical_only[0].alert_name, "LowDisk");

        // Query by rule_id
        let rule_1 = persistence
            .query_history(&HistoryFilter {
                rule_id: Some(1),
                ..HistoryFilter::new()
            })
            .await
            .unwrap();
        assert_eq!(rule_1.len(), 1);
        assert_eq!(rule_1[0].alert_name, "HighCPU");
    }

    #[tokio::test]
    async fn test_delete_rule() {
        let tmp = TempDir::new().unwrap();
        let persistence = FileAlertPersistence::new(tmp.path());

        let rule = AlertRule {
            name: "Test".to_string(),
            metric_name: "test".to_string(),
            condition: RuleCondition::Above(50.0),
            duration_ns: 0,
            severity: AlertSeverity::Low,
            labels: std::collections::HashMap::new(),
            annotations: std::collections::HashMap::new(),
            message_template: "test".to_string(),
        };

        persistence.save_rule(1, &rule).await.unwrap();
        persistence.save_rule(2, &rule).await.unwrap();
        assert_eq!(persistence.load_rules().await.unwrap().len(), 2);

        persistence.delete_rule(1).await.unwrap();
        let remaining = persistence.load_rules().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].0, 2);
    }
}
