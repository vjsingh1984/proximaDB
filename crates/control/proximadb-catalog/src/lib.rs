//! xCatalog contract types and the `Catalog` trait.
//!
//! These serializable contracts are shared by catalog implementations, query planning, storage
//! selection, and protocol/API boundaries without requiring the root runtime crate.
//!
//! ## Key exports
//! - `TableIdentifier` — namespace + table name tuple for addressing tables
//! - `Catalog` — the core async trait every catalog backend implements
//! - All `Catalog*` types used in trait method signatures

use arrow_schema::DataType as ArrowDataType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

pub mod cache;
pub mod canonical_precision;
// Collection-level DR / CRR engine contract (P1 of
// COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc).
pub mod collection_dr_policy;
// Embedding-precision rollout (PR 6 of EMBEDDING_PRECISION_LLD_2026_05_22).
pub mod embedding_precision_policy;
pub mod oltp;
pub mod relational;
pub mod schema;
pub mod system_columns;

/// Storage pool class for a namespace's bytes. The path resolver routes
/// writes to the matching bucket/container and refuses cross-class writes.
/// Tier-to-pool mapping is operator policy and lives in the operator layer;
/// the engine only knows the enum.
///
/// See `docs/12-design/COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc` "Storage Pool
/// Classes".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StoragePoolClass {
    /// Shared pool — lowest operational overhead, no per-collection CRR.
    /// Default for legacy rows backfilled by the P0.5 migration.
    #[default]
    Pooled,
    /// Shared Business pool — prefix-scoped CRR allowed.
    Business,
    /// Shared Enterprise pool — stricter KMS, monitoring, and rule budgeting.
    Enterprise,
    /// Dedicated bucket/storage-account pair per tenant per region pair.
    EnterpriseDedicated,
}

/// Namespace metadata.
///
/// Serves two roles:
///
/// 1. **Iceberg-REST federation identifier** — `levels`, `properties`,
///    `owner`, `location`, timestamps. Compatible with the Iceberg REST
///    catalog wire format.
/// 2. **Engine multi-tenant authority** — `namespace_id`, `tenant_id`,
///    `region_home`, `default_dr_region_pair_id`, `storage_pool_class`.
///    Drives the physical path layout
///    `data/{tenant_id}/{namespace_id}/{collection_id}/` and the DR
///    policy authority boundary.
///
/// New engine-authoritative fields are `Option<>` for backwards
/// compatibility with the P0.5 migration. The DR-strict path
/// (reconciler, path resolver guard) refuses null `namespace_id` /
/// `tenant_id`. The Iceberg-REST handler projects only the federation
/// fields so external Iceberg clients see the wire format they expect.
///
/// See `docs/12-design/COLLECTION_DR_CRR_ENGINE_CONTRACT.adoc` "Namespace
/// As The DR Authority Boundary".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogNamespace {
    // --- Iceberg-REST federation fields (mutable labels) ---
    /// Namespace hierarchy (e.g., ["database", "schema"])
    pub levels: Vec<String>,
    /// Namespace properties
    pub properties: HashMap<String, String>,
    /// Owner principal
    pub owner: Option<String>,
    /// Storage location
    pub location: Option<String>,
    /// Creation timestamp (millis since epoch)
    pub created_at_ms: i64,
    /// Last update timestamp (millis since epoch)
    pub updated_at_ms: i64,

    // --- Engine multi-tenant authority fields (stable identifiers) ---
    /// Opaque, stable, server-issued ULID. Never reused, never changes on
    /// rename. Drives physical paths and provider rule filters. `None`
    /// for legacy rows pending P0.5 migration backfill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace_id: Option<String>,
    /// Owning tenant. A namespace cannot be re-parented. `None` for
    /// legacy rows pending migration backfill (target value:
    /// `"tnt_legacy_system"` until operator re-parents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    /// Region where authoritative writes land. `None` is allowed only
    /// for namespaces with no DR policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_home: Option<String>,
    /// Operator-curated default that new collections inherit at DR
    /// enablement time. Recommended canonical form
    /// `{provider}:{source_region}:{destination_region}`; engine treats
    /// it as opaque.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_dr_region_pair_id: Option<String>,
    /// Storage pool class. Path resolver refuses writes that target a
    /// bucket/container outside the matching class. Defaults to `Pooled`
    /// for backwards compatibility with legacy rows.
    #[serde(default)]
    pub storage_pool_class: StoragePoolClass,
}

impl CatalogNamespace {
    /// Create a new namespace with only the Iceberg-REST federation
    /// fields populated. Engine multi-tenant fields are `None` /
    /// defaults; use `with_tenant`, `with_namespace_id`, etc. to set
    /// them when constructing an engine-authoritative namespace.
    pub fn new(levels: Vec<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Self {
            levels,
            properties: HashMap::new(),
            owner: None,
            location: None,
            created_at_ms: now,
            updated_at_ms: now,
            namespace_id: None,
            tenant_id: None,
            region_home: None,
            default_dr_region_pair_id: None,
            storage_pool_class: StoragePoolClass::default(),
        }
    }

    /// Get fully qualified name
    pub fn fqn(&self) -> String {
        self.levels.join(".")
    }

    /// Add a property
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Set owner
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    /// Set location
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Set the engine-authoritative namespace ID (server-issued ULID).
    pub fn with_namespace_id(mut self, namespace_id: impl Into<String>) -> Self {
        self.namespace_id = Some(namespace_id.into());
        self
    }

    /// Set the owning tenant.
    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Set the home region for authoritative writes.
    pub fn with_region_home(mut self, region: impl Into<String>) -> Self {
        self.region_home = Some(region.into());
        self
    }

    /// Set the default region-pair ID for DR enablement.
    pub fn with_default_dr_region_pair(mut self, pair_id: impl Into<String>) -> Self {
        self.default_dr_region_pair_id = Some(pair_id.into());
        self
    }

    /// Set the storage pool class.
    pub fn with_storage_pool_class(mut self, class: StoragePoolClass) -> Self {
        self.storage_pool_class = class;
        self
    }

    /// True when both `namespace_id` and `tenant_id` are populated. The
    /// DR path resolver and reconciler require this; legacy namespaces
    /// pending migration backfill return false.
    pub fn is_dr_addressable(&self) -> bool {
        self.namespace_id.is_some() && self.tenant_id.is_some()
    }
}

/// Column definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogColumn {
    /// Column ID (stable across renames)
    pub id: i32,
    /// Column name
    pub name: String,
    /// Data type
    pub data_type: CatalogDataType,
    /// Is nullable
    pub nullable: bool,
    /// Default value (SQL expression)
    pub default_value: Option<String>,
    /// Column comment
    pub comment: Option<String>,
    /// Column metadata/properties
    pub properties: HashMap<String, String>,
}

impl CatalogColumn {
    /// Create a new column
    pub fn new(id: i32, name: impl Into<String>, data_type: CatalogDataType) -> Self {
        Self {
            id,
            name: name.into(),
            data_type,
            nullable: true,
            default_value: None,
            comment: None,
            properties: HashMap::new(),
        }
    }

    /// Set nullable
    pub fn nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    /// Set default value
    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default_value = Some(default.into());
        self
    }

    /// Set comment
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }
}

/// Data types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogDataType {
    /// Boolean true/false value
    Boolean,
    /// 8-bit signed integer
    Int8,
    /// 16-bit signed integer
    Int16,
    /// 32-bit signed integer
    Int32,
    /// 64-bit signed integer
    Int64,
    /// 32-bit IEEE 754 floating-point number
    Float32,
    /// 64-bit IEEE 754 floating-point number
    Float64,
    /// UTF-8 encoded string
    String,
    /// Arbitrary binary data
    Binary,
    /// Calendar date (no time component)
    Date,
    /// Time of day (no date component)
    Time,
    /// Timestamp without timezone
    Timestamp,
    /// Timestamp with timezone
    TimestampTz,
    /// Exact decimal numeric value
    Decimal,
    /// Universally unique identifier (UUID v4)
    Uuid,
    /// JSON document stored as text
    Json,
    /// Fixed-size array of floats for vector embeddings
    Vector,
    /// Sparse vector (map of index to value)
    SparseVector,
    /// Binary vector (packed bits)
    BinaryVector,
}

impl CatalogDataType {
    /// Convert to proto DataType value
    pub fn to_proto_i32(&self) -> i32 {
        match self {
            CatalogDataType::Boolean => 1,
            CatalogDataType::Int8 => 2,
            CatalogDataType::Int16 => 3,
            CatalogDataType::Int32 => 4,
            CatalogDataType::Int64 => 5,
            CatalogDataType::Float32 => 6,
            CatalogDataType::Float64 => 7,
            CatalogDataType::String => 8,
            CatalogDataType::Binary => 9,
            CatalogDataType::Date => 10,
            CatalogDataType::Time => 11,
            CatalogDataType::Timestamp => 12,
            CatalogDataType::TimestampTz => 13,
            CatalogDataType::Decimal => 14,
            CatalogDataType::Uuid => 15,
            CatalogDataType::Json => 16,
            CatalogDataType::Vector => 20,
            CatalogDataType::SparseVector => 21,
            CatalogDataType::BinaryVector => 22,
        }
    }

    /// Create from proto DataType value
    pub fn from_proto_i32(value: i32) -> Self {
        match value {
            1 => CatalogDataType::Boolean,
            2 => CatalogDataType::Int8,
            3 => CatalogDataType::Int16,
            4 => CatalogDataType::Int32,
            5 => CatalogDataType::Int64,
            6 => CatalogDataType::Float32,
            7 => CatalogDataType::Float64,
            8 => CatalogDataType::String,
            9 => CatalogDataType::Binary,
            10 => CatalogDataType::Date,
            11 => CatalogDataType::Time,
            12 => CatalogDataType::Timestamp,
            13 => CatalogDataType::TimestampTz,
            14 => CatalogDataType::Decimal,
            15 => CatalogDataType::Uuid,
            16 => CatalogDataType::Json,
            20 => CatalogDataType::Vector,
            21 => CatalogDataType::SparseVector,
            22 => CatalogDataType::BinaryVector,
            _ => CatalogDataType::String,
        }
    }

    /// Convert to Arrow DataType
    pub fn to_arrow_datatype(&self) -> ArrowDataType {
        match self {
            CatalogDataType::Boolean => ArrowDataType::Boolean,
            CatalogDataType::Int8 => ArrowDataType::Int8,
            CatalogDataType::Int16 => ArrowDataType::Int16,
            CatalogDataType::Int32 => ArrowDataType::Int32,
            CatalogDataType::Int64 => ArrowDataType::Int64,
            CatalogDataType::Float32 => ArrowDataType::Float32,
            CatalogDataType::Float64 => ArrowDataType::Float64,
            CatalogDataType::String => ArrowDataType::Utf8,
            CatalogDataType::Binary => ArrowDataType::Binary,
            CatalogDataType::Date => ArrowDataType::Date32,
            CatalogDataType::Time => ArrowDataType::Time64(arrow_schema::TimeUnit::Nanosecond),
            CatalogDataType::Timestamp => {
                ArrowDataType::Timestamp(arrow_schema::TimeUnit::Nanosecond, None)
            }
            CatalogDataType::TimestampTz => {
                ArrowDataType::Timestamp(arrow_schema::TimeUnit::Nanosecond, Some("UTC".into()))
            }
            CatalogDataType::Decimal => ArrowDataType::Decimal128(38, 10),
            CatalogDataType::Uuid => ArrowDataType::Utf8, // UUID as string
            CatalogDataType::Json => ArrowDataType::Utf8, // JSON as string
            CatalogDataType::Vector => ArrowDataType::List(
                Box::new(arrow_schema::Field::new(
                    "item",
                    ArrowDataType::Float32,
                    true,
                ))
                .into(),
            ),
            CatalogDataType::SparseVector => ArrowDataType::Map(
                Box::new(arrow_schema::Field::new(
                    "entries",
                    ArrowDataType::Struct(
                        vec![
                            arrow_schema::Field::new("key", ArrowDataType::Int32, false),
                            arrow_schema::Field::new("value", ArrowDataType::Float32, false),
                        ]
                        .into(),
                    ),
                    false,
                ))
                .into(),
                false,
            ),
            CatalogDataType::BinaryVector => ArrowDataType::Binary, // Packed bits as binary
        }
    }
}

impl CatalogDataType {
    /// Convert to the canonical [`proximadb_data_model::ProximaType`].
    ///
    /// This is the single bridge between the catalog layer and the wire type
    /// system (spec §4). Call site example:
    /// ```ignore
    /// let pt = CatalogDataType::Decimal.to_proxima_type();
    /// let oid = pt.pgwire_oid(); // 1700
    /// ```
    pub fn to_proxima_type(&self) -> proximadb_data_model::ProximaType {
        use proximadb_data_model::{ProximaType, TimeUnit, VectorElement};
        match self {
            CatalogDataType::Boolean => ProximaType::Boolean,
            CatalogDataType::Int8 => ProximaType::Int8,
            CatalogDataType::Int16 => ProximaType::Int16,
            CatalogDataType::Int32 => ProximaType::Int32,
            CatalogDataType::Int64 => ProximaType::Int64,
            CatalogDataType::Float32 => ProximaType::Float32,
            CatalogDataType::Float64 => ProximaType::Float64,
            CatalogDataType::String => ProximaType::String,
            CatalogDataType::Binary => ProximaType::Binary,
            CatalogDataType::Date => ProximaType::Date,
            CatalogDataType::Time => ProximaType::Time(TimeUnit::Nanosecond),
            CatalogDataType::Timestamp => ProximaType::Timestamp(TimeUnit::Nanosecond),
            CatalogDataType::TimestampTz => ProximaType::TimestampTz(TimeUnit::Nanosecond),
            CatalogDataType::Decimal => ProximaType::Decimal {
                precision: 38,
                scale: 10,
            },
            CatalogDataType::Uuid => ProximaType::Uuid,
            CatalogDataType::Json => ProximaType::Json,
            CatalogDataType::Vector => ProximaType::DenseVector {
                element: VectorElement::Float32,
                dim: 0,
            },
            CatalogDataType::SparseVector => ProximaType::SparseVector {
                element: VectorElement::Float32,
            },
            CatalogDataType::BinaryVector => ProximaType::BinaryVector { dim: 0 },
        }
    }
}

