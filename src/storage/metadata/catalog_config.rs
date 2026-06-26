//! Neutral, wire-independent persistence of per-index and quantization config.
//!
//! The collection catalog drops everything except the coarse shape (dimension,
//! distance metric, storage engine) when it serializes a collection, so a
//! `GetCollection` after `CreateCollection` returned `m=0`, `is_primary=false`,
//! and quantization disabled even though the request set them (TD-122). This
//! affects both persistence layers:
//!
//! * the `MetadataStore` WAL bag (`HashMap<String, serde_json::Value>`), and
//! * the xCatalog table-asset properties bag (`HashMap<String, String>`), which
//!   is the read-authoritative source on the live (catalog-backed) path.
//!
//! Rather than serialize the v1 wire protos straight into storage (which would
//! couple the internal catalog to a frozen wire type), we persist small
//! **neutral** structs owned by the storage layer. The v1 proto types appear
//! only at the read boundary, where the rest of the catalog pipeline still
//! consumes them — the *stored* representation stays decoupled from the wire.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::proto::proximadb_v1::{
    CollectionConfig, HnswConfig, IndexConfig, IndexPolicy, IvfConfig, QuantizationConfig,
    RecordSchemaConfig, TextStorageConfig,
};

/// Catalog-bag key holding the neutral per-index config array.
pub const INDEX_CONFIGS_KEY: &str = "index_configs";
/// Catalog-bag key holding the neutral quantization config.
pub const QUANTIZATION_KEY: &str = "quantization";
/// Catalog-bag key holding the neutral index routing policy (ADR-028).
pub const INDEX_POLICY_KEY: &str = "index_policy";

/// Neutral HNSW parameters retained across a catalog round-trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredHnsw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    m: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ef_construction: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ef_search: Option<u32>,
}

/// Neutral IVF parameters retained across a catalog round-trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredIvf {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n_lists: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n_probe: Option<u32>,
}

/// Neutral per-index config retained across a catalog round-trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredIndex {
    index_name: String,
    /// v1 `IndexingAlgorithm` enum value (a stable scalar, not a wire message).
    algorithm: i32,
    #[serde(default)]
    is_primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hnsw: Option<StoredHnsw>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ivf: Option<StoredIvf>,
}

/// Neutral quantization config retained across a catalog round-trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredQuant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    /// v1 `QuantizationConfig.Strategy` enum value (a stable scalar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    strategy: Option<i32>,
}

/// Neutral index routing policy retained across a catalog round-trip (ADR-028).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredIndexPolicy {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    rehydrate: String,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    byte_budget: u64,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    nprobe: u32,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

// --- wire <-> neutral conversions (the only place the v1 proto is touched) ---

fn to_stored_indexes(config: &CollectionConfig) -> Vec<StoredIndex> {
    config
        .index_configs
        .iter()
        .map(|ic| StoredIndex {
            index_name: ic.index_name.clone(),
            algorithm: ic.algorithm,
            is_primary: ic.is_primary.unwrap_or(false),
            hnsw: ic.hnsw_config.as_ref().map(|h| StoredHnsw {
                m: h.m,
                ef_construction: h.ef_construction,
                ef_search: h.ef_search,
            }),
            ivf: ic.ivf_config.as_ref().map(|i| StoredIvf {
                n_lists: i.n_lists,
                n_probe: i.n_probe,
            }),
        })
        .collect()
}

fn from_stored_index(s: StoredIndex) -> IndexConfig {
    IndexConfig {
        index_name: s.index_name,
        algorithm: s.algorithm,
        hnsw_config: s.hnsw.map(|h| HnswConfig {
            m: h.m,
            ef_construction: h.ef_construction,
            ef_search: h.ef_search,
            ..Default::default()
        }),
        ivf_config: s.ivf.map(|i| IvfConfig {
            n_lists: i.n_lists,
            n_probe: i.n_probe,
            ..Default::default()
        }),
        is_primary: Some(s.is_primary),
        ..Default::default()
    }
}

fn to_stored_quant(config: &CollectionConfig) -> Option<StoredQuant> {
    config.quantization.as_ref().map(|q| StoredQuant {
        enabled: q.enabled,
        strategy: q.strategy,
    })
}

fn from_stored_quant(s: StoredQuant) -> QuantizationConfig {
    QuantizationConfig {
        enabled: s.enabled,
        strategy: s.strategy,
        ..Default::default()
    }
}

