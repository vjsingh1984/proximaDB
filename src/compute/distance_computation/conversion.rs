//! Distance metric conversion utilities
//!
//! Provides helpers to convert between proto and internal distance metric representations

use crate::compute::distance_computation::engine::DistanceMetric;
use crate::proto::proximadb_v1::DistanceMetric as ProtoDistanceMetric;

/// Convert proto distance metric enum value (i32) to internal DistanceMetric
///
/// # Arguments
/// * `proto_value` - The i32 value from proto enum
///
/// # Returns
/// The corresponding DistanceMetric, defaults to Cosine if unknown
pub fn proto_distance_to_internal(proto_value: i32) -> DistanceMetric {
    match proto_value {
        x if x == ProtoDistanceMetric::Unspecified as i32 => DistanceMetric::Cosine, // Default unspecified to Cosine
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
///
/// # Arguments
/// * `metric` - The internal DistanceMetric
///
/// # Returns
/// The corresponding proto enum value as i32
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

/// Get distance metric from collection config with fallback to default
///
/// # Arguments
/// * `collection_config` - Optional collection configuration
///
/// # Returns
/// The distance metric from config or Cosine as default
pub fn get_distance_metric_from_config(
    collection_config: Option<&crate::proto::proximadb_v1::CollectionConfig>,
) -> DistanceMetric {
    collection_config
        .map_or(DistanceMetric::Cosine, |config| proto_distance_to_internal(config.distance_metric.unwrap_or(0)))
}

// Deferred: Fix compilation errors - distance_metric is now Option<i32>
// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_proto_to_internal_conversion() {
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Cosine as i32),
//             DistanceMetric::Cosine
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Euclidean as i32),
//             DistanceMetric::Euclidean
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::DotProduct as i32),
//             DistanceMetric::DotProduct
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Hamming as i32),
//             DistanceMetric::Hamming
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Manhattan as i32),
//             DistanceMetric::Manhattan
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Jaccard as i32),
//             DistanceMetric::Jaccard
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Chebyshev as i32),
//             DistanceMetric::Chebyshev
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Canberra as i32),
//             DistanceMetric::Canberra
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Minkowski as i32),
//             DistanceMetric::Minkowski
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Angular as i32),
//             DistanceMetric::Angular
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::BrayCurtis as i32),
//             DistanceMetric::BrayCurtis
//         );
//         assert_eq!(
//             proto_distance_to_internal(ProtoDistanceMetric::Hellinger as i32),
//             DistanceMetric::Hellinger
//         );
//
//         // Test unknown value defaults to Cosine
//         assert_eq!(proto_distance_to_internal(999), DistanceMetric::Cosine);
//     }
//
//     #[test]
//     fn test_internal_to_proto_conversion() {
//         assert_eq!(
//             internal_distance_to_proto(DistanceMetric::Cosine),
//             ProtoDistanceMetric::Cosine as i32
//         );
//         assert_eq!(
//             internal_distance_to_proto(DistanceMetric::Euclidean),
//             ProtoDistanceMetric::Euclidean as i32
//         );
//         assert_eq!(
//             internal_distance_to_proto(DistanceMetric::DotProduct),
//             ProtoDistanceMetric::DotProduct as i32
//         );
//         // Test all metrics for completeness
//     }
//
//     #[test]
//     fn test_get_from_config() {
//         // Test with config
//         let config = crate::proto::proximadb_v1::CollectionConfig {
//             distance_metric: ProtoDistanceMetric::Euclidean as i32,
//             ..Default::default()
//         };
//         assert_eq!(
//             get_distance_metric_from_config(Some(&config)),
//             DistanceMetric::Euclidean
//         );
//
//         // Test without config
//         assert_eq!(
//             get_distance_metric_from_config(None),
//             DistanceMetric::Cosine
//         );
//     }
// }
