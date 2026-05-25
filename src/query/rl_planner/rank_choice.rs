//! Per-query ranking phase choices the RL planner can vary.
//!
//! [`PlannerAction`] wraps the existing [`ExecutionAction`] (retrieval
//! plan) with a [`RankPhaseChoice`]. The wrapper pattern keeps the
//! retrieval-plan struct unchanged — its many literal constructors in
//! `paths/*` aren't disturbed — while letting the planner emit a joint
//! action that varies both retrieval and ranking dimensions.
//!
//! Per spec §4.8 — joint-action extension.

use crate::query::rl_planner::action::ExecutionAction;
use serde::{Deserialize, Serialize};

/// One position in the rank-phase action sub-space. Hashable + Eq so
/// the bandit can use it as a discrete arm component.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RankPhaseChoice {
    /// Skip the per-shard second phase (cross-encoder rerank). The
    /// first-phase heap is delivered straight to merge + global.
    pub skip_second: bool,
    /// Skip the post-merge global phase (cross-modal reranker / LLM
    /// listwise). The merged first/second-phase top-K is the final
    /// answer.
    pub skip_global: bool,
    /// Second-phase rerank-count override (clamped to ≤ heap_size at
    /// pipeline-materialize time). 0 = use profile default.
    pub second_phase_k: u16,
    /// Cross-encoder batch size override (clamped to model's
    /// max_batch_size). 0 = use model default.
    pub batch_size: u16,
}

impl RankPhaseChoice {
    /// Default — all phases enabled, profile/model defaults for sizes.
    pub fn enabled_default() -> Self {
        Self {
            skip_second: false,
            skip_global: false,
            second_phase_k: 0,
            batch_size: 0,
        }
    }

    /// Skip every optional phase — minimum latency, lowest quality.
    /// First-phase output goes straight to the caller.
    pub fn first_phase_only() -> Self {
        Self {
            skip_second: true,
            skip_global: true,
            second_phase_k: 0,
            batch_size: 0,
        }
    }

    /// Aggressive variant: keep all phases, rerank a wide top-K (k=200).
    /// Highest quality, highest latency. Used when the optimization
    /// target prioritizes recall.
    pub fn aggressive(batch_size: u16) -> Self {
        Self {
            skip_second: false,
            skip_global: false,
            second_phase_k: 200,
            batch_size,
        }
    }

    /// Discrete arm id for the multi-armed bandit. Packs the boolean
    /// flags + numeric buckets into a small `u8` so the action space
    /// stays tractable. Bucket choices:
    /// - second_phase_k: 0=default, ≤50, ≤100, ≤200, >200 (3 bits)
    /// - batch_size:     0=default, ≤8, ≤32, ≤64, >64 (3 bits)
    ///
    /// Layout: `[skip_global:1][skip_second:1][k_bucket:3][batch_bucket:3]` → 8 bits.
    pub fn arm_id(&self) -> u8 {
        let k_bucket = if self.second_phase_k == 0 {
            0
        } else if self.second_phase_k <= 50 {
            1
        } else if self.second_phase_k <= 100 {
            2
        } else if self.second_phase_k <= 200 {
            3
        } else {
            4
        };
        let b_bucket = if self.batch_size == 0 {
            0
        } else if self.batch_size <= 8 {
            1
        } else if self.batch_size <= 32 {
            2
        } else if self.batch_size <= 64 {
            3
        } else {
            4
        };
        let mut id: u8 = 0;
        id |= (self.skip_global as u8) << 7;
        id |= (self.skip_second as u8) << 6;
        id |= (k_bucket & 0x7) << 3;
        id |= b_bucket & 0x7;
        id
    }
}

impl Default for RankPhaseChoice {
    fn default() -> Self {
        Self::enabled_default()
    }
}

/// Joint action: retrieval plan + (optional) rank-phase choice. Emitted
/// by the RL planner per query. The wrapper avoids disturbing the many
/// `ExecutionAction { ... }` literal constructors in `paths/*` while
/// still letting the planner explore a joint action space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerAction {
    pub execution: ExecutionAction,
    /// `None` = no rank profile attached (legacy retrieval-only path).
    /// `Some` = pipeline runs with this phase configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_choice: Option<RankPhaseChoice>,
}

