//! JSON/document structural projection primitives.
//!
//! These helpers profile canonical `ProximaRecord.props` trees and build
//! rebuildable Layer 2 path/shape projections. They do not replace the exact
//! `props` envelope; PAX column 8 remains the round-trip source of truth until a
//! cataloged projection is fresh and explicitly eligible for query pruning.

use std::collections::{BTreeMap, BTreeSet};

use proximadb_data_model::ProximaValue;
use proximadb_records::{ProximaTree, ProximaTreeNode};

const DEFAULT_MIN_FREQUENCY_RATIO: f64 = 0.50;
const DEFAULT_MIN_PRESENT_COUNT: usize = 2;
const DEFAULT_MAX_PROMOTED_PATHS: usize = 64;
const PROPS_FALLBACK_COLUMN: &str = "props";

/// Stable type family used when deciding whether a JSON path can be promoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JsonLeafKind {
    Null,
    Boolean,
    SignedInteger,
    UnsignedInteger,
    Float,
    Decimal,
    String,
    Symbol,
    Binary,
    Date,
    Time,
    Timestamp,
    TimestampTz,
    Identifier,
    Json,
    Jsonb,
    Array,
    Object,
    Map,
    Struct,
    Vector,
}

impl JsonLeafKind {
    /// Whether this kind is suitable for a row-aligned typed leaf stripe.
    pub fn is_typed_leaf_promotable(self) -> bool {
        matches!(
            self,
            Self::Boolean
                | Self::SignedInteger
                | Self::UnsignedInteger
                | Self::Float
                | Self::Decimal
                | Self::String
                | Self::Symbol
                | Self::Binary
                | Self::Date
                | Self::Time
                | Self::Timestamp
                | Self::TimestampTz
                | Self::Identifier
        )
    }

    /// Whether this kind should be tracked as a structural presence summary.
    pub fn is_container_presence(self) -> bool {
        matches!(self, Self::Array | Self::Object | Self::Map | Self::Struct)
    }
}

/// Per-path statistics collected from a batch of document props trees.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonPathStats {
    pub path: String,
    pub present_count: usize,
    pub null_count: usize,
    pub kind_counts: BTreeMap<JsonLeafKind, usize>,
    pub avg_value_bytes: f64,
}

impl JsonPathStats {
    /// Ratio of documents where the path was present.
    pub fn presence_ratio(&self, document_count: usize) -> f64 {
        if document_count == 0 {
            0.0
        } else {
            self.present_count as f64 / document_count as f64
        }
    }

    /// Ratio of present values that were explicit nulls.
    pub fn null_ratio(&self) -> f64 {
        if self.present_count == 0 {
            0.0
        } else {
            self.null_count as f64 / self.present_count as f64
        }
    }

    /// Stable non-null kind, if all non-null observations agree.
    pub fn stable_non_null_kind(&self) -> Option<JsonLeafKind> {
        let mut non_null_kinds = self
            .kind_counts
            .keys()
            .copied()
            .filter(|kind| *kind != JsonLeafKind::Null);

        let first = non_null_kinds.next()?;
        if non_null_kinds.next().is_none() {
            Some(first)
        } else {
            None
        }
    }
}

/// Deterministic structural variation id for a document shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JsonShapeId(pub u32);

/// Frequency information for one structural shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonShapeStats {
    pub shape_id: JsonShapeId,
    pub row_count: usize,
    pub signature_paths: Vec<String>,
}

/// Batch profile used by compaction before it decides which paths to promote.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonStructuralProfile {
    pub document_count: usize,
    pub shape_ids_by_document: Vec<JsonShapeId>,
    pub shape_stats: Vec<JsonShapeStats>,
    pub path_stats: Vec<JsonPathStats>,
}

impl JsonStructuralProfile {
    pub fn stats_for_path(&self, path: &str) -> Option<&JsonPathStats> {
        self.path_stats.iter().find(|stats| stats.path == path)
    }
}

