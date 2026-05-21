//! Field-statistics refresher used by the ingest scheduler.

#![allow(missing_docs)]

use crate::query::federated::optimizer::selectivity::{FieldStatistics, HistogramBucket};
use proximadb_catalog::CatalogTableStatistics;

#[derive(Debug, Clone)]
pub struct RefresherConfig {
    pub histogram_buckets: usize,
    pub include_nullable_fields: bool,
}

impl Default for RefresherConfig {
    fn default() -> Self {
        Self {
            histogram_buckets: 16,
            include_nullable_fields: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FieldStatsRefresher {
    config: RefresherConfig,
}

impl Default for FieldStatsRefresher {
    fn default() -> Self {
        Self::new(RefresherConfig::default())
    }
}

impl FieldStatsRefresher {
    pub fn new(config: RefresherConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &RefresherConfig {
        &self.config
    }

    /// Convert catalog min/max/null-count metadata into the query optimizer's
    /// `FieldStatistics` shape. Categorical and co-occurrence stats require
    /// sampled value counts, so this refresher only fills the row count and
    /// conservative one-bucket numeric histograms from catalog statistics.
    pub fn from_catalog_statistics(&self, stats: &CatalogTableStatistics) -> FieldStatistics {
        let mut field_stats = FieldStatistics {
            row_count: stats.row_count,
            ..FieldStatistics::default()
        };
        if stats.row_count == 0 {
            return field_stats;
        }

        for (field, column) in stats.column_stats.iter() {
            let Some(min_value) = column.min_value.as_deref() else {
                continue;
            };
            let Some(max_value) = column.max_value.as_deref() else {
                continue;
            };
            let Some(lo) = decode_numeric_stat(min_value) else {
                continue;
            };
            let Some(hi) = decode_numeric_stat(max_value) else {
                continue;
            };
            if !self.config.include_nullable_fields && column.null_count.unwrap_or(0) > 0 {
                continue;
            }
            let non_null = stats
                .row_count
                .saturating_sub(column.null_count.unwrap_or_default());
            if non_null == 0 {
                continue;
            }
            field_stats.range_histograms.insert(
                field.clone(),
                vec![HistogramBucket {
                    lo,
                    hi: if hi > lo { hi } else { lo + f64::EPSILON },
                    count: non_null,
                }],
            );
        }
        field_stats
    }
}

fn decode_numeric_stat(value: &str) -> Option<f64> {
    value
        .parse::<i64>()
        .map(|v| v as f64)
        .or_else(|_| value.parse::<f64>())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_catalog::CatalogColumnStatistics;
    use std::collections::HashMap;

    #[test]
    fn empty_catalog_stats_produce_empty_field_stats() {
        let refresher = FieldStatsRefresher::default();
        let stats = CatalogTableStatistics::default();
        let out = refresher.from_catalog_statistics(&stats);
        assert_eq!(out.row_count, 0);
        assert!(out.range_histograms.is_empty());
    }

    #[test]
    fn numeric_minmax_becomes_single_bucket_histogram() {
        let refresher = FieldStatsRefresher::default();
        let mut column_stats = HashMap::new();
        column_stats.insert(
            "score".to_string(),
            CatalogColumnStatistics {
                null_count: Some(2),
                min_value: Some("+0000000000000000010".to_string()),
                max_value: Some("+0000000000000000090".to_string()),
                ..CatalogColumnStatistics::default()
            },
        );
        let out = refresher.from_catalog_statistics(&CatalogTableStatistics {
            row_count: 12,
            column_stats,
            ..CatalogTableStatistics::default()
        });
        let bucket = &out.range_histograms["score"][0];
        assert_eq!(bucket.lo, 10.0);
        assert_eq!(bucket.hi, 90.0);
        assert_eq!(bucket.count, 10);
    }
}
