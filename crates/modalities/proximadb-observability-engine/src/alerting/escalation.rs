// Alert escalation policies
//
// Defines staged escalation with increasing notification urgency.
// Each stage specifies a delay, notification channels, and optional repeat interval.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// An escalation policy with ordered stages.
#[derive(Debug, Clone)]
pub struct EscalationPolicy {
    pub name: String,
    pub stages: Vec<EscalationStage>,
}

/// A single escalation stage.
#[derive(Debug, Clone)]
pub struct EscalationStage {
    /// Delay from alert fire (or previous stage) before this stage activates.
    pub delay: Duration,
    /// Notification channel names to notify at this stage.
    pub channels: Vec<String>,
    /// Optional repeat interval for re-notification if still unacknowledged.
    pub repeat_interval: Option<Duration>,
}

/// Tracks escalation progress for active alerts.
pub struct EscalationTracker {
    active: HashMap<String, EscalationState>,
}

/// State for a single active escalation.
#[derive(Debug, Clone)]
pub struct EscalationState {
    /// Index into the policy's stages vec.
    pub current_stage: usize,
    /// When the alert was originally fired.
    pub started_at: Instant,
    /// When the last notification was sent for the current stage.
    pub last_notification_at: Option<Instant>,
}

/// Actions the escalation tracker may recommend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscalationAction {
    /// Notify the channels for the given stage index.
    Notify { stage_index: usize },
    /// No action needed right now.
    Wait,
}

impl EscalationTracker {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
        }
    }

    /// Start tracking escalation for an alert.
    pub fn start(&mut self, alert_key: &str) {
        self.active.insert(
            alert_key.to_string(),
            EscalationState {
                current_stage: 0,
                started_at: Instant::now(),
                last_notification_at: None,
            },
        );
    }

    /// Stop tracking (alert resolved or acknowledged).
    pub fn stop(&mut self, alert_key: &str) {
        self.active.remove(alert_key);
    }

    /// Check what action should be taken for a given alert.
    pub fn check(
        &self,
        alert_key: &str,
        policy: &EscalationPolicy,
        now: Instant,
    ) -> EscalationAction {
        let Some(state) = self.active.get(alert_key) else {
            return EscalationAction::Wait;
        };

        let elapsed = now.duration_since(state.started_at);

        // Find the highest stage whose delay has passed
        let mut target_stage = None;
        let mut cumulative_delay = Duration::ZERO;
        for (i, stage) in policy.stages.iter().enumerate() {
            cumulative_delay += stage.delay;
            if elapsed >= cumulative_delay {
                target_stage = Some(i);
            } else {
                break;
            }
        }

        let Some(stage_idx) = target_stage else {
            return EscalationAction::Wait;
        };

        // Check if we need to notify this stage
        if stage_idx > state.current_stage {
            // Advanced to a new stage — notify
            return EscalationAction::Notify {
                stage_index: stage_idx,
            };
        }

        // Same stage — check repeat interval
        if let Some(repeat) = policy.stages[stage_idx].repeat_interval {
            if let Some(last) = state.last_notification_at {
                if now.duration_since(last) >= repeat {
                    return EscalationAction::Notify {
                        stage_index: stage_idx,
                    };
                }
            } else {
                // First notification for this stage
                return EscalationAction::Notify {
                    stage_index: stage_idx,
                };
            }
        } else if state.last_notification_at.is_none() {
            // No repeat, but haven't notified yet
            return EscalationAction::Notify {
                stage_index: stage_idx,
            };
        }

        EscalationAction::Wait
    }

    /// Record that a notification was sent for the given alert at the given stage.
    pub fn record_notification(&mut self, alert_key: &str, stage_index: usize) {
        if let Some(state) = self.active.get_mut(alert_key) {
            state.current_stage = stage_index;
            state.last_notification_at = Some(Instant::now());
        }
    }

    /// Number of actively tracked escalations.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
}

impl Default for EscalationTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy() -> EscalationPolicy {
        EscalationPolicy {
            name: "default".to_string(),
            stages: vec![
                EscalationStage {
                    delay: Duration::from_secs(0),
                    channels: vec!["slack".to_string()],
                    repeat_interval: None,
                },
                EscalationStage {
                    delay: Duration::from_secs(300),
                    channels: vec!["pagerduty".to_string()],
                    repeat_interval: Some(Duration::from_secs(600)),
                },
                EscalationStage {
                    delay: Duration::from_secs(900),
                    channels: vec!["phone".to_string()],
                    repeat_interval: Some(Duration::from_secs(300)),
                },
            ],
        }
    }

    #[test]
    fn test_escalation_immediate_notify() {
        let mut tracker = EscalationTracker::new();
        let policy = test_policy();

        tracker.start("alert-1");
        let now = Instant::now();

        let action = tracker.check("alert-1", &policy, now);
        assert_eq!(action, EscalationAction::Notify { stage_index: 0 });
    }

    #[test]
    fn test_escalation_stage_progression() {
        let mut tracker = EscalationTracker::new();
        let policy = test_policy();
        let start = Instant::now();

        tracker.start("alert-1");
        // Override started_at to control timing
        tracker.active.get_mut("alert-1").unwrap().started_at = start;

        // Notify stage 0
        tracker.record_notification("alert-1", 0);

        // After 5 minutes, should advance to stage 1
        let after_5m = start + Duration::from_secs(301);
        let action = tracker.check("alert-1", &policy, after_5m);
        assert_eq!(action, EscalationAction::Notify { stage_index: 1 });
    }

    #[test]
    fn test_escalation_stop() {
        let mut tracker = EscalationTracker::new();
        tracker.start("alert-1");
        assert_eq!(tracker.active_count(), 1);

        tracker.stop("alert-1");
        assert_eq!(tracker.active_count(), 0);

        let policy = test_policy();
        let action = tracker.check("alert-1", &policy, Instant::now());
        assert_eq!(action, EscalationAction::Wait);
    }

    #[test]
    fn test_escalation_no_repeat_waits() {
        let mut tracker = EscalationTracker::new();
        let policy = test_policy();

        tracker.start("alert-1");
        tracker.record_notification("alert-1", 0);

        // Stage 0 has no repeat — should wait
        let action = tracker.check("alert-1", &policy, Instant::now());
        assert_eq!(action, EscalationAction::Wait);
    }
}