/// Controls path promotion for a structural projection build.
#[derive(Debug, Clone, Copy)]
pub struct JsonStructuralProjectionOptions {
    /// Minimum fraction of documents where a path must be present.
    pub min_frequency_ratio: f64,
    /// Minimum absolute number of documents where a path must be present.
    pub min_present_count: usize,
    /// Require all non-null values on a promoted path to share one type family.
    pub require_stable_type: bool,
    /// Upper bound on typed leaf stripes emitted for one projection.
    pub max_promoted_paths: usize,
    /// Include frequent arrays/objects/maps/structs as presence summaries.
    pub include_container_presence: bool,
}

impl Default for JsonStructuralProjectionOptions {
    fn default() -> Self {
        Self {
            min_frequency_ratio: DEFAULT_MIN_FREQUENCY_RATIO,
            min_present_count: DEFAULT_MIN_PRESENT_COUNT,
            require_stable_type: true,
            max_promoted_paths: DEFAULT_MAX_PROMOTED_PATHS,
            include_container_presence: true,
        }
    }
}

/// Metadata carried with a JSON structural projection snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonStructuralProjectionMetadata {
    pub document_count: usize,
    pub shape_count: usize,
    pub promoted_path_count: usize,
    pub presence_summary_count: usize,
    /// Name of the exact fallback envelope that must remain readable.
    pub exact_fallback_column: String,
    /// Always true for this pilot; structural stripes are projection data only.
    pub exact_fallback_required: bool,
}

/// Row-aligned values for one promoted JSON path.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonTypedPathStripe {
    pub path_id: u32,
    pub path: String,
    pub kind: JsonLeafKind,
    /// `None` means missing from that row; `Some(ProximaValue::Null)` is an
    /// explicit null and remains distinguishable from missing.
    pub values: Vec<Option<ProximaValue>>,
    pub present_count: usize,
    pub null_count: usize,
}

impl JsonTypedPathStripe {
    pub fn value_for_row(&self, row_id: usize) -> Option<&ProximaValue> {
        self.values.get(row_id).and_then(|value| value.as_ref())
    }
}

/// Frequent container path presence used for coarse JSON pruning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPresenceSummary {
    pub path_id: u32,
    pub path: String,
    pub kind: JsonLeafKind,
    pub present_count: usize,
    pub null_count: usize,
}

/// Rebuildable JSON shape/path projection for one document batch.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonStructuralProjection {
    pub metadata: JsonStructuralProjectionMetadata,
    pub shape_ids_by_document: Vec<JsonShapeId>,
    pub shape_stats: Vec<JsonShapeStats>,
    pub path_dictionary: Vec<String>,
    pub typed_stripes: Vec<JsonTypedPathStripe>,
    pub presence_summaries: Vec<JsonPresenceSummary>,
}

impl JsonStructuralProjection {
    pub fn stripe_for_path(&self, path: &str) -> Option<&JsonTypedPathStripe> {
        self.typed_stripes.iter().find(|stripe| stripe.path == path)
    }

    pub fn presence_for_path(&self, path: &str) -> Option<&JsonPresenceSummary> {
        self.presence_summaries
            .iter()
            .find(|summary| summary.path == path)
    }

    pub fn path_id(&self, path: &str) -> Option<u32> {
        self.path_dictionary
            .iter()
            .position(|candidate| candidate == path)
            .map(|index| index as u32)
    }
}

/// Profile a batch of canonical document props trees.
pub fn profile_props(documents: &[ProximaTree]) -> JsonStructuralProfile {
    let mut path_accumulators = BTreeMap::<String, PathAccumulator>::new();
    let mut signatures = Vec::with_capacity(documents.len());

    for document in documents {
        let mut observations = Vec::new();
        collect_tree_observations(None, document, &mut observations);

        let mut signature = BTreeSet::new();
        let mut observed_paths = BTreeSet::new();

        for observation in observations {
            signature.insert(format!("{}:{:?}", observation.path, observation.kind));

            if observed_paths.insert(observation.path.clone()) {
                let accumulator = path_accumulators.entry(observation.path).or_default();
                accumulator.present_count += 1;
                if observation.kind == JsonLeafKind::Null {
                    accumulator.null_count += 1;
                }
                *accumulator.kind_counts.entry(observation.kind).or_default() += 1;
                accumulator.total_value_bytes += observation.encoded_bytes;
            }
        }

        signatures.push(signature.into_iter().collect::<Vec<_>>());
    }

    let (shape_ids_by_document, shape_stats) = assign_shape_ids(&signatures);
    let path_stats = path_accumulators
        .into_iter()
        .map(|(path, accumulator)| accumulator.into_stats(path))
        .collect();

    JsonStructuralProfile {
        document_count: documents.len(),
        shape_ids_by_document,
        shape_stats,
        path_stats,
    }
}

