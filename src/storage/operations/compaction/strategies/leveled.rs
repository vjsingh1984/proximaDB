//! Leveled Compaction Strategy
//!
//! Implements LSM-tree style leveled compaction, ideal for write-heavy workloads.
//! Used by SST engine and similar sorted-string-table based stores.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;

use super::{
    CompactionCostEstimate, CompactionParameters, CompactionPlan, CompactionStrategy,
    FileMetadata, FileStatistics,
};

/// Leveled compaction strategy (LSM-tree style)
///
/// # Algorithm
///
/// 1. Level 0 (L0): Holds freshly flushed SSTables, may have overlapping keys
/// 2. Level N (N > 0): Non-overlapping, sorted by key range
/// 3. Compaction triggers when level exceeds size ratio
/// 4. Files from level N merge with overlapping files in level N+1
///
/// # Performance Characteristics
///
/// - **Read Amplification**: O(log N) - excellent for reads
/// - **Write Amplification**: O(10-30x) - higher due to re-merging
/// - **Space Amplification**: O(1.1x) - very space efficient
///
/// # Best For
///
/// - Read-heavy workloads after initial ingestion
/// - Range queries (data sorted by key)
/// - Space-constrained deployments
#[derive(Debug, Clone)]
pub struct LeveledCompactionStrategy {
    /// Size ratio between levels (typically 10)
    level_size_ratio: f64,
    /// Maximum L0 files before triggering compaction
    max_l0_files: usize,
    /// Target file size in bytes
    target_file_size: u64,
    /// Maximum levels
    max_levels: u32,
    /// Minimum files to compact together
    min_files_to_compact: usize,
}

impl Default for LeveledCompactionStrategy {
    fn default() -> Self {
        Self {
            level_size_ratio: 10.0,
            max_l0_files: 4,
            target_file_size: 64 * 1024 * 1024, // 64MB
            max_levels: 7,
            min_files_to_compact: 2,
        }
    }
}

