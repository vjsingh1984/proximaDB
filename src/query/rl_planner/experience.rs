//! Experience Replay Buffer
//!
//! Stores (state, action, reward) tuples for batch learning and analysis.
//! Uses a circular buffer to maintain fixed memory usage.

use std::collections::VecDeque;

use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use super::action::ExecutionAction;
use super::state::PlannerState;

/// Single experience tuple
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    /// State at decision time
    pub state: PlannerState,
    /// Action taken
    pub action: ExecutionAction,
    /// Observed reward
    pub reward: f32,
    /// Timestamp (epoch seconds)
    pub timestamp: u64,
}

impl Experience {
    /// Create new experience
    pub fn new(state: PlannerState, action: ExecutionAction, reward: f32) -> Self {
        Self {
            state,
            action,
            reward,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}

/// Circular buffer for experience replay
#[derive(Debug, Clone)]
pub struct ExperienceBuffer {
    /// Experiences in order of arrival
    buffer: VecDeque<Experience>,
    /// Maximum buffer size
    max_size: usize,
    /// Total experiences added (including evicted)
    total_added: u64,
}

impl ExperienceBuffer {
    /// Create new experience buffer with maximum size
    pub fn new(max_size: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(max_size.min(10_000)),
            max_size,
            total_added: 0,
        }
    }

    /// Add new experience to buffer
    pub fn add(&mut self, state: PlannerState, action: ExecutionAction, reward: f32) {
        let experience = Experience::new(state, action, reward);

        // Evict oldest if at capacity
        if self.buffer.len() >= self.max_size {
            self.buffer.pop_front();
        }

        self.buffer.push_back(experience);
        self.total_added += 1;
    }

    /// Get current buffer length
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Sample random experiences for batch learning
    pub fn sample(&self, n: usize) -> Vec<(PlannerState, ExecutionAction, f32)> {
        let n = n.min(self.buffer.len());
        if n == 0 {
            return Vec::new();
        }

        let mut indices: Vec<usize> = (0..self.buffer.len()).collect();
        indices.shuffle(&mut rand::thread_rng());

        indices[..n]
            .iter()
            .filter_map(|&i| self.buffer.get(i))
            .map(|e| (e.state.clone(), e.action.clone(), e.reward))
            .collect()
    }

    /// Get most recent experiences
    pub fn recent(&self, n: usize) -> Vec<&Experience> {
        self.buffer.iter().rev().take(n).collect()
    }

    /// Get experiences for a specific engine
    pub fn for_engine(&self, engine: &str) -> Vec<&Experience> {
        self.buffer
            .iter()
            .filter(|e| e.state.storage_engine.to_string() == engine)
            .collect()
    }

    /// Get average reward for experiences
    pub fn average_reward(&self) -> f32 {
        if self.buffer.is_empty() {
            return 0.0;
        }
        self.buffer.iter().map(|e| e.reward).sum::<f32>() / self.buffer.len() as f32
    }

    /// Get reward statistics
    pub fn reward_stats(&self) -> RewardStats {
        if self.buffer.is_empty() {
            return RewardStats::default();
        }

        let rewards: Vec<f32> = self.buffer.iter().map(|e| e.reward).collect();
        let mean = rewards.iter().sum::<f32>() / rewards.len() as f32;
        let variance =
            rewards.iter().map(|r| (r - mean).powi(2)).sum::<f32>() / rewards.len() as f32;

        let mut sorted = rewards.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        RewardStats {
            mean,
            std_dev: variance.sqrt(),
            min: sorted.first().copied().unwrap_or(0.0),
            max: sorted.last().copied().unwrap_or(0.0),
            median: sorted[sorted.len() / 2],
            p25: sorted[sorted.len() / 4],
            p75: sorted[sorted.len() * 3 / 4],
        }
    }

    /// Get total experiences added (including evicted)
    pub fn total_added(&self) -> u64 {
        self.total_added
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Iterate over all experiences
    pub fn iter(&self) -> impl Iterator<Item = &Experience> {
        self.buffer.iter()
    }

    /// Get experiences within time range
    pub fn in_time_range(&self, start: u64, end: u64) -> Vec<&Experience> {
        self.buffer
            .iter()
            .filter(|e| e.timestamp >= start && e.timestamp <= end)
            .collect()
    }

    /// Serialize buffer to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let experiences: Vec<&Experience> = self.buffer.iter().collect();
        serde_json::to_string(&experiences)
    }

    /// Load experiences from JSON
    pub fn from_json(json: &str, max_size: usize) -> Result<Self, serde_json::Error> {
        let experiences: Vec<Experience> = serde_json::from_str(json)?;
        let mut buffer = Self::new(max_size);
        for exp in experiences {
            if buffer.buffer.len() < buffer.max_size {
                buffer.buffer.push_back(exp);
            }
        }
        Ok(buffer)
    }
}

impl Default for ExperienceBuffer {
    fn default() -> Self {
        Self::new(10_000)
    }
}

/// Reward statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RewardStats {
    pub mean: f32,
    pub std_dev: f32,
    pub min: f32,
    pub max: f32,
    pub median: f32,
    pub p25: f32,
    pub p75: f32,
}