fn to_stored_index_policy(config: &CollectionConfig) -> Option<StoredIndexPolicy> {
    config.index_policy.as_ref().map(|p| StoredIndexPolicy {
        mode: p.mode.clone(),
        rehydrate: p.rehydrate.clone(),
        byte_budget: p.byte_budget,
        nprobe: p.nprobe,
    })
}

fn from_stored_index_policy(s: StoredIndexPolicy) -> IndexPolicy {
    IndexPolicy {
        mode: s.mode,
        rehydrate: s.rehydrate,
        byte_budget: s.byte_budget,
        nprobe: s.nprobe,
    }
}

// --- JSON `Value` bag API (MetadataStore WAL) ---

/// Serialize the per-index and quantization config into the neutral `Value`
/// catalog bag. No-op for fields that are absent.
pub(crate) fn write_index_and_quant(
    config: &CollectionConfig,
    map: &mut HashMap<String, Value>,
) -> Result<()> {
    if !config.index_configs.is_empty() {
        let stored = to_stored_indexes(config);
        map.insert(
            INDEX_CONFIGS_KEY.to_string(),
            serde_json::to_value(&stored)
                .map_err(|e| anyhow::anyhow!("serialize index_configs for catalog: {e}"))?,
        );
    }
    if let Some(stored) = to_stored_quant(config) {
        map.insert(
            QUANTIZATION_KEY.to_string(),
            serde_json::to_value(&stored)
                .map_err(|e| anyhow::anyhow!("serialize quantization for catalog: {e}"))?,
        );
    }
    if let Some(stored) = to_stored_index_policy(config) {
        map.insert(
            INDEX_POLICY_KEY.to_string(),
            serde_json::to_value(&stored)
                .map_err(|e| anyhow::anyhow!("serialize index_policy for catalog: {e}"))?,
        );
    }
    Ok(())
}

/// Reconstruct the per-index config (as wire `IndexConfig`) from the neutral
/// `Value` bag. Empty when nothing was persisted; a corrupt entry is ignored.
pub(crate) fn read_index_configs(map: &HashMap<String, Value>) -> Vec<IndexConfig> {
    let Some(value) = map.get(INDEX_CONFIGS_KEY) else {
        return Vec::new();
    };
    match serde_json::from_value::<Vec<StoredIndex>>(value.clone()) {
        Ok(stored) => stored.into_iter().map(from_stored_index).collect(),
        Err(e) => {
            tracing::warn!("⚠️ catalog index_configs decode failed, ignoring: {e}");
            Vec::new()
        }
    }
}

/// Reconstruct the quantization config (as wire `QuantizationConfig`) from the
/// neutral `Value` bag, or `None` when nothing was persisted.
pub(crate) fn read_quantization(map: &HashMap<String, Value>) -> Option<QuantizationConfig> {
    let value = map.get(QUANTIZATION_KEY)?;
    match serde_json::from_value::<StoredQuant>(value.clone()) {
        Ok(stored) => Some(from_stored_quant(stored)),
        Err(e) => {
            tracing::warn!("⚠️ catalog quantization decode failed, ignoring: {e}");
            None
        }
    }
}

/// Reconstruct the index routing policy (as wire `IndexPolicy`) from the neutral
/// `Value` bag, or `None` when nothing was persisted (ADR-028). A corrupt entry
/// is ignored (defaults to mode=auto downstream).
pub(crate) fn read_index_policy(map: &HashMap<String, Value>) -> Option<IndexPolicy> {
    let value = map.get(INDEX_POLICY_KEY)?;
    match serde_json::from_value::<StoredIndexPolicy>(value.clone()) {
        Ok(stored) => Some(from_stored_index_policy(stored)),
        Err(e) => {
            tracing::warn!("⚠️ catalog index_policy decode failed, ignoring: {e}");
            None
        }
    }
}

// --- JSON-string API (xCatalog table-asset `String` properties bag) ---

/// Serialize the per-index config to a neutral JSON string for the catalog
/// properties bag, or `None` when there are no indexes to persist.
pub(crate) fn index_configs_to_json(config: &CollectionConfig) -> Result<Option<String>> {
    if config.index_configs.is_empty() {
        return Ok(None);
    }
    let stored = to_stored_indexes(config);
    serde_json::to_string(&stored)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("serialize index_configs for catalog asset: {e}"))
}

/// Serialize the quantization config to a neutral JSON string, or `None`.
pub(crate) fn quantization_to_json(config: &CollectionConfig) -> Result<Option<String>> {
    match to_stored_quant(config) {
        Some(stored) => serde_json::to_string(&stored)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("serialize quantization for catalog asset: {e}")),
        None => Ok(None),
    }
}