/// Build a structural projection using the default promotion policy.
pub fn build_structural_projection(documents: &[ProximaTree]) -> JsonStructuralProjection {
    build_structural_projection_with_options(documents, JsonStructuralProjectionOptions::default())
}

/// Build a structural projection using explicit promotion options.
pub fn build_structural_projection_with_options(
    documents: &[ProximaTree],
    options: JsonStructuralProjectionOptions,
) -> JsonStructuralProjection {
    let profile = profile_props(documents);
    let promoted_paths = select_typed_leaf_paths(&profile, options);
    let presence_paths = select_presence_paths(&profile, options);
    let path_dictionary = build_path_dictionary(&promoted_paths, &presence_paths);

    let typed_stripes = promoted_paths
        .iter()
        .map(|candidate| build_typed_stripe(documents, &path_dictionary, candidate))
        .collect::<Vec<_>>();

    let presence_summaries = presence_paths
        .iter()
        .filter_map(|candidate| {
            let path_id = path_dictionary
                .iter()
                .position(|path| path == &candidate.stats.path)? as u32;
            Some(JsonPresenceSummary {
                path_id,
                path: candidate.stats.path.clone(),
                kind: candidate.kind,
                present_count: candidate.stats.present_count,
                null_count: candidate.stats.null_count,
            })
        })
        .collect::<Vec<_>>();

    JsonStructuralProjection {
        metadata: JsonStructuralProjectionMetadata {
            document_count: profile.document_count,
            shape_count: profile.shape_stats.len(),
            promoted_path_count: typed_stripes.len(),
            presence_summary_count: presence_summaries.len(),
            exact_fallback_column: PROPS_FALLBACK_COLUMN.to_string(),
            exact_fallback_required: true,
        },
        shape_ids_by_document: profile.shape_ids_by_document,
        shape_stats: profile.shape_stats,
        path_dictionary,
        typed_stripes,
        presence_summaries,
    }
}

#[derive(Debug, Clone)]
struct PathObservation {
    path: String,
    kind: JsonLeafKind,
    encoded_bytes: usize,
}

#[derive(Debug, Default)]
struct PathAccumulator {
    present_count: usize,
    null_count: usize,
    kind_counts: BTreeMap<JsonLeafKind, usize>,
    total_value_bytes: usize,
}

impl PathAccumulator {
    fn into_stats(self, path: String) -> JsonPathStats {
        let avg_value_bytes = if self.present_count == 0 {
            0.0
        } else {
            self.total_value_bytes as f64 / self.present_count as f64
        };

        JsonPathStats {
            path,
            present_count: self.present_count,
            null_count: self.null_count,
            kind_counts: self.kind_counts,
            avg_value_bytes,
        }
    }
}

#[derive(Debug, Clone)]
struct PromotionCandidate<'a> {
    stats: &'a JsonPathStats,
    kind: JsonLeafKind,
}

fn collect_tree_observations(
    prefix: Option<&str>,
    tree: &ProximaTree,
    observations: &mut Vec<PathObservation>,
) {
    let mut fields = tree.iter().collect::<Vec<_>>();
    fields.sort_by_key(|(k, _)| *k);

    for (key, node) in fields {
        let path = join_path(prefix, key);
        match node {
            ProximaTreeNode::Value(value) => {
                collect_value_observations(path, value, observations);
            }
            ProximaTreeNode::Object(subtree) => {
                observations.push(PathObservation {
                    path: path.clone(),
                    kind: JsonLeafKind::Object,
                    encoded_bytes: 0,
                });
                collect_tree_observations(Some(&path), subtree, observations);
            }
        }
    }
}

