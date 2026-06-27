//! Catalog ⇄ proto `Collection` mapping (Slice D).
//!
//! Bidirectional conversion between the catalog's `CatalogTableSchema`/
//! `TableIdentifier` and the v1 proto `Collection`. It lives in the storage
//! metadata layer — not in `services` — because it is driven from the storage
//! WAL recovery path and round-trips through the storage-owned neutral config
//! serialization in [`super::catalog_config`]; the collection service calls
//! *down* into it. Moved verbatim from `CollectionService` (behaviour-neutral)
//! so `src/storage` no longer reaches up into `crate::services::collection`.

use anyhow::{Context, Result};

use proximadb_catalog::{
    CatalogColumn, CatalogIndex, CatalogIndexType, CatalogPhysicalFormat, CatalogProjection,
    CatalogProjectionKind, CatalogStorageLayout, CatalogStorageLayoutKind, CatalogTableSchema,
    ProjectionFreshness, TableIdentifier,
};
use proximadb_data_model::ProximaType;

use crate::proto::proximadb_v1::{
    Collection, CollectionConfig, CollectionStats, FilterableColumnSpec, FilterableDataType,
    IndexConfig, IndexingAlgorithm, StorageAssignment, StorageEngine,
};

pub(crate) fn collection_from_catalog_schema(
    table_id: &TableIdentifier,
    schema: &CatalogTableSchema,
) -> Result<Option<Collection>> {
    // A catalog asset is readable as a collection when it is either a pure
    // collection asset (`asset.kind == "collection"`) OR a table that gained
    // vector capability via `upsert_collection_catalog_asset`'s adopt branch
    // (`asset.capability.vector == "true"`). That branch deliberately
    // preserves the existing `asset.kind` (e.g. an agentic_mixed DDL table)
    // and signals vector usability through the capability flag, so gating on
    // `asset.kind` alone hides such tables from `get_collection`, breaking
    // DML INSERT into agentic-DDL tables. `collection.id` below still gates.
    let is_collection_asset = schema
        .properties
        .get("asset.kind")
        .is_some_and(|kind| kind == "collection");
    let is_vector_capable = schema
        .properties
        .get("asset.capability.vector")
        .is_some_and(|flag| flag == "true");
    if !is_collection_asset && !is_vector_capable {
        return Ok(None);
    }

    let Some(id) = schema.properties.get("collection.id").cloned() else {
        return Ok(None);
    };

    let name = schema
        .properties
        .get("collection.name")
        .cloned()
        .unwrap_or_else(|| table_id.to_fqn());
    let dimension = schema
        .properties
        .get("vector.dimension")
        .and_then(|dimension| dimension.parse::<u32>().ok())
        .or_else(|| {
            schema
                .columns
                .iter()
                .find(|column| column.name == "embedding")
                .and_then(|column| column.properties.get("dimension"))
                .and_then(|dimension| dimension.parse::<u32>().ok())
        })
        .unwrap_or_default();

    let storage_engine = schema
        .storage_layouts
        .first()
        .and_then(|layout| layout.properties.get("storage_engine"))
        .map(|engine| storage_engine_from_catalog(engine))
        .unwrap_or(StorageEngine::Sst as i32);

    // Round-trip canonical_embedding_precision from the catalog
    // schema. Mirror of the forward mapping in
    // `catalog_schema_from_collection`. Unset / Fp32 maps back to
    // None so legacy collections keep their existing serialized
    // shape (no behavior change for fp32 callers).
    let canonical_embedding_precision = {
        use crate::proto::proximadb_v1::EmbeddingPrecision;
        match schema.canonical_embedding_precision {
            proximadb_records::EmbeddingScalarType::Fp32 => None,
            proximadb_records::EmbeddingScalarType::Fp16 => Some(EmbeddingPrecision::Fp16 as i32),
            proximadb_records::EmbeddingScalarType::Bf16 => Some(EmbeddingPrecision::Bf16 as i32),
            proximadb_records::EmbeddingScalarType::Int8Scalar => {
                Some(EmbeddingPrecision::Int8 as i32)
            }
            proximadb_records::EmbeddingScalarType::UInt8Scalar => {
                Some(EmbeddingPrecision::Uint8 as i32)
            }
        }
    };

    let mut config = CollectionConfig {
        name,
        dimension,
        storage_engine: Some(storage_engine),
        owner: schema.properties.get("owner").cloned(),
        tags: schema
            .properties
            .get("tags")
            .map(|tags| {
                tags.split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        canonical_embedding_precision,
        ..Default::default()
    };

    config.filterable_columns = schema
        .columns
        .iter()
        .filter(|column| column.id >= 100)
        .map(|column| {
            let indexed = schema
                .indexes
                .iter()
                .any(|index| index.columns.iter().any(|name| name == &column.name));
            let supports_range = schema.indexes.iter().any(|index| {
                index.columns.iter().any(|name| name == &column.name)
                    && index.index_type == CatalogIndexType::BTree
            });
            FilterableColumnSpec {
                name: column.name.clone(),
                data_type: filterable_data_type(&column.data_type),
                indexed,
                supports_range,
                estimated_cardinality: None,
            }
        })
        .collect();

    config.distance_metric = schema
        .properties
        .get("vector.distance_metric")
        .and_then(|metric| metric.parse::<i32>().ok());

    config.index_configs = schema
        .indexes
        .iter()
        .filter(|index| index.columns.iter().any(|column| column == "embedding"))
        .map(|index| IndexConfig {
            index_name: index.name.clone(),
            algorithm: indexing_algorithm(index.index_type),
            parameters: index.properties.clone(),
            enabled: Some(true),
            ..Default::default()
        })
        .collect();

    // TD-122: prefer the neutral per-index/quant blob when present so the
    // detailed HNSW/IVF params, is_primary, and quantization survive the
    // round-trip. Legacy collections persisted before this lack the blob and
    // keep the coarse reconstruction above (mixed-read-safe).
    if let Some(json) = schema.properties.get("collection.index_config") {
        let restored = super::catalog_config::index_configs_from_json(json);
        if !restored.is_empty() {
            config.index_configs = restored;
        }
    }
    if let Some(json) = schema.properties.get("collection.quantization") {
        config.quantization = super::catalog_config::quantization_from_json(json);
    }
    // ADR-028: restore the per-collection index routing policy (the lossless
    // config_json below is authoritative when present; this per-field property
    // also feeds pg_catalog introspection).
    if let Some(json) = schema.properties.get("collection.index_policy") {
        config.index_policy = super::catalog_config::index_policy_from_json(json);
    }
    // TD-122: restore the ProximaRecord schema config (enable flag, enforcement,
    // text columns) so the v2 get surface can reconstruct the schema/flags.
    if let Some(json) = schema.properties.get("collection.record_schema") {
        super::catalog_config::apply_record_schema_from_json(&mut config, json);
    }
    // Lossless round-trip: if the asset carries the full serialized config it
    // is authoritative — it captures every field (including ones not mapped to
    // a typed catalog property), so no collection config is ever silently
    // dropped on read. The per-field properties above remain for pg_catalog
    // introspection.
    if let Some(json) = schema.properties.get("collection.config_json")
        && let Ok(full) = serde_json::from_str::<CollectionConfig>(json)
    {
        config = full;
    }

    let location = schema
        .storage_layouts
        .first()
        .and_then(|layout| layout.location.clone())
        .or_else(|| schema.location.clone())
        .unwrap_or_default();
    let storage_assignment = if location.is_empty() {
        None
    } else {
        Some(StorageAssignment {
            primary_path: location.clone(),
            engine: storage_engine,
            base_location: location,
            ..Default::default()
        })
    };

    Ok(Some(Collection {
        id,
        config: Some(config),
        stats: Some(CollectionStats {
            vector_count: schema
                .properties
                .get("stats.row_count")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            data_size_bytes: schema
                .properties
                .get("stats.data_size_bytes")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            index_size_bytes: schema
                .properties
                .get("stats.index_size_bytes")
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
        }),
        created_at: schema.created_at_ms * 1000,
        updated_at: schema.updated_at_ms * 1000,
        storage_assignment,
    }))
}

pub(crate) fn collection_table_identifier(config: &CollectionConfig) -> TableIdentifier {
    let parsed = TableIdentifier::parse(&config.name);
    if parsed.namespace.is_empty() {
        TableIdentifier::new(vec!["default".to_string()], parsed.name)
    } else {
        parsed
    }
}

pub(crate) fn catalog_schema_from_collection(
    collection: &Collection,
) -> Result<CatalogTableSchema> {
    let config = collection
        .config
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Collection has no config"))?;
    let identifier = collection_table_identifier(config);
    let mut embedding_column = CatalogColumn::new(
        20,
        "embedding",
        ProximaType::DenseVector {
            element: proximadb_data_model::VectorElement::Float32,
            dim: 0,
        },
    );
    embedding_column
        .properties
        .insert("dimension".to_string(), config.dimension.to_string());

    let mut schema = CatalogTableSchema::new(identifier.name.clone())
        .with_column(CatalogColumn::new(0, "oid", ProximaType::String).nullable(false))
        .with_column(CatalogColumn::new(1, "tenant_id", ProximaType::String))
        .with_column(CatalogColumn::new(
            2,
            "created_at_ns",
            ProximaType::TimestampTz(proximadb_data_model::TimeUnit::Nanosecond),
        ))
        .with_column(CatalogColumn::new(
            3,
            "updated_at_ns",
            ProximaType::TimestampTz(proximadb_data_model::TimeUnit::Nanosecond),
        ))
        .with_column(CatalogColumn::new(8, "props", ProximaType::Json))
        .with_column(embedding_column)
        .with_primary_key(vec!["oid".to_string()]);

    for (idx, column) in config.filterable_columns.iter().enumerate() {
        if column.name.is_empty() {
            continue;
        }
        schema = schema.with_column(CatalogColumn::new(
            100 + idx as i32,
            column.name.clone(),
            catalog_data_type(column.data_type),
        ));

        if column.indexed {
            let index_type = if column.supports_range {
                CatalogIndexType::BTree
            } else {
                CatalogIndexType::Hash
            };
            schema = schema.with_index(CatalogIndex::new(
                format!("idx_{}_{}", identifier.name, column.name),
                vec![column.name.clone()],
                index_type,
            ));
        }
    }

    for index in &config.index_configs {
        let index_type = catalog_index_type(index.algorithm);
        schema = schema.with_index(CatalogIndex::new(
            if index.index_name.is_empty() {
                format!("idx_{}_embedding", identifier.name)
            } else {
                index.index_name.clone()
            },
            vec!["embedding".to_string()],
            index_type,
        ));

        let mut projection = CatalogProjection::rebuildable(
            if index.index_name.is_empty() {
                format!("{}_ann", identifier.name)
            } else {
                index.index_name.clone()
            },
            CatalogProjectionKind::VectorAnn,
            "primary",
        );
        projection.physical_format = CatalogPhysicalFormat::ProximaBlock;
        projection.freshness = ProjectionFreshness::Lazy;
        schema = schema.with_projection(projection);
    }

    let mut layout = CatalogStorageLayout::internal(
        "primary",
        match config.storage_engine.unwrap_or(StorageEngine::Sst as i32) {
            value if value == StorageEngine::Viper as i32 => CatalogStorageLayoutKind::Columnar,
            value if value == StorageEngine::Nova as i32 => CatalogStorageLayoutKind::Columnar,
            value if value == StorageEngine::Helix as i32 => CatalogStorageLayoutKind::LsmRecord,
            _ => CatalogStorageLayoutKind::RowRecord,
        },
    );
    layout.location = collection
        .storage_assignment
        .as_ref()
        .map(|assignment| assignment.base_location.clone())
        .filter(|location| !location.is_empty());
    layout
        .properties
        .insert("collection_id".to_string(), collection.id.clone());
    layout.properties.insert(
        "storage_engine".to_string(),
        config
            .storage_engine
            .and_then(|engine| StorageEngine::try_from(engine).ok())
            .map(|engine| format!("{:?}", engine))
            .unwrap_or_else(|| "Sst".to_string()),
    );

    schema.storage_layouts = vec![layout];
    schema.location = collection
        .storage_assignment
        .as_ref()
        .map(|assignment| assignment.base_location.clone())
        .filter(|location| !location.is_empty());
    schema.created_at_ms = collection.created_at / 1000;
    schema.updated_at_ms = collection.updated_at / 1000;
    schema
        .properties
        .insert("asset.kind".to_string(), "collection".to_string());
    schema
        .properties
        .insert("asset.capability.vector".to_string(), "true".to_string());
    schema
        .properties
        .insert("collection.id".to_string(), collection.id.clone());
    schema
        .properties
        .insert("collection.name".to_string(), config.name.clone());
    schema
        .properties
        .insert("vector.dimension".to_string(), config.dimension.to_string());
    if let Some(metric) = config.distance_metric {
        schema
            .properties
            .insert("vector.distance_metric".to_string(), metric.to_string());
    }
    if let Some(owner) = &config.owner {
        schema.properties.insert("owner".to_string(), owner.clone());
    }
    if !config.tags.is_empty() {
        schema
            .properties
            .insert("tags".to_string(), config.tags.join(","));
    }
    if let Some(stats) = &collection.stats {
        schema.properties.insert(
            "stats.row_count".to_string(),
            stats.vector_count.to_string(),
        );
        schema.properties.insert(
            "stats.data_size_bytes".to_string(),
            stats.data_size_bytes.to_string(),
        );
        schema.properties.insert(
            "stats.index_size_bytes".to_string(),
            stats.index_size_bytes.to_string(),
        );
    }

    // Map the proto EmbeddingPrecision discriminant to the catalog's
    // EmbeddingScalarType so the canonical_embedding_precision field
    // (read by CanonicalPrecisionResolver) reflects whatever the
    // create-collection request asked for. Unspecified / Fp32 stays
    // on the legacy default.
    if let Some(precision_value) = config.canonical_embedding_precision {
        use crate::proto::proximadb_v1::EmbeddingPrecision;
        schema.canonical_embedding_precision = match EmbeddingPrecision::try_from(precision_value) {
            Ok(EmbeddingPrecision::Fp16) => proximadb_records::EmbeddingScalarType::Fp16,
            Ok(EmbeddingPrecision::Bf16) => proximadb_records::EmbeddingScalarType::Bf16,
            Ok(EmbeddingPrecision::Int8) => proximadb_records::EmbeddingScalarType::Int8Scalar,
            Ok(EmbeddingPrecision::Uint8) => proximadb_records::EmbeddingScalarType::UInt8Scalar,
            // Unspecified / Fp32 / unknown all map to the legacy default
            _ => proximadb_records::EmbeddingScalarType::Fp32,
        };
    }

    // TD-122: persist the detailed per-index (HNSW m/ef, IVF n_lists/n_probe,
    // is_primary) and quantization (enabled, strategy) config in a neutral,
    // wire-independent form so GetCollection echoes back what CreateCollection
    // set. The CatalogIndex entries above only carry the index identity/type;
    // these JSON blobs carry the tuning knobs the catalog schema can't model.
    if let Some(json) = super::catalog_config::index_configs_to_json(config)? {
        schema
            .properties
            .insert("collection.index_config".to_string(), json);
    }
    if let Some(json) = super::catalog_config::quantization_to_json(config)? {
        schema
            .properties
            .insert("collection.quantization".to_string(), json);
    }
    // ADR-028: persist the per-collection index routing policy neutrally (also
    // captured in collection.config_json below; this typed property feeds
    // pg_catalog introspection).
    if let Some(json) = super::catalog_config::index_policy_to_json(config)? {
        schema
            .properties
            .insert("collection.index_policy".to_string(), json);
    }
    // TD-122: persist the ProximaRecord schema config (enable flag, enforcement,
    // text columns) neutrally so get_collection_v2 echoes the schema/flags set
    // at create time.
    if let Some(json) = super::catalog_config::record_schema_to_json(config)? {
        schema
            .properties
            .insert("collection.record_schema".to_string(), json);
    }
    // Lossless round-trip: store the full serialized config so the catalog
    // asset never drops any collection field (the typed properties above stay
    // for pg_catalog introspection). This makes the catalog a complete,
    // sole-authority store for collection metadata.
    schema.properties.insert(
        "collection.config_json".to_string(),
        serde_json::to_string(config).context("serializing collection config for catalog asset")?,
    );

    Ok(schema)
}

fn catalog_data_type(data_type: i32) -> ProximaType {
    use proximadb_data_model::TimeUnit;
    match FilterableDataType::try_from(data_type).ok() {
        Some(FilterableDataType::FilterableInteger) => ProximaType::Int64,
        Some(FilterableDataType::FilterableFloat) => ProximaType::Float64,
        Some(FilterableDataType::FilterableBoolean) => ProximaType::Boolean,
        Some(FilterableDataType::FilterableDatetime) => {
            ProximaType::Timestamp(TimeUnit::Nanosecond)
        }
        Some(FilterableDataType::FilterableDecimal) => ProximaType::Decimal {
            precision: 38,
            scale: 10,
        },
        Some(FilterableDataType::FilterableTimestampTz) => {
            ProximaType::TimestampTz(TimeUnit::Nanosecond)
        }
        Some(FilterableDataType::FilterableDate) => ProximaType::Date,
        Some(FilterableDataType::FilterableTime) => ProximaType::Time(TimeUnit::Nanosecond),
        Some(FilterableDataType::FilterableUuid) => ProximaType::Uuid,
        Some(FilterableDataType::FilterableBinary) => ProximaType::Binary,
        Some(FilterableDataType::FilterableJson)
        | Some(FilterableDataType::FilterableMapStringAny) => ProximaType::Json,
        _ => ProximaType::String,
    }
}

fn catalog_index_type(algorithm: i32) -> CatalogIndexType {
    match IndexingAlgorithm::try_from(algorithm).ok() {
        Some(IndexingAlgorithm::Hnsw) => CatalogIndexType::Hnsw,
        Some(IndexingAlgorithm::Ivf) => CatalogIndexType::Ivf,
        Some(IndexingAlgorithm::Pq) => CatalogIndexType::Pq,
        _ => CatalogIndexType::Hnsw,
    }
}

fn filterable_data_type(data_type: &ProximaType) -> i32 {
    match data_type {
        ProximaType::Int8 | ProximaType::Int16 | ProximaType::Int32 | ProximaType::Int64 => {
            FilterableDataType::FilterableInteger as i32
        }
        ProximaType::Float32 | ProximaType::Float64 => FilterableDataType::FilterableFloat as i32,
        ProximaType::Boolean => FilterableDataType::FilterableBoolean as i32,
        ProximaType::Timestamp(_) => FilterableDataType::FilterableDatetime as i32,
        ProximaType::TimestampTz(_) => FilterableDataType::FilterableTimestampTz as i32,
        ProximaType::Decimal { .. } => FilterableDataType::FilterableDecimal as i32,
        ProximaType::Date => FilterableDataType::FilterableDate as i32,
        ProximaType::Time(_) => FilterableDataType::FilterableTime as i32,
        ProximaType::Uuid => FilterableDataType::FilterableUuid as i32,
        ProximaType::Binary => FilterableDataType::FilterableBinary as i32,
        ProximaType::Json => FilterableDataType::FilterableJson as i32,
        _ => FilterableDataType::FilterableString as i32,
    }
}

fn indexing_algorithm(index_type: CatalogIndexType) -> i32 {
    match index_type {
        CatalogIndexType::Ivf => IndexingAlgorithm::Ivf as i32,
        CatalogIndexType::Pq => IndexingAlgorithm::Pq as i32,
        CatalogIndexType::Hnsw => IndexingAlgorithm::Hnsw as i32,
        _ => IndexingAlgorithm::Hnsw as i32,
    }
}

fn storage_engine_from_catalog(engine: &str) -> i32 {
    match engine.to_ascii_uppercase().as_str() {
        "VIPER" => StorageEngine::Viper as i32,
        "NOVA" => StorageEngine::Nova as i32,
        "HELIX" => StorageEngine::Helix as i32,
        "SWIFT" => StorageEngine::Swift as i32,
        "RAPTOR" => StorageEngine::Raptor as i32,
        "MMAP" => StorageEngine::Mmap as i32,
        "HYBRID" => StorageEngine::Hybrid as i32,
        "TST" => StorageEngine::Tst as i32,
        "CEDAR" => StorageEngine::Cedar as i32,
        "TITAN" => StorageEngine::Titan as i32,
        "CHRONO" => StorageEngine::Chrono as i32,
        _ => StorageEngine::Sst as i32,
    }
}
