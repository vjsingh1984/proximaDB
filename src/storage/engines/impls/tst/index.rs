//! Index Module
//!
//! Time-series specific indexing for efficient queries.

use anyhow::Result;
use std::collections::BTreeMap;
use chrono::{DateTime, Utc};

/// Time-range index
///
/// Maps time ranges to partitions for efficient partition pruning.
pub struct TimeRangeIndex {
    /// Index entries
    /// Key: Partition start time
    /// Value: (min timestamp, max timestamp)
    index: BTreeMap<DateTime<Utc>, (DateTime<Utc>, DateTime<Utc>)>,
}

impl TimeRangeIndex {
    /// Create a new time-range index
    pub fn new() -> Self {
        Self {
            index: BTreeMap::new(),
        }
    }

    /// Add a partition to the index
    pub fn add_partition(
        &mut self,
        partition_start: DateTime<Utc>,
        min_ts: DateTime<Utc>,
        max_ts: DateTime<Utc>,
    ) -> Result<()> {
        self.index.insert(partition_start, (min_ts, max_ts));
        Ok(())
    }

    /// Find partitions that overlap with a time range
    pub fn find_partitions(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<DateTime<Utc>> {
        self.index
            .range(..=end)
            .filter(|(_, (min, max))| *max >= start && *min <= end)
            .map(|(start, _)| *start)
            .collect()
    }

    /// Remove a partition from the index
    pub fn remove_partition(&mut self, partition_start: DateTime<Utc>) -> Result<()> {
        self.index.remove(&partition_start);
        Ok(())
    }

    /// Get index size
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Check if index is empty
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

impl Default for TimeRangeIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_range_index() {
        let mut index = TimeRangeIndex::new();

        let p1_start = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let p1_end = DateTime::parse_from_rfc3339("2024-01-01T23:59:59Z")
            .unwrap()
            .with_timezone(&Utc);

        index.add_partition(p1_start, p1_start, p1_end).unwrap();

        let query_start = DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let query_end = DateTime::parse_from_rfc3339("2024-01-01T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let results = index.find_partitions(query_start, query_end);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], p1_start);
    }
}
