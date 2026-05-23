//! Compact ranking framework primitives.
//!
//! `PhaseId`, `ScoreComponent`, `ScoreVector` are canonical — they live in
//! `proximadb-kernel` and are re-exported via `crate::lib`.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.1.2.

/// Index of an executor in a `RankProgram` (post-resolution).
///
/// `u16` is the chosen width because a single profile is expected to bound
/// its DAG well under 64k executors (the spec caps DAG depth at 256). Width
/// is part of the on-wire format for explain / training-data export.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ExecutorIdx(pub u16);

/// {executor_index, output_index} — O(1) wiring after DAG resolution.
///
/// Compactly packed so per-doc evaluation can keep many of these in registers
/// without spilling. The layout-guard test asserts the total size stays ≤ 4
/// bytes (one cache line holds 16 of them).
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FeatureRef {
    pub executor: ExecutorIdx,
    pub output: u8,
}

impl FeatureRef {
    pub const fn new(executor: ExecutorIdx, output: u8) -> Self {
        Self { executor, output }
    }
}

/// Handle to a document being scored. Local to the segment / shard.
///
/// `u32` matches the existing internal doc-id width in the codebase
/// (`OptimizedSearchRecord::version`, etc.) and gives 4B docs per segment
/// of headroom.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct DocHandle(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_ref_fits_in_4_bytes() {
        // Layout guard: a single cache line should hold ≥ 16 FeatureRefs so
        // expression bytecode can stream through them with minimal cache
        // pressure. If this assertion fails we've accidentally bloated the
        // hot-path primitive.
        assert!(
            std::mem::size_of::<FeatureRef>() <= 4,
            "FeatureRef is {} bytes — must stay ≤ 4 for cache density",
            std::mem::size_of::<FeatureRef>()
        );
    }

    #[test]
    fn executor_idx_is_u16_sized() {
        assert_eq!(std::mem::size_of::<ExecutorIdx>(), 2);
    }

    #[test]
    fn doc_handle_is_u32_sized() {
        assert_eq!(std::mem::size_of::<DocHandle>(), 4);
    }

    #[test]
    fn feature_ref_serializes_compactly() {
        let r = FeatureRef::new(ExecutorIdx(7), 2);
        let j = serde_json::to_string(&r).unwrap();
        // executor field uses transparent serde so this is just two integers.
        assert_eq!(j, r#"{"executor":7,"output":2}"#);
    }
}