impl PlannerAction {
    pub fn retrieval_only(execution: ExecutionAction) -> Self {
        Self {
            execution,
            rank_choice: None,
        }
    }

    pub fn with_rank(execution: ExecutionAction, rank_choice: RankPhaseChoice) -> Self {
        Self {
            execution,
            rank_choice: Some(rank_choice),
        }
    }

    /// Composite arm id: high 24 bits = retrieval action id, low 8 bits
    /// = rank-choice arm (or 0 when `rank_choice == None`).
    pub fn arm_id(&self) -> u32 {
        let retrieval = self.execution.to_action_id();
        let rank = self.rank_choice.map(|r| r.arm_id() as u32).unwrap_or(0);
        (retrieval << 8) | (rank & 0xFF)
    }
}

impl From<ExecutionAction> for PlannerAction {
    fn from(execution: ExecutionAction) -> Self {
        Self::retrieval_only(execution)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_default_runs_all_phases() {
        let c = RankPhaseChoice::enabled_default();
        assert!(!c.skip_second);
        assert!(!c.skip_global);
        assert_eq!(c.second_phase_k, 0);
        assert_eq!(c.batch_size, 0);
    }

    #[test]
    fn first_phase_only_skips_downstream() {
        let c = RankPhaseChoice::first_phase_only();
        assert!(c.skip_second);
        assert!(c.skip_global);
    }

    #[test]
    fn aggressive_uses_wide_k_and_supplied_batch() {
        let c = RankPhaseChoice::aggressive(32);
        assert!(!c.skip_second);
        assert!(!c.skip_global);
        assert_eq!(c.second_phase_k, 200);
        assert_eq!(c.batch_size, 32);
    }

    #[test]
    fn arm_id_default_is_zero() {
        // Default → all bits zero → arm 0.
        assert_eq!(RankPhaseChoice::default().arm_id(), 0);
    }

    #[test]
    fn arm_id_first_phase_only_uses_top_bits() {
        // skip_global + skip_second + bucket 0 + bucket 0 → 0b11_000_000 = 192
        assert_eq!(RankPhaseChoice::first_phase_only().arm_id(), 0b1100_0000);
    }

    #[test]
    fn arm_id_distinguishes_buckets() {
        // Different k buckets → different arm ids (all other bits the same).
        let arms: Vec<u8> = [0u16, 50, 100, 200, 500]
            .iter()
            .map(|k| {
                let c = RankPhaseChoice {
                    second_phase_k: *k,
                    ..Default::default()
                };
                c.arm_id()
            })
            .collect();
        let unique: std::collections::HashSet<_> = arms.iter().collect();
        assert_eq!(unique.len(), 5, "5 k buckets must produce distinct arms: {arms:?}");
    }

    #[test]
    fn planner_action_default_has_no_rank_choice() {
        let a = PlannerAction::retrieval_only(ExecutionAction::default());
        assert!(a.rank_choice.is_none());
    }

    #[test]
    fn planner_action_with_rank_carries_choice() {
        let a = PlannerAction::with_rank(
            ExecutionAction::default(),
            RankPhaseChoice::aggressive(32),
        );
        assert!(a.rank_choice.is_some());
        assert_eq!(a.rank_choice.unwrap().second_phase_k, 200);
    }

    #[test]
    fn planner_action_arm_id_components_combine_distinctly() {
        // Two planner actions that differ only in rank_choice must
        // produce distinct arm ids.
        let exec = ExecutionAction::default();
        let a = PlannerAction::retrieval_only(exec.clone());
        let b = PlannerAction::with_rank(exec, RankPhaseChoice::aggressive(32));
        assert_ne!(a.arm_id(), b.arm_id());
    }

    #[test]
    fn arm_id_stays_in_u8() {
        // Most-aggressive setting must still fit a u8 (we packed into 8 bits).
        let c = RankPhaseChoice {
            skip_second: true,
            skip_global: true,
            second_phase_k: u16::MAX,
            batch_size: u16::MAX,
        };
        let _ = c.arm_id(); // type system enforces; this just exercises the path.
    }
}
