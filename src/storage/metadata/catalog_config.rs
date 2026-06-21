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
    CollectionConfig, HnswConfig, IndexConfig, IvfConfig, QuantizationConfig,
};

/// Catalog-bag key holding the neutral per-index config array.
pub const INDEX_CONFIGS_KEY: &str = "index_configs";
/// Catalog-bag key holding the neutral quantization config.
pub const QUANTIZATION_KEY: &str = "quantization";

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
    }
}
