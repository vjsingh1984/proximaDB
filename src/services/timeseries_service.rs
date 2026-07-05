// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! In-process service that surfaces the TST time-series engine (TD-TS-1).
//!
//! The `TimeSeriesEngine` (`crate::storage::engines::tst`) is a full time-partitioned
//! columnar engine, but its native methods sit on no trait and were unreachable from
//! the network layer. This service wraps it so the v2 REST timeseries surface
//! (`src/network/rest/v2/timeseries.rs`) can create collections, ingest points, and
//! query / aggregate — the contract the Python SDK already expects. It calls the
//! TST-native methods (`insert_record` / `query_time_range`) directly, never the
//! stubbed vector-shaped trait methods.

use crate::proto::proximadb_v1::{SqlValue, VectorRecord, sql_value};
use crate::storage::engines::tst::{TimeSeriesConfig, TimeSeriesEngine};
use anyhow::{Result, anyhow};
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

static TIMESERIES_SERVICE: OnceLock<Arc<TimeSeriesService>> = OnceLock::new();

/// Initialise the process-global time-series service (idempotent — first wins).
/// Called once at server bootstrap with `<data_dir>/timeseries` as the base path.
pub fn init_timeseries_service(base_path: PathBuf) -> Result<()> {
    if TIMESERIES_SERVICE.get().is_some() {
        return Ok(());
    }
    let service = Arc::new(TimeSeriesService::new(base_path)?);
    let _ = TIMESERIES_SERVICE.set(service);
    Ok(())
}

/// The process-global time-series service, if initialised.
pub fn timeseries_service() -> Option<Arc<TimeSeriesService>> {
    TIMESERIES_SERVICE.get().cloned()
}

/// A value column in a time-series collection (mirrors the SDK `ValueColumn`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsValueColumn {
    pub name: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub aggregation: Option<String>,
}

/// Time-series collection config (mirrors the SDK `TimeSeriesCollectionConfig`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsCollectionConfig {
    pub name: String,
    #[serde(default = "default_timestamp_column")]
    pub timestamp_column: String,
    #[serde(default)]
    pub value_columns: Vec<TsValueColumn>,
    #[serde(default)]
    pub tag_columns: Vec<String>,
    #[serde(default)]
    pub retention_ms: Option<i64>,
}

fn default_timestamp_column() -> String {
    "timestamp".to_string()
}

/// A single time-series point: epoch-millis timestamp + named numeric values + string tags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsPoint {
    pub timestamp: i64,
    #[serde(default)]
    pub values: HashMap<String, f64>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

/// Wraps a single `TimeSeriesEngine` (multiplexed by `collection_id`) plus a resident
/// registry of the declared collection configs.
pub struct TimeSeriesService {
    /// `insert_record` takes `&mut self`, so the engine is guarded for interior
    /// mutability (writes exclusive, `query_time_range` reads shared).
    engine: RwLock<TimeSeriesEngine>,
    collections: RwLock<HashMap<String, TsCollectionConfig>>,
}

impl TimeSeriesService {
    /// Build the service with the engine rooted at `base_path` (e.g. `<data_dir>/timeseries`).
    pub fn new(base_path: PathBuf) -> Result<Self> {
        let config = TimeSeriesConfig {
            base_path,
            ..Default::default()
        };
        let engine = TimeSeriesEngine::with_config(config)
            .map_err(|e| anyhow!("failed to init TimeSeriesEngine: {e}"))?;
        Ok(Self {
            engine: RwLock::new(engine),
            collections: RwLock::new(HashMap::new()),
        })
    }

    pub async fn create_collection(&self, config: TsCollectionConfig) -> Result<()> {
        self.collections
            .write()
            .await
            .insert(config.name.clone(), config);
        Ok(())
    }

    pub async fn list_collections(&self) -> Vec<TsCollectionConfig> {
        self.collections.read().await.values().cloned().collect()
    }

    pub async fn delete_collection(&self, name: &str) -> bool {
        self.collections.write().await.remove(name).is_some()
    }