/// Table schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogTableSchema {
    /// Table name
    pub name: String,
    /// Table columns
    pub columns: Vec<CatalogColumn>,
    /// Primary key columns (by name)
    pub primary_key: Vec<String>,
    /// Indexes
    pub indexes: Vec<CatalogIndex>,
    /// Cataloged storage layouts and authority modes.
    #[serde(default)]
    pub storage_layouts: Vec<CatalogStorageLayout>,
    /// Cataloged rebuildable projections/access methods.
    #[serde(default)]
    pub projections: Vec<CatalogProjection>,
    /// Optional relational integrity and transaction capabilities.
    #[serde(default)]
    pub relational_capabilities: RelationalCapabilities,
    /// Intended workload profile for layout/query planning.
    #[serde(default)]
    pub workload_profile: CatalogWorkloadProfile,
    /// Primary storage specialization selected for this table/profile.
    #[serde(default)]
    pub storage_specialization: CatalogStorageSpecialization,
    /// ANN filtering routing policy. Controls selectivity thresholds for
    /// pre-filter vs. inline vs. post-filter mode selection. Applies to all
    /// vector ANN projections registered for this table.
    #[serde(default)]
    pub ann_filtering_policy: AnnFilteringPolicy,
    /// Props auto-promotion policy. Controls whether high-frequency msgpack
    /// props keys are promoted to typed PAX columns during compaction.
    /// Must be enabled for document tables to benefit from column-level pruning
    /// and Iceberg/Spark predicate pushdown into props fields.
    #[serde(default)]
    pub props_auto_promotion: PropsAutoPromotionPolicy,
    /// Observability compression hint. Applies to tables with
    /// `CatalogWorkloadProfile::Observability`. Instructs the PAX block writer
    /// to sort by series key + timestamp and apply delta-delta/XOR encoding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observability_compression: Option<ObservabilityCompressionHint>,
    /// Machine-readable codec/layout profiling feedback for PAX, projection,
    /// and open-format planners.
    #[serde(default)]
    pub compression_stats_profiles: Vec<CatalogCompressionStatsProfile>,

    // === Embedding-precision rollout (PR 6 of EMBEDDING_PRECISION_LLD_2026_05_22) ===
    /// Reference to the precision policy row in `embedding_precision_policy`.
    /// `None` = inherit the cluster's `GLOBAL_DEFAULT_POLICY_ID` seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_precision_policy_id: Option<String>,
    /// Locked policy version this collection's writes resolve against. Bumps
    /// when an operator promotes the collection to a new policy revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_precision_policy_version: Option<u64>,
    /// Monotonically-increasing epoch tagged onto every WAL record + segment
    /// header. Starts at 0; bumped each time the canonical precision changes.
    #[serde(default)]
    pub current_precision_epoch: u64,
    /// Canonical (default) precision for new writes. Legacy schemas
    /// deserialize as `Fp32` via `Default`.
    #[serde(default)]
    pub canonical_embedding_precision: proximadb_records::EmbeddingScalarType,
    /// Precisions ingest will accept (subject to `ingest_mismatch` policy).
    /// Empty = inherit from policy.
    #[serde(default)]
    pub allowed_embedding_precisions: Vec<proximadb_records::EmbeddingScalarType>,
    /// Per-metric recall@10/@100 SLO; LLD §Q13 defaults seeded automatically.
    #[serde(default)]
    pub embedding_recall_slo: embedding_precision_policy::RecallSlo,
    /// Migration lifecycle state for in-flight precision changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision_migration_state: Option<embedding_precision_policy::PrecisionMigrationState>,

    /// Schema version
    pub schema_version: i32,
    /// Table properties
    pub properties: HashMap<String, String>,
    /// Storage location
    pub location: Option<String>,
    /// Creation timestamp
    pub created_at_ms: i64,
    /// Last update timestamp
    pub updated_at_ms: i64,
}

impl Default for CatalogTableSchema {
    fn default() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Self {
            name: String::new(),
            columns: Vec::new(),
            primary_key: Vec::new(),
            indexes: Vec::new(),
            storage_layouts: vec![CatalogStorageLayout::default()],
            projections: Vec::new(),
            relational_capabilities: RelationalCapabilities::default(),
            workload_profile: CatalogWorkloadProfile::default(),
            storage_specialization: CatalogStorageSpecialization::default(),
            ann_filtering_policy: AnnFilteringPolicy::default(),
            props_auto_promotion: PropsAutoPromotionPolicy::default(),
            observability_compression: None,
            compression_stats_profiles: Vec::new(),
            // PR 6: inherit cluster default policy; fp32-only baseline.
            embedding_precision_policy_id: None,
            embedding_precision_policy_version: None,
            current_precision_epoch: 0,
            canonical_embedding_precision: proximadb_records::EmbeddingScalarType::Fp32,
            allowed_embedding_precisions: Vec::new(),
            embedding_recall_slo: embedding_precision_policy::RecallSlo::lld_defaults(),
            precision_migration_state: None,
            schema_version: 1,
            properties: HashMap::new(),
            location: None,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

impl CatalogTableSchema {
    /// Create a new table schema
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Add a column
    pub fn with_column(mut self, column: CatalogColumn) -> Self {
        self.columns.push(column);
        self
    }

    /// Set primary key
    pub fn with_primary_key(mut self, columns: Vec<String>) -> Self {
        self.primary_key = columns;
        self
    }

    /// Add an index
    pub fn with_index(mut self, index: CatalogIndex) -> Self {
        self.indexes.push(index);
        self
    }

    /// Add a storage layout/authority descriptor.
    pub fn with_storage_layout(mut self, layout: CatalogStorageLayout) -> Self {
        self.storage_layouts.push(layout);
        self
    }

    /// Add a rebuildable projection descriptor.
    pub fn with_projection(mut self, projection: CatalogProjection) -> Self {
        self.projections.push(projection);
        self
    }

    /// Set optional relational capability metadata.
    pub fn with_relational_capabilities(mut self, capabilities: RelationalCapabilities) -> Self {
        self.relational_capabilities = capabilities;
        self
    }

    /// Set the intended workload profile.
    pub fn with_workload_profile(mut self, profile: CatalogWorkloadProfile) -> Self {
        self.workload_profile = profile;
        self
    }

    /// Set the primary storage specialization.
    pub fn with_storage_specialization(
        mut self,
        specialization: CatalogStorageSpecialization,
    ) -> Self {
        self.storage_specialization = specialization;
        self
    }

    /// Set the ANN filtering routing policy.
    pub fn with_ann_filtering_policy(mut self, policy: AnnFilteringPolicy) -> Self {
        self.ann_filtering_policy = policy;
        self
    }

    /// Enable props auto-promotion for document-heavy tables.
    pub fn with_props_auto_promotion(mut self, policy: PropsAutoPromotionPolicy) -> Self {
        self.props_auto_promotion = policy;
        self
    }

    /// Set observability compression hint for time-series/metrics tables.
    pub fn with_observability_compression(mut self, hint: ObservabilityCompressionHint) -> Self {
        self.observability_compression = Some(hint);
        self
    }

    /// Add codec/layout profiling feedback for planners and EXPLAIN.
    pub fn with_compression_stats_profile(
        mut self,
        profile: CatalogCompressionStatsProfile,
    ) -> Self {
        self.compression_stats_profiles.push(profile);
        self
    }
}

/// xCatalog feedback record for one measured compression/layout profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogCompressionStatsProfile {
    /// Stable profile id referenced by PAX metadata, projections, and EXPLAIN fixtures.
    pub profile_id: String,
    /// Optional cataloged layout that produced this profile.
    pub layout_name: Option<String>,
    /// Optional projection/access-method that produced this profile.
    pub projection_id: Option<String>,
    /// Selected codec or pilot codec family.
    pub selected_scheme: String,
    /// Raw input bytes measured by the profiler.
    pub raw_bytes: u64,
    /// Encoded payload bytes measured by the profiler.
    pub encoded_bytes: u64,
    /// Number of logical values covered by this profile.
    pub value_count: u64,
    /// Measured raw/encoded ratio.
    pub measured_ratio: f64,
    /// Whether exact visible values can be reconstructed from the encoded payload.
    pub exact_reconstruction: bool,
    /// Measured encode CPU per block, if known.
    pub encode_cpu_ms_per_block: Option<f64>,
    /// Measured decode cost per logical value, if known.
    pub decode_ns_per_value: Option<f64>,
    /// Rejected alternatives and reasons.
    #[serde(default)]
    pub rejected_candidates: Vec<CatalogCompressionRejectedCandidate>,
    /// Extension fields for benchmark or engine-specific metadata.
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

impl CatalogCompressionStatsProfile {
    pub fn new(
        profile_id: impl Into<String>,
        selected_scheme: impl Into<String>,
        raw_bytes: u64,
        encoded_bytes: u64,
        value_count: u64,
        exact_reconstruction: bool,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            layout_name: None,
            projection_id: None,
            selected_scheme: selected_scheme.into(),
            raw_bytes,
            encoded_bytes,
            value_count,
            measured_ratio: measured_ratio(raw_bytes, encoded_bytes),
            exact_reconstruction,
            encode_cpu_ms_per_block: None,
            decode_ns_per_value: None,
            rejected_candidates: Vec::new(),
            properties: HashMap::new(),
        }
    }

    pub fn with_layout_name(mut self, layout_name: impl Into<String>) -> Self {
        self.layout_name = Some(layout_name.into());
        self
    }

    pub fn with_projection_id(mut self, projection_id: impl Into<String>) -> Self {
        self.projection_id = Some(projection_id.into());
        self
    }

    pub fn with_decode_ns_per_value(mut self, decode_ns_per_value: f64) -> Self {
        self.decode_ns_per_value = Some(decode_ns_per_value);
        self
    }

    pub fn bytes_per_value(&self) -> f64 {
        if self.value_count == 0 {
            0.0
        } else {
            self.encoded_bytes as f64 / self.value_count as f64
        }
    }
}

/// xCatalog rejected codec candidate recorded from profiling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogCompressionRejectedCandidate {
    pub scheme: String,
    pub reason: String,
    pub expected_ratio: Option<f32>,
}

fn measured_ratio(raw_bytes: u64, encoded_bytes: u64) -> f64 {
    if raw_bytes == 0 || encoded_bytes == 0 {
        0.0
    } else {
        raw_bytes as f64 / encoded_bytes as f64
    }
}

/// Index definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogIndex {
    /// Index name
    pub name: String,
    /// Indexed columns
    pub columns: Vec<String>,
    /// Index type
    pub index_type: CatalogIndexType,
    /// Is unique index
    pub is_unique: bool,
    /// Index properties
    pub properties: HashMap<String, String>,
}

impl CatalogIndex {
    /// Create a new index
    pub fn new(
        name: impl Into<String>,
        columns: Vec<String>,
        index_type: CatalogIndexType,
    ) -> Self {
        Self {
            name: name.into(),
            columns,
            index_type,
            is_unique: false,
            properties: HashMap::new(),
        }
    }

    /// Set unique
    pub fn unique(mut self) -> Self {
        self.is_unique = true;
        self
    }
}

/// Index types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogIndexType {
    /// B-tree index
    BTree,
    /// Hash index
    Hash,
    /// Full-text index
    FullText,
    /// PostgreSQL-compatible GIN index for JSONB/path-heavy document projections
    Gin,
    /// HNSW vector index
    Hnsw,
    /// IVF vector index
    Ivf,
    /// Product quantization
    Pq,
}

/// Workload profile used by xCatalog, pgwire DDL, and planners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogWorkloadProfile {
    /// Row-oriented point reads/writes and transactional DML.
    Oltp,
    /// Scan-heavy analytical reads and external/open-table publication.
    Olap,
    /// Hybrid transactional/analytical processing over the same record model.
    #[default]
    Htap,
    /// Vector-heavy workload with ANN projections.
    Vector,
    /// Document-heavy workload with JSON/path projections.
    Document,
    /// Graph-heavy workload with adjacency/topology projections.
    Graph,
    /// Observability/time-series workload.
    Observability,
    /// Mixed multimodal table/profile.
    Mixed,
}

impl CatalogWorkloadProfile {
    /// Parse common SQL/catalog option spellings.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "oltp" | "transactional" | "row" => Some(Self::Oltp),
            "olap" | "analytic" | "analytics" | "analytical" | "columnar" => Some(Self::Olap),
            "htap" | "hybrid" | "pax" => Some(Self::Htap),
            "vector" | "ann" => Some(Self::Vector),
            "document" | "json" | "jsonb" => Some(Self::Document),
            "graph" | "pgq" | "cypher" => Some(Self::Graph),
            "observability" | "time_series" | "timeseries" | "metrics" | "traces" => {
                Some(Self::Observability)
            }
            "mixed" | "multimodal" => Some(Self::Mixed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oltp => "oltp",
            Self::Olap => "olap",
            Self::Htap => "htap",
            Self::Vector => "vector",
            Self::Document => "document",
            Self::Graph => "graph",
            Self::Observability => "observability",
            Self::Mixed => "mixed",
        }
    }
}

/// Primary storage specialization. Specialization is a physical/access-method
/// hint, not a separate semantic authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogStorageSpecialization {
    /// Generic durable relational record layout.
    GenericRelational,
    /// PAX row-family with row directory plus column stripes.
    #[default]
    PaxRowFamily,
    /// PAX OLTP mode, row-directory optimized.
    PaxOltp,
    /// PAX OLAP mode, column-stripe optimized.
    PaxOlap,
    /// SST/LSM write-optimized family.
    LsmWriteOptimized,
    /// Columnar analytics family.
    ColumnarAnalytics,
    /// Vector ANN specialty projection family.
    VectorAnn,
    /// Document JSON/path specialty projection family.
    DocumentJson,
    /// Graph adjacency/topology specialty projection family.
    GraphTopology,
    /// Observability/time-series specialty projection family.
    ObservabilityTimeSeries,
    /// External/open-table mapping.
    ExternalOpenTable,
}