impl LeveledCompactionStrategy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure level size ratio (default 10)
    pub fn with_level_ratio(mut self, ratio: f64) -> Self {
        self.level_size_ratio = ratio;
        self
    }

    /// Configure max L0 files (default 4)
    pub fn with_max_l0_files(mut self, max: usize) -> Self {
        self.max_l0_files = max;
        self
    }

    /// Configure target file size (default 64MB)
    pub fn with_target_file_size(mut self, size: u64) -> Self {
        self.target_file_size = size;
        self
    }

    /// Calculate target size for a level
    fn level_target_size(&self, level: u32) -> u64 {
        if level == 0 {
            return self.target_file_size * self.max_l0_files as u64;
        }
        let base = self.target_file_size * 10; // L1 base size
        (base as f64 * self.level_size_ratio.powi(level as i32 - 1)) as u64
    }

    /// Group files by level
    fn group_by_level<'a>(&self, files: &'a [FileMetadata]) -> HashMap<u32, Vec<&'a FileMetadata>> {
        let mut levels: HashMap<u32, Vec<&'a FileMetadata>> = HashMap::new();
        for file in files {
            levels.entry(file.level).or_default().push(file);
        }
        levels
    }

    /// Find overlapping files in target level
    fn find_overlapping_files<'a>(
        &self,
        source_files: &[&FileMetadata],
        target_level_files: &[&'a FileMetadata],
    ) -> Vec<&'a FileMetadata> {
        // Get key range from source files
        let source_min = source_files
            .iter()
            .filter_map(|f| f.min_key.as_ref())
            .min();
        let source_max = source_files
            .iter()
            .filter_map(|f| f.max_key.as_ref())
            .max();

        match (source_min, source_max) {
            (Some(min), Some(max)) => {
                target_level_files
                    .iter()
                    .filter(|f| {
                        if let (Some(f_min), Some(f_max)) = (&f.min_key, &f.max_key) {
                            // Check for overlap
                            f_min <= max && f_max >= min
                        } else {
                            // Conservative: assume overlap if no key info
                            true
                        }
                    })
                    .copied()
                    .collect()
            }
            _ => {
                // No key info, include all target files (conservative)
                target_level_files.iter().copied().collect()
            }
        }
    }

    /// Select L0 compaction (L0 → L1)
    fn select_l0_compaction(
        &self,
        collection_id: &str,
        levels: &HashMap<u32, Vec<&FileMetadata>>,
    ) -> Option<CompactionPlan> {
        let l0_files = levels.get(&0)?;

        if l0_files.len() < self.max_l0_files {
            return None;
        }

        // Get L1 files that overlap with L0
        let l1_files = levels.get(&1).cloned().unwrap_or_default();
        let overlapping_l1 = self.find_overlapping_files(l0_files, &l1_files);

        let mut input_files: Vec<FileMetadata> = l0_files.iter().map(|f| (*f).clone()).collect();
        input_files.extend(overlapping_l1.iter().map(|f| (*f).clone()));

        let estimated_output_size: u64 = input_files.iter().map(|f| f.size_bytes).sum();

        Some(CompactionPlan {
            plan_id: format!("leveled_l0_l1_{}", chrono::Utc::now().timestamp_millis()),
            collection_id: collection_id.to_string(),
            input_files,
            target_level: 1,
            estimated_output_size,
            priority: 100.0, // L0 compaction is highest priority
            strategy_name: "leveled".to_string(),
            parameters: CompactionParameters {
                target_file_size_bytes: self.target_file_size,
                apply_requantization: false,
                compression_level: 6,
                rebuild_bloom_filters: true,
                max_output_files: 10,
            },
        })
    }

    /// Select level N → N+1 compaction
    fn select_level_compaction(
        &self,
        collection_id: &str,
        levels: &HashMap<u32, Vec<&FileMetadata>>,
    ) -> Option<CompactionPlan> {
        // Check each level starting from L1
        for level in 1..self.max_levels {
            let level_files = match levels.get(&level) {
                Some(files) => files,
                None => continue,
            };

            let level_size: u64 = level_files.iter().map(|f| f.size_bytes).sum();
            let target_size = self.level_target_size(level);

            if level_size <= target_size {
                continue;
            }

            // Select files to compact - pick oldest or most overlapping
            let files_to_compact: Vec<&FileMetadata> = level_files
                .iter()
                .take(self.min_files_to_compact.max(2))
                .copied()
                .collect();

            // Find overlapping files in next level
            let next_level_files = levels.get(&(level + 1)).cloned().unwrap_or_default();
            let overlapping = self.find_overlapping_files(&files_to_compact, &next_level_files);

            let mut input_files: Vec<FileMetadata> =
                files_to_compact.iter().map(|f| (*f).clone()).collect();
            input_files.extend(overlapping.iter().map(|f| (*f).clone()));

            let estimated_output_size: u64 = input_files.iter().map(|f| f.size_bytes).sum();

            return Some(CompactionPlan {
                plan_id: format!(
                    "leveled_l{}_l{}_{}",
                    level,
                    level + 1,
                    chrono::Utc::now().timestamp_millis()
                ),
                collection_id: collection_id.to_string(),
                input_files,
                target_level: level + 1,
                estimated_output_size,
                priority: 50.0 / level as f64, // Lower levels have higher priority
                strategy_name: "leveled".to_string(),
                parameters: CompactionParameters {
                    target_file_size_bytes: self.target_file_size,
                    apply_requantization: false,
                    compression_level: 6,
                    rebuild_bloom_filters: true,
                    max_output_files: ((estimated_output_size / self.target_file_size) + 1) as usize,
                },
            });
        }

        None
    }
}