    /// Ingest points → TST-native `insert_record` (values in metadata, timestamp partitioned).
    pub async fn ingest(&self, collection: &str, points: Vec<TsPoint>) -> Result<usize> {
        let mut engine = self.engine.write().await;
        let mut inserted = 0usize;
        for point in points {
            let ts = Utc
                .timestamp_millis_opt(point.timestamp)
                .single()
                .ok_or_else(|| anyhow!("invalid timestamp {}", point.timestamp))?;
            let mut metadata: HashMap<String, SqlValue> = HashMap::new();
            let mut vector: Vec<f32> = Vec::with_capacity(point.values.len());
            for (k, v) in &point.values {
                metadata.insert(
                    k.clone(),
                    SqlValue {
                        value: Some(sql_value::Value::NumberValue(*v)),
                    },
                );
                vector.push(*v as f32);
            }
            for (k, v) in &point.tags {
                metadata.insert(
                    k.clone(),
                    SqlValue {
                        value: Some(sql_value::Value::StringValue(v.clone())),
                    },
                );
            }
            let record = VectorRecord {
                id: format!("{collection}:{}", point.timestamp),
                vector,
                metadata,
                timestamp: Some(point.timestamp),
                ..Default::default()
            };
            engine
                .insert_record(collection, ts, record)
                .await
                .map_err(|e| anyhow!("insert_record failed: {e}"))?;
            inserted += 1;
        }
        Ok(inserted)
    }

    /// Query a time range → raw points (TST-native `query_time_range`).
    pub async fn query(
        &self,
        collection: &str,
        start_ms: i64,
        end_ms: i64,
        limit: Option<usize>,
    ) -> Result<Vec<TsPoint>> {
        let start = Utc
            .timestamp_millis_opt(start_ms)
            .single()
            .ok_or_else(|| anyhow!("invalid start_time"))?;
        let end = Utc
            .timestamp_millis_opt(end_ms)
            .single()
            .ok_or_else(|| anyhow!("invalid end_time"))?;
        let records = self
            .engine
            .read()
            .await
            .query_time_range(collection, start, end, limit)
            .await
            .map_err(|e| anyhow!("query_time_range failed: {e}"))?;
        Ok(records.into_iter().map(record_to_point).collect())
    }

    /// Aggregate a time range into fixed `bucket_ms` buckets, applying `aggregation`
    /// (avg | sum | min | max | count | stddev | first | last) to each value column.
    pub async fn aggregate(
        &self,
        collection: &str,
        start_ms: i64,
        end_ms: i64,
        aggregation: &str,
        bucket_ms: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let points = self.query(collection, start_ms, end_ms, None).await?;
        let bucket = bucket_ms.max(1);
        let mut buckets: BTreeMap<i64, HashMap<String, Vec<f64>>> = BTreeMap::new();
        for point in points {
            let key = (point.timestamp / bucket) * bucket;
            let cols = buckets.entry(key).or_default();
            for (name, value) in point.values {
                cols.entry(name).or_default().push(value);
            }
        }
        Ok(buckets
            .into_iter()
            .map(|(bucket_start, cols)| {
                let values: serde_json::Map<String, serde_json::Value> = cols
                    .into_iter()
                    .map(|(name, vals)| {
                        (
                            name,
                            serde_json::json!(apply_aggregation(aggregation, &vals)),
                        )
                    })
                    .collect();
                serde_json::json!({ "bucket_start": bucket_start, "values": values })
            })
            .collect())
    }
}

fn record_to_point(record: VectorRecord) -> TsPoint {
    let mut values = HashMap::new();
    let mut tags = HashMap::new();
    for (k, sv) in record.metadata {
        match sv.value {
            Some(sql_value::Value::NumberValue(n)) => {
                values.insert(k, n);
            }
            Some(sql_value::Value::Int64Value(i)) => {
                values.insert(k, i as f64);
            }
            Some(sql_value::Value::StringValue(s)) => {
                tags.insert(k, s);
            }
            _ => {}
        }
    }
    TsPoint {
        timestamp: record.timestamp.unwrap_or(0),
        values,
        tags,
    }
}

fn apply_aggregation(aggregation: &str, vals: &[f64]) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    let n = vals.len() as f64;
    match aggregation {
        "sum" => vals.iter().sum(),
        "min" => vals.iter().cloned().fold(f64::INFINITY, f64::min),
        "max" => vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        "count" => n,
        "first" => vals[0],
        "last" => *vals.last().unwrap_or(&0.0),
        "stddev" => {
            let mean = vals.iter().sum::<f64>() / n;
            (vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt()
        }
        // "avg" / "mean" and anything else → mean
        _ => vals.iter().sum::<f64>() / n,
    }
}