impl CatalogStorageSpecialization {
    /// Parse SQL/catalog layout or specialization option values.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "generic" | "relational" | "row" | "rowrecord" | "row_record" => {
                Some(Self::GenericRelational)
            }
            "pax" | "hybrid" | "htap" | "pax_row_family" => Some(Self::PaxRowFamily),
            "oltp" | "pax_oltp" | "rowdir" | "row_directory" => Some(Self::PaxOltp),
            "olap" | "pax_olap" => Some(Self::PaxOlap),
            "sst" | "lsm" | "lsm_record" => Some(Self::LsmWriteOptimized),
            "columnar" | "column" | "analytics" => Some(Self::ColumnarAnalytics),
            "vector" | "ann" | "hnsw" | "ivf" | "pq" => Some(Self::VectorAnn),
            "document" | "json" | "jsonb" => Some(Self::DocumentJson),
            "graph" | "adjacency" | "csr" | "coo" => Some(Self::GraphTopology),
            "observability" | "time_series" | "timeseries" => Some(Self::ObservabilityTimeSeries),
            "external" | "open_table" | "opentable" | "lakehouse" => Some(Self::ExternalOpenTable),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GenericRelational => "generic_relational",
            Self::PaxRowFamily => "pax_row_family",
            Self::PaxOltp => "pax_oltp",
            Self::PaxOlap => "pax_olap",
            Self::LsmWriteOptimized => "lsm_write_optimized",
            Self::ColumnarAnalytics => "columnar_analytics",
            Self::VectorAnn => "vector_ann",
            Self::DocumentJson => "document_json",
            Self::GraphTopology => "graph_topology",
            Self::ObservabilityTimeSeries => "observability_time_series",
            Self::ExternalOpenTable => "external_open_table",
        }
    }
}

/// Source-of-truth mode for a cataloged table, stream, projection, or external format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogAuthorityMode {
    /// ProximaRecord plus WAL/log/manifest own durable truth.
    #[default]
    InternalCanonical,
    /// Explicit name for ProximaDB WAL + ProximaRecord authority.
    ProximaAuthoritative,
    /// An external table/source owns durable truth; ProximaDB maps and governs access.
    ExternalAuthoritative,
    /// Point-in-time import from an external source.
    ImportedSnapshot,
    /// Publication generated from canonical records.
    ExportedPublication,
    /// Explicit name for a publication generated from canonical records.
    ProjectionPublication,
    /// Rebuildable structure derived from canonical records or events.
    RebuildableProjection,
    /// Read-only federation over an external source without importing record authority.
    FederatedRead,
}

impl CatalogAuthorityMode {
    /// Returns true when ProximaDB WAL + ProximaRecord storage own durable truth.
    pub fn is_proxima_authoritative(self) -> bool {
        matches!(
            self,
            CatalogAuthorityMode::InternalCanonical | CatalogAuthorityMode::ProximaAuthoritative
        )
    }

    /// Returns true when durable truth is outside ProximaDB's canonical record store.
    pub fn is_external_authoritative(self) -> bool {
        matches!(self, CatalogAuthorityMode::ExternalAuthoritative)
    }

    /// Returns true when the layout/projection is rebuildable from another source.
    pub fn is_rebuildable_or_publication(self) -> bool {
        matches!(
            self,
            CatalogAuthorityMode::ExportedPublication
                | CatalogAuthorityMode::ProjectionPublication
                | CatalogAuthorityMode::RebuildableProjection
        )
    }

    /// Stable xCatalog ownership-mode string used by adapters and docs.
    pub fn ownership_mode_name(self) -> &'static str {
        match self {
            CatalogAuthorityMode::InternalCanonical
            | CatalogAuthorityMode::ProximaAuthoritative => "ProximaAuthoritative",
            CatalogAuthorityMode::ExternalAuthoritative => "ExternalAuthoritative",
            CatalogAuthorityMode::ImportedSnapshot => "ImportedSnapshot",
            CatalogAuthorityMode::ExportedPublication
            | CatalogAuthorityMode::ProjectionPublication => "ProjectionPublication",
            CatalogAuthorityMode::RebuildableProjection => "RebuildableProjection",
            CatalogAuthorityMode::FederatedRead => "FederatedRead",
        }
    }
}

/// Physical layout family selected for a cataloged object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogStorageLayoutKind {
    /// Mutable record/row-oriented layout.
    #[default]
    RowRecord,
    /// Log-structured merge layout for write-heavy data.
    LsmRecord,
    /// Partition Attributes Across hybrid row/column layout.
    Pax,
    /// Columnar segment layout.
    Columnar,
    /// Append-only event stream layout.
    AppendOnlyEvent,
    /// Vector ANN projection or fragment layout.
    VectorAnn,
    /// Graph topology projection such as adjacency, CSR, or COO.
    GraphTopology,
    /// Time-series compression or partition block.
    TimeSeriesBlock,
    /// Externally owned or externally formatted table.
    ExternalTable,
}

/// Physical format used by a storage layout or projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogPhysicalFormat {
    /// Native ProximaDB record/block format.
    #[default]
    ProximaBlock,
    /// Sorted string table or SST-family layout.
    Sst,
    /// Apache Arrow in-memory or IPC format.
    Arrow,
    /// Delimited text, usually comma-separated values.
    Csv,
    /// JSON or JSON Lines.
    Json,
    /// XML documents or rows.
    Xml,
    /// Apache Avro object container or schema-registry payloads.
    Avro,
    /// Apache Parquet file format.
    Parquet,
    /// Apache ORC file format.
    Orc,
    /// Apache Iceberg table format.
    Iceberg,
    /// Delta Lake table format.
    Delta,
    /// Apache Hudi table format.
    Hudi,
    /// GraphAr-style graph lake format.
    GraphAr,
    /// Format not yet modeled as a first-class enum variant.
    External(String),
}

/// Write and refresh behavior for a cataloged layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogWriteMode {
    /// In-place or logically mutable writes are accepted.
    #[default]
    Mutable,
    /// Writes append immutable records/events.
    AppendOnly,
    /// Writes create new snapshots or file sets.
    CopyOnWrite,
    /// Data is refreshed from an external authority.
    ExternalRefresh,
    /// Data arrives through a stream/CDC connector.
    StreamingIngest,
    /// Read-only mapping.
    ReadOnly,
}

/// Freshness semantics for a rebuildable projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProjectionFreshness {
    /// Projection is updated in the same write transaction as canonical records.
    Synchronous,
    /// Projection may lag but must stay within a declared bound.
    BoundedLag,
    /// Projection is refreshed lazily on demand or by background work.
    #[default]
    Lazy,
    /// Projection is refreshed only by explicit command.
    Manual,
}

/// Runtime freshness state for a rebuildable projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProjectionFreshnessState {
    /// Projection includes all committed records required by its freshness SLA.
    #[default]
    Fresh,
    /// Projection update is in progress.
    Updating,
    /// Projection is known to lag canonical WAL/record state.
    Stale,
    /// Projection cannot be incrementally repaired and must be rebuilt.
    RebuildRequired,
    /// External table/publication snapshot has been registered in xCatalog.
    ExternalSnapshotRegistered,
    /// Physical projection is not usable.
    Unavailable,
}

/// Projection or access-method family cataloged below xCatalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogProjectionKind {
    /// JSON path or generated-column projection.
    JsonPath,
    /// Full-text or lexical search projection.
    FullText,
    /// Dense/sparse/binary vector ANN projection.
    VectorAnn,
    /// Graph adjacency, CSR, COO, or graph algorithm projection.
    GraphTopology,
    /// Columnar materialization.
    Columnar,
    /// Time-series rollup or compression projection.
    TimeSeries,
    /// Trace assembly, service map, or observability correlation projection.
    Observability,
    /// Materialized SQL/logical view.
    MaterializedView,
    /// Generic projection until a more specific family is introduced.
    #[default]
    Other,
}

/// Catalog metadata shared by internal layouts and external table mappings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogStorageLayout {
    /// Stable layout identifier inside the table/profile/collection.
    pub name: String,
    /// Source-of-truth role for this layout.
    pub authority: CatalogAuthorityMode,
    /// Physical layout family.
    pub layout_kind: CatalogStorageLayoutKind,
    /// Concrete physical format.
    pub physical_format: CatalogPhysicalFormat,
    /// Write/refresh behavior.
    pub write_mode: CatalogWriteMode,
    /// Optional URI/path/catalog identifier for the physical data.
    pub location: Option<String>,
    /// Snapshot or isolation semantics, e.g. "mvcc", "iceberg-snapshot", "external-latest".
    pub snapshot_semantics: Option<String>,
    /// Whether policy/RLS enforcement happens inside ProximaDB before rows leave this layout.
    pub policy_enforced_in_proxima: bool,
    /// Names of ProximaType fields that require lossy conversion in this format, if any.
    pub lossy_type_mappings: Vec<String>,
    /// Additional implementation-specific metadata.
    pub properties: HashMap<String, String>,
}

impl Default for CatalogStorageLayout {
    fn default() -> Self {
        Self {
            name: "primary".to_string(),
            authority: CatalogAuthorityMode::InternalCanonical,
            layout_kind: CatalogStorageLayoutKind::RowRecord,
            physical_format: CatalogPhysicalFormat::ProximaBlock,
            write_mode: CatalogWriteMode::Mutable,
            location: None,
            snapshot_semantics: None,
            policy_enforced_in_proxima: true,
            lossy_type_mappings: Vec::new(),
            properties: HashMap::new(),
        }
    }
}

impl CatalogStorageLayout {
    /// Create an internal canonical layout with default policy enforcement.
    pub fn internal(name: impl Into<String>, layout_kind: CatalogStorageLayoutKind) -> Self {
        Self {
            name: name.into(),
            authority: CatalogAuthorityMode::ProximaAuthoritative,
            layout_kind,
            ..Default::default()
        }
    }

    /// Create the recommended ProximaDB-authoritative PAX relational layout.
    pub fn proxima_authoritative_pax(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            authority: CatalogAuthorityMode::ProximaAuthoritative,
            layout_kind: CatalogStorageLayoutKind::Pax,
            physical_format: CatalogPhysicalFormat::ProximaBlock,
            write_mode: CatalogWriteMode::Mutable,
            snapshot_semantics: Some("mvcc".to_string()),
            policy_enforced_in_proxima: true,
            ..Default::default()
        }
    }

    /// Create an external authoritative table mapping.
    pub fn external_authoritative(
        name: impl Into<String>,
        format: CatalogPhysicalFormat,
        location: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            authority: CatalogAuthorityMode::ExternalAuthoritative,
            layout_kind: CatalogStorageLayoutKind::ExternalTable,
            physical_format: format,
            write_mode: CatalogWriteMode::ExternalRefresh,
            location: Some(location.into()),
            snapshot_semantics: Some("external-latest".to_string()),
            policy_enforced_in_proxima: false,
            ..Default::default()
        }
    }

    /// Create a rebuildable external/open-format publication from ProximaDB-owned records.
    pub fn projection_publication(
        name: impl Into<String>,
        format: CatalogPhysicalFormat,
        location: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            authority: CatalogAuthorityMode::ProjectionPublication,
            layout_kind: CatalogStorageLayoutKind::ExternalTable,
            physical_format: format,
            write_mode: CatalogWriteMode::CopyOnWrite,
            location: Some(location.into()),
            snapshot_semantics: Some("published-snapshot".to_string()),
            policy_enforced_in_proxima: true,
            ..Default::default()
        }
    }

    /// Create a rebuildable internal specialty projection/access-method layout.
    pub fn specialty_projection(
        name: impl Into<String>,
        layout_kind: CatalogStorageLayoutKind,
        physical_format: CatalogPhysicalFormat,
    ) -> Self {
        Self {
            name: name.into(),
            authority: CatalogAuthorityMode::RebuildableProjection,
            layout_kind,
            physical_format,
            write_mode: CatalogWriteMode::ReadOnly,
            snapshot_semantics: Some("rebuildable-projection".to_string()),
            policy_enforced_in_proxima: true,
            ..Default::default()
        }
    }

    /// Create a point-in-time imported snapshot. ProximaDB owns records after import.
    pub fn imported_snapshot(
        name: impl Into<String>,
        format: CatalogPhysicalFormat,
        location: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            authority: CatalogAuthorityMode::ImportedSnapshot,
            layout_kind: CatalogStorageLayoutKind::ExternalTable,
            physical_format: format,
            write_mode: CatalogWriteMode::ReadOnly,
            location: Some(location.into()),
            snapshot_semantics: Some("imported-snapshot".to_string()),
            policy_enforced_in_proxima: true,
            ..Default::default()
        }
    }

    /// Create a federated read mapping without importing or owning record state.
    pub fn federated_read(
        name: impl Into<String>,
        format: CatalogPhysicalFormat,
        location: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            authority: CatalogAuthorityMode::FederatedRead,
            layout_kind: CatalogStorageLayoutKind::ExternalTable,
            physical_format: format,
            write_mode: CatalogWriteMode::ReadOnly,
            location: Some(location.into()),
            snapshot_semantics: Some("external-snapshot".to_string()),
            policy_enforced_in_proxima: false,
            ..Default::default()
        }
    }

    /// Whether this layout needs an explicit external ownership/policy contract.
    pub fn requires_external_contract(&self) -> bool {
        matches!(
            self.authority,
            CatalogAuthorityMode::ExternalAuthoritative | CatalogAuthorityMode::FederatedRead
        )
    }
}

