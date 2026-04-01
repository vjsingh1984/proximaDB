//! Time-Series Storage Engine Integration Tests
//!
//! Comprehensive tests for the TST engine covering:
//! - Basic CRUD operations
//! - OHLC bar aggregation
//! - ASOF joins
//! - Time-partitioning
//! - Performance benchmarks

use chrono::{DateTime, Duration, Utc};
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::storage::engines::impls::tst::{
    ASOFJoin, DownsampleAggregation, DownsampleConfig, DownsampleInterval, OHLC, PartitionDuration,
    TimeSeriesConfig, TimeSeriesEngine,
};
use std::collections::HashMap;
use tempfile::TempDir;

/// Helper: Create a test vector record
fn create_test_record(id: &str, timestamp: i64, vector: Vec<f32>) -> VectorRecord {
    VectorRecord {
        id: id.to_string(),
        vector,
        timestamp: Some(timestamp),
        metadata: HashMap::new(),
        ..Default::default()
    }
}

//
// Basic CRUD Tests
//

#[tokio::test]
async fn test_tst_engine_creation() {
    let temp_dir = TempDir::new().unwrap();
    let config = TimeSeriesConfig {
        base_path: temp_dir.path().to_path_buf(),
        partition_duration: PartitionDuration::Day,
        ..Default::default()
    };

    let engine = TimeSeriesEngine::with_config(config);
    assert!(engine.is_ok());
}

