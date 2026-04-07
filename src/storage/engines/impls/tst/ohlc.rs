//! OHLC (Open, High, Low, Close) Module
//!
//! Provides OHLC bar aggregation for trading and financial time-series data.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// OHLC bar data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OHLCBar {
    /// Symbol/security identifier
    pub symbol: String,

    /// Bar timestamp
    pub timestamp: DateTime<Utc>,

    /// Opening price
    pub open: f64,

    /// Highest price
    pub high: f64,

    /// Lowest price
    pub low: f64,

    /// Closing price
    pub close: f64,

    /// Trading volume
    pub volume: i64,
}

impl OHLCBar {
    /// Create a new OHLC bar from a single price
    pub fn from_price(symbol: String, timestamp: DateTime<Utc>, price: f64) -> Self {
        Self {
            symbol,
            timestamp,
            open: price,
            high: price,
            low: price,
            close: price,
            volume: 1,
        }
    }

    /// Update this OHLC bar with a new price
    pub fn update(&mut self, price: f64, volume: i64) {
        self.high = self.high.max(price);
        self.low = self.low.min(price);
        self.close = price;
        self.volume += volume;
    }

    /// Merge another OHLC bar into this one
    pub fn merge(&mut self, other: &OHLCBar) {
        self.high = self.high.max(other.high);
        self.low = self.low.min(other.low);
        self.close = other.close;
        self.volume += other.volume;
    }
}

/// OHLC aggregator
///
/// Aggregates price data into OHLC bars for specified time intervals.
#[derive(Debug, Clone)]
pub struct OHLC {
    /// Symbol being aggregated
    symbol: String,

    /// Current bars being built
    /// Key: timestamp truncated to aggregation interval
    current_bars: BTreeMap<DateTime<Utc>, OHLCBar>,

    /// Aggregation interval in seconds
    interval_seconds: i64,
}

use std::collections::BTreeMap;

impl OHLC {
    /// Create a new OHLC aggregator
    pub fn new(symbol: String, interval_seconds: i64) -> Self {
        Self {
            symbol,
            current_bars: BTreeMap::new(),
            interval_seconds,
        }
    }

    /// Add a price point to the aggregator
    pub fn add_price(
        &mut self,
        timestamp: DateTime<Utc>,
        price: f64,
        volume: i64,
    ) -> anyhow::Result<()> {
        // Truncate timestamp to interval
        let truncated = self.truncate_to_interval(timestamp);

        // Get or create bar for this interval
        let bar = self
            .current_bars
            .entry(truncated)
            .or_insert_with(|| OHLCBar::from_price(self.symbol.clone(), truncated, price));

        // Update bar
        bar.update(price, volume);

        Ok(())
    }

    /// Get the current OHLC bar for a timestamp
    pub fn get_bar(&self, timestamp: DateTime<Utc>) -> Option<&OHLCBar> {
        let truncated = self.truncate_to_interval(timestamp);
        self.current_bars.get(&truncated)
    }

    /// Get all completed bars
    pub fn get_all_bars(&self) -> Vec<&OHLCBar> {
        self.current_bars.values().collect()
    }

    /// Get bars in a time range
    pub fn get_bars_in_range(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<&OHLCBar> {
        self.current_bars
            .range(start..=end)
            .map(|(_, bar)| bar)
            .collect()
    }

    /// Clear bars older than a timestamp
    pub fn clear_before(&mut self, timestamp: DateTime<Utc>) {
        self.current_bars = self.current_bars.split_off(&timestamp);
    }

    /// Clear all bars
    pub fn clear(&mut self) {
        self.current_bars.clear();
    }

    /// Truncate timestamp to aggregation interval
    fn truncate_to_interval(&self, timestamp: DateTime<Utc>) -> DateTime<Utc> {
        let ts = timestamp.timestamp();
        let truncated_ts = (ts / self.interval_seconds) * self.interval_seconds;
        DateTime::from_timestamp(truncated_ts, 0).unwrap_or(timestamp)
    }

    /// Finalize and return all bars, consuming the aggregator
    pub fn finalize(self) -> Vec<OHLCBar> {
        self.current_bars.into_values().collect()
    }
}

/// OHLC query
#[derive(Debug, Clone)]
pub struct OHLCQuery {
    /// Symbol to query
    pub symbol: String,

