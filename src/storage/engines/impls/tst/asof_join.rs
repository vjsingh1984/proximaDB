//! ASOF Join Module
//!
//! Implements ASOF (As-Of) joins for time-series data.
//!
//! ASOF joins match records from two time-series based on the closest
//! timestamp in the right series that is <= the timestamp in the left series.
//!
//! This is essential for trading systems where you need to join trades
//! with quotes - each trade should match the most recent quote as of the trade time.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::proto::proximadb_v1::VectorRecord;

/// ASOF join result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ASOFJoinResult {
    /// Left series record
    pub left_record: VectorRecord,

    /// Matching right series record (if found)
    pub right_record: Option<VectorRecord>,

    /// Time difference between records
    pub time_diff: Option<Duration>,
}

/// ASOF join engine
pub struct ASOFJoin {
    /// Maximum time difference for matching
    tolerance: Option<Duration>,
}

impl ASOFJoin {
    /// Create a new ASOF join engine
    pub fn new() -> Self {
        Self { tolerance: None }
    }

    /// Set tolerance for ASOF join
    pub fn with_tolerance(mut self, tolerance: Duration) -> Self {
        self.tolerance = Some(tolerance);
        self
    }

    /// Execute ASOF join
    pub async fn execute(
        &self,
        left_series: Vec<VectorRecord>,
        right_series: Vec<VectorRecord>,
        tolerance: Option<Duration>,
    ) -> Result<Vec<ASOFJoinResult>> {
        let tolerance = tolerance.or(self.tolerance);

        // Sort right series by timestamp for efficient lookup
        let mut sorted_right = right_series.clone();
        sorted_right.sort_by_key(|r| r.timestamp.unwrap_or(0));

        let mut results = Vec::new();

        for left_record in left_series {
            let left_ts = left_record.timestamp.unwrap_or(0);
            let left_dt = DateTime::from_timestamp(left_ts, 0).unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());

            // Find the most recent right record <= left timestamp
            let match_result = sorted_right
                .iter()
                .rev()
                .find(|right| {
                    let right_ts = right.timestamp.unwrap_or(0);
                    right_ts <= left_ts
                });

            if let Some(right_record) = match_result {
                let right_ts = right_record.timestamp.unwrap_or(0);
                let time_diff = Duration::seconds(left_ts - right_ts);

                // Check tolerance
                if let Some(tol) = tolerance {
                    if time_diff > tol {
                        // Outside tolerance, no match
                        results.push(ASOFJoinResult {
                            left_record,
                            right_record: None,
                            time_diff: None,
                        });
                        continue;
                    }
                }

                results.push(ASOFJoinResult {
                    left_record,
                    right_record: Some(right_record.clone()),
                    time_diff: Some(time_diff.abs()),
                });
            } else {
                // No matching right record
                results.push(ASOFJoinResult {
                    left_record,
                    right_record: None,
                    time_diff: None,
                });
            }
        }

        Ok(results)
    }
}

/// ASOF join query
#[derive(Debug, Clone)]
pub struct ASOFJoinQuery {
    /// Left series collection ID
    pub left_collection: String,

    /// Right series collection ID
    pub right_collection: String,

    /// Time range for join
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,

    /// Tolerance for matching
    pub tolerance: Option<Duration>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(id: &str, timestamp: i64) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
            timestamp: Some(DateTime::from_timestamp(timestamp, 0).unwrap()),
            ..Default::default()
        }
    }

    #[test]
    fn test_asof_join() {
        let left = vec![
            make_record("left1", 100),
            make_record("left2", 150),
            make_record("left3", 200),
        ];

        let right = vec![
            make_record("right1", 90),
            make_record("right2", 140),
            make_record("right3", 180),
        ];

        let asof = ASOFJoin::new();

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                let results = asof.execute(left, right, None).await.unwrap();

                assert_eq!(results.len(), 3);

                // left1 at 100 should match right2 at 90 (closest <= 100)
                assert_eq!(results[0].left_record.id, "left1");
                assert_eq!(results[0].right_record.as_ref().unwrap().id, "right1");

                // left2 at 150 should match right2 at 140
                assert_eq!(results[1].left_record.id, "left2");
                assert_eq!(results[1].right_record.as_ref().unwrap().id, "right2");

                // left3 at 200 should match right3 at 180
                assert_eq!(results[2].left_record.id, "left3");
                assert_eq!(results[2].right_record.as_ref().unwrap().id, "right3");
            });
    }

    #[test]
    fn test_asof_join_with_tolerance() {
        let left = vec![make_record("left1", 100)];

        let right = vec![make_record("right1", 80)]; // 20 seconds before

        let asof = ASOFJoin::new();
        let tolerance = Duration::seconds(15);

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                let results = asof.execute(left, right, Some(tolerance)).await.unwrap();

                // Should not match because time difference (20s) > tolerance (15s)
                assert_eq!(results.len(), 1);
                assert!(results[0].right_record.is_none());
            });
    }
}