/// Reconstruct the per-index config from a neutral JSON string. Empty when the
/// string is unparseable (legacy/corrupt) so the caller can fall back.
pub(crate) fn index_configs_from_json(json: &str) -> Vec<IndexConfig> {
    match serde_json::from_str::<Vec<StoredIndex>>(json) {
        Ok(stored) => stored.into_iter().map(from_stored_index).collect(),
        Err(e) => {
            tracing::warn!("⚠️ catalog-asset index_configs decode failed, ignoring: {e}");
            Vec::new()
        }
    }
}

/// Reconstruct the quantization config from a neutral JSON string, or `None`.
pub(crate) fn quantization_from_json(json: &str) -> Option<QuantizationConfig> {
    match serde_json::from_str::<StoredQuant>(json) {
        Ok(stored) => Some(from_stored_quant(stored)),
        Err(e) => {
            tracing::warn!("⚠️ catalog-asset quantization decode failed, ignoring: {e}");
            None
        }
    }
}

/// Serialize the index routing policy to a neutral JSON string, or `None`
/// (ADR-028).
pub(crate) fn index_policy_to_json(config: &CollectionConfig) -> Result<Option<String>> {
    match to_stored_index_policy(config) {
        Some(stored) => serde_json::to_string(&stored)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("serialize index_policy for catalog asset: {e}")),
        None => Ok(None),
    }
}

/// Reconstruct the index routing policy from a neutral JSON string, or `None`
/// (ADR-028).
pub(crate) fn index_policy_from_json(json: &str) -> Option<IndexPolicy> {
    match serde_json::from_str::<StoredIndexPolicy>(json) {
        Ok(stored) => Some(from_stored_index_policy(stored)),
        Err(e) => {
            tracing::warn!("⚠️ catalog-asset index_policy decode failed, ignoring: {e}");
            None
        }
    }
}

// --- ProximaRecord schema (enable flag + enforcement + text columns) ---

/// Neutral text-column sidecar config retained across a catalog round-trip
/// (only the fields the read path reconstructs).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredTextStorage {
    column_name: String,
    chunk_size: u32,
}

/// Neutral ProximaRecord schema config retained across a catalog round-trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredSchema {
    enable_proxima_record: bool,
    enforcement: i32,
    auto_evolve: bool,
    schema_id: String,
    schema_version: String,
    #[serde(default)]
    text_columns: Vec<String>,
    #[serde(default)]
    text_storage: Vec<StoredTextStorage>,
}

/// Serialize the ProximaRecord schema config (enable flag, enforcement, text
/// columns) to a neutral JSON string, or `None` when ProximaRecord is unset.
pub(crate) fn record_schema_to_json(config: &CollectionConfig) -> Result<Option<String>> {
    if !config.enable_proxima_record.unwrap_or(false) && config.record_schema.is_none() {
        return Ok(None);
    }
    let rs = config.record_schema.as_ref();
    let stored = StoredSchema {
        enable_proxima_record: config.enable_proxima_record.unwrap_or(false),
        enforcement: rs.map(|r| r.enforcement).unwrap_or(0),
        auto_evolve: rs.map(|r| r.auto_evolve).unwrap_or(true),
        schema_id: rs.map(|r| r.schema_id.clone()).unwrap_or_default(),
        schema_version: rs.map(|r| r.schema_version.clone()).unwrap_or_default(),
        text_columns: config.text_columns.clone(),
        text_storage: config
            .text_storage_configs
            .iter()
            .map(|t| StoredTextStorage {
                column_name: t.column_name.clone(),
                chunk_size: t.chunk_size,
            })
            .collect(),
    };
    serde_json::to_string(&stored)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("serialize record_schema for catalog asset: {e}"))
}