/// Catalog metadata for a rebuildable projection/access method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProjection {
    /// Projection name.
    pub name: String,
    /// Projection family.
    pub kind: CatalogProjectionKind,
    /// Physical format backing the projection.
    pub physical_format: CatalogPhysicalFormat,
    /// Canonical layout/table/stream used to rebuild this projection.
    pub rebuild_source: String,
    /// Freshness semantics.
    pub freshness: ProjectionFreshness,
    /// Runtime freshness state used by planners and EXPLAIN.
    #[serde(default)]
    pub freshness_state: ProjectionFreshnessState,
    /// Maximum accepted lag in milliseconds for bounded-lag projections.
    /// Required when freshness == BoundedLag. None means "no bound declared"
    /// which the planner treats as Stale-OK. For ANN freshness, this directly
    /// impacts recall: new records not yet in the HNSW index are missed results.
    pub max_lag_ms: Option<i64>,
    /// Source WAL range, snapshot, or manifest used by this projection.
    #[serde(default)]
    pub source_range: Option<String>,
    /// Last commit, snapshot, or manifest included by this projection.
    #[serde(default)]
    pub last_included_position: Option<String>,
    /// Whether the projection can be rebuilt without data loss.
    pub rebuildable: bool,
    /// Quantitative rebuild time objective. None = unverified / not benchmarked.
    /// Planners should reject routing through this projection for latency-sensitive
    /// queries if an active rebuild has exceeded `rto.max_rebuild_wait_seconds`.
    #[serde(default)]
    pub rebuild_rto: Option<RebuildRtoSpec>,
    /// Invalidation policy, e.g. synchronous, enqueue, mark-stale, rebuild-required.
    #[serde(default)]
    pub invalidation_policy: Option<String>,
    /// Policy/RLS boundary for this projection.
    #[serde(default)]
    pub policy_boundary: Option<String>,
    /// Whether using this projection can change recall or ranking quality.
    pub lossy: bool,
    /// Benchmark or smoke gate required before supported/default use.
    #[serde(default)]
    pub benchmark_gate: Option<String>,
    /// Human-readable support status: experimental, beta, supported, deprecated.
    pub support_status: String,
    /// Additional implementation-specific metadata.
    pub properties: HashMap<String, String>,
}

impl CatalogProjection {
    /// Create a rebuildable projection with lazy freshness by default.
    pub fn rebuildable(
        name: impl Into<String>,
        kind: CatalogProjectionKind,
        rebuild_source: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            physical_format: CatalogPhysicalFormat::ProximaBlock,
            rebuild_source: rebuild_source.into(),
            freshness: ProjectionFreshness::Lazy,
            freshness_state: ProjectionFreshnessState::Fresh,
            max_lag_ms: None,
            source_range: None,
            last_included_position: None,
            rebuildable: true,
            rebuild_rto: None,
            invalidation_policy: Some("mark-stale".to_string()),
            policy_boundary: Some("engine-enforced".to_string()),
            lossy: false,
            benchmark_gate: None,
            support_status: "experimental".to_string(),
            properties: HashMap::new(),
        }
    }

    /// Declare bounded-lag freshness for planner and EXPLAIN decisions.
    pub fn with_bounded_lag(mut self, max_lag_ms: i64) -> Self {
        self.freshness = ProjectionFreshness::BoundedLag;
        self.max_lag_ms = Some(max_lag_ms);
        self
    }

    /// Attach a quantitative rebuild RTO specification.
    /// Without this, planners must treat the projection as having an
    /// unknown rebuild duration and may refuse to use it in SLA-critical paths.
    pub fn with_rebuild_rto(mut self, rto: RebuildRtoSpec) -> Self {
        self.rebuild_rto = Some(rto);
        self
    }

    /// Update runtime freshness state.
    pub fn with_freshness_state(mut self, state: ProjectionFreshnessState) -> Self {
        self.freshness_state = state;
        self
    }

    /// Record projection lineage used for repair and freshness checks.
    pub fn with_lineage(
        mut self,
        source_range: impl Into<String>,
        last_included_position: impl Into<String>,
    ) -> Self {
        self.source_range = Some(source_range.into());
        self.last_included_position = Some(last_included_position.into());
        self
    }

    /// Record policy boundary and benchmark gate metadata.
    pub fn with_policy_and_gate(
        mut self,
        policy_boundary: impl Into<String>,
        benchmark_gate: impl Into<String>,
    ) -> Self {
        self.policy_boundary = Some(policy_boundary.into());
        self.benchmark_gate = Some(benchmark_gate.into());
        self
    }
}

// ─── ANN Filtering ───────────────────────────────────────────────────────────

/// Selectivity-based routing mode for approximate nearest-neighbor (ANN) queries
/// that carry scalar filter predicates. Determined by the planner from catalog
/// statistics and exposed in EXPLAIN output (ADR-004 FilterDiagnostics).
///
/// Literature basis: FAVOR (arXiv:2605.07770), Filtered ANN Survey
/// (arXiv:2602.11443), Learning-based Filtered-ANN planning (arXiv:2602.17914).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AnnFilteringMode {
    /// Run the scalar filter first, then ANN over the surviving candidate set.
    /// Correct when selectivity is very low (< `pre_filter_threshold`).
    /// Avoids expensive ANN traversal over the full corpus.
    PreFilter,
    /// Interleave scalar predicate evaluation during HNSW/IVF graph traversal.
    /// Correct for moderate selectivity (`pre_filter_threshold`..`post_filter_threshold`).
    /// Requires predicate-aware graph index support (AXIS primitive).
    #[default]
    Inline,
    /// Run ANN first for a larger candidate set, then filter results.
    /// Correct when selectivity is high (> `post_filter_threshold`) so that
    /// nearly all ANN neighbors survive the filter anyway.
    PostFilter,
}

impl AnnFilteringMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreFilter => "pre_filter",
            Self::Inline => "inline",
            Self::PostFilter => "post_filter",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pre_filter" | "prefilter" | "pre" => Some(Self::PreFilter),
            "inline" | "intra_filter" | "intra" => Some(Self::Inline),
            "post_filter" | "postfilter" | "post" => Some(Self::PostFilter),
            _ => None,
        }
    }
}

/// Catalog-level policy controlling how the planner selects `AnnFilteringMode`.
///
/// Thresholds are selectivity fractions in [0.0, 1.0] where lower = fewer
/// records pass the filter.  The planner reads `estimated_selectivity` from
/// `FilterDiagnostics` (ADR-004) and routes as follows:
///
/// ```text
/// selectivity < pre_filter_threshold   → PreFilter
/// selectivity > post_filter_threshold  → PostFilter
/// otherwise                            → Inline
/// ```
///
/// Defaults are empirically grounded in the FAVOR paper (2605.07770) and the
/// Filtered ANN Survey (2602.11443):
///   pre_filter_threshold  = 0.05  (< 5 % of corpus passes filter → pre-filter)
///   post_filter_threshold = 0.50  (> 50 % passes filter → post-filter)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnFilteringPolicy {
    /// Selectivity below which PreFilter is preferred. Default 0.05.
    pub pre_filter_threshold: f64,
    /// Selectivity above which PostFilter is preferred. Default 0.50.
    pub post_filter_threshold: f64,
    /// Override: force a specific mode regardless of selectivity.
    pub force_mode: Option<AnnFilteringMode>,
    /// Minimum candidate oversample factor used in PostFilter mode to compensate
    /// for filter rejection. Default 2.0 (fetch 2× top-k before filtering).
    pub post_filter_oversample_factor: f64,
}

impl Default for AnnFilteringPolicy {
    fn default() -> Self {
        Self {
            pre_filter_threshold: 0.05,
            post_filter_threshold: 0.50,
            force_mode: None,
            post_filter_oversample_factor: 2.0,
        }
    }
}

impl AnnFilteringPolicy {
    /// Determine the routing mode from an estimated selectivity fraction.
    pub fn routing_mode(&self, estimated_selectivity: f64) -> AnnFilteringMode {
        if let Some(forced) = self.force_mode {
            return forced;
        }
        if estimated_selectivity < self.pre_filter_threshold {
            AnnFilteringMode::PreFilter
        } else if estimated_selectivity > self.post_filter_threshold {
            AnnFilteringMode::PostFilter
        } else {
            AnnFilteringMode::Inline
        }
    }

    /// Effective top-k to request from ANN in PostFilter mode (before scalar filtering).
    pub fn effective_top_k_for_post_filter(&self, top_k: usize) -> usize {
        ((top_k as f64) * self.post_filter_oversample_factor).ceil() as usize
    }
}

// ─── Projection Rebuild RTO ───────────────────────────────────────────────────

/// Quantitative rebuild time objective for a rebuildable L2 projection.
///
/// These bounds are operational commitments, not aspirational. A missing RTO
/// means the projection has not been benchmarked and MUST NOT be relied upon
/// for latency-sensitive query paths after a rebuild event.
///
/// Background: ADR-010 mandates that PAX blocks are Layer 2 (rebuildable
/// projections). Without an RTO bound, "rebuildable" is an architectural
/// claim, not an operational guarantee.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildRtoSpec {
    /// Estimated rebuild duration in seconds per 10 GB of L1 (ProximaRecord) data.
    /// Planner uses this to estimate total rebuild time from `size_bytes`.
    pub rebuild_seconds_per_10gb: f64,
    /// Whether queries fall back to full L1 scans while this projection rebuilds.
    /// If false, queries on this projection are rejected or degrade to `Unavailable`.
    pub degraded_scan_during_rebuild: bool,
    /// Maximum tolerated rebuild lag at which point queries must route around this
    /// projection. If the rebuild has been running longer than this, the planner
    /// marks the projection `Unavailable` for planning purposes.
    pub max_rebuild_wait_seconds: f64,
    /// Whether this projection supports incremental rebuild from a WAL checkpoint
    /// (faster) or requires a full L1 scan (slower).
    pub supports_incremental_rebuild: bool,
    pub estimate_source: RebuildEstimateSource,
}

const BYTES_PER_10_GIB: f64 = 10.0 * 1024.0 * 1024.0 * 1024.0;

impl Default for RebuildRtoSpec {
    fn default() -> Self {
        Self {
            rebuild_seconds_per_10gb: 60.0,
            degraded_scan_during_rebuild: true,
            max_rebuild_wait_seconds: 300.0,
            supports_incremental_rebuild: false,
            estimate_source: RebuildEstimateSource::Unverified,
        }
    }
}

impl RebuildRtoSpec {
    /// Estimate total rebuild duration in seconds given a projected data volume.
    pub fn estimated_rebuild_seconds(&self, size_bytes: u64) -> f64 {
        let size_gb = (size_bytes as f64) / BYTES_PER_10_GIB;
        size_gb * self.rebuild_seconds_per_10gb
    }

    /// Return an RTO spec for a benchmarked HNSW projection over L1 PAX blocks.
    /// Grounded in FGIM (arXiv:2603.21710) graph-index merge benchmarks.
    pub fn hnsw_benchmarked() -> Self {
        Self {
            rebuild_seconds_per_10gb: 45.0,
            degraded_scan_during_rebuild: true,
            max_rebuild_wait_seconds: 600.0,
            supports_incremental_rebuild: true,
            estimate_source: RebuildEstimateSource::Benchmarked,
        }
    }

    /// Return an RTO spec for a CSR graph topology projection.
    /// CSR rebuild requires a full sorted adjacency scan from L1 edge records.
    pub fn csr_estimated() -> Self {
        Self {
            rebuild_seconds_per_10gb: 90.0,
            degraded_scan_during_rebuild: false,
            max_rebuild_wait_seconds: 900.0,
            supports_incremental_rebuild: false,
            estimate_source: RebuildEstimateSource::Estimated,
        }
    }

    /// Return an RTO spec for a columnar analytics projection (PAX → Parquet).
    pub fn columnar_estimated() -> Self {
        Self {
            rebuild_seconds_per_10gb: 30.0,
            degraded_scan_during_rebuild: true,
            max_rebuild_wait_seconds: 300.0,
            supports_incremental_rebuild: true,
            estimate_source: RebuildEstimateSource::Estimated,
        }
    }
}

// ─── Props Auto-Promotion ─────────────────────────────────────────────────────

/// When props auto-promotion is evaluated: during background compaction or only
/// on explicit DDL (`ALTER TABLE ... PROMOTE PROPS KEY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PropsEvaluationCadence {
    /// Evaluate promotion candidates automatically during compaction.
    #[default]
    Compaction,
    /// Only promote when the operator issues an explicit DDL command.
    Explicit,
}

impl PropsEvaluationCadence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compaction => "compaction",
            Self::Explicit => "explicit",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "compaction" | "auto" => Some(Self::Compaction),
            "explicit" | "manual" | "ddl" => Some(Self::Explicit),
            _ => None,
        }
    }
}

/// Source of a `RebuildRtoSpec` estimate, distinguishing measured from assumed values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RebuildEstimateSource {
    /// Measured in a controlled benchmark against this deployment's hardware.
    Benchmarked,
    /// Derived from published benchmarks or analogous system measurements.
    Estimated,
    /// No measurement; the value is a placeholder that must not be used for SLA commitments.
    #[default]
    Unverified,
}

impl RebuildEstimateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Benchmarked => "benchmarked",
            Self::Estimated => "estimated",
            Self::Unverified => "unverified",
        }
    }
}

/// Policy for automatically promoting high-frequency document props keys to
/// typed PAX/Iceberg columns. Without promotion, props are stored as opaque
/// msgpack blobs (column ID 8) and cannot be pruned at column or block level,
/// making document queries over `props` effectively full-block scans.
///
/// Promotion creates a new user-defined column (IDs 100+) and reindexes the
/// promoted key into that column during the next compaction cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropsAutoPromotionPolicy {
    pub enabled: bool,
    /// Fraction of records that must contain a key before it is promoted.
    /// Default 0.50 — key must appear in ≥ 50 % of records.
    pub frequency_threshold: f64,
    /// Minimum number of records required before promotion eligibility is evaluated.
    /// Prevents premature promotion on sparse datasets. Default 10_000.
    pub min_record_count: u64,
    /// Maximum number of auto-promoted props columns per table. Default 32.
    pub max_promoted_columns: u32,
    /// When promotion candidates are evaluated.
    pub evaluation_cadence: PropsEvaluationCadence,
    /// `CatalogDataType` variants eligible for promotion. Keys whose observed
    /// values do not match an eligible type are retained in the msgpack blob.
    pub eligible_types: Vec<CatalogDataType>,
    /// Keys that have already been promoted, mapping props key → `props__<key>` column name.
    /// Written by `SchemaChange::PromotePropsKey` and read by the compaction writer
    /// to know which msgpack keys to route into typed columns.
    #[serde(default)]
    pub promoted_keys: HashMap<String, String>,
}

