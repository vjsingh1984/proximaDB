//! Distance metric conversion utilities
//!
//! Provides helpers to convert between proto and internal distance metric representations.

use super::DistanceMetric;
use proximadb_proto::v1::DistanceMetric as ProtoDistanceMetric;

/// Convert proto distance metric enum value (i32) to internal DistanceMetric
pub fn proto_distance_to_internal(proto_value: i32) -> DistanceMetric {
    match proto_value {
        x if x == ProtoDistanceMetric::Unspecified as i32 => DistanceMetric::Cosine,
        x if x == ProtoDistanceMetric::Cosine as i32 => DistanceMetric::Cosine,
        x if x == ProtoDistanceMetric::Euclidean as i32 => DistanceMetric::Euclidean,
        x if x == ProtoDistanceMetric::DotProduct as i32 => DistanceMetric::DotProduct,
        x if x == ProtoDistanceMetric::Hamming as i32 => DistanceMetric::Hamming,
        x if x == ProtoDistanceMetric::Manhattan as i32 => DistanceMetric::Manhattan,
        x if x == ProtoDistanceMetric::Jaccard as i32 => DistanceMetric::Jaccard,
        x if x == ProtoDistanceMetric::Custom as i32 => DistanceMetric::Custom,
        x if x == ProtoDistanceMetric::Chebyshev as i32 => DistanceMetric::Chebyshev,
        x if x == ProtoDistanceMetric::Canberra as i32 => DistanceMetric::Canberra,
        x if x == ProtoDistanceMetric::Minkowski as i32 => DistanceMetric::Minkowski,
        x if x == ProtoDistanceMetric::Angular as i32 => DistanceMetric::Angular,
        x if x == ProtoDistanceMetric::BrayCurtis as i32 => DistanceMetric::BrayCurtis,
        x if x == ProtoDistanceMetric::Hellinger as i32 => DistanceMetric::Hellinger,
        _ => {
            tracing::warn!(
                "Unknown distance metric value: {}, defaulting to Cosine",
                proto_value
            );
            DistanceMetric::Cosine
        }
    }
}

/// Convert internal DistanceMetric to proto enum value (i32)
pub fn internal_distance_to_proto(metric: DistanceMetric) -> i32 {
    match metric {
        DistanceMetric::Unspecified => ProtoDistanceMetric::Unspecified as i32,
        DistanceMetric::Cosine => ProtoDistanceMetric::Cosine as i32,
        DistanceMetric::Euclidean => ProtoDistanceMetric::Euclidean as i32,
        DistanceMetric::DotProduct => ProtoDistanceMetric::DotProduct as i32,
        DistanceMetric::Hamming => ProtoDistanceMetric::Hamming as i32,
        DistanceMetric::Manhattan => ProtoDistanceMetric::Manhattan as i32,
        DistanceMetric::Jaccard => ProtoDistanceMetric::Jaccard as i32,
        DistanceMetric::Custom => ProtoDistanceMetric::Custom as i32,
        DistanceMetric::Chebyshev => ProtoDistanceMetric::Chebyshev as i32,
        DistanceMetric::Canberra => ProtoDistanceMetric::Canberra as i32,
        DistanceMetric::Minkowski => ProtoDistanceMetric::Minkowski as i32,
        DistanceMetric::Angular => ProtoDistanceMetric::Angular as i32,
        DistanceMetric::BrayCurtis => ProtoDistanceMetric::BrayCurtis as i32,
        DistanceMetric::Hellinger => ProtoDistanceMetric::Hellinger as i32,
    }
}

/// Get distance metric from collection config with fallback to Cosine default
pub fn get_distance_metric_from_config(
    collection_config: Option<&proximadb_proto::v1::CollectionConfig>,
) -> DistanceMetric {
    collection_config.map_or(DistanceMetric::Cosine, |config| {
        proto_distance_to_internal(config.distance_metric.unwrap_or(0))
    })
}
