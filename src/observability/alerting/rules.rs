// Alert rule definitions
//
// Provides:
// - Rule definition structures
// - Condition types
// - Rule templates

use std::collections::HashMap;

use super::AlertSeverity;

/// Alert rule identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlertRuleId(pub u64);

/// Alert rule definition
#[derive(Debug, Clone)]
pub struct AlertRule {
    /// Rule name
    pub name: String,
    /// Metric name to monitor
    pub metric_name: String,
    /// Condition to trigger alert
    pub condition: RuleCondition,
    /// Duration the condition must hold (nanoseconds)
    pub duration_ns: i64,
    /// Alert severity
    pub severity: AlertSeverity,
    /// Labels for grouping
    pub labels: HashMap<String, String>,
    /// Annotations for context
    pub annotations: HashMap<String, String>,
    /// Message template
    pub message_template: String,
}

impl AlertRule {
    /// Create a new alert rule builder
    pub fn builder(name: &str) -> AlertRuleBuilder {
        AlertRuleBuilder::new(name)
    }
}

/// Builder for alert rules
pub struct AlertRuleBuilder {
    name: String,
    metric_name: Option<String>,
    condition: Option<RuleCondition>,
    duration_ns: i64,
    severity: AlertSeverity,
    labels: HashMap<String, String>,
    annotations: HashMap<String, String>,
    message_template: String,
}

impl AlertRuleBuilder {
    /// Create a new builder
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            metric_name: None,
            condition: None,
            duration_ns: 0,
            severity: AlertSeverity::Medium,
            labels: HashMap::new(),
            annotations: HashMap::new(),
            message_template: "Alert: {{metric}} = {{value}}".to_string(),
        }
    }

    /// Set metric name
    pub fn metric(mut self, name: &str) -> Self {
        self.metric_name = Some(name.to_string());
        self
    }

    /// Set condition
    pub fn condition(mut self, condition: RuleCondition) -> Self {
        self.condition = Some(condition);
        self
    }

    /// Set duration
    pub fn duration_ns(mut self, duration: i64) -> Self {
        self.duration_ns = duration;
        self
    }

    /// Set severity
    pub fn severity(mut self, severity: AlertSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Add label
    pub fn label(mut self, key: &str, value: &str) -> Self {
        self.labels.insert(key.to_string(), value.to_string());
        self
    }

    /// Add annotation
    pub fn annotation(mut self, key: &str, value: &str) -> Self {
        self.annotations.insert(key.to_string(), value.to_string());
        self
    }

    /// Set message template
    pub fn message(mut self, template: &str) -> Self {
        self.message_template = template.to_string();
        self
    }

    /// Build the rule
    pub fn build(self) -> Option<AlertRule> {
        Some(AlertRule {
            name: self.name,
            metric_name: self.metric_name?,
            condition: self.condition?,
            duration_ns: self.duration_ns,
            severity: self.severity,
            labels: self.labels,
            annotations: self.annotations,
            message_template: self.message_template,
        })
    }
}

/// Rule condition types
#[derive(Debug, Clone, PartialEq)]
pub enum RuleCondition {
    /// Value above threshold
    Above(f64),
    /// Value below threshold
    Below(f64),
    /// Value equals threshold
    Equal(f64),
    /// Value not equal to threshold
    NotEqual(f64),
    /// Value above or equal to threshold
    AboveOrEqual(f64),
    /// Value below or equal to threshold
    BelowOrEqual(f64),
    /// Value between two thresholds
    Between(f64, f64),
    /// Value outside two thresholds
    Outside(f64, f64),
    /// Rate of change exceeds threshold
    RateOfChange(f64),
    /// Composite condition combining multiple sub-conditions
    Composite {
        /// Logical operator to combine sub-conditions
        operator: LogicalOp,
        /// Sub-conditions to evaluate
        conditions: Vec<RuleCondition>,
    },
}

/// Logical operators for composite conditions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalOp {
    /// All sub-conditions must match.
    And,
    /// At least one sub-condition must match.
    Or,
    /// Negates the first sub-condition.
    Not,
}

impl RuleCondition {
    /// Check if a value matches this condition
    pub fn matches(&self, value: f64) -> bool {
        match self {
            RuleCondition::Above(threshold) => value > *threshold,
            RuleCondition::Below(threshold) => value < *threshold,
            RuleCondition::Equal(threshold) => (value - threshold).abs() < f64::EPSILON,
            RuleCondition::NotEqual(threshold) => (value - threshold).abs() >= f64::EPSILON,
            RuleCondition::AboveOrEqual(threshold) => value >= *threshold,
            RuleCondition::BelowOrEqual(threshold) => value <= *threshold,
            RuleCondition::Between(low, high) => value >= *low && value <= *high,
            RuleCondition::Outside(low, high) => value < *low || value > *high,
            RuleCondition::RateOfChange(_) => false, // Requires historical data
            RuleCondition::Composite { operator, conditions } => match operator {
                LogicalOp::And => conditions.iter().all(|c| c.matches(value)),
                LogicalOp::Or => conditions.iter().any(|c| c.matches(value)),
                LogicalOp::Not => {
                    conditions.first().map_or(true, |c| !c.matches(value))
                }
            },
        }
    }