impl Default for PropsAutoPromotionPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            frequency_threshold: 0.50,
            min_record_count: 10_000,
            max_promoted_columns: 32,
            evaluation_cadence: PropsEvaluationCadence::Compaction,
            eligible_types: vec![
                CatalogDataType::String,
                CatalogDataType::Int64,
                CatalogDataType::Float64,
                CatalogDataType::Boolean,
                CatalogDataType::TimestampTz,
            ],
            promoted_keys: HashMap::new(),
        }
    }
}

impl PropsAutoPromotionPolicy {
    /// Return the default policy for document-heavy tables.
    /// Enables promotion with standard thresholds.
    pub fn document_default() -> Self {
        Self {
            enabled: true,
            ..Default::default()
        }
    }
}

// ─── Observability Compression Hint ──────────────────────────────────────────

/// Compression strategy hint for observability / time-series ProximaRecords.
///
/// A ProximaRecord per metric sample loses Gorilla-style (Pelkonen et al.,
/// VLDB 2015) cross-sample compression. This hint instructs the storage layer
/// to apply series-aware compression during compaction when PAX blocks are
/// written for observability tables.
///
/// The hint does NOT change the semantic record model — each write remains a
/// canonical ProximaRecord. Compression is applied by the PAX block writer
/// when sorting by (series_key_column, timestamp_column) and encoding the
/// resulting column stripe with delta-delta + varint encoding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityCompressionHint {
    /// Column name that identifies the time-series key (e.g., metric name + labels hash).
    pub series_key_column: String,
    /// Column name of the timestamp in nanoseconds (usually `__proxima_created_at_ns`).
    pub timestamp_ns_column: String,
    /// Column name of the numeric value to delta-delta encode.
    pub value_column: String,
    /// Target block sort order: sort by (series_key, timestamp_ns) before encoding.
    pub sort_by_series_key: bool,
    /// Delta-delta bit width for timestamp encoding. 0 = auto-select.
    pub timestamp_delta_bits: u8,
    /// Gorilla-style XOR encoding for float value column.
    pub xor_float_encoding: bool,
}

impl Default for ObservabilityCompressionHint {
    fn default() -> Self {
        Self {
            // "series_key" is the conventional user-defined column; operator must
            // set this to the actual metric-name+labels hash column for the table.
            series_key_column: "series_key".to_string(),
            timestamp_ns_column: "__proxima_created_at_ns".to_string(),
            value_column: "value".to_string(),
            sort_by_series_key: true,
            timestamp_delta_bits: 0,
            xor_float_encoding: true,
        }
    }
}

// ─── Protocol Type Coercion ───────────────────────────────────────────────────

/// Lossy conversion descriptor for a single ProximaType when exposed through
/// a specific protocol. Used by planners and EXPLAIN to surface type fidelity
/// warnings before data is written to or read from an external protocol surface.
///
/// See ADR-013 for the full coercion matrix across pgwire, Arrow Flight,
/// Iceberg REST, and REST/gRPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolTypeCoercionNote {
    /// The ProximaType field or type name being coerced.
    pub proxima_type: String,
    /// Target protocol surface: "pgwire", "arrow_flight", "iceberg_rest", "rest_grpc".
    pub protocol: String,
    /// The wire type used in the protocol.
    pub wire_type: String,
    /// Whether the conversion is lossless.
    pub lossless: bool,
    /// Description of what information is lost or altered.
    pub loss_description: Option<String>,
}

impl ProtocolTypeCoercionNote {
    /// Standard coercion table entries grounded in ADR-007 and ADR-013.
    ///
    /// Returns a reference to a process-lifetime static; do not call this per-query
    /// once wired into EXPLAIN — clone the slice once at plan construction time.
    pub fn standard_notes() -> &'static [Self] {
        static NOTES: OnceLock<Vec<ProtocolTypeCoercionNote>> = OnceLock::new();
        NOTES.get_or_init(|| {
            vec![
                Self {
                    proxima_type: "ProximaValue::Map(string, ProximaValue)".to_string(),
                    protocol: "iceberg_rest".to_string(),
                    wire_type: "map<string, binary> (msgpack)".to_string(),
                    lossless: false,
                    loss_description: Some(
                        "Spark/Trino cannot push predicates into msgpack binary; \
                         use props auto-promotion (PropsAutoPromotionPolicy) to \
                         promote high-frequency keys to typed columns."
                            .to_string(),
                    ),
                },
                Self {
                    proxima_type: "Embedding([f32; dim])".to_string(),
                    protocol: "iceberg_rest".to_string(),
                    wire_type: "list<float> (dimension in table properties)".to_string(),
                    lossless: false,
                    loss_description: Some(
                        "Iceberg list<float> does not carry fixed dimension. \
                         Dimension is stored in table property 'proximadb.dim_<model_id>'. \
                         Clients must read the property to validate vector dimensions."
                            .to_string(),
                    ),
                },
                Self {
                    proxima_type: "Embedding([f32; dim])".to_string(),
                    protocol: "pgwire".to_string(),
                    wire_type: "float4[] (pgvector compatible)".to_string(),
                    lossless: true,
                    loss_description: None,
                },
                Self {
                    proxima_type: "Embedding([f32; dim])".to_string(),
                    protocol: "arrow_flight".to_string(),
                    wire_type: "FixedSizeList<float32>[dim]".to_string(),
                    lossless: true,
                    loss_description: None,
                },
                Self {
                    proxima_type: "ProximaValue::Variant".to_string(),
                    protocol: "iceberg_rest".to_string(),
                    wire_type: "binary (not representable as Iceberg column)".to_string(),
                    lossless: false,
                    loss_description: Some(
                        "Iceberg does not have a union/variant type. \
                         Variant values serialize as opaque binary. \
                         Use a typed column instead."
                            .to_string(),
                    ),
                },
                Self {
                    proxima_type: "Edge { source, target, edge_type, weight }".to_string(),
                    protocol: "pgwire".to_string(),
                    wire_type: "4 separate nullable columns (no native graph type)".to_string(),
                    lossless: true,
                    loss_description: None,
                },
            ]
        })
    }
}

/// Optional relational semantics for a table/profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelationalCapabilities {
    /// Primary key columns, if enforced.
    pub primary_key: Vec<String>,
    /// Unique indexes or constraints.
    pub unique_indexes: Vec<CatalogIndex>,
    /// Non-unique secondary indexes.
    pub secondary_indexes: Vec<CatalogIndex>,
    /// Foreign-key and check constraints.
    pub constraints: Vec<ColumnConstraint>,
    /// Materialized view names derived from this table/profile.
    pub materialized_views: Vec<String>,
    /// Transaction/isolation profile name.
    pub transaction_profile: Option<String>,
    /// Schema evolution policy name.
    pub schema_evolution_policy: Option<String>,
}

impl RelationalCapabilities {
    /// Returns true when this table opts into any relational integrity semantics.
    pub fn has_enforced_semantics(&self) -> bool {
        !self.primary_key.is_empty()
            || !self.unique_indexes.is_empty()
            || !self.secondary_indexes.is_empty()
            || !self.constraints.is_empty()
            || !self.materialized_views.is_empty()
            || self.transaction_profile.is_some()
            || self.schema_evolution_policy.is_some()
    }
}

/// Table statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogTableStatistics {
    /// Row count
    pub row_count: u64,
    /// Size in bytes
    pub size_bytes: u64,
    /// Number of files
    pub file_count: u64,
    /// Last analyze timestamp
    pub last_analyzed_ms: Option<i64>,
    /// Column statistics
    pub column_stats: HashMap<String, CatalogColumnStatistics>,
}

impl CatalogTableStatistics {
    /// Returns true if these statistics should be treated as stale by the planner.
    ///
    /// Stats are stale when:
    /// - `last_analyzed_ms` is `None` (never updated), or
    /// - `now_ms - last_analyzed_ms > ttl_ms` (older than the configured freshness window).
    ///
    /// Planners should fall back to defaults (or trigger a refresh) when this returns true.
    pub fn is_stale(&self, now_ms: i64, ttl_ms: i64) -> bool {
        match self.last_analyzed_ms {
            None => true,
            Some(last_ms) => now_ms.saturating_sub(last_ms) > ttl_ms,
        }
    }
}

/// Column statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogColumnStatistics {
    /// Number of distinct values
    pub distinct_count: Option<u64>,
    /// Number of null values
    pub null_count: Option<u64>,
    /// Min value (as string)
    pub min_value: Option<String>,
    /// Max value (as string)
    pub max_value: Option<String>,
    /// Average size in bytes
    pub avg_size_bytes: Option<f64>,
}

/// Partition specification
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogPartitionSpec {
    /// Spec ID
    pub spec_id: i32,
    /// Partition fields
    pub fields: Vec<CatalogPartitionField>,
}

/// Partition field (Iceberg-compatible)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogPartitionField {
    /// Source column ID
    pub source_id: i32,
    /// Field ID (assigned by partition spec)
    pub field_id: i32,
    /// Field name
    pub name: String,
    /// Transform type
    pub transform: PartitionTransform,
}

/// Type alias for backwards compatibility
pub type PartitionField = CatalogPartitionField;

/// Partition transforms (Iceberg-compatible)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PartitionTransform {
    /// Identity transform (no transformation)
    #[default]
    Identity,
    /// Bucket transform (hash into N buckets)
    Bucket(u32),
    /// Truncate transform (truncate to width)
    Truncate(u32),
    /// Year transform (extract year from timestamp/date)
    Year,
    /// Month transform (extract month from timestamp/date)
    Month,
    /// Day transform (extract day from timestamp/date)
    Day,
    /// Hour transform (extract hour from timestamp)
    Hour,
    /// Void transform (always produces null)
    Void,
}

impl std::fmt::Display for PartitionTransform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartitionTransform::Identity => write!(f, "identity"),
            PartitionTransform::Bucket(n) => write!(f, "bucket[{}]", n),
            PartitionTransform::Truncate(w) => write!(f, "truncate[{}]", w),
            PartitionTransform::Year => write!(f, "year"),
            PartitionTransform::Month => write!(f, "month"),
            PartitionTransform::Day => write!(f, "day"),
            PartitionTransform::Hour => write!(f, "hour"),
            PartitionTransform::Void => write!(f, "void"),
        }
    }
}

impl PartitionTransform {
    /// Parse from string (Iceberg format)
    pub fn parse_from_iceberg_format(s: &str) -> Self {
        let lower = s.to_lowercase();
        if lower == "identity" {
            PartitionTransform::Identity
        } else if lower.starts_with("bucket[") {
            let n: u32 = lower
                .trim_start_matches("bucket[")
                .trim_end_matches(']')
                .parse()
                .unwrap_or(16);
            PartitionTransform::Bucket(n)
        } else if lower.starts_with("truncate[") {
            let w: u32 = lower
                .trim_start_matches("truncate[")
                .trim_end_matches(']')
                .parse()
                .unwrap_or(16);
            PartitionTransform::Truncate(w)
        } else if lower == "year" {
            PartitionTransform::Year
        } else if lower == "month" {
            PartitionTransform::Month
        } else if lower == "day" {
            PartitionTransform::Day
        } else if lower == "hour" {
            PartitionTransform::Hour
        } else if lower == "void" {
            PartitionTransform::Void
        } else {
            PartitionTransform::Identity
        }
    }
}

/// Sort order
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogSortOrder {
    /// Order ID
    pub order_id: i32,
    /// Sort fields
    pub fields: Vec<CatalogSortField>,
}

/// Sort field (Iceberg-compatible)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSortField {
    /// Source column ID
    pub source_id: i32,
    /// Transform to apply before sorting
    pub transform: PartitionTransform,
    /// Sort direction
    pub direction: SortDirection,
    /// Null ordering
    pub null_order: NullOrder,
}

/// Type alias for backwards compatibility
pub type SortField = CatalogSortField;

/// Sort direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    /// Sort values from smallest to largest (ASC)
    Ascending,
    /// Sort values from largest to smallest (DESC)
    Descending,
}

/// Null ordering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NullOrder {
    /// NULL values sort before non-NULL values
    NullsFirst,
    /// NULL values sort after non-NULL values
    NullsLast,
}

/// Schema evolution request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSchemaEvolution {
    /// Changes to apply
    pub changes: Vec<SchemaChange>,
}

/// Schema change types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchemaChange {
    /// Add a new column
    AddColumn {
        /// Name of the new column
        name: String,
        /// Data type of the new column
        data_type: CatalogDataType,
        /// Whether the column accepts NULL values
        nullable: bool,
        /// Optional SQL expression used as the column default
        default_value: Option<String>,
        /// Optional human-readable description of the column
        comment: Option<String>,
        /// Position: None = end, Some(col) = after column
        after: Option<String>,
    },
    /// Drop a column
    DropColumn {
        /// Name of the column to drop
        name: String,
    },
    /// Rename a column
    RenameColumn {
        /// Current column name
        old_name: String,
        /// Desired new column name
        new_name: String,
    },
    /// Change column type (must be compatible)
    ChangeType {
        /// Name of the column whose type should change
        name: String,
        /// New data type (must be compatible with the existing type)
        new_type: CatalogDataType,
    },
    /// Update column comment
    UpdateComment {
        /// Name of the column to update
        name: String,
        /// New comment text
        comment: String,
    },
    /// Make column nullable (DROP NOT NULL)
    MakeNullable {
        /// Name of the column to make nullable
        name: String,
    },
    /// Make column NOT NULL (SET NOT NULL)
    MakeNotNullable {
        /// Name of the column to make non-nullable
        name: String,
    },
    /// Add default value
    SetDefault {
        /// Name of the column to update
        name: String,
        /// SQL expression to use as the default value
        default_value: String,
    },
    /// Remove default value
    DropDefault {
        /// Name of the column whose default should be removed
        name: String,
    },
    /// Move column position (FIRST or AFTER another column)
    MoveColumn {
        /// Name of the column to move
        name: String,
        /// Position: None = FIRST, Some(col) = AFTER column
        after: Option<String>,
    },
    /// Add a constraint to a column or table
    AddConstraint {
        /// Constraint name (optional for some DBs)
        constraint_name: Option<String>,
        /// Constraint type
        constraint: ColumnConstraint,
    },
    /// Drop a constraint
    DropConstraint {
        /// Name of the constraint to drop
        constraint_name: String,
    },
    /// Promote a high-frequency props key to a typed PAX/Iceberg column.
    ///
    /// The promoted column name follows the `props__<key>` convention (double
    /// underscore) so it is distinguishable from user-defined schema columns.
    /// The storage layer re-routes the key from the msgpack blob (column ID 8)
    /// into the new typed column during the next compaction cycle.
    PromotePropsKey {
        /// The key inside the `props` msgpack blob to promote.
        key: String,
        /// Target catalog data type for the promoted column.
        column_type: CatalogDataType,
        /// Optional human-readable description for the new column.
        comment: Option<String>,
    },
    /// Set a table-level option (ALTER TABLE … SET (key = 'value')).
    ///
    /// Recognised option keys:
    /// - `props_auto_promotion`: `'enabled'` / `'disabled'`
    SetTableOption {
        /// Option name (case-insensitive).
        key: String,
        /// Option value.
        value: String,
    },
}

