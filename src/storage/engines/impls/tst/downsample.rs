//! Downsampling Module
//!
//! Provides time-based downsampling for reducing data granularity.

use anyhow::Result;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{DownsampleAggregation, OHLCBar, TimePartition};

/// Downsampling configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownsampleConfig {
    /// Time interval for downsampling
    pub interval: DownsampleInterval,

    /// Aggregation type
    pub aggregation: DownsampleAggregation,
}

/// Downsample interval
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum DownsampleInterval {
    Second,
    Minute,
    FiveMinutes,
    FifteenMinutes,
    Hour,
    FourHours,
    Day,
    Week,
}

impl DownsampleInterval {
    /// Get interval in seconds
    pub fn as_seconds(&self) -> i64 {
        match self {
            DownsampleInterval::Second => 1,
            DownsampleInterval::Minute => 60,
            DownsampleInterval::FiveMinutes => 300,
            DownsampleInterval::FifteenMinutes => 900,
            DownsampleInterval::Hour => 3600,
            DownsampleInterval::FourHours => 14400,
            DownsampleInterval::Day => 86400,
            DownsampleInterval::Week => 604800,
        }
    }

    /// Truncate timestamp to interval boundary
    pub fn truncate_timestamp(&self, ts: i64) -> i64 {
        let secs = self.as_seconds();
        ts - (ts % secs)
    }
}

/// Downsampling engine
pub struct Downsampler {
    /// Downsampling configuration
    config: DownsampleConfig,

    /// Minimum records before triggering downsampling
    min_records: usize,
}

impl Downsampler {
    /// Create a new downsampler
    pub fn new(config: DownsampleConfig) -> Self {
        Self {
            config,
            min_records: 100, // Default: at least 100 records
        }
    }

    /// Set minimum records threshold
    pub fn with_min_records(mut self, min_records: usize) -> Self {
        self.min_records = min_records;
        self
    }

    /// Get the configuration
    pub fn config(&self) -> &DownsampleConfig {
        &self.config
    }

    /// Check if downsampling should be triggered
    pub async fn should_trigger(&self, partition: &TimePartition) -> Result<bool> {
        // Check if partition has enough records
        Ok(partition.record_count() >= self.min_records)
    }

    /// Downsample a partition
    pub async fn downsample(&self, partition: &TimePartition) -> Result<Vec<OHLCBar>> {
        // Get all OHLC bars from partition
        let all_bars = partition.all_ohlc_bars().await?;

        if all_bars.is_empty() {
            return Ok(Vec::new());
        }

        match self.config.aggregation {
            DownsampleAggregation::OHLC => self.downsample_ohlc(&all_bars),
            DownsampleAggregation::Avg => self.downsample_avg(&all_bars),
            DownsampleAggregation::Sum => self.downsample_sum(&all_bars),
            DownsampleAggregation::MinMax => self.downsample_minmax(&all_bars),
            DownsampleAggregation::FirstLast => self.downsample_first_last(&all_bars),
            DownsampleAggregation::Count => self.downsample_count(&all_bars),
        }
    }

    /// Downsample using OHLC aggregation
    fn downsample_ohlc(&self, bars: &[OHLCBar]) -> Result<Vec<OHLCBar>> {
        let mut by_symbol: HashMap<&str, Vec<&OHLCBar>> = HashMap::new();

        // Group by symbol
        for bar in bars {
            by_symbol
                .entry(&bar.symbol)
                .or_insert_with(Vec::new)
                .push(bar);
        }

        let mut result = Vec::new();

        // Downsample each symbol
        for (symbol, symbol_bars) in by_symbol {
            let mut buckets: HashMap<i64, Vec<&OHLCBar>> = HashMap::new();

            // Group by time bucket
            for bar in symbol_bars {
                let ts = bar.timestamp.timestamp_nanos_opt().unwrap_or(0) / 1_000_000_000;
                let bucket_ts = self.config.interval.truncate_timestamp(ts);
                buckets.entry(bucket_ts).or_insert_with(Vec::new).push(bar);
            }

            // Aggregate each bucket into OHLC
            for (bucket_ts, bucket_bars) in buckets {
                let open = bucket_bars.first().map(|b| b.open).unwrap_or(0.0);
                let high = bucket_bars.iter().map(|b| b.high).fold(f64::NAN, f64::max);
                let low = bucket_bars.iter().map(|b| b.low).fold(f64::NAN, f64::min);
                let close = bucket_bars.last().map(|b| b.close).unwrap_or(0.0);
                let volume = bucket_bars.iter().map(|b| b.volume).sum();

                let dt = Utc
                    .timestamp_opt(bucket_ts, 0)
                    .single()
                    .unwrap_or_else(|| Utc::now());

                result.push(OHLCBar {
                    symbol: symbol.to_string(),
                    timestamp: dt,
                    open,
                    high,
                    low,
                    close,
                    volume,
                });
            }
        }

        // Sort by timestamp
        result.sort_by_key(|b| b.timestamp);

        Ok(result)
    }