    /// Start time
    pub start: DateTime<Utc>,

    /// End time
    pub end: DateTime<Utc>,

    /// Aggregation interval in seconds
    pub interval_seconds: Option<i64>,

    /// Maximum number of bars to return
    pub limit: Option<usize>,
}

impl OHLCQuery {
    /// Create a new OHLC query
    pub fn new(symbol: String, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self {
            symbol,
            start,
            end,
            interval_seconds: None,
            limit: None,
        }
    }

    /// Set aggregation interval
    pub fn with_interval(mut self, interval_seconds: i64) -> Self {
        self.interval_seconds = Some(interval_seconds);
        self
    }

    /// Set limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// OHLC aggregation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OHLCResult {
    /// Bars returned
    pub bars: Vec<OHLCBar>,

    /// Total bars available in range
    pub total_count: usize,

    /// Whether more bars are available beyond the limit
    pub has_more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn test_ohlc_bar_from_price() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
            .expect("valid RFC3339 timestamp")
            .with_timezone(&Utc);

        let bar = OHLCBar::from_price("AAPL".to_string(), timestamp, 100.0);

        assert_eq!(bar.open, 100.0);
        assert_eq!(bar.high, 100.0);
        assert_eq!(bar.low, 100.0);
        assert_eq!(bar.close, 100.0);
        assert_eq!(bar.volume, 1);
    }

    #[test]
    fn test_ohlc_bar_update() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
            .expect("valid RFC3339 timestamp")
            .with_timezone(&Utc);

        let mut bar = OHLCBar::from_price("AAPL".to_string(), timestamp, 100.0);

        bar.update(105.0, 100);
        bar.update(98.0, 50);

        assert_eq!(bar.open, 100.0);
        assert_eq!(bar.high, 105.0);
        assert_eq!(bar.low, 98.0);
        assert_eq!(bar.close, 98.0);
        assert_eq!(bar.volume, 151);
    }

    #[test]
    fn test_ohlc_aggregator() {
        let mut aggregator = OHLC::new("AAPL".to_string(), 3600); // 1-hour intervals

        let t1 = DateTime::parse_from_rfc3339("2024-01-01T10:30:00Z")
            .expect("valid RFC3339 timestamp")
            .with_timezone(&Utc);
        let t2 = DateTime::parse_from_rfc3339("2024-01-01T10:45:00Z")
            .expect("valid RFC3339 timestamp")
            .with_timezone(&Utc);
        let t3 = DateTime::parse_from_rfc3339("2024-01-01T11:15:00Z")
            .expect("valid RFC3339 timestamp")
            .with_timezone(&Utc);

        aggregator
            .add_price(t1, 100.0, 100)
            .expect("add_price should succeed");
        aggregator
            .add_price(t2, 102.0, 50)
            .expect("add_price should succeed");
        aggregator
            .add_price(t3, 101.0, 75)
            .expect("add_price should succeed");

        // Should have 2 bars (10:00-11:00 and 11:00-12:00)
        let bars = aggregator.get_all_bars();
        assert_eq!(bars.len(), 2);

        // First bar should have high of 102
        let first_bar = bars
            .iter()
            .find(|b| b.timestamp.hour() == 10)
            .expect("first bar at hour 10 should exist");
        assert_eq!(first_bar.high, 102.0);

        // Second bar should have close of 101
        let second_bar = bars
            .iter()
            .find(|b| b.timestamp.hour() == 11)
            .expect("second bar at hour 11 should exist");
        assert_eq!(second_bar.close, 101.0);
    }

    #[test]
    fn test_ohlc_query() {
        let start = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .expect("valid RFC3339 timestamp")
            .with_timezone(&Utc);
        let end = DateTime::parse_from_rfc3339("2024-01-01T23:59:59Z")
            .expect("valid RFC3339 timestamp")
            .with_timezone(&Utc);

        let query = OHLCQuery::new("AAPL".to_string(), start, end)
            .with_interval(3600)
            .with_limit(10);

        assert_eq!(query.symbol, "AAPL");
        assert_eq!(query.interval_seconds, Some(3600));
        assert_eq!(query.limit, Some(10));
    }
}