/// Column constraint types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnConstraint {
    /// UNIQUE constraint on one or more columns
    Unique {
        /// Column names included in the uniqueness constraint
        columns: Vec<String>,
    },
    /// CHECK constraint with SQL expression
    Check {
        /// SQL boolean expression that rows must satisfy
        expression: String,
    },
    /// FOREIGN KEY constraint
    ForeignKey {
        /// Local column names that form the foreign key
        columns: Vec<String>,
        /// Name of the referenced table
        references_table: String,
        /// Column names in the referenced table
        references_columns: Vec<String>,
        /// Action to take when the referenced row is deleted
        on_delete: Option<ReferentialAction>,
        /// Action to take when the referenced row is updated
        on_update: Option<ReferentialAction>,
    },
}

/// Referential actions for foreign key constraints
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferentialAction {
    /// CASCADE - propagate changes to referencing rows
    Cascade,
    /// SET NULL - set referencing columns to NULL
    SetNull,
    /// SET DEFAULT - set referencing columns to their default values
    SetDefault,
    /// RESTRICT - prevent the action if there are referencing rows
    Restrict,
    /// NO ACTION - similar to RESTRICT but deferred
    NoAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namespace_fqn() {
        let ns = CatalogNamespace::new(vec!["catalog".into(), "database".into()]);
        assert_eq!(ns.fqn(), "catalog.database");
    }

    #[test]
    fn namespace_defaults_are_legacy_compatible() {
        // Legacy callers that build a namespace without DR fields get
        // backwards-compatible defaults: no tenant/namespace ID, no
        // region home, Pooled pool class. The migration backfills these.
        let ns = CatalogNamespace::new(vec!["legacy".into()]);
        assert!(ns.namespace_id.is_none());
        assert!(ns.tenant_id.is_none());
        assert!(ns.region_home.is_none());
        assert!(ns.default_dr_region_pair_id.is_none());
        assert_eq!(ns.storage_pool_class, StoragePoolClass::Pooled);
        assert!(!ns.is_dr_addressable());
    }

    #[test]
    fn namespace_dr_builders_compose() {
        let ns = CatalogNamespace::new(vec!["catalog".into(), "db".into()])
            .with_tenant("tnt_acme")
            .with_namespace_id("ns_01HX7Q8K2N5R9P3M1B2C3D4E5F")
            .with_region_home("us-east-1")
            .with_default_dr_region_pair("aws:us-east-1:us-west-2")
            .with_storage_pool_class(StoragePoolClass::Business);

        assert_eq!(ns.tenant_id.as_deref(), Some("tnt_acme"));
        assert_eq!(
            ns.namespace_id.as_deref(),
            Some("ns_01HX7Q8K2N5R9P3M1B2C3D4E5F"),
        );
        assert_eq!(ns.region_home.as_deref(), Some("us-east-1"));
        assert_eq!(
            ns.default_dr_region_pair_id.as_deref(),
            Some("aws:us-east-1:us-west-2"),
        );
        assert_eq!(ns.storage_pool_class, StoragePoolClass::Business);
        assert!(ns.is_dr_addressable());
    }

    #[test]
    fn namespace_serde_round_trips_legacy_rows() {
        // A namespace serialized before this migration has none of the
        // new fields. Deserializing must succeed and leave them at
        // their defaults.
        let legacy_json = r#"{
            "levels": ["db", "schema"],
            "properties": {},
            "owner": null,
            "location": null,
            "created_at_ms": 1000,
            "updated_at_ms": 2000
        }"#;
        let ns: CatalogNamespace = serde_json::from_str(legacy_json)
            .expect("legacy namespace JSON must deserialize");
        assert!(ns.namespace_id.is_none());
        assert!(ns.tenant_id.is_none());
        assert_eq!(ns.storage_pool_class, StoragePoolClass::Pooled);

        // Re-serializing must skip the None fields so legacy consumers
        // still see only the federation fields.
        let reserialized = serde_json::to_string(&ns).expect("serialize");
        assert!(!reserialized.contains("namespace_id"));
        assert!(!reserialized.contains("tenant_id"));
        assert!(!reserialized.contains("region_home"));
        assert!(!reserialized.contains("default_dr_region_pair_id"));
        // `storage_pool_class` is non-Option so it does show up; that's
        // expected because legacy rows backfill to "pooled".
        assert!(reserialized.contains("\"storage_pool_class\":\"pooled\""));
    }

    #[test]
    fn storage_pool_class_serde_uses_snake_case() {
        let classes = [
            (StoragePoolClass::Pooled, "\"pooled\""),
            (StoragePoolClass::Business, "\"business\""),
            (StoragePoolClass::Enterprise, "\"enterprise\""),
            (StoragePoolClass::EnterpriseDedicated, "\"enterprise_dedicated\""),
        ];
        for (variant, expected_json) in classes {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s, expected_json, "variant {variant:?}");
            let back: StoragePoolClass = serde_json::from_str(expected_json).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn storage_pool_class_default_is_pooled() {
        assert_eq!(StoragePoolClass::default(), StoragePoolClass::Pooled);
    }

    #[test]
    fn test_column_builder() {
        let col = CatalogColumn::new(1, "id", CatalogDataType::Int64)
            .nullable(false)
            .with_comment("Primary key");

        assert_eq!(col.name, "id");
        assert!(!col.nullable);
        assert_eq!(col.comment, Some("Primary key".to_string()));
    }

    #[test]
    fn test_table_schema_builder() {
        let schema = CatalogTableSchema::new("users")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int64).nullable(false))
            .with_column(CatalogColumn::new(2, "name", CatalogDataType::String))
            .with_primary_key(vec!["id".into()]);

        assert_eq!(schema.name, "users");
        assert_eq!(schema.columns.len(), 2);
        assert_eq!(schema.primary_key, vec!["id"]);
        assert_eq!(schema.storage_layouts.len(), 1);
        assert_eq!(
            schema.storage_layouts[0].authority,
            CatalogAuthorityMode::InternalCanonical
        );
    }

    #[test]
    fn test_data_type_roundtrip() {
        let types = vec![
            CatalogDataType::Boolean,
            CatalogDataType::Int64,
            CatalogDataType::Float64,
            CatalogDataType::String,
            CatalogDataType::Vector,
        ];

        for dt in types {
            let proto = dt.to_proto_i32();
            let back = CatalogDataType::from_proto_i32(proto);
            assert_eq!(dt, back);
        }
    }

    #[test]
    fn catalog_data_type_proto_and_arrow_mappings_cover_all_variants() {
        let all = [
            (CatalogDataType::Boolean, 1),
            (CatalogDataType::Int8, 2),
            (CatalogDataType::Int16, 3),
            (CatalogDataType::Int32, 4),
            (CatalogDataType::Int64, 5),
            (CatalogDataType::Float32, 6),
            (CatalogDataType::Float64, 7),
            (CatalogDataType::String, 8),
            (CatalogDataType::Binary, 9),
            (CatalogDataType::Date, 10),
            (CatalogDataType::Time, 11),
            (CatalogDataType::Timestamp, 12),
            (CatalogDataType::TimestampTz, 13),
            (CatalogDataType::Decimal, 14),
            (CatalogDataType::Uuid, 15),
            (CatalogDataType::Json, 16),
            (CatalogDataType::Vector, 20),
            (CatalogDataType::SparseVector, 21),
            (CatalogDataType::BinaryVector, 22),
        ];

        for (data_type, proto_id) in all {
            assert_eq!(data_type.to_proto_i32(), proto_id);
            assert_eq!(CatalogDataType::from_proto_i32(proto_id), data_type);
            let _ = data_type.to_arrow_datatype();
        }
        assert_eq!(
            CatalogDataType::from_proto_i32(999),
            CatalogDataType::String
        );

        assert_eq!(
            CatalogDataType::Boolean.to_arrow_datatype(),
            ArrowDataType::Boolean
        );
        assert_eq!(
            CatalogDataType::Int8.to_arrow_datatype(),
            ArrowDataType::Int8
        );
        assert_eq!(
            CatalogDataType::Int16.to_arrow_datatype(),
            ArrowDataType::Int16
        );
        assert_eq!(
            CatalogDataType::Int32.to_arrow_datatype(),
            ArrowDataType::Int32
        );
        assert_eq!(
            CatalogDataType::Int64.to_arrow_datatype(),
            ArrowDataType::Int64
        );
        assert_eq!(
            CatalogDataType::Float32.to_arrow_datatype(),
            ArrowDataType::Float32
        );
        assert_eq!(
            CatalogDataType::Float64.to_arrow_datatype(),
            ArrowDataType::Float64
        );
        assert_eq!(
            CatalogDataType::String.to_arrow_datatype(),
            ArrowDataType::Utf8
        );
        assert_eq!(
            CatalogDataType::Binary.to_arrow_datatype(),
            ArrowDataType::Binary
        );
        assert_eq!(
            CatalogDataType::Date.to_arrow_datatype(),
            ArrowDataType::Date32
        );
        assert!(matches!(
            CatalogDataType::Vector.to_arrow_datatype(),
            ArrowDataType::List(_)
        ));
        assert!(matches!(
            CatalogDataType::SparseVector.to_arrow_datatype(),
            ArrowDataType::Map(_, false)
        ));
    }

    #[test]
    fn workload_and_storage_specialization_options_parse_and_render_stable_names() {
        let workload_cases = [
            ("transactional", CatalogWorkloadProfile::Oltp, "oltp"),
            ("analytics", CatalogWorkloadProfile::Olap, "olap"),
            ("pax", CatalogWorkloadProfile::Htap, "htap"),
            ("ann", CatalogWorkloadProfile::Vector, "vector"),
            ("jsonb", CatalogWorkloadProfile::Document, "document"),
            ("cypher", CatalogWorkloadProfile::Graph, "graph"),
            (
                "time_series",
                CatalogWorkloadProfile::Observability,
                "observability",
            ),
            ("multimodal", CatalogWorkloadProfile::Mixed, "mixed"),
        ];
        for (input, expected, rendered) in workload_cases {
            assert_eq!(CatalogWorkloadProfile::parse(input), Some(expected));
            assert_eq!(expected.as_str(), rendered);
        }
        assert_eq!(CatalogWorkloadProfile::parse("unknown"), None);

        let specialization_cases = [
            (
                "row_record",
                CatalogStorageSpecialization::GenericRelational,
                "generic_relational",
            ),
            (
                "pax",
                CatalogStorageSpecialization::PaxRowFamily,
                "pax_row_family",
            ),
            ("rowdir", CatalogStorageSpecialization::PaxOltp, "pax_oltp"),
            (
                "pax_olap",
                CatalogStorageSpecialization::PaxOlap,
                "pax_olap",
            ),
            (
                "lsm",
                CatalogStorageSpecialization::LsmWriteOptimized,
                "lsm_write_optimized",
            ),
            (
                "analytics",
                CatalogStorageSpecialization::ColumnarAnalytics,
                "columnar_analytics",
            ),
            (
                "hnsw",
                CatalogStorageSpecialization::VectorAnn,
                "vector_ann",
            ),
            (
                "json",
                CatalogStorageSpecialization::DocumentJson,
                "document_json",
            ),
            (
                "csr",
                CatalogStorageSpecialization::GraphTopology,
                "graph_topology",
            ),
            (
                "timeseries",
                CatalogStorageSpecialization::ObservabilityTimeSeries,
                "observability_time_series",
            ),
            (
                "lakehouse",
                CatalogStorageSpecialization::ExternalOpenTable,
                "external_open_table",
            ),
        ];
        for (input, expected, rendered) in specialization_cases {
            assert_eq!(CatalogStorageSpecialization::parse(input), Some(expected));
            assert_eq!(expected.as_str(), rendered);
        }
        assert_eq!(CatalogStorageSpecialization::parse("unknown"), None);
    }

    #[test]
    fn authority_layout_projection_and_compression_contracts_cover_builder_surface() {
        let authority_cases = [
            (
                CatalogAuthorityMode::InternalCanonical,
                true,
                false,
                false,
                "ProximaAuthoritative",
            ),
            (
                CatalogAuthorityMode::ProximaAuthoritative,
                true,
                false,
                false,
                "ProximaAuthoritative",
            ),
            (
                CatalogAuthorityMode::ExternalAuthoritative,
                false,
                true,
                false,
                "ExternalAuthoritative",
            ),
            (
                CatalogAuthorityMode::ImportedSnapshot,
                false,
                false,
                false,
                "ImportedSnapshot",
            ),
            (
                CatalogAuthorityMode::ExportedPublication,
                false,
                false,
                true,
                "ProjectionPublication",
            ),
            (
                CatalogAuthorityMode::ProjectionPublication,
                false,
                false,
                true,
                "ProjectionPublication",
            ),
            (
                CatalogAuthorityMode::RebuildableProjection,
                false,
                false,
                true,
                "RebuildableProjection",
            ),
            (
                CatalogAuthorityMode::FederatedRead,
                false,
                false,
                false,
                "FederatedRead",
            ),
        ];
        for (mode, proxima, external, rebuildable, name) in authority_cases {
            assert_eq!(mode.is_proxima_authoritative(), proxima);
            assert_eq!(mode.is_external_authoritative(), external);
            assert_eq!(mode.is_rebuildable_or_publication(), rebuildable);
            assert_eq!(mode.ownership_mode_name(), name);
        }

        let pax = CatalogStorageLayout::proxima_authoritative_pax("primary");
        assert_eq!(pax.layout_kind, CatalogStorageLayoutKind::Pax);
        assert_eq!(pax.snapshot_semantics.as_deref(), Some("mvcc"));
        assert!(!pax.requires_external_contract());

        let publication = CatalogStorageLayout::projection_publication(
            "iceberg_pub",
            CatalogPhysicalFormat::Iceberg,
            "s3://pub",
        );
        assert_eq!(
            publication.authority,
            CatalogAuthorityMode::ProjectionPublication
        );
        assert_eq!(publication.write_mode, CatalogWriteMode::CopyOnWrite);

        let specialty = CatalogStorageLayout::specialty_projection(
            "hnsw",
            CatalogStorageLayoutKind::VectorAnn,
            CatalogPhysicalFormat::ProximaBlock,
        );
        assert_eq!(
            specialty.authority,
            CatalogAuthorityMode::RebuildableProjection
        );

        let imported = CatalogStorageLayout::imported_snapshot(
            "import",
            CatalogPhysicalFormat::Parquet,
            "s3://import",
        );
        assert_eq!(imported.authority, CatalogAuthorityMode::ImportedSnapshot);

        let federated =
            CatalogStorageLayout::federated_read("fed", CatalogPhysicalFormat::Delta, "s3://delta");
        assert!(federated.requires_external_contract());

        let projection =
            CatalogProjection::rebuildable("col", CatalogProjectionKind::Columnar, "primary")
                .with_bounded_lag(1_000)
                .with_rebuild_rto(RebuildRtoSpec::columnar_estimated())
                .with_freshness_state(ProjectionFreshnessState::Updating)
                .with_lineage("wal:1", "wal:2")
                .with_policy_and_gate("engine", "smoke");
        assert_eq!(projection.max_lag_ms, Some(1_000));
        assert!(
            projection
                .rebuild_rto
                .as_ref()
                .unwrap()
                .supports_incremental_rebuild
        );
        assert_eq!(
            projection.freshness_state,
            ProjectionFreshnessState::Updating
        );
        assert_eq!(projection.source_range.as_deref(), Some("wal:1"));
        assert_eq!(projection.benchmark_gate.as_deref(), Some("smoke"));

        let profile = CatalogCompressionStatsProfile::new("p", "codec", 0, 10, 0, true)
            .with_layout_name("primary")
            .with_projection_id("proj")
            .with_decode_ns_per_value(4.0);
        assert_eq!(profile.measured_ratio, 0.0);
        assert_eq!(profile.bytes_per_value(), 0.0);
        assert_eq!(profile.decode_ns_per_value, Some(4.0));
    }

    #[test]
    fn ann_rto_props_observability_protocol_partition_and_health_helpers_are_covered() {
        for (input, mode, rendered) in [
            ("pre", AnnFilteringMode::PreFilter, "pre_filter"),
            ("intra", AnnFilteringMode::Inline, "inline"),
            ("post", AnnFilteringMode::PostFilter, "post_filter"),
        ] {
            assert_eq!(AnnFilteringMode::parse(input), Some(mode));
            assert_eq!(mode.as_str(), rendered);
        }
        assert_eq!(AnnFilteringMode::parse("bad"), None);

        let policy = AnnFilteringPolicy::default();
        assert_eq!(policy.routing_mode(0.01), AnnFilteringMode::PreFilter);
        assert_eq!(policy.routing_mode(0.20), AnnFilteringMode::Inline);
        assert_eq!(policy.routing_mode(0.90), AnnFilteringMode::PostFilter);
        assert_eq!(policy.effective_top_k_for_post_filter(5), 10);
        let forced = AnnFilteringPolicy {
            force_mode: Some(AnnFilteringMode::Inline),
            ..AnnFilteringPolicy::default()
        };
        assert_eq!(forced.routing_mode(0.0), AnnFilteringMode::Inline);

        let default_rto = RebuildRtoSpec::default();
        assert_eq!(
            default_rto.estimate_source,
            RebuildEstimateSource::Unverified
        );
        assert_eq!(
            RebuildRtoSpec::hnsw_benchmarked().estimate_source,
            RebuildEstimateSource::Benchmarked
        );
        assert!(!RebuildRtoSpec::csr_estimated().degraded_scan_during_rebuild);
        assert_eq!(
            RebuildRtoSpec::columnar_estimated().estimated_rebuild_seconds(10 * 1024 * 1024 * 1024),
            30.0
        );
        assert_eq!(RebuildEstimateSource::Benchmarked.as_str(), "benchmarked");
        assert_eq!(RebuildEstimateSource::Estimated.as_str(), "estimated");
        assert_eq!(RebuildEstimateSource::Unverified.as_str(), "unverified");

        assert_eq!(
            PropsEvaluationCadence::parse("auto"),
            Some(PropsEvaluationCadence::Compaction)
        );
        assert_eq!(
            PropsEvaluationCadence::parse("ddl"),
            Some(PropsEvaluationCadence::Explicit)
        );
        assert_eq!(PropsEvaluationCadence::parse("bad"), None);
        assert_eq!(PropsEvaluationCadence::Compaction.as_str(), "compaction");
        assert_eq!(PropsEvaluationCadence::Explicit.as_str(), "explicit");
        let document_policy = PropsAutoPromotionPolicy::document_default();
        assert!(document_policy.enabled);
        assert!(
            document_policy
                .eligible_types
                .contains(&CatalogDataType::TimestampTz)
        );

        let observability = ObservabilityCompressionHint::default();
        assert_eq!(observability.series_key_column, "series_key");
        assert_eq!(observability.timestamp_ns_column, "__proxima_created_at_ns");
        assert!(observability.sort_by_series_key);
        assert!(observability.xor_float_encoding);

        let notes = ProtocolTypeCoercionNote::standard_notes();
        assert!(
            notes
                .iter()
                .any(|note| note.protocol == "pgwire" && note.lossless)
        );
        assert!(std::ptr::eq(
            notes.as_ptr(),
            ProtocolTypeCoercionNote::standard_notes().as_ptr()
        ));

        for (input, expected, rendered) in [
            ("identity", PartitionTransform::Identity, "identity"),
            ("bucket[32]", PartitionTransform::Bucket(32), "bucket[32]"),
            ("bucket[x]", PartitionTransform::Bucket(16), "bucket[16]"),
            (
                "truncate[8]",
                PartitionTransform::Truncate(8),
                "truncate[8]",
            ),
            (
                "truncate[x]",
                PartitionTransform::Truncate(16),
                "truncate[16]",
            ),
            ("year", PartitionTransform::Year, "year"),
            ("month", PartitionTransform::Month, "month"),
            ("day", PartitionTransform::Day, "day"),
            ("hour", PartitionTransform::Hour, "hour"),
            ("void", PartitionTransform::Void, "void"),
            ("unknown", PartitionTransform::Identity, "identity"),
        ] {
            let parsed = PartitionTransform::parse_from_iceberg_format(input);
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), rendered);
        }

        assert_eq!(TableIdentifier::parse("table").to_fqn(), "table");
        assert_eq!(
            TableIdentifier::parse("db.schema.table").to_string(),
            "db.schema.table"
        );
        let healthy = CatalogHealth::healthy(12);
        assert!(healthy.is_healthy);
        assert_eq!(healthy.latency_ms, 12);
        let unhealthy = CatalogHealth::unhealthy("down");
        assert!(!unhealthy.is_healthy);
        assert_eq!(unhealthy.error.as_deref(), Some("down"));
    }

    // ----- Phase A: catalog → ProximaType bridge tests -----

    #[test]
    fn test_catalog_to_proxima_type_lossless() {
        let all = [
            CatalogDataType::Boolean,
            CatalogDataType::Int8,
            CatalogDataType::Int16,
            CatalogDataType::Int32,
            CatalogDataType::Int64,
            CatalogDataType::Float32,
            CatalogDataType::Float64,
            CatalogDataType::String,
            CatalogDataType::Binary,
            CatalogDataType::Date,
            CatalogDataType::Time,
            CatalogDataType::Timestamp,
            CatalogDataType::TimestampTz,
            CatalogDataType::Decimal,
            CatalogDataType::Uuid,
            CatalogDataType::Json,
            CatalogDataType::Vector,
            CatalogDataType::SparseVector,
            CatalogDataType::BinaryVector,
        ];
        for dt in &all {
            let pt = dt.to_proxima_type();
            // Every variant must map — no panics, no unhandled cases
            let _ = format!("{:?}", pt);
        }
    }

    #[test]
    fn test_catalog_decimal_maps_to_proxima_decimal() {
        use proximadb_data_model::ProximaType;
        let pt = CatalogDataType::Decimal.to_proxima_type();
        assert!(matches!(
            pt,
            ProximaType::Decimal {
                precision: 38,
                scale: 10
            }
        ));
    }

    #[test]
    fn test_catalog_timestamptz_pgwire_oid() {
        // Through the bridge: CatalogDataType → ProximaType → pgwire OID
        let oid = CatalogDataType::TimestampTz.to_proxima_type().pgwire_oid();
        assert_eq!(oid, 1184, "TimestampTz OID must be 1184");
    }

    #[test]
    fn test_catalog_uuid_pgwire_oid() {
        let oid = CatalogDataType::Uuid.to_proxima_type().pgwire_oid();
        assert_eq!(oid, 2950, "UUID OID must be 2950");
    }

    #[test]
    fn test_storage_layout_defaults_to_internal_authority() {
        let layout = CatalogStorageLayout::default();

        assert_eq!(layout.authority, CatalogAuthorityMode::InternalCanonical);
        assert_eq!(layout.layout_kind, CatalogStorageLayoutKind::RowRecord);
        assert_eq!(layout.physical_format, CatalogPhysicalFormat::ProximaBlock);
        assert_eq!(layout.write_mode, CatalogWriteMode::Mutable);
        assert!(layout.policy_enforced_in_proxima);
        assert!(layout.lossy_type_mappings.is_empty());
    }

    #[test]
    fn test_external_authoritative_layout_declares_boundary() {
        let layout = CatalogStorageLayout::external_authoritative(
            "iceberg_orders",
            CatalogPhysicalFormat::Iceberg,
            "s3://warehouse/orders",
        );

        assert_eq!(
            layout.authority,
            CatalogAuthorityMode::ExternalAuthoritative
        );
        assert_eq!(layout.layout_kind, CatalogStorageLayoutKind::ExternalTable);
        assert_eq!(layout.physical_format, CatalogPhysicalFormat::Iceberg);
        assert_eq!(layout.write_mode, CatalogWriteMode::ExternalRefresh);
        assert_eq!(layout.location.as_deref(), Some("s3://warehouse/orders"));
        assert_eq!(
            layout.snapshot_semantics.as_deref(),
            Some("external-latest")
        );
        assert!(!layout.policy_enforced_in_proxima);
    }

    #[test]
    fn test_projection_records_rebuild_source_and_freshness() {
        let projection = CatalogProjection::rebuildable(
            "orders_hnsw",
            CatalogProjectionKind::VectorAnn,
            "orders.primary",
        )
        .with_bounded_lag(5_000)
        .with_freshness_state(ProjectionFreshnessState::Stale)
        .with_lineage("wal:100-200", "commit:180")
        .with_policy_and_gate("engine-enforced", "hybrid-vector-smoke");

        assert_eq!(projection.kind, CatalogProjectionKind::VectorAnn);
        assert_eq!(projection.rebuild_source, "orders.primary");
        assert_eq!(projection.freshness, ProjectionFreshness::BoundedLag);
        assert_eq!(projection.freshness_state, ProjectionFreshnessState::Stale);
        assert_eq!(projection.max_lag_ms, Some(5_000));
        assert_eq!(projection.source_range.as_deref(), Some("wal:100-200"));
        assert_eq!(
            projection.last_included_position.as_deref(),
            Some("commit:180")
        );
        assert_eq!(
            projection.policy_boundary.as_deref(),
            Some("engine-enforced")
        );
        assert_eq!(
            projection.benchmark_gate.as_deref(),
            Some("hybrid-vector-smoke")
        );
        assert!(projection.rebuildable);
        assert!(!projection.lossy);
    }

    #[test]
    fn test_relational_capabilities_are_optional() {
        let empty = RelationalCapabilities::default();
        assert!(!empty.has_enforced_semantics());

        let with_pk = RelationalCapabilities {
            primary_key: vec!["id".to_string()],
            ..Default::default()
        };
        assert!(with_pk.has_enforced_semantics());
    }

    #[test]
    fn test_table_schema_persists_layout_projection_and_relational_metadata() {
        let layout = CatalogStorageLayout::internal("pax_hot", CatalogStorageLayoutKind::Pax);
        let projection =
            CatalogProjection::rebuildable("docs_text", CatalogProjectionKind::FullText, "primary");
        let capabilities = RelationalCapabilities {
            primary_key: vec!["id".to_string()],
            ..Default::default()
        };

        let schema = CatalogTableSchema::new("docs")
            .with_storage_layout(layout)
            .with_projection(projection)
            .with_relational_capabilities(capabilities);

        assert_eq!(schema.storage_layouts.len(), 2);
        assert_eq!(
            schema.storage_layouts[1].layout_kind,
            CatalogStorageLayoutKind::Pax
        );
        assert_eq!(schema.projections.len(), 1);
        assert_eq!(schema.projections[0].kind, CatalogProjectionKind::FullText);
        assert!(schema.relational_capabilities.has_enforced_semantics());
    }

    #[test]
    fn test_table_schema_persists_compression_stats_profiles() {
        let profile = CatalogCompressionStatsProfile::new(
            "bench_23/vector/base_xor",
            "VectorBaseXorEntropy",
            1_024,
            256,
            128,
            true,
        )
        .with_layout_name("pax_vector_spatial")
        .with_projection_id("embedding_exact")
        .with_decode_ns_per_value(42.0);

        let schema =
            CatalogTableSchema::new("vectors").with_compression_stats_profile(profile.clone());

        assert_eq!(profile.measured_ratio, 4.0);
        assert_eq!(profile.bytes_per_value(), 2.0);
        assert_eq!(schema.compression_stats_profiles.len(), 1);
        assert_eq!(
            schema.compression_stats_profiles[0]
                .projection_id
                .as_deref(),
            Some("embedding_exact")
        );

        let encoded = serde_json::to_string(&schema).unwrap();
        let decoded: CatalogTableSchema = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.compression_stats_profiles[0], profile);
    }

    #[test]
    fn catalog_table_statistics_is_stale_when_never_analyzed() {
        let stats = CatalogTableStatistics::default();
        assert!(
            stats.is_stale(1_000_000, 60_000),
            "stats with no last_analyzed_ms must be considered stale"
        );
    }

    #[test]
    fn catalog_table_statistics_is_fresh_within_ttl_window() {
        let stats = CatalogTableStatistics {
            row_count: 10,
            last_analyzed_ms: Some(950_000),
            ..Default::default()
        };
        // now=1_000_000, last=950_000, ttl=60_000 -> delta=50_000 < 60_000 -> fresh
        assert!(
            !stats.is_stale(1_000_000, 60_000),
            "stats within TTL must NOT be stale"
        );
    }

    #[test]
    fn catalog_table_statistics_is_stale_past_ttl_window() {
        let stats = CatalogTableStatistics {
            row_count: 10,
            last_analyzed_ms: Some(900_000),
            ..Default::default()
        };
        // now=1_000_000, last=900_000, ttl=60_000 -> delta=100_000 > 60_000 -> stale
        assert!(
            stats.is_stale(1_000_000, 60_000),
            "stats older than TTL must be stale"
        );
    }

    // === PR 6b: CatalogTableSchema precision fields ===

    #[test]
    fn catalog_table_schema_default_inherits_global_precision_policy() {
        let schema = CatalogTableSchema::default();
        assert!(schema.embedding_precision_policy_id.is_none());
        assert!(schema.embedding_precision_policy_version.is_none());
        assert_eq!(schema.current_precision_epoch, 0);
        assert_eq!(
            schema.canonical_embedding_precision,
            proximadb_records::EmbeddingScalarType::Fp32
        );
        assert!(schema.allowed_embedding_precisions.is_empty());
        let slo = schema.embedding_recall_slo;
        assert_eq!(slo.cosine.at_10, 0.99);
        assert_eq!(slo.dot.at_10, 0.995);
        assert!(schema.precision_migration_state.is_none());
    }

    #[test]
    fn catalog_table_schema_serde_back_compat_with_pre_pr6_json() {
        let pre_pr6_json = serde_json::json!({
            "name": "legacy_collection",
            "columns": [],
            "primary_key": [],
            "indexes": [],
            "schema_version": 1,
            "properties": {},
            "location": null,
            "created_at_ms": 1700000000000_i64,
            "updated_at_ms": 1700000000000_i64,
        });
        let schema: CatalogTableSchema = serde_json::from_value(pre_pr6_json).unwrap();
        assert_eq!(schema.name, "legacy_collection");
        assert!(schema.embedding_precision_policy_id.is_none());
        assert_eq!(schema.current_precision_epoch, 0);
        assert_eq!(
            schema.canonical_embedding_precision,
            proximadb_records::EmbeddingScalarType::Fp32
        );
    }

    #[test]
    fn catalog_table_schema_serde_omits_none_policy_fields() {
        let schema = CatalogTableSchema::new("test");
        let json = serde_json::to_value(&schema).unwrap();
        assert!(json.get("embedding_precision_policy_id").is_none());
        assert!(json.get("embedding_precision_policy_version").is_none());
        assert!(json.get("precision_migration_state").is_none());
    }

    #[test]
    fn catalog_table_schema_round_trips_explicit_policy_reference() {
        let mut schema = CatalogTableSchema::new("fp16_collection");
        schema.embedding_precision_policy_id = Some("tenant_business_fp16".to_string());
        schema.embedding_precision_policy_version = Some(7);
        schema.current_precision_epoch = 3;
        schema.canonical_embedding_precision = proximadb_records::EmbeddingScalarType::Fp16;
        schema.allowed_embedding_precisions = vec![
            proximadb_records::EmbeddingScalarType::Fp16,
            proximadb_records::EmbeddingScalarType::Fp32,
        ];
        schema.precision_migration_state =
            Some(embedding_precision_policy::PrecisionMigrationState::ShadowingTarget);

        let json = serde_json::to_string(&schema).unwrap();
        let back: CatalogTableSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.embedding_precision_policy_id.as_deref(),
            Some("tenant_business_fp16")
        );
        assert_eq!(back.embedding_precision_policy_version, Some(7));
        assert_eq!(back.current_precision_epoch, 3);
        assert_eq!(
            back.canonical_embedding_precision,
            proximadb_records::EmbeddingScalarType::Fp16
        );
        assert_eq!(back.allowed_embedding_precisions.len(), 2);
        assert_eq!(
            back.precision_migration_state,
            Some(embedding_precision_policy::PrecisionMigrationState::ShadowingTarget)
        );
    }
}