    /// Get the threshold value(s) for display
    pub fn threshold_description(&self) -> String {
        match self {
            RuleCondition::Above(t) => format!("> {}", t),
            RuleCondition::Below(t) => format!("< {}", t),
            RuleCondition::Equal(t) => format!("= {}", t),
            RuleCondition::NotEqual(t) => format!("!= {}", t),
            RuleCondition::AboveOrEqual(t) => format!(">= {}", t),
            RuleCondition::BelowOrEqual(t) => format!("<= {}", t),
            RuleCondition::Between(low, high) => format!("between {} and {}", low, high),
            RuleCondition::Outside(low, high) => format!("outside {} - {}", low, high),
            RuleCondition::RateOfChange(rate) => format!("rate of change > {}", rate),
            RuleCondition::Composite { operator, conditions } => {
                let op_str = match operator {
                    LogicalOp::And => "AND",
                    LogicalOp::Or => "OR",
                    LogicalOp::Not => "NOT",
                };
                let parts: Vec<_> = conditions.iter().map(|c| c.threshold_description()).collect();
                format!("({} {})", op_str, parts.join(", "))
            }
        }
    }
}

/// Predefined rule templates
pub struct RuleTemplates;

impl RuleTemplates {
    /// High CPU usage rule
    pub fn high_cpu(threshold: f64) -> AlertRule {
        AlertRule::builder("HighCPU")
            .metric("cpu_usage")
            .condition(RuleCondition::Above(threshold))
            .severity(AlertSeverity::High)
            .message(
                "CPU usage is {{value}}% (threshold: {})"
                    .replace("{}", &threshold.to_string())
                    .as_str(),
            )
            .build()
            .unwrap_or_else(|| AlertRule {
                name: "HighCPU".to_string(),
                metric_name: "cpu_usage".to_string(),
                condition: RuleCondition::Above(threshold),
                duration_ns: 0,
                severity: AlertSeverity::High,
                labels: HashMap::new(),
                annotations: HashMap::new(),
                message_template: format!("CPU usage is {{value}}% (threshold: {})", threshold),
            })
    }

    /// Low memory rule
    pub fn low_memory(threshold: f64) -> AlertRule {
        AlertRule::builder("LowMemory")
            .metric("memory_available_bytes")
            .condition(RuleCondition::Below(threshold))
            .severity(AlertSeverity::Critical)
            .message(
                "Available memory is {{value}} bytes (threshold: {})"
                    .replace("{}", &threshold.to_string())
                    .as_str(),
            )
            .build()
            .unwrap_or_else(|| AlertRule {
                name: "LowMemory".to_string(),
                metric_name: "memory_available_bytes".to_string(),
                condition: RuleCondition::Below(threshold),
                duration_ns: 0,
                severity: AlertSeverity::Critical,
                labels: HashMap::new(),
                annotations: HashMap::new(),
                message_template: format!(
                    "Available memory is {{value}} bytes (threshold: {})",
                    threshold
                ),
            })
    }

    /// High error rate rule
    pub fn high_error_rate(threshold: f64) -> AlertRule {
        AlertRule::builder("HighErrorRate")
            .metric("error_rate")
            .condition(RuleCondition::Above(threshold))
            .severity(AlertSeverity::High)
            .message(
                "Error rate is {{value}}% (threshold: {})"
                    .replace("{}", &threshold.to_string())
                    .as_str(),
            )
            .build()
            .unwrap_or_else(|| AlertRule {
                name: "HighErrorRate".to_string(),
                metric_name: "error_rate".to_string(),
                condition: RuleCondition::Above(threshold),
                duration_ns: 0,
                severity: AlertSeverity::High,
                labels: HashMap::new(),
                annotations: HashMap::new(),
                message_template: format!("Error rate is {{value}}% (threshold: {})", threshold),
            })
    }

    /// High latency rule
    pub fn high_latency(threshold_ms: f64) -> AlertRule {
        AlertRule::builder("HighLatency")
            .metric("request_latency_ms")
            .condition(RuleCondition::Above(threshold_ms))
            .severity(AlertSeverity::Medium)
            .message(
                "Request latency is {{value}}ms (threshold: {}ms)"
                    .replace("{}", &threshold_ms.to_string())
                    .as_str(),
            )
            .build()
            .unwrap_or_else(|| AlertRule {
                name: "HighLatency".to_string(),
                metric_name: "request_latency_ms".to_string(),
                condition: RuleCondition::Above(threshold_ms),
                duration_ns: 0,
                severity: AlertSeverity::Medium,
                labels: HashMap::new(),
                annotations: HashMap::new(),
                message_template: format!(
                    "Request latency is {{value}}ms (threshold: {}ms)",
                    threshold_ms
                ),
            })
    }