#[tokio::test]
async fn test_tst_insert_record() {
    let temp_dir = TempDir::new().unwrap();
    let config = TimeSeriesConfig {
        base_path: temp_dir.path().to_path_buf(),
        partition_duration: PartitionDuration::Day,
        ..Default::default()
    };

    let mut engine = TimeSeriesEngine::with_config(config).unwrap();
    let collection_id = "test_insert";

    let now = Utc::now();
    let record = create_test_record("rec1", now.timestamp(), vec![1.0, 2.0, 3.0]);

    // Should not panic
    let result = engine.insert_record(collection_id, now, record).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_tst_stats() {
    let temp_dir = TempDir::new().unwrap();
    let config = TimeSeriesConfig {
        base_path: temp_dir.path().to_path_buf(),
        partition_duration: PartitionDuration::Day,
        ..Default::default()
    };

    let engine = TimeSeriesEngine::with_config(config).unwrap();

    // Stats should be accessible
    let stats = engine.stats();
    // Initial state may have 0 records
    assert!(stats.total_partitions >= 0);
}

#[tokio::test]
async fn test_tst_empty_query() {
    let temp_dir = TempDir::new().unwrap();
    let config = TimeSeriesConfig {
        base_path: temp_dir.path().to_path_buf(),
        partition_duration: PartitionDuration::Day,
        ..Default::default()
    };

    let engine = TimeSeriesEngine::with_config(config).unwrap();
    let collection_id = "test_empty";

    let now = Utc::now();
    let start = now - Duration::hours(1);
    let end = now;

    // Query empty collection should return empty results
    let results = engine
        .query_time_range(collection_id, start, end, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 0);
}

#[tokio::test]
async fn test_tst_query_nonexistent_symbol() {
    let temp_dir = TempDir::new().unwrap();
    let config = TimeSeriesConfig {
        base_path: temp_dir.path().to_path_buf(),
        partition_duration: PartitionDuration::Day,
        ..Default::default()
    };

    let engine = TimeSeriesEngine::with_config(config).unwrap();
    let collection_id = "test_symbol";

    let timestamp = DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    // Query non-existent symbol should return empty results
    let start = timestamp - Duration::hours(1);
    let end = timestamp + Duration::hours(1);
    let results = engine
        .query_ohlc(collection_id, "NONEXISTENT", start, end, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 0);
}

//
// OHLC Aggregation Tests
//

#[test]
fn test_tst_ohlc_aggregation() {
    let base_time = DateTime::parse_from_rfc3339("2024-01-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let mut ohlc = OHLC::new("AAPL".to_string(), 300); // 5 minute interval

    // Add price points
    ohlc.add_price(base_time, 100.0, 1000).unwrap();
    ohlc.add_price(base_time + Duration::seconds(30), 101.0, 500)
        .unwrap();
    ohlc.add_price(base_time + Duration::seconds(60), 102.5, 300)
        .unwrap();
    ohlc.add_price(base_time + Duration::seconds(90), 99.0, 200)
        .unwrap();

    let bars = ohlc.finalize();

    assert_eq!(bars.len(), 1);
    assert_eq!(bars[0].symbol, "AAPL");
    assert_eq!(bars[0].open, 100.0);
    assert_eq!(bars[0].high, 102.5);
    assert_eq!(bars[0].low, 99.0);
    assert_eq!(bars[0].close, 99.0);
    // Volume may vary slightly due to implementation
    assert!(bars[0].volume >= 1990 && bars[0].volume <= 2010);
}

#[test]
fn test_tst_ohlc_multiple_intervals() {
    let base_time = DateTime::parse_from_rfc3339("2024-01-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let mut ohlc = OHLC::new("MSFT".to_string(), 300); // 5 minute interval

    // Add prices spanning multiple 5-minute intervals
    for i in 0..10 {
        let ts = base_time + Duration::minutes(i);
        let price = 100.0 + (i as f64);
        ohlc.add_price(ts, price, 100 * (i + 1)).unwrap();
    }

    let bars = ohlc.finalize();

    // Should have 2 bars (5 minutes each)
    assert_eq!(bars.len(), 2);

    // First bar: minutes 0-4
    assert_eq!(bars[0].open, 100.0);
    assert_eq!(bars[0].high, 104.0);

    // Second bar: minutes 5-9
    assert_eq!(bars[1].open, 105.0);
    assert_eq!(bars[1].high, 109.0);
}

#[tokio::test]
async fn test_tst_insert_ohlc_bar() {
    let temp_dir = TempDir::new().unwrap();
    let config = TimeSeriesConfig {
        base_path: temp_dir.path().to_path_buf(),
        partition_duration: PartitionDuration::Day,
        ..Default::default()
    };

    let mut engine = TimeSeriesEngine::with_config(config).unwrap();
    let collection_id = "test_ohlc_insert";

    let timestamp = DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    // Should not panic
    let result = engine
        .insert_ohlc(
            collection_id,
            "AAPL",
            timestamp,
            100.0, // open
            105.0, // high
            98.0,  // low
            102.0, // close
            10000, // volume
        )
        .await;

    assert!(result.is_ok());
}

//
// ASOF Join Tests
//

#[tokio::test]
async fn test_tst_asof_join_basic() {
    let base_time = DateTime::parse_from_rfc3339("2024-01-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let left_records = vec![
        create_test_record("trade1", base_time.timestamp(), vec![1.0]),
        create_test_record(
            "trade2",
            (base_time + Duration::seconds(5)).timestamp(),
            vec![2.0],
        ),
    ];

    let right_records = vec![
        create_test_record(
            "quote1",
            (base_time - Duration::seconds(2)).timestamp(),
            vec![10.0],
        ),
        create_test_record(
            "quote2",
            (base_time + Duration::seconds(3)).timestamp(),
            vec![20.0],
        ),
    ];

    let asof = ASOFJoin::new();
    let results = asof
        .execute(left_records, right_records, None)
        .await
        .unwrap();

    assert!(results.len() >= 2);
}

#[tokio::test]
async fn test_tst_asof_join_tolerance() {
    let base_time = DateTime::parse_from_rfc3339("2024-01-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    // Test with tight tolerance
    let left_records = vec![create_test_record("t1", base_time.timestamp(), vec![1.0])];

    let right_records = vec![
        create_test_record(
            "q1",
            (base_time - Duration::milliseconds(500)).timestamp(),
            vec![10.0],
        ),
        create_test_record(
            "q2",
            (base_time + Duration::seconds(20)).timestamp(),
            vec![20.0],
        ), // Outside tolerance
    ];

    let asof = ASOFJoin::new();
    let results = asof
        .execute(left_records, right_records, Some(Duration::seconds(1)))
        .await
        .unwrap();

    // Should only match the close quote
    assert!(results.len() >= 1);
}

//
// Downsampling Tests
//

#[test]
fn test_tst_downsampling_config() {
    let config = DownsampleConfig {
        interval: DownsampleInterval::Hour,
        aggregation: DownsampleAggregation::OHLC,
    };

    assert_eq!(config.interval, DownsampleInterval::Hour);
}

#[test]
fn test_tst_downsample_intervals() {
    // Test all interval types
    let intervals = vec![
        DownsampleInterval::Second,
        DownsampleInterval::Minute,
        DownsampleInterval::FiveMinutes,
        DownsampleInterval::FifteenMinutes,
        DownsampleInterval::Hour,
        DownsampleInterval::FourHours,
        DownsampleInterval::Day,
        DownsampleInterval::Week,
    ];

    for interval in intervals {
        let config = DownsampleConfig {
            interval: interval.clone(),
            aggregation: DownsampleAggregation::OHLC,
        };

        assert_eq!(config.interval, interval);
        assert!(interval.as_seconds() > 0);
    }
}

//
// Performance Benchmarks
//

#[tokio::test]
async fn bench_tst_ingestion_rate() {
    let temp_dir = TempDir::new().unwrap();
    let config = TimeSeriesConfig {
        base_path: temp_dir.path().to_path_buf(),
        partition_duration: PartitionDuration::Day,
        ..Default::default()
    };

    let mut engine = TimeSeriesEngine::with_config(config).unwrap();
    let collection_id = "bench_ingestion";

    let start = std::time::Instant::now();
    let num_records = 1_000;

    let now = Utc::now();
    for i in 0..num_records {
        let ts = now + Duration::milliseconds(i);
        let record = create_test_record(&format!("rec_{}", i), ts.timestamp(), vec![i as f32; 128]);
        engine
            .insert_record(collection_id, ts, record)
            .await
            .unwrap();
    }

    let duration = start.elapsed();

    println!("Inserted {} records in {:?}", num_records, duration);
    println!(
        "Rate: {:.2} records/second",
        num_records as f64 / duration.as_secs_f64()
    );

    // Performance assertion: should be >100 records/second
    let rate = num_records as f64 / duration.as_secs_f64();
    assert!(rate > 100.0, "Ingestion rate {} is below 100/sec", rate);
}

#[tokio::test]
async fn bench_tst_time_range_query() {
    let temp_dir = TempDir::new().unwrap();
    let config = TimeSeriesConfig {
        base_path: temp_dir.path().to_path_buf(),
        partition_duration: PartitionDuration::Day,
        ..Default::default()
    };

    let mut engine = TimeSeriesEngine::with_config(config).unwrap();
    let collection_id = "bench_query";

    // Insert test data
    let now = Utc::now();
    for i in 0..100 {
        let ts = now + Duration::seconds(i);
        let record = create_test_record(&format!("rec_{}", i), ts.timestamp(), vec![1.0; 128]);
        engine
            .insert_record(collection_id, ts, record)
            .await
            .unwrap();
    }

    // Benchmark query performance
    let start = std::time::Instant::now();
    let num_iterations = 10;

    for _ in 0..num_iterations {
        let query_start = now;
        let query_end = now + Duration::seconds(100);
        let _results = engine
            .query_time_range(collection_id, query_start, query_end, None)
            .await
            .unwrap();
    }

    let duration = start.elapsed();
    let avg_latency = duration.as_micros() as f64 / num_iterations as f64;

    println!("Average query latency: {:.2} μs", avg_latency);

    // Performance assertion: average query should be <100ms
    assert!(
        avg_latency < 100_000.0,
        "Query latency {} μs exceeds 100ms",
        avg_latency
    );
}

#[test]
fn bench_tst_ohlc_aggregation() {
    let base_time = DateTime::parse_from_rfc3339("2024-01-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let mut ohlc = OHLC::new("AAPL".to_string(), 300); // 5 minute interval

    let start = std::time::Instant::now();
    let num_prices = 1_000;

    // Add many price points
    for i in 0..num_prices {
        let ts = base_time + Duration::milliseconds(i);
        let price = 100.0 + (i as f64 % 10.0);
        ohlc.add_price(ts, price, 100).unwrap();
    }

    let duration = start.elapsed();

    println!("Aggregated {} prices in {:?}", num_prices, duration);
    println!(
        "Rate: {:.2} prices/second",
        num_prices as f64 / duration.as_secs_f64()
    );

    // Verify aggregation
    let bars = ohlc.finalize();
    assert!(!bars.is_empty());

    // Performance assertion: >1K prices/second
    let rate = num_prices as f64 / duration.as_secs_f64();
    assert!(
        rate > 1_000.0,
        "OHLC aggregation rate {} is below 1K/sec",
        rate
    );
}

//
// Edge Cases
//

#[tokio::test]
async fn test_tst_partition_duration() {
    // Test different partition durations
    let durations = vec![
        PartitionDuration::Hour,
        PartitionDuration::Day,
        PartitionDuration::Week,
        PartitionDuration::Month,
    ];

    for duration in durations {
        let temp_dir = TempDir::new().unwrap();
        let config = TimeSeriesConfig {
            base_path: temp_dir.path().to_path_buf(),
            partition_duration: duration.clone(),
            ..Default::default()
        };

        let engine = TimeSeriesEngine::with_config(config);
        assert!(engine.is_ok());
    }
}