// ---------------------------------------------------------------------------
// TableIdentifier
// ---------------------------------------------------------------------------

/// Fully-qualified table address: namespace path + table name.
///
/// Mirrors the Iceberg REST catalog `{namespace}/{table}` addressing scheme
/// so that OLTP, lake, and Iceberg REST catalog backends share one type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TableIdentifier {
    /// Namespace hierarchy (e.g., `["db", "schema"]`).
    pub namespace: Vec<String>,
    /// Unqualified table name.
    pub name: String,
}

impl TableIdentifier {
    pub fn new(namespace: Vec<String>, name: impl Into<String>) -> Self {
        Self {
            namespace,
            name: name.into(),
        }
    }

    /// Parse from a dot-separated fully-qualified name (e.g., `"db.schema.table"`).
    pub fn parse(s: &str) -> Self {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() == 1 {
            Self::new(vec![], parts[0])
        } else {
            let namespace = parts[..parts.len() - 1]
                .iter()
                .map(|p| p.to_string())
                .collect();
            Self::new(namespace, parts[parts.len() - 1])
        }
    }

    /// Dot-joined fully-qualified name.
    pub fn to_fqn(&self) -> String {
        if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.namespace.join("."), self.name)
        }
    }
}

impl std::fmt::Display for TableIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_fqn())
    }
}