    /// Downsample using average aggregation
    fn downsample_avg(&self, bars: &[OHLCBar]) -> Result<Vec<OHLCBar>> {
        // For simplicity, use close price as the value
        let mut by_symbol: HashMap<&str, HashMap<i64, Vec<f64>>> = HashMap::new();

        for bar in bars {
            let ts = bar.timestamp.timestamp_nanos_opt().unwrap_or(0) / 1_000_000_000;
            let bucket_ts = self.config.interval.truncate_timestamp(ts);
            by_symbol
                .entry(&bar.symbol)
                .or_insert_with(HashMap::new)
                .entry(bucket_ts)
                .or_insert_with(Vec::new)
                .push(bar.close);
        }

        let mut result = Vec::new();

        for (symbol, buckets) in by_symbol {
            for (bucket_ts, values) in buckets {
                let avg = values.iter().sum::<f64>() / values.len() as f64;
                let sum = values.len() as i64; // Use count as volume

                let dt = Utc
                    .timestamp_opt(bucket_ts, 0)
                    .single()
                    .unwrap_or_else(|| Utc::now());

                result.push(OHLCBar {
                    symbol: symbol.to_string(),
                    timestamp: dt,
                    open: avg,
                    high: avg,
                    low: avg,
                    close: avg,
                    volume: sum,
                });
            }
        }

        result.sort_by_key(|b| b.timestamp);
        Ok(result)
    }

    /// Downsample using sum aggregation
    fn downsample_sum(&self, bars: &[OHLCBar]) -> Result<Vec<OHLCBar>> {
        let mut by_symbol: HashMap<&str, HashMap<i64, Vec<&OHLCBar>>> = HashMap::new();

        for bar in bars {
            let ts = bar.timestamp.timestamp_nanos_opt().unwrap_or(0) / 1_000_000_000;
            let bucket_ts = self.config.interval.truncate_timestamp(ts);
            by_symbol
                .entry(&bar.symbol)
                .or_insert_with(HashMap::new)
                .entry(bucket_ts)
                .or_insert_with(Vec::new)
                .push(bar);
        }

        let mut result = Vec::new();

        for (symbol, buckets) in by_symbol {
            for (bucket_ts, bucket_bars) in buckets {
                let sum: f64 = bucket_bars.iter().map(|b| b.close).sum();
                let volume = bucket_bars.iter().map(|b| b.volume).sum();

                let dt = Utc
                    .timestamp_opt(bucket_ts, 0)
                    .single()
                    .unwrap_or_else(|| Utc::now());

                result.push(OHLCBar {
                    symbol: symbol.to_string(),
                    timestamp: dt,
                    open: sum,
                    high: sum,
                    low: sum,
                    close: sum,
                    volume,
                });
            }
        }

        result.sort_by_key(|b| b.timestamp);
        Ok(result)
    }

    /// Downsample using min/max aggregation
    fn downsample_minmax(&self, bars: &[OHLCBar]) -> Result<Vec<OHLCBar>> {
        let mut by_symbol: HashMap<&str, HashMap<i64, Vec<&OHLCBar>>> = HashMap::new();

        for bar in bars {
            let ts = bar.timestamp.timestamp_nanos_opt().unwrap_or(0) / 1_000_000_000;
            let bucket_ts = self.config.interval.truncate_timestamp(ts);
            by_symbol
                .entry(&bar.symbol)
                .or_insert_with(HashMap::new)
                .entry(bucket_ts)
                .or_insert_with(Vec::new)
                .push(bar);
        }

        let mut result = Vec::new();

        for (symbol, buckets) in by_symbol {
            for (bucket_ts, bucket_bars) in buckets {
                let min_val = bucket_bars
                    .iter()
                    .map(|b| b.low)
                    .fold(f64::INFINITY, f64::min);
                let max_val = bucket_bars
                    .iter()
                    .map(|b| b.high)
                    .fold(f64::NEG_INFINITY, f64::max);
                let volume = bucket_bars.iter().map(|b| b.volume).sum();

                let dt = Utc
                    .timestamp_opt(bucket_ts, 0)
                    .single()
                    .unwrap_or_else(|| Utc::now());

                result.push(OHLCBar {
                    symbol: symbol.to_string(),
                    timestamp: dt,
                    open: min_val,
                    high: max_val,
                    low: min_val,
                    close: max_val,
                    volume,
                });
            }
        }

        result.sort_by_key(|b| b.timestamp);
        Ok(result)
    }