fn collect_value_observations(
    path: String,
    value: &ProximaValue,
    observations: &mut Vec<PathObservation>,
) {
    let kind = kind_for_value(value);
    observations.push(PathObservation {
        path: path.clone(),
        kind,
        encoded_bytes: estimated_value_bytes(value),
    });

    match value {
        ProximaValue::Map(fields) => {
            let mut fields = fields.iter().collect::<Vec<_>>();
            fields.sort_by_key(|(k, _)| *k);
            for (key, child) in fields {
                collect_value_observations(join_path(Some(&path), key), child, observations);
            }
        }
        ProximaValue::Struct(fields) => {
            let mut fields = fields.iter().collect::<Vec<_>>();
            fields.sort_by_key(|(k, _)| *k);
            for (key, child) in fields {
                collect_value_observations(join_path(Some(&path), key), child, observations);
            }
        }
        _ => {}
    }
}

fn assign_shape_ids(signatures: &[Vec<String>]) -> (Vec<JsonShapeId>, Vec<JsonShapeStats>) {
    let mut counts = BTreeMap::<Vec<String>, usize>::new();
    for signature in signatures {
        *counts.entry(signature.clone()).or_default() += 1;
    }

    let mut signature_to_id = BTreeMap::new();
    let mut shape_stats = Vec::with_capacity(counts.len());
    for (next_id, (signature, row_count)) in counts.into_iter().enumerate() {
        let shape_id = JsonShapeId(next_id as u32);
        signature_to_id.insert(signature.clone(), shape_id);
        shape_stats.push(JsonShapeStats {
            shape_id,
            row_count,
            signature_paths: signature,
        });
    }

    let shape_ids_by_document = signatures
        .iter()
        .map(|signature| {
            signature_to_id
                .get(signature)
                .copied()
                .expect("shape id must exist for every observed signature")
        })
        .collect();

    (shape_ids_by_document, shape_stats)
}

fn select_typed_leaf_paths(
    profile: &JsonStructuralProfile,
    options: JsonStructuralProjectionOptions,
) -> Vec<PromotionCandidate<'_>> {
    let mut candidates = profile
        .path_stats
        .iter()
        .filter_map(|stats| promotion_candidate(stats, profile.document_count, options))
        .filter(|candidate| candidate.kind.is_typed_leaf_promotable())
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        right
            .stats
            .present_count
            .cmp(&left.stats.present_count)
            .then_with(|| {
                right
                    .stats
                    .avg_value_bytes
                    .total_cmp(&left.stats.avg_value_bytes)
            })
            .then_with(|| left.stats.path.cmp(&right.stats.path))
    });
    candidates.truncate(options.max_promoted_paths);
    candidates
}

fn select_presence_paths(
    profile: &JsonStructuralProfile,
    options: JsonStructuralProjectionOptions,
) -> Vec<PromotionCandidate<'_>> {
    if !options.include_container_presence {
        return Vec::new();
    }

    let mut candidates = profile
        .path_stats
        .iter()
        .filter_map(|stats| promotion_candidate(stats, profile.document_count, options))
        .filter(|candidate| candidate.kind.is_container_presence())
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        right
            .stats
            .present_count
            .cmp(&left.stats.present_count)
            .then_with(|| left.stats.path.cmp(&right.stats.path))
    });
    candidates
}

fn promotion_candidate<'a>(
    stats: &'a JsonPathStats,
    document_count: usize,
    options: JsonStructuralProjectionOptions,
) -> Option<PromotionCandidate<'a>> {
    if stats.present_count < options.min_present_count {
        return None;
    }

    if stats.presence_ratio(document_count) < options.min_frequency_ratio {
        return None;
    }

    let kind = if options.require_stable_type {
        stats.stable_non_null_kind()?
    } else {
        dominant_non_null_kind(stats)?
    };

    Some(PromotionCandidate { stats, kind })
}

fn dominant_non_null_kind(stats: &JsonPathStats) -> Option<JsonLeafKind> {
    stats
        .kind_counts
        .iter()
        .filter(|(kind, _)| **kind != JsonLeafKind::Null)
        .max_by_key(|(_, count)| *count)
        .map(|(kind, _)| *kind)
}