// ---------------------------------------------------------------------------
// CatalogHealth
// ---------------------------------------------------------------------------

/// Catalog backend health and connectivity status.
#[derive(Debug, Clone)]
pub struct CatalogHealth {
    pub is_healthy: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
    pub details: HashMap<String, String>,
}

impl CatalogHealth {
    pub fn healthy(latency_ms: u64) -> Self {
        Self {
            is_healthy: true,
            latency_ms,
            error: None,
            details: HashMap::new(),
        }
    }

    pub fn unhealthy(error: impl Into<String>) -> Self {
        Self {
            is_healthy: false,
            latency_ms: 0,
            error: Some(error.into()),
            details: HashMap::new(),
        }
    }

    /// Attach an additional key-value detail to the health status.
    /// Returns `self` for builder-style chaining. Ported from the
    /// local CatalogHealth as part of Option B consolidation.
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Catalog trait
// ---------------------------------------------------------------------------

/// Core catalog trait — every ProximaDB catalog backend implements this.
///
/// The trait uses only types from this crate (`proximadb-catalog`) so
/// implementations can live in any workspace crate without depending on the
/// root `proximadb` crate.
#[async_trait::async_trait]
pub trait Catalog: Send + Sync {
    fn name(&self) -> &str;
    fn catalog_type(&self) -> &str;

    // Namespace operations
    async fn create_namespace(
        &self,
        namespace: &[String],
        properties: HashMap<String, String>,
    ) -> anyhow::Result<CatalogNamespace>;

    async fn drop_namespace(&self, namespace: &[String], cascade: bool) -> anyhow::Result<bool>;
    async fn list_namespaces(
        &self,
        parent: Option<&[String]>,
    ) -> anyhow::Result<Vec<CatalogNamespace>>;
    async fn namespace_exists(&self, namespace: &[String]) -> anyhow::Result<bool>;
    async fn get_namespace(&self, namespace: &[String]) -> anyhow::Result<CatalogNamespace>;
    async fn update_namespace_properties(
        &self,
        namespace: &[String],
        updates: HashMap<String, String>,
        removals: Vec<String>,
    ) -> anyhow::Result<()>;

    // Table operations
    async fn create_table(
        &self,
        identifier: &TableIdentifier,
        schema: CatalogTableSchema,
    ) -> anyhow::Result<CatalogTableSchema>;

    async fn drop_table(&self, identifier: &TableIdentifier, purge: bool) -> anyhow::Result<bool>;
    async fn list_tables(&self, namespace: &[String]) -> anyhow::Result<Vec<TableIdentifier>>;
    async fn table_exists(&self, identifier: &TableIdentifier) -> anyhow::Result<bool>;
    async fn get_table(&self, identifier: &TableIdentifier) -> anyhow::Result<CatalogTableSchema>;
    async fn rename_table(
        &self,
        from: &TableIdentifier,
        to: &TableIdentifier,
    ) -> anyhow::Result<()>;

    // Schema evolution
    async fn evolve_schema(
        &self,
        identifier: &TableIdentifier,
        evolution: CatalogSchemaEvolution,
    ) -> anyhow::Result<CatalogTableSchema>;

    async fn get_schema_version(&self, identifier: &TableIdentifier) -> anyhow::Result<i32>;
    async fn get_schema_by_version(
        &self,
        identifier: &TableIdentifier,
        version: i32,
    ) -> anyhow::Result<CatalogTableSchema>;

    // Index operations
    async fn create_index(
        &self,
        identifier: &TableIdentifier,
        index: CatalogIndex,
    ) -> anyhow::Result<CatalogIndex>;

    async fn drop_index(
        &self,
        identifier: &TableIdentifier,
        index_name: &str,
    ) -> anyhow::Result<bool>;
    async fn list_indexes(&self, identifier: &TableIdentifier)
    -> anyhow::Result<Vec<CatalogIndex>>;

    // Statistics
    async fn get_statistics(
        &self,
        identifier: &TableIdentifier,
    ) -> anyhow::Result<CatalogTableStatistics>;
    async fn update_statistics(
        &self,
        identifier: &TableIdentifier,
        stats: CatalogTableStatistics,
    ) -> anyhow::Result<()>;

    // Partitioning (default: not supported)
    async fn get_partition_spec(
        &self,
        identifier: &TableIdentifier,
    ) -> anyhow::Result<Option<CatalogPartitionSpec>> {
        let _ = identifier;
        Ok(None)
    }

    async fn update_partition_spec(
        &self,
        identifier: &TableIdentifier,
        spec: CatalogPartitionSpec,
    ) -> anyhow::Result<()> {
        let _ = (identifier, spec);
        Err(anyhow::anyhow!(
            "partitioning not supported by this catalog"
        ))
    }

    // Sort Order (for Iceberg-compatible catalogs; default: not supported)
    async fn get_sort_order(
        &self,
        identifier: &TableIdentifier,
    ) -> anyhow::Result<Option<CatalogSortOrder>> {
        let _ = identifier;
        Ok(None)
    }

    async fn update_sort_order(
        &self,
        identifier: &TableIdentifier,
        order: CatalogSortOrder,
    ) -> anyhow::Result<()> {
        let _ = (identifier, order);
        Err(anyhow::anyhow!(
            "sort order updates not supported by this catalog"
        ))
    }

    // Note: cache() / invalidate_cache were on the legacy local Catalog
    // trait, but had zero external trait-method callers (CatalogManager::cache()
    // is a separate struct method). Not ported — implementors that want
    // a cache accessor can expose an inherent method instead.

    async fn health_check(&self) -> anyhow::Result<CatalogHealth> {
        Ok(CatalogHealth::healthy(0))
    }

    async fn close(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

// =============================================================================
// LakehouseExtension — table-format-specific operations for lakehouse catalogs
// (Iceberg / Delta / Hudi / Polaris). Ported from the local trait at
// src/catalog/traits.rs as part of Option B consolidation.
// =============================================================================

/// Extension trait for lakehouse table format operations.
///
/// Implemented by `IcebergCatalog`, `DeltaCatalog`, `PolarisCatalog`
/// — anything that exposes snapshot / location / schema-history
/// semantics that are specific to open table formats.
#[async_trait::async_trait]
pub trait LakehouseExtension: Catalog {
    /// Get the table format (Iceberg / Delta / Hudi / etc.).
    fn table_format(&self) -> TableFormat;

    /// Get the table's storage location URI.
    async fn get_table_location(&self, identifier: &TableIdentifier) -> anyhow::Result<String>;

    /// Get the current snapshot id (Iceberg) / version (Delta).
    async fn get_current_snapshot(
        &self,
        identifier: &TableIdentifier,
    ) -> anyhow::Result<Option<i64>>;

    /// List all snapshot / version ids in chronological order.
    async fn list_snapshots(&self, identifier: &TableIdentifier) -> anyhow::Result<Vec<i64>>;

    /// Get the table's schema-version history (list of schema version ids).
    async fn get_schema_history(&self, identifier: &TableIdentifier) -> anyhow::Result<Vec<i32>>;
}

/// Table format for lakehouse tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableFormat {
    /// Native ProximaDB table format.
    ProximaDB,
    /// Apache Iceberg open table format.
    Iceberg,
    /// Linux Foundation Delta Lake format.
    Delta,
    /// Apache Hudi transactional data lake format.
    Hudi,
    /// Raw Apache Parquet files (read-only, no transaction log).
    Parquet,
}

impl std::fmt::Display for TableFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TableFormat::ProximaDB => write!(f, "proximadb"),
            TableFormat::Iceberg => write!(f, "iceberg"),
            TableFormat::Delta => write!(f, "delta"),
            TableFormat::Hudi => write!(f, "hudi"),
            TableFormat::Parquet => write!(f, "parquet"),
        }
    }
}