    /// Disk usage rule
    pub fn high_disk_usage(threshold: f64) -> AlertRule {
        AlertRule::builder("HighDiskUsage")
            .metric("disk_usage_percent")
            .condition(RuleCondition::Above(threshold))
            .severity(AlertSeverity::Medium)
            .message(
                "Disk usage is {{value}}% (threshold: {})"
                    .replace("{}", &threshold.to_string())
                    .as_str(),
            )
            .build()
            .unwrap_or_else(|| AlertRule {
                name: "HighDiskUsage".to_string(),
                metric_name: "disk_usage_percent".to_string(),
                condition: RuleCondition::Above(threshold),
                duration_ns: 0,
                severity: AlertSeverity::Medium,
                labels: HashMap::new(),
                annotations: HashMap::new(),
                message_template: format!("Disk usage is {{value}}% (threshold: {})", threshold),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_condition_matches() {
        assert!(RuleCondition::Above(90.0).matches(95.0));
        assert!(!RuleCondition::Above(90.0).matches(85.0));

        assert!(RuleCondition::Below(50.0).matches(45.0));
        assert!(!RuleCondition::Below(50.0).matches(55.0));

        assert!(RuleCondition::Between(10.0, 20.0).matches(15.0));
        assert!(!RuleCondition::Between(10.0, 20.0).matches(25.0));

        assert!(RuleCondition::Outside(10.0, 20.0).matches(5.0));
        assert!(RuleCondition::Outside(10.0, 20.0).matches(25.0));
        assert!(!RuleCondition::Outside(10.0, 20.0).matches(15.0));
    }

    #[test]
    fn test_rule_builder() {
        let rule = AlertRule::builder("TestRule")
            .metric("test_metric")
            .condition(RuleCondition::Above(50.0))
            .severity(AlertSeverity::High)
            .label("env", "prod")
            .message("Test alert: {{value}}")
            .build()
            .unwrap();

        assert_eq!(rule.name, "TestRule");
        assert_eq!(rule.metric_name, "test_metric");
        assert_eq!(rule.severity, AlertSeverity::High);
        assert_eq!(rule.labels.get("env"), Some(&"prod".to_string()));
    }

    #[test]
    fn test_rule_templates() {
        let cpu_rule = RuleTemplates::high_cpu(90.0);
        assert_eq!(cpu_rule.name, "HighCPU");
        assert!(matches!(cpu_rule.condition, RuleCondition::Above(90.0)));

        let memory_rule = RuleTemplates::low_memory(1_000_000.0);
        assert_eq!(memory_rule.name, "LowMemory");
        assert_eq!(memory_rule.severity, AlertSeverity::Critical);
    }

    #[test]
    fn test_composite_condition_and() {
        let cond = RuleCondition::Composite {
            operator: LogicalOp::And,
            conditions: vec![RuleCondition::Above(80.0), RuleCondition::Below(100.0)],
        };
        assert!(cond.matches(90.0)); // 90 > 80 AND 90 < 100
        assert!(!cond.matches(75.0)); // 75 > 80 is false
        assert!(!cond.matches(105.0)); // 105 < 100 is false
    }

    #[test]
    fn test_composite_condition_or() {
        let cond = RuleCondition::Composite {
            operator: LogicalOp::Or,
            conditions: vec![RuleCondition::Above(95.0), RuleCondition::Below(5.0)],
        };
        assert!(cond.matches(99.0)); // 99 > 95
        assert!(cond.matches(2.0)); // 2 < 5
        assert!(!cond.matches(50.0)); // neither
    }

    #[test]
    fn test_composite_condition_not() {
        let cond = RuleCondition::Composite {
            operator: LogicalOp::Not,
            conditions: vec![RuleCondition::Above(90.0)],
        };
        assert!(cond.matches(85.0)); // NOT(85 > 90) = NOT(false) = true
        assert!(!cond.matches(95.0)); // NOT(95 > 90) = NOT(true) = false
    }

    #[test]
    fn test_composite_nested() {
        // (Above(80) AND Below(100)) OR Equal(50)
        let cond = RuleCondition::Composite {
            operator: LogicalOp::Or,
            conditions: vec![
                RuleCondition::Composite {
                    operator: LogicalOp::And,
                    conditions: vec![RuleCondition::Above(80.0), RuleCondition::Below(100.0)],
                },
                RuleCondition::Equal(50.0),
            ],
        };
        assert!(cond.matches(90.0)); // in range 80-100
        assert!(cond.matches(50.0)); // equals 50
        assert!(!cond.matches(70.0)); // neither
    }
}