fn build_path_dictionary(
    typed_paths: &[PromotionCandidate<'_>],
    presence_paths: &[PromotionCandidate<'_>],
) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for candidate in typed_paths.iter().chain(presence_paths.iter()) {
        paths.insert(candidate.stats.path.clone());
    }
    paths.into_iter().collect()
}

fn build_typed_stripe(
    documents: &[ProximaTree],
    path_dictionary: &[String],
    candidate: &PromotionCandidate<'_>,
) -> JsonTypedPathStripe {
    let values = documents
        .iter()
        .map(|document| value_at_path(document, &candidate.stats.path).cloned())
        .collect::<Vec<_>>();

    let path_id = path_dictionary
        .iter()
        .position(|path| path == &candidate.stats.path)
        .expect("promoted path must be in path dictionary") as u32;

    JsonTypedPathStripe {
        path_id,
        path: candidate.stats.path.clone(),
        kind: candidate.kind,
        values,
        present_count: candidate.stats.present_count,
        null_count: candidate.stats.null_count,
    }
}

fn value_at_path<'a>(tree: &'a ProximaTree, path: &str) -> Option<&'a ProximaValue> {
    let (head, tail) = split_path(path)?;
    match tree.get(head)? {
        ProximaTreeNode::Value(value) => match tail {
            Some(rest) => value_child_at_path(value, rest),
            None => Some(value),
        },
        ProximaTreeNode::Object(subtree) => match tail {
            Some(rest) => value_at_path(subtree, rest),
            None => None,
        },
    }
}

fn value_child_at_path<'a>(value: &'a ProximaValue, path: &str) -> Option<&'a ProximaValue> {
    let (head, tail) = split_path(path)?;
    let child = match value {
        ProximaValue::Map(fields) | ProximaValue::Struct(fields) => fields.get(head)?,
        _ => return None,
    };

    match tail {
        Some(rest) => value_child_at_path(child, rest),
        None => Some(child),
    }
}

fn split_path(path: &str) -> Option<(&str, Option<&str>)> {
    let mut parts = path.splitn(2, '.');
    let head = parts.next()?;
    if head.is_empty() {
        return None;
    }
    Some((head, parts.next()))
}

fn kind_for_value(value: &ProximaValue) -> JsonLeafKind {
    match value {
        ProximaValue::Null => JsonLeafKind::Null,
        ProximaValue::Boolean(_) => JsonLeafKind::Boolean,
        ProximaValue::Int8(_)
        | ProximaValue::Int16(_)
        | ProximaValue::Int32(_)
        | ProximaValue::Int64(_) => JsonLeafKind::SignedInteger,
        ProximaValue::UInt8(_)
        | ProximaValue::UInt16(_)
        | ProximaValue::UInt32(_)
        | ProximaValue::UInt64(_) => JsonLeafKind::UnsignedInteger,
        ProximaValue::Float16(_) | ProximaValue::Float32(_) | ProximaValue::Float64(_) => {
            JsonLeafKind::Float
        }
        ProximaValue::Decimal(_) => JsonLeafKind::Decimal,
        ProximaValue::String(_) => JsonLeafKind::String,
        ProximaValue::Symbol(_) => JsonLeafKind::Symbol,
        ProximaValue::Binary(_) => JsonLeafKind::Binary,
        ProximaValue::Date(_) => JsonLeafKind::Date,
        ProximaValue::Time(_, _) => JsonLeafKind::Time,
        ProximaValue::Timestamp(_, _) => JsonLeafKind::Timestamp,
        ProximaValue::TimestampTz(_, _) => JsonLeafKind::TimestampTz,
        ProximaValue::Uuid(_) | ProximaValue::ULID(_) => JsonLeafKind::Identifier,
        ProximaValue::Json(_) => JsonLeafKind::Json,
        ProximaValue::Jsonb(_) => JsonLeafKind::Jsonb,
        ProximaValue::Array(_) => JsonLeafKind::Array,
        ProximaValue::Map(_) => JsonLeafKind::Map,
        ProximaValue::Struct(_) => JsonLeafKind::Struct,
        ProximaValue::DenseVector(_)
        | ProximaValue::SparseVector { .. }
        | ProximaValue::BinaryVector(_) => JsonLeafKind::Vector,
    }
}

