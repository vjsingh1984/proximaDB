// Alert evaluation engine
//
// Provides:
// - Rule evaluation
// - Threshold detection
// - Anomaly detection
// - Alert state management

use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::info;

use super::Alert;
use super::rules::{AlertRule, AlertRuleId, RuleCondition};

/// Alert evaluation engine
pub struct AlertEngine {
    /// Registered rules
    rules: RwLock<HashMap<AlertRuleId, AlertRule>>,
    /// Rule evaluation state
    state: RwLock<HashMap<AlertRuleId, RuleState>>,
    /// Next rule ID
    next_id: RwLock<u64>,
}

/// State for a single rule
struct RuleState {
    /// Last evaluation time
    last_eval_ns: i64,
    /// Current evaluation window samples
    samples: Vec<f64>,
    /// Whether the rule is currently firing
    firing: bool,
    /// When the rule started firing
    firing_since: Option<i64>,
}

impl AlertEngine {
    /// Create a new alert engine
    pub fn new() -> Self {
        Self {
            rules: RwLock::new(HashMap::new()),
            state: RwLock::new(HashMap::new()),
            next_id: RwLock::new(1),
        }
    }

    /// Register a new alert rule
    pub async fn register_rule(&self, rule: AlertRule) -> Result<AlertRuleId> {
        let mut next_id = self.next_id.write().await;
        let id = AlertRuleId(*next_id);
        *next_id += 1;

        let mut rules = self.rules.write().await;
        rules.insert(id.clone(), rule);

        let mut state = self.state.write().await;
        state.insert(
            id.clone(),
            RuleState {
                last_eval_ns: 0,
                samples: Vec::new(),
                firing: false,
                firing_since: None,
            },
        );

        info!("Registered alert rule: {:?}", id);

        Ok(id)
    }

    /// Unregister an alert rule
    pub async fn unregister_rule(&self, rule_id: &AlertRuleId) -> Result<()> {
        let mut rules = self.rules.write().await;
        rules.remove(rule_id);

        let mut state = self.state.write().await;
        state.remove(rule_id);

        info!("Unregistered alert rule: {:?}", rule_id);

        Ok(())
    }

    /// Get a rule by ID
    pub async fn get_rule(&self, rule_id: &AlertRuleId) -> Option<AlertRule> {
        self.rules.read().await.get(rule_id).cloned()
    }

    /// Evaluate all rules
    pub async fn evaluate_all(&self, metrics: &HashMap<String, f64>) -> Vec<Alert> {
        let rules = self.rules.read().await;
        let mut alerts = Vec::new();

        for (rule_id, rule) in rules.iter() {
            if let Some(alert) = self.evaluate_rule(rule_id, rule, metrics).await {
                alerts.push(alert);
            }
        }

        alerts
    }

    /// Evaluate a single rule
    async fn evaluate_rule(
        &self,
        rule_id: &AlertRuleId,
        rule: &AlertRule,
        metrics: &HashMap<String, f64>,
    ) -> Option<Alert> {
        let value = metrics.get(&rule.metric_name)?;

        let triggered = match &rule.condition {
            RuleCondition::Above(threshold) => *value > *threshold,
            RuleCondition::Below(threshold) => *value < *threshold,
            RuleCondition::Equal(threshold) => (*value - *threshold).abs() < f64::EPSILON,
            RuleCondition::NotEqual(threshold) => (*value - *threshold).abs() >= f64::EPSILON,
            RuleCondition::AboveOrEqual(threshold) => *value >= *threshold,
            RuleCondition::BelowOrEqual(threshold) => *value <= *threshold,
            RuleCondition::Between(low, high) => *value >= *low && *value <= *high,
            RuleCondition::Outside(low, high) => *value < *low || *value > *high,
            RuleCondition::RateOfChange(rate) => {
                // Need historical data for rate of change
                self.check_rate_of_change(rule_id, *value, *rate).await
            }
            RuleCondition::Composite { .. } => {
                // Composite conditions require recursive evaluation - not yet implemented
                false
            }
        };

        // Update state
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        {
            let mut state = self.state.write().await;
            if let Some(rule_state) = state.get_mut(rule_id) {
                rule_state.last_eval_ns = now;
                rule_state.samples.push(*value);

                // Keep only last 100 samples
                if rule_state.samples.len() > 100 {
                    rule_state.samples.remove(0);
                }

                if triggered && !rule_state.firing {
                    rule_state.firing = true;
                    rule_state.firing_since = Some(now);
                } else if !triggered && rule_state.firing {
                    rule_state.firing = false;
                    rule_state.firing_since = None;
                }
            }
        }

        if triggered {
            let threshold = match &rule.condition {
                RuleCondition::Above(t)
                | RuleCondition::Below(t)
                | RuleCondition::Equal(t)
                | RuleCondition::NotEqual(t)
                | RuleCondition::AboveOrEqual(t)
                | RuleCondition::BelowOrEqual(t) => Some(*t),
                RuleCondition::Between(low, _) => Some(*low),
                RuleCondition::Outside(_, high) => Some(*high),
                RuleCondition::RateOfChange(rate) => Some(*rate),
                RuleCondition::Composite { .. } => None, // No single threshold for composite conditions
            };

            Some(Alert {
                name: rule.name.clone(),
                message: self.format_message(rule, *value),
                severity: rule.severity,
                source: rule.labels.get("source").cloned().unwrap_or_default(),
                rule_id: Some(rule_id.clone()),
                labels: rule.labels.clone(),
                annotations: rule.annotations.clone(),
                value: Some(*value),
                threshold,
            })
        } else {
            None
        }
    }