/// Reconstruct the ProximaRecord schema config from a neutral JSON string onto
/// the collection config (enable flag, record_schema, text columns). No-op on
/// a decode error (legacy/corrupt) so the caller keeps its defaults.
pub(crate) fn apply_record_schema_from_json(config: &mut CollectionConfig, json: &str) {
    let stored: StoredSchema = match serde_json::from_str(json) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("⚠️ catalog-asset record_schema decode failed, ignoring: {e}");
            return;
        }
    };
    config.enable_proxima_record = Some(stored.enable_proxima_record);
    config.record_schema = Some(RecordSchemaConfig {
        schema_id: stored.schema_id,
        schema_version: stored.schema_version,
        enforcement: stored.enforcement,
        auto_evolve: stored.auto_evolve,
        columns: Vec::new(),
    });
    config.text_columns = stored.text_columns;
    config.text_storage_configs = stored
        .text_storage
        .into_iter()
        .map(|t| TextStorageConfig {
            column_name: t.column_name,
            chunk_size: t.chunk_size,
            strategy: 1, // TextStorage::Chunked (mirrors the create/PUT default)
            ..Default::default()
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> CollectionConfig {
        CollectionConfig {
            index_configs: vec![IndexConfig {
                index_name: "primary_hnsw".to_string(),
                algorithm: 1,
                hnsw_config: Some(HnswConfig {
                    m: Some(24),
                    ef_construction: Some(150),
                    ef_search: Some(64),
                    ..Default::default()
                }),
                ivf_config: Some(IvfConfig {
                    n_lists: Some(256),
                    n_probe: Some(16),
                    ..Default::default()
                }),
                is_primary: Some(true),
                ..Default::default()
            }],
            quantization: Some(QuantizationConfig {
                enabled: Some(true),
                strategy: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn assert_round_trip(indexes: &[IndexConfig], quant: &Option<QuantizationConfig>) {
        assert_eq!(indexes.len(), 1);
        let ic = &indexes[0];
        assert_eq!(ic.index_name, "primary_hnsw");
        assert_eq!(ic.is_primary, Some(true));
        let hnsw = ic.hnsw_config.as_ref().expect("hnsw");
        assert_eq!(hnsw.m, Some(24));
        assert_eq!(hnsw.ef_construction, Some(150));
        assert_eq!(hnsw.ef_search, Some(64));
        let ivf = ic.ivf_config.as_ref().expect("ivf");
        assert_eq!(ivf.n_lists, Some(256));
        assert_eq!(ivf.n_probe, Some(16));
        let q = quant.as_ref().expect("quant");
        assert_eq!(q.enabled, Some(true));
        assert_eq!(q.strategy, Some(3));
    }

    #[test]
    fn round_trips_through_value_bag() {
        let config = sample_config();
        let mut map = HashMap::new();
        write_index_and_quant(&config, &mut map).expect("write");
        assert_round_trip(&read_index_configs(&map), &read_quantization(&map));
    }

    #[test]
    fn round_trips_through_json_strings() {
        let config = sample_config();
        let idx_json = index_configs_to_json(&config)
            .expect("idx json")
            .expect("some");
        let quant_json = quantization_to_json(&config)
            .expect("quant json")
            .expect("some");
        assert_round_trip(
            &index_configs_from_json(&idx_json),
            &quantization_from_json(&quant_json),
        );
    }

    #[test]
    fn empty_config_persists_nothing() {
        let config = CollectionConfig::default();
        let mut map = HashMap::new();
        write_index_and_quant(&config, &mut map).expect("write");
        assert!(map.is_empty());
        assert!(read_index_configs(&map).is_empty());
        assert!(read_quantization(&map).is_none());
        assert!(index_configs_to_json(&config).expect("idx").is_none());
        assert!(quantization_to_json(&config).expect("quant").is_none());
        assert!(record_schema_to_json(&config).expect("schema").is_none());
    }

    #[test]
    fn round_trips_record_schema_through_json() {
        let config = CollectionConfig {
            enable_proxima_record: Some(true),
            record_schema: Some(RecordSchemaConfig {
                schema_id: "schema_x".to_string(),
                schema_version: "1.0.0".to_string(),
                enforcement: 3,
                auto_evolve: true,
                columns: Vec::new(),
            }),
            text_columns: vec!["body".to_string()],
            text_storage_configs: vec![TextStorageConfig {
                column_name: "body".to_string(),
                chunk_size: 512,
                ..Default::default()
            }],
            ..Default::default()
        };
        let json = record_schema_to_json(&config).expect("ser").expect("some");

        let mut restored = CollectionConfig::default();
        apply_record_schema_from_json(&mut restored, &json);
        assert_eq!(restored.enable_proxima_record, Some(true));
        let rs = restored.record_schema.expect("record_schema");
        assert_eq!(rs.enforcement, 3);
        assert!(rs.auto_evolve);
        assert_eq!(rs.schema_id, "schema_x");
        assert_eq!(restored.text_columns, vec!["body".to_string()]);
        assert_eq!(restored.text_storage_configs.len(), 1);
        assert_eq!(restored.text_storage_configs[0].column_name, "body");
        assert_eq!(restored.text_storage_configs[0].chunk_size, 512);
    }
}