/// Prioritized experience replay buffer
///
/// Samples experiences with probability proportional to their TD-error
/// (difference between expected and observed reward).
#[derive(Debug, Clone)]
pub struct PrioritizedExperienceBuffer {
    /// Experiences with priorities
    buffer: VecDeque<(Experience, f32)>,
    /// Maximum buffer size
    max_size: usize,
    /// Priority exponent (alpha)
    alpha: f32,
    /// Importance sampling exponent (beta)
    beta: f32,
}

impl PrioritizedExperienceBuffer {
    /// Create new prioritized buffer
    pub fn new(max_size: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(max_size.min(10_000)),
            max_size,
            alpha: 0.6, // How much to prioritize high TD-error
            beta: 0.4,  // Importance sampling correction
        }
    }

    /// Add experience with initial priority
    pub fn add(
        &mut self,
        state: PlannerState,
        action: ExecutionAction,
        reward: f32,
        td_error: f32,
    ) {
        let experience = Experience::new(state, action, reward);
        let priority = (td_error.abs() + 0.01).powf(self.alpha);

        if self.buffer.len() >= self.max_size {
            self.buffer.pop_front();
        }

        self.buffer.push_back((experience, priority));
    }

    /// Sample experiences proportional to priority
    pub fn sample(&self, n: usize) -> Vec<(PlannerState, ExecutionAction, f32, f32)> {
        if self.buffer.is_empty() {
            return Vec::new();
        }

        let total_priority: f32 = self.buffer.iter().map(|(_, p)| p).sum();
        let mut samples = Vec::with_capacity(n);
        let mut rng = rand::thread_rng();

        for _ in 0..n {
            let target = rand::Rng::gen_range(&mut rng, 0.0..total_priority);
            let mut cumulative = 0.0;

            for (exp, priority) in self.buffer.iter() {
                cumulative += priority;
                if cumulative >= target {
                    // Importance sampling weight
                    let prob = priority / total_priority;
                    let weight = (self.buffer.len() as f32 * prob).powf(-self.beta);

                    samples.push((exp.state.clone(), exp.action.clone(), exp.reward, weight));
                    break;
                }
            }
        }

        samples
    }

    /// Get buffer length
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_experience_buffer_capacity() {
        let mut buffer = ExperienceBuffer::new(5);

        for i in 0..10 {
            buffer.add(
                PlannerState::default(),
                ExecutionAction::default(),
                i as f32 / 10.0,
            );
        }

        // Should only keep last 5
        assert_eq!(buffer.len(), 5);
        assert_eq!(buffer.total_added(), 10);

        // Most recent should have highest reward
        let recent = buffer.recent(1);
        assert!((recent[0].reward - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_sampling() {
        let mut buffer = ExperienceBuffer::new(100);

        for i in 0..50 {
            buffer.add(
                PlannerState::default(),
                ExecutionAction::default(),
                i as f32 / 50.0,
            );
        }

        let samples = buffer.sample(10);
        assert_eq!(samples.len(), 10);
    }

    #[test]
    fn test_reward_stats() {
        let mut buffer = ExperienceBuffer::new(100);

        for i in 0..100 {
            buffer.add(
                PlannerState::default(),
                ExecutionAction::default(),
                i as f32 / 100.0,
            );
        }

        let stats = buffer.reward_stats();
        assert!((stats.mean - 0.495).abs() < 0.1);
        assert!(stats.min < 0.1);
        assert!(stats.max > 0.9);
    }

    #[test]
    fn test_json_roundtrip() {
        let mut buffer = ExperienceBuffer::new(10);
        buffer.add(PlannerState::default(), ExecutionAction::default(), 0.5);
        buffer.add(
            PlannerState::default(),
            ExecutionAction::with_hnsw(100),
            0.8,
        );

        let json = buffer.to_json().unwrap();
        let loaded = ExperienceBuffer::from_json(&json, 10).unwrap();

        assert_eq!(buffer.len(), loaded.len());
    }

    #[test]
    fn test_prioritized_sampling() {
        let mut buffer = PrioritizedExperienceBuffer::new(100);

        // Add high-priority experience
        buffer.add(
            PlannerState::default(),
            ExecutionAction::with_hnsw(100),
            0.9,
            0.5, // High TD-error
        );

        // Add low-priority experiences
        for _ in 0..10 {
            buffer.add(
                PlannerState::default(),
                ExecutionAction::default(),
                0.5,
                0.01, // Low TD-error
            );
        }

        // High-priority should be sampled more often
        let mut high_priority_count = 0;
        for _ in 0..100 {
            let samples = buffer.sample(1);
            if !samples.is_empty() && samples[0].2 > 0.8 {
                high_priority_count += 1;
            }
        }

        // Should sample high-priority more than uniform would
        assert!(high_priority_count > 15); // > 15% vs 9% uniform
    }
}