fn estimated_value_bytes(value: &ProximaValue) -> usize {
    match value {
        ProximaValue::Null => 0,
        ProximaValue::Boolean(_) => 1,
        ProximaValue::Int8(_) | ProximaValue::UInt8(_) => 1,
        ProximaValue::Int16(_) | ProximaValue::UInt16(_) => 2,
        ProximaValue::Int32(_)
        | ProximaValue::UInt32(_)
        | ProximaValue::Float16(_)
        | ProximaValue::Float32(_) => 4,
        ProximaValue::Int64(_)
        | ProximaValue::UInt64(_)
        | ProximaValue::Float64(_)
        | ProximaValue::Date(_)
        | ProximaValue::Time(_, _)
        | ProximaValue::Timestamp(_, _)
        | ProximaValue::TimestampTz(_, _) => 8,
        ProximaValue::Decimal(value)
        | ProximaValue::String(value)
        | ProximaValue::Symbol(value) => value.len(),
        ProximaValue::Binary(value) | ProximaValue::BinaryVector(value) => value.len(),
        ProximaValue::Uuid(value) | ProximaValue::ULID(value) => value.len(),
        ProximaValue::DenseVector(values) => values.len() * std::mem::size_of::<f32>(),
        ProximaValue::SparseVector { indices, values } => {
            (indices.len() * std::mem::size_of::<u32>())
                + (values.len() * std::mem::size_of::<f32>())
        }
        ProximaValue::Json(value) | ProximaValue::Jsonb(value) => {
            serde_json::to_vec(value).map_or(0, |encoded| encoded.len())
        }
        ProximaValue::Array(values) => values.iter().map(estimated_value_bytes).sum(),
        ProximaValue::Map(fields) | ProximaValue::Struct(fields) => fields
            .iter()
            .map(|(key, value)| key.len() + estimated_value_bytes(value))
            .sum(),
    }
}