    /// Downsample using first/last aggregation
    fn downsample_first_last(&self, bars: &[OHLCBar]) -> Result<Vec<OHLCBar>> {
        let mut by_symbol: HashMap<&str, HashMap<i64, Vec<&OHLCBar>>> = HashMap::new();

        for bar in bars {
            let ts = bar.timestamp.timestamp_nanos_opt().unwrap_or(0) / 1_000_000_000;
            let bucket_ts = self.config.interval.truncate_timestamp(ts);
            by_symbol
                .entry(&bar.symbol)
                .or_insert_with(HashMap::new)
                .entry(bucket_ts)
                .or_insert_with(Vec::new)
                .push(bar);
        }

        let mut result = Vec::new();

        for (symbol, buckets) in by_symbol {
            for (bucket_ts, bucket_bars) in buckets {
                let first = bucket_bars.first().map(|b| b.close).unwrap_or(0.0);
                let last = bucket_bars.last().map(|b| b.close).unwrap_or(0.0);
                let volume = bucket_bars.iter().map(|b| b.volume).sum();

                let dt = Utc
                    .timestamp_opt(bucket_ts, 0)
                    .single()
                    .unwrap_or_else(|| Utc::now());

                result.push(OHLCBar {
                    symbol: symbol.to_string(),
                    timestamp: dt,
                    open: first,
                    high: first.max(last),
                    low: first.min(last),
                    close: last,
                    volume,
                });
            }
        }

        result.sort_by_key(|b| b.timestamp);
        Ok(result)
    }

    /// Downsample using count aggregation
    fn downsample_count(&self, bars: &[OHLCBar]) -> Result<Vec<OHLCBar>> {
        let mut by_symbol: HashMap<&str, HashMap<i64, usize>> = HashMap::new();

        for bar in bars {
            let ts = bar.timestamp.timestamp_nanos_opt().unwrap_or(0) / 1_000_000_000;
            let bucket_ts = self.config.interval.truncate_timestamp(ts);
            *by_symbol
                .entry(&bar.symbol)
                .or_insert_with(HashMap::new)
                .entry(bucket_ts)
                .or_insert(0) += 1;
        }

        let mut result = Vec::new();

        for (symbol, buckets) in by_symbol {
            for (bucket_ts, count) in buckets {
                let dt = Utc
                    .timestamp_opt(bucket_ts, 0)
                    .single()
                    .unwrap_or_else(|| Utc::now());

                result.push(OHLCBar {
                    symbol: symbol.to_string(),
                    timestamp: dt,
                    open: count as f64,
                    high: count as f64,
                    low: count as f64,
                    close: count as f64,
                    volume: count as i64,
                });
            }
        }

        result.sort_by_key(|b| b.timestamp);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::impls::tst::partition::TimePartition;

    #[test]
    fn test_downsample_interval() {
        assert_eq!(DownsampleInterval::Minute.as_seconds(), 60);
        assert_eq!(DownsampleInterval::Hour.as_seconds(), 3600);
        assert_eq!(DownsampleInterval::Day.as_seconds(), 86400);
    }

    #[test]
    fn test_truncate_timestamp() {
        let interval = DownsampleInterval::Hour;

        // Use actual hour boundary
        let hour_start = 1705316400; // Exactly on an hour boundary
        let ts = hour_start + 1800 + 45; // + 30 min + 45 sec = 1705318845
        let truncated = interval.truncate_timestamp(ts);
        assert_eq!(truncated, hour_start);

        // Test: 10:45 into the hour
        let ts2 = hour_start + 600 + 45; // + 10 min + 45 sec
        let truncated2 = interval.truncate_timestamp(ts2);
        assert_eq!(truncated2, hour_start);

        // Test: exactly on the hour
        let truncated3 = interval.truncate_timestamp(hour_start);
        assert_eq!(truncated3, hour_start);
    }

    #[test]
    fn test_downsampler_creation() {
        let config = DownsampleConfig {
            interval: DownsampleInterval::Hour,
            aggregation: DownsampleAggregation::OHLC,
        };

        let downsampler = Downsampler::new(config);
        assert_eq!(downsampler.min_records, 100);
    }

    #[test]
    fn test_downsampler_with_min_records() {
        let config = DownsampleConfig {
            interval: DownsampleInterval::Minute,
            aggregation: DownsampleAggregation::OHLC,
        };

        let downsampler = Downsampler::new(config).with_min_records(50);
        assert_eq!(downsampler.min_records, 50);
    }
}