    /// Check rate of change
    async fn check_rate_of_change(&self, rule_id: &AlertRuleId, value: f64, rate: f64) -> bool {
        let state = self.state.read().await;
        if let Some(rule_state) = state.get(rule_id)
            && rule_state.samples.len() >= 2
        {
            let prev = rule_state.samples[rule_state.samples.len() - 1];
            let change_rate = (value - prev).abs();
            return change_rate > rate;
        }
        false
    }

    /// Format alert message
    fn format_message(&self, rule: &AlertRule, value: f64) -> String {
        let mut message = rule.message_template.clone();
        message = message.replace("{{value}}", &format!("{:.2}", value));
        message = message.replace("{{metric}}", &rule.metric_name);
        message
    }
}

impl AlertEngine {
    /// Register a set of standard production rules for ProximaDB monitoring
    ///
    /// These rules cover the most common operational concerns:
    /// - High CPU / memory usage
    /// - Query latency degradation
    /// - Error rate spikes
    /// - Storage capacity warnings
    /// - Search throughput drops
    pub async fn register_production_rules(&self) -> Result<Vec<AlertRuleId>> {
        let rules = vec![
            AlertRule {
                name: "HighCPU".to_string(),
                metric_name: "cpu_usage_percent".to_string(),
                condition: RuleCondition::Above(90.0),
                duration_ns: 300_000_000_000, // 5 minutes
                severity: super::AlertSeverity::High,
                labels: HashMap::from([("source".to_string(), "system".to_string())]),
                annotations: HashMap::from([(
                    "runbook".to_string(),
                    "Check for runaway queries or compaction storms".to_string(),
                )]),
                message_template: "CPU usage at {{value}}% exceeds 90% threshold".to_string(),
            },
            AlertRule {
                name: "HighMemory".to_string(),
                metric_name: "memory_usage_percent".to_string(),
                condition: RuleCondition::Above(85.0),
                duration_ns: 300_000_000_000,
                severity: super::AlertSeverity::High,
                labels: HashMap::from([("source".to_string(), "system".to_string())]),
                annotations: HashMap::new(),
                message_template: "Memory usage at {{value}}% exceeds 85% threshold".to_string(),
            },
            AlertRule {
                name: "HighQueryLatency".to_string(),
                metric_name: "query_latency_p99_ms".to_string(),
                condition: RuleCondition::Above(500.0),
                duration_ns: 120_000_000_000, // 2 minutes
                severity: super::AlertSeverity::High,
                labels: HashMap::from([("source".to_string(), "query".to_string())]),
                annotations: HashMap::new(),
                message_template: "Query p99 latency at {{value}}ms exceeds 500ms SLO".to_string(),
            },
            AlertRule {
                name: "HighErrorRate".to_string(),
                metric_name: "error_rate_percent".to_string(),
                condition: RuleCondition::Above(5.0),
                duration_ns: 60_000_000_000, // 1 minute
                severity: super::AlertSeverity::Critical,
                labels: HashMap::from([("source".to_string(), "api".to_string())]),
                annotations: HashMap::new(),
                message_template: "Error rate at {{value}}% exceeds 5% threshold".to_string(),
            },
            AlertRule {
                name: "StorageNearFull".to_string(),
                metric_name: "disk_usage_percent".to_string(),
                condition: RuleCondition::Above(90.0),
                duration_ns: 600_000_000_000, // 10 minutes
                severity: super::AlertSeverity::Medium,
                labels: HashMap::from([("source".to_string(), "storage".to_string())]),
                annotations: HashMap::from([(
                    "runbook".to_string(),
                    "Run compaction or expand storage".to_string(),
                )]),
                message_template: "Disk usage at {{value}}% — approaching capacity".to_string(),
            },
            AlertRule {
                name: "LowSearchThroughput".to_string(),
                metric_name: "search_qps".to_string(),
                condition: RuleCondition::Below(10.0),
                duration_ns: 300_000_000_000,
                severity: super::AlertSeverity::Medium,
                labels: HashMap::from([("source".to_string(), "query".to_string())]),
                annotations: HashMap::new(),
                message_template: "Search throughput dropped to {{value}} QPS".to_string(),
            },
        ];

        let mut ids = Vec::new();
        for rule in rules {
            ids.push(self.register_rule(rule).await?);
        }
        info!("Registered {} production alert rules", ids.len());
        Ok(ids)
    }