#[async_trait]
impl CompactionStrategy for LeveledCompactionStrategy {
    fn name(&self) -> &'static str {
        "leveled"
    }

    async fn select_files(
        &self,
        collection_id: &str,
        files: &[FileMetadata],
    ) -> Result<Option<CompactionPlan>> {
        if files.is_empty() {
            return Ok(None);
        }

        let levels = self.group_by_level(files);

        // Priority 1: L0 compaction (always most urgent)
        if let Some(plan) = self.select_l0_compaction(collection_id, &levels) {
            tracing::debug!(
                "LeveledCompaction: selected L0→L1 compaction with {} files",
                plan.input_files.len()
            );
            return Ok(Some(plan));
        }

        // Priority 2: Level N → N+1 compaction
        if let Some(plan) = self.select_level_compaction(collection_id, &levels) {
            tracing::debug!(
                "LeveledCompaction: selected L{}→L{} compaction with {} files",
                plan.target_level - 1,
                plan.target_level,
                plan.input_files.len()
            );
            return Ok(Some(plan));
        }

        Ok(None)
    }

    fn priority_score(&self, stats: &FileStatistics) -> f64 {
        let mut score = 0.0;

        // High priority if L0 has many files
        if !stats.files_per_level.is_empty() {
            let l0_files = stats.files_per_level[0];
            if l0_files >= self.max_l0_files {
                score += 100.0;
            } else if l0_files >= self.max_l0_files / 2 {
                score += 50.0;
            }
        }

        // Increase priority based on read amplification
        score += stats.read_amplification * 10.0;

        // Increase priority based on space amplification
        if stats.space_amplification > 1.5 {
            score += (stats.space_amplification - 1.0) * 20.0;
        }

        // High tombstone ratio increases priority
        score += stats.tombstone_ratio * 50.0;

        score
    }

    fn estimate_cost(&self, plan: &CompactionPlan) -> CompactionCostEstimate {
        let input_size: u64 = plan.input_files.iter().map(|f| f.size_bytes).sum();
        let output_size = plan.estimated_output_size;

        // Total I/O = read input + write output
        let total_io = input_size + output_size;

        // Estimate ~100 MB/s throughput
        let throughput_bytes_per_sec = 100 * 1024 * 1024;
        let estimated_seconds = total_io as f64 / throughput_bytes_per_sec as f64;

        CompactionCostEstimate {
            estimated_time: Duration::from_secs_f64(estimated_seconds),
            estimated_io_bytes: total_io,
            estimated_cpu_cost: plan.input_files.len() as f64 * 10.0, // Merge sort cost
            expected_bytes_freed: (input_size as f64 * 0.1) as u64,   // ~10% reduction
            priority_score: plan.priority,
        }
    }

    fn applies_to_engine(&self, engine_name: &str) -> bool {
        matches!(engine_name.to_lowercase().as_str(), "sst" | "nova" | "raptor")
    }

    fn optimization_hints(&self) -> Vec<String> {
        vec![
            format!("level_size_ratio: {}", self.level_size_ratio),
            format!("max_l0_files: {}", self.max_l0_files),
            format!(
                "target_file_size: {} MB",
                self.target_file_size / (1024 * 1024)
            ),
            format!("max_levels: {}", self.max_levels),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_files() -> Vec<FileMetadata> {
        vec![
            FileMetadata::new("l0_1", "/data/l0_1.sst", 32 * 1024 * 1024)
                .with_level(0)
                .with_key_range("a", "m"),
            FileMetadata::new("l0_2", "/data/l0_2.sst", 32 * 1024 * 1024)
                .with_level(0)
                .with_key_range("f", "z"),
            FileMetadata::new("l0_3", "/data/l0_3.sst", 32 * 1024 * 1024)
                .with_level(0)
                .with_key_range("c", "p"),
            FileMetadata::new("l0_4", "/data/l0_4.sst", 32 * 1024 * 1024)
                .with_level(0)
                .with_key_range("b", "k"),
            FileMetadata::new("l1_1", "/data/l1_1.sst", 64 * 1024 * 1024)
                .with_level(1)
                .with_key_range("a", "g"),
            FileMetadata::new("l1_2", "/data/l1_2.sst", 64 * 1024 * 1024)
                .with_level(1)
                .with_key_range("h", "n"),
            FileMetadata::new("l1_3", "/data/l1_3.sst", 64 * 1024 * 1024)
                .with_level(1)
                .with_key_range("o", "z"),
        ]
    }

    #[tokio::test]
    async fn test_l0_compaction_trigger() {
        let strategy = LeveledCompactionStrategy::new();
        let files = create_test_files();

        let plan = strategy.select_files("test", &files).await.unwrap();
        assert!(plan.is_some());

        let plan = plan.unwrap();
        assert_eq!(plan.target_level, 1);
        assert!(plan.priority >= 100.0); // L0 compaction should be high priority
    }

    #[tokio::test]
    async fn test_no_compaction_needed() {
        let strategy = LeveledCompactionStrategy::new();
        let files = vec![
            FileMetadata::new("l0_1", "/data/l0_1.sst", 32 * 1024 * 1024).with_level(0),
        ];

        let plan = strategy.select_files("test", &files).await.unwrap();
        assert!(plan.is_none()); // Only 1 L0 file, no compaction needed
    }

    #[test]
    fn test_priority_score_calculation() {
        let strategy = LeveledCompactionStrategy::new();

        let stats = FileStatistics {
            files_per_level: vec![5, 2, 1], // 5 L0 files
            read_amplification: 3.0,
            space_amplification: 1.8,
            tombstone_ratio: 0.1,
            ..Default::default()
        };

        let score = strategy.priority_score(&stats);
        assert!(score > 100.0); // Should be high due to L0 file count
    }

    #[test]
    fn test_applies_to_engine() {
        let strategy = LeveledCompactionStrategy::new();

        assert!(strategy.applies_to_engine("sst"));
        assert!(strategy.applies_to_engine("SST"));
        assert!(strategy.applies_to_engine("nova"));
        assert!(!strategy.applies_to_engine("viper"));
        assert!(!strategy.applies_to_engine("helix"));
    }
}