fn join_path(prefix: Option<&str>, key: &str) -> String {
    match prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}.{key}"),
        _ => key.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn value_string(value: &str) -> ProximaTreeNode {
        ProximaTreeNode::Value(ProximaValue::String(value.to_string()))
    }

    fn value_i64(value: i64) -> ProximaTreeNode {
        ProximaTreeNode::Value(ProximaValue::Int64(value))
    }

    fn value_bool(value: bool) -> ProximaTreeNode {
        ProximaTreeNode::Value(ProximaValue::Boolean(value))
    }

    fn value_null() -> ProximaTreeNode {
        ProximaTreeNode::Value(ProximaValue::Null)
    }

    fn object(fields: impl IntoIterator<Item = (String, ProximaTreeNode)>) -> ProximaTreeNode {
        ProximaTreeNode::Object(ProximaTree::from_iter(fields))
    }

    fn document(name: &str, plan: &str, active: bool, age: i64) -> ProximaTree {
        HashMap::from([
            ("name".to_string(), value_string(name)),
            ("plan".to_string(), value_string(plan)),
            ("active".to_string(), value_bool(active)),
            ("age".to_string(), value_i64(age)),
            (
                "account".to_string(),
                object([
                    ("tier".to_string(), value_string(plan)),
                    ("region".to_string(), value_string("us")),
                ]),
            ),
        ])
    }

    #[test]
    fn profile_assigns_shape_ids_and_path_stats_deterministically() {
        let docs = vec![
            document("Ada", "pro", true, 37),
            document("Grace", "pro", true, 42),
            HashMap::from([
                ("name".to_string(), value_string("Linus")),
                ("active".to_string(), value_bool(false)),
                ("age".to_string(), value_i64(55)),
            ]),
        ];

        let profile = profile_props(&docs);

        assert_eq!(profile.document_count, 3);
        assert_eq!(profile.shape_stats.len(), 2);
        assert_eq!(
            profile.shape_ids_by_document[0],
            profile.shape_ids_by_document[1]
        );
        assert_ne!(
            profile.shape_ids_by_document[0],
            profile.shape_ids_by_document[2]
        );

        let age = profile.stats_for_path("age").unwrap();
        assert_eq!(age.present_count, 3);
        assert_eq!(
            age.stable_non_null_kind(),
            Some(JsonLeafKind::SignedInteger)
        );

        let account = profile.stats_for_path("account").unwrap();
        assert_eq!(account.present_count, 2);
        assert_eq!(account.stable_non_null_kind(), Some(JsonLeafKind::Object));
    }

    #[test]
    fn projection_promotes_frequent_stable_scalar_paths() {
        let docs = vec![
            document("Ada", "pro", true, 37),
            document("Grace", "pro", true, 42),
            document("Barbara", "team", false, 45),
        ];

        let projection = build_structural_projection(&docs);

        assert_eq!(projection.metadata.document_count, 3);
        assert_eq!(projection.metadata.exact_fallback_column, "props");
        assert!(projection.metadata.exact_fallback_required);
        assert!(projection.stripe_for_path("active").is_some());
        assert!(projection.stripe_for_path("age").is_some());
        assert!(projection.stripe_for_path("account.tier").is_some());

        let age = projection.stripe_for_path("age").unwrap();
        assert_eq!(age.kind, JsonLeafKind::SignedInteger);
        assert_eq!(age.present_count, 3);
        assert!(matches!(
            age.value_for_row(1),
            Some(ProximaValue::Int64(42))
        ));
    }

    #[test]
    fn projection_rejects_mixed_type_paths_when_stability_is_required() {
        let docs = vec![
            HashMap::from([("score".to_string(), value_i64(10))]),
            HashMap::from([("score".to_string(), value_string("10"))]),
            HashMap::from([("score".to_string(), value_i64(11))]),
        ];

        let projection = build_structural_projection_with_options(
            &docs,
            JsonStructuralProjectionOptions {
                min_frequency_ratio: 0.50,
                min_present_count: 2,
                require_stable_type: true,
                max_promoted_paths: 16,
                include_container_presence: true,
            },
        );

        assert!(projection.stripe_for_path("score").is_none());
        assert!(projection.path_id("score").is_none());
    }

    #[test]
    fn projection_tracks_container_presence_without_replacing_props() {
        let docs = vec![
            document("Ada", "pro", true, 37),
            document("Grace", "pro", true, 42),
            HashMap::from([
                ("name".to_string(), value_string("Radia")),
                ("account".to_string(), value_null()),
            ]),
        ];
        let original = docs.clone();

        let projection = build_structural_projection(&docs);

        let account = projection.presence_for_path("account").unwrap();
        assert_eq!(account.kind, JsonLeafKind::Object);
        assert_eq!(account.present_count, 3);
        assert_eq!(account.null_count, 1);
        assert_eq!(docs, original, "projection must not mutate exact props");
        assert_eq!(projection.metadata.exact_fallback_column, "props");
        assert!(projection.metadata.exact_fallback_required);
    }

    #[test]
    fn map_and_struct_values_can_contribute_nested_promoted_paths() {
        let docs = vec![
            HashMap::from([(
                "payload".to_string(),
                ProximaTreeNode::Value(ProximaValue::Map(HashMap::from([(
                    "kind".to_string(),
                    ProximaValue::String("event".to_string()),
                )]))),
            )]),
            HashMap::from([(
                "payload".to_string(),
                ProximaTreeNode::Value(ProximaValue::Map(HashMap::from([(
                    "kind".to_string(),
                    ProximaValue::String("event".to_string()),
                )]))),
            )]),
        ];

        let projection = build_structural_projection(&docs);

        assert!(projection.presence_for_path("payload").is_some());
        let kind = projection.stripe_for_path("payload.kind").unwrap();
        assert_eq!(kind.kind, JsonLeafKind::String);
        assert!(matches!(
            kind.value_for_row(0),
            Some(ProximaValue::String(value)) if value == "event"
        ));
    }
}