    /// Evaluate all rules and dispatch alerts to a notification callback
    ///
    /// The callback receives each fired alert and can route it to Slack,
    /// email, webhook, PagerDuty, etc.
    pub async fn evaluate_and_notify<F>(
        &self,
        metrics: &HashMap<String, f64>,
        notify: F,
    ) -> Vec<Alert>
    where
        F: Fn(&Alert),
    {
        let alerts = self.evaluate_all(metrics).await;
        for alert in &alerts {
            notify(alert);
        }
        alerts
    }

    /// Get all currently firing rules (rules in firing state)
    pub async fn firing_rules(&self) -> Vec<AlertRuleId> {
        let state = self.state.read().await;
        state
            .iter()
            .filter(|(_, s)| s.firing)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

impl Default for AlertEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::alerting::AlertSeverity;
    use crate::observability::alerting::rules::AlertRule;

    #[tokio::test]
    async fn test_register_rule() {
        let engine = AlertEngine::new();

        let rule = AlertRule {
            name: "HighCPU".to_string(),
            metric_name: "cpu_usage".to_string(),
            condition: RuleCondition::Above(90.0),
            duration_ns: 60_000_000_000,
            severity: AlertSeverity::High,
            labels: HashMap::new(),
            annotations: HashMap::new(),
            message_template: "CPU usage is {{value}}%".to_string(),
        };

        let id = engine.register_rule(rule).await.unwrap();
        assert!(engine.get_rule(&id).await.is_some());
    }

    #[tokio::test]
    async fn test_production_rules_registration() {
        let engine = AlertEngine::new();
        let ids = engine.register_production_rules().await.unwrap();
        assert_eq!(ids.len(), 6);

        // Verify all rules are retrievable
        for id in &ids {
            assert!(engine.get_rule(id).await.is_some());
        }
    }

    #[tokio::test]
    async fn test_production_rules_fire_on_high_cpu() {
        let engine = AlertEngine::new();
        engine.register_production_rules().await.unwrap();

        let metrics = HashMap::from([("cpu_usage_percent".to_string(), 95.0)]);
        let alerts = engine.evaluate_all(&metrics).await;

        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].name, "HighCPU");
    }

    #[tokio::test]
    async fn test_evaluate_and_notify() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let engine = AlertEngine::new();
        engine.register_production_rules().await.unwrap();

        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();

        let metrics = HashMap::from([
            ("cpu_usage_percent".to_string(), 95.0),
            ("error_rate_percent".to_string(), 10.0),
        ]);

        let alerts = engine
            .evaluate_and_notify(&metrics, |_alert| {
                count_clone.fetch_add(1, Ordering::Relaxed);
            })
            .await;

        assert_eq!(alerts.len(), 2);
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_firing_rules() {
        let engine = AlertEngine::new();
        engine.register_production_rules().await.unwrap();

        // Initially no rules firing
        assert!(engine.firing_rules().await.is_empty());

        // Trigger a rule
        let metrics = HashMap::from([("cpu_usage_percent".to_string(), 95.0)]);
        engine.evaluate_all(&metrics).await;

        let firing = engine.firing_rules().await;
        assert_eq!(firing.len(), 1);
    }

    #[tokio::test]
    async fn test_evaluate_above() {
        let engine = AlertEngine::new();

        let rule = AlertRule {
            name: "HighCPU".to_string(),
            metric_name: "cpu_usage".to_string(),
            condition: RuleCondition::Above(90.0),
            duration_ns: 0,
            severity: AlertSeverity::High,
            labels: HashMap::new(),
            annotations: HashMap::new(),
            message_template: "CPU usage is {{value}}%".to_string(),
        };

        engine.register_rule(rule).await.unwrap();

        // Below threshold
        let metrics = HashMap::from([("cpu_usage".to_string(), 80.0)]);
        let alerts = engine.evaluate_all(&metrics).await;
        assert!(alerts.is_empty());

        // Above threshold
        let metrics = HashMap::from([("cpu_usage".to_string(), 95.0)]);
        let alerts = engine.evaluate_all(&metrics).await;
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].name, "HighCPU");
    }
}
