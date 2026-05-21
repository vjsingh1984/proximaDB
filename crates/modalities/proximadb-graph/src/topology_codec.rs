//! Exact graph topology codecs for rebuildable CSR/CSC projections.
//!
//! These helpers are pilot primitives for PCX-006. They compress sorted
//! adjacency lists but do not make the compressed projection authoritative.
//! Canonical node/edge `ProximaRecord`s and the adjacency projection remain the
//! rebuild source.

use anyhow::{Result, bail};
use std::collections::BTreeSet;

use crate::projection::{
    GraphTopologyDescriptor, GraphTopologyProjectionMetadata, TopologyCompression,
    TopologyDirection, TopologyEpoch, TopologyFormat, TopologyVertexOrdering,
};

const RAW_HEADER_LEN: usize = 4;
const DEFAULT_MIN_COMPRESSION_RATIO: f64 = 1.10;
const DEFAULT_RAW_DEGREE_CUTOFF: usize = 4;

/// Why a compressed topology candidate was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyCodecRejection {
    /// The list is small enough that fixed-width raw is cheaper to decode.
    DegreeBelowRawCutoff { degree: usize, cutoff: usize },
    /// Gap-varint requires non-decreasing neighbor ids to remain exact and cheap.
    UnsortedNeighbors,
    /// Encoded candidate did not beat the required ratio.
    RatioBelowThreshold {
        raw_bytes: usize,
        encoded_bytes: usize,
        required_ratio_x1000: u32,
    },
}

/// Options for selecting a topology list codec.
#[derive(Debug, Clone, Copy)]
pub struct TopologyCodecOptions {
    /// Minimum raw/encoded ratio required before selecting compressed output.
    pub min_compression_ratio: f64,
    /// Lists at or below this degree stay raw for predictable hot-path latency.
    pub raw_degree_cutoff: usize,
}

impl Default for TopologyCodecOptions {
    fn default() -> Self {
        Self {
            min_compression_ratio: DEFAULT_MIN_COMPRESSION_RATIO,
            raw_degree_cutoff: DEFAULT_RAW_DEGREE_CUTOFF,
        }
    }
}

/// Profile for one encoded adjacency list.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyCodecProfile {
    /// Selected exact compression.
    pub compression: TopologyCompression,
    /// Neighbor count.
    pub degree: usize,
    /// Raw fixed-width payload bytes, excluding container metadata.
    pub raw_bytes: usize,
    /// Encoded bytes, including the list-local count header.
    pub encoded_bytes: usize,
    /// raw_bytes / encoded_bytes.
    pub compression_ratio: f64,
    /// Rejection/fallback reason when raw was selected.
    pub rejection: Option<TopologyCodecRejection>,
}

impl TopologyCodecProfile {
    pub fn should_use_compressed(&self) -> bool {
        self.rejection.is_none() && !matches!(self.compression, TopologyCompression::RawU64)
    }
}

/// Encoded exact adjacency list.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodedNeighborList {
    pub profile: TopologyCodecProfile,
    pub data: Vec<u8>,
}

/// Encoded CSR/CSC-style topology projection built from adjacency lists.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodedTopologyProjection {
    /// Catalog/EXPLAIN-facing projection metadata.
    pub metadata: GraphTopologyProjectionMetadata,
    adjacency: Vec<EncodedNeighborList>,
}

impl EncodedTopologyProjection {
    /// Return decoded neighbors for one vertex ordinal.
    pub fn neighbors(&self, vertex_ordinal: usize) -> Result<Vec<u64>> {
        let list = self
            .adjacency
            .get(vertex_ordinal)
            .ok_or_else(|| anyhow::anyhow!("topology vertex ordinal out of range"))?;
        decode_neighbor_list(list)
    }

    pub fn vertex_count(&self) -> usize {
        self.adjacency.len()
    }

    pub fn compressed_list_count(&self) -> usize {
        self.adjacency
            .iter()
            .filter(|list| list.profile.should_use_compressed())
            .count()
    }

    pub fn raw_fallback_count(&self) -> usize {
        self.adjacency.len() - self.compressed_list_count()
    }

    pub fn list_profiles(&self) -> impl Iterator<Item = &TopologyCodecProfile> {
        self.adjacency.iter().map(|list| &list.profile)
    }
}

/// Build an encoded topology projection from vertex-ordinal adjacency lists.
pub fn encode_topology_projection(
    graph_id: impl Into<String>,
    topology_epoch: TopologyEpoch,
    direction: TopologyDirection,
    vertex_ordering: TopologyVertexOrdering,
    edge_label: Option<String>,
    adjacency_lists: &[Vec<u64>],
    options: TopologyCodecOptions,
) -> Result<EncodedTopologyProjection> {
    let graph_id = graph_id.into();
    let mut encoded_lists = Vec::with_capacity(adjacency_lists.len());
    let mut raw_bytes = 0usize;
    let mut encoded_bytes = 0usize;
    let mut edge_count = 0usize;

    for list in adjacency_lists {
        let encoded = encode_neighbor_list(list, options)?;
        raw_bytes = raw_bytes
            .checked_add(encoded.profile.raw_bytes)
            .ok_or_else(|| anyhow::anyhow!("topology raw byte count overflow"))?;
        encoded_bytes = encoded_bytes
            .checked_add(encoded.profile.encoded_bytes)
            .ok_or_else(|| anyhow::anyhow!("topology encoded byte count overflow"))?;
        edge_count = edge_count
            .checked_add(encoded.profile.degree)
            .ok_or_else(|| anyhow::anyhow!("topology edge count overflow"))?;
        encoded_lists.push(encoded);
    }

    let compression = aggregate_compression(&encoded_lists);
    let fallback_reason = aggregate_fallback_reason(&encoded_lists);
    let metadata = GraphTopologyProjectionMetadata {
        descriptor: GraphTopologyDescriptor::new(
            format!("topology-{graph_id}-{direction:?}"),
            graph_id,
            TopologyFormat::Csr,
        ),
        topology_epoch,
        direction,
        vertex_ordering,
        compression,
        edge_label,
        vertex_count: adjacency_lists.len(),
        edge_count,
        raw_bytes,
        encoded_bytes,
        fallback_reason,
    };

    Ok(EncodedTopologyProjection {
        metadata,
        adjacency: encoded_lists,
    })
}

/// Select and encode one neighbor list.
pub fn encode_neighbor_list(
    neighbors: &[u64],
    options: TopologyCodecOptions,
) -> Result<EncodedNeighborList> {
    if neighbors.len() <= options.raw_degree_cutoff {
        return raw_list(
            neighbors,
            Some(TopologyCodecRejection::DegreeBelowRawCutoff {
                degree: neighbors.len(),
                cutoff: options.raw_degree_cutoff,
            }),
        );
    }

    if !is_non_decreasing(neighbors) {
        return raw_list(neighbors, Some(TopologyCodecRejection::UnsortedNeighbors));
    }

    let gap_data = encode_gap_varint_payload(neighbors)?;
    let raw_bytes = raw_payload_bytes(neighbors.len())?;
    let gap_profile = make_profile(
        TopologyCompression::GapVarint,
        neighbors.len(),
        raw_bytes,
        gap_data.len(),
        None,
    );
    if gap_profile.compression_ratio >= options.min_compression_ratio {
        return Ok(EncodedNeighborList {
            profile: gap_profile,
            data: gap_data,
        });
    }

    raw_list(
        neighbors,
        Some(TopologyCodecRejection::RatioBelowThreshold {
            raw_bytes,
            encoded_bytes: gap_data.len(),
            required_ratio_x1000: (options.min_compression_ratio * 1000.0).round() as u32,
        }),
    )
}

/// Decode an encoded neighbor list.
pub fn decode_neighbor_list(encoded: &EncodedNeighborList) -> Result<Vec<u64>> {
    match encoded.profile.compression {
        TopologyCompression::RawU64 => decode_raw_payload(&encoded.data),
        TopologyCompression::GapVarint => decode_gap_varint_payload(&encoded.data),
        other => bail!("unsupported topology compression: {other:?}"),
    }
}

/// Build catalog/EXPLAIN-facing projection metadata from a list profile.
pub fn metadata_for_neighbor_list(
    graph_id: impl Into<String>,
    topology_epoch: TopologyEpoch,
    direction: TopologyDirection,
    vertex_ordering: TopologyVertexOrdering,
    edge_label: Option<String>,
    profile: &TopologyCodecProfile,
) -> GraphTopologyProjectionMetadata {
    let graph_id = graph_id.into();
    GraphTopologyProjectionMetadata {
        descriptor: GraphTopologyDescriptor::new(
            format!("topology-{graph_id}-{direction:?}"),
            graph_id,
            TopologyFormat::Csr,
        ),
        topology_epoch,
        direction,
        vertex_ordering,
        compression: profile.compression,
        edge_label,
        vertex_count: 1,
        edge_count: profile.degree,
        raw_bytes: profile.raw_bytes,
        encoded_bytes: profile.encoded_bytes,
        fallback_reason: profile
            .rejection
            .as_ref()
            .map(|reason| format!("{reason:?}")),
    }
}

fn raw_list(
    neighbors: &[u64],
    rejection: Option<TopologyCodecRejection>,
) -> Result<EncodedNeighborList> {
    let data = encode_raw_payload(neighbors)?;
    let raw_bytes = raw_payload_bytes(neighbors.len())?;
    let profile = make_profile(
        TopologyCompression::RawU64,
        neighbors.len(),
        raw_bytes,
        data.len(),
        rejection,
    );
    Ok(EncodedNeighborList { profile, data })
}

fn encode_raw_payload(neighbors: &[u64]) -> Result<Vec<u8>> {
    let count = u32::try_from(neighbors.len())
        .map_err(|_| anyhow::anyhow!("neighbor list length exceeds u32"))?;
    let mut data = Vec::with_capacity(RAW_HEADER_LEN + neighbors.len() * 8);
    data.extend_from_slice(&count.to_le_bytes());
    for neighbor in neighbors {
        data.extend_from_slice(&neighbor.to_le_bytes());
    }
    Ok(data)
}

fn decode_raw_payload(data: &[u8]) -> Result<Vec<u64>> {
    let (count, mut pos) = read_count(data)?;
    let byte_len = count
        .checked_mul(8)
        .ok_or_else(|| anyhow::anyhow!("raw neighbor payload byte count overflow"))?;
    ensure_remaining(data, pos, byte_len, "raw neighbor payload ended early")?;
    let mut neighbors = Vec::with_capacity(count);
    for _ in 0..count {
        neighbors.push(u64::from_le_bytes(data[pos..pos + 8].try_into()?));
        pos += 8;
    }
    if pos != data.len() {
        bail!("raw neighbor payload has trailing bytes");
    }
    Ok(neighbors)
}

fn encode_gap_varint_payload(neighbors: &[u64]) -> Result<Vec<u8>> {
    let count = u32::try_from(neighbors.len())
        .map_err(|_| anyhow::anyhow!("neighbor list length exceeds u32"))?;
    let mut data = Vec::with_capacity(RAW_HEADER_LEN + neighbors.len());
    data.extend_from_slice(&count.to_le_bytes());
    let mut previous = 0u64;
    for (idx, neighbor) in neighbors.iter().copied().enumerate() {
        if idx > 0 && neighbor < previous {
            bail!("gap-varint topology codec requires sorted neighbor ids");
        }
        let gap = if idx == 0 {
            neighbor
        } else {
            neighbor - previous
        };
        encode_varint_u64(&mut data, gap);
        previous = neighbor;
    }
    Ok(data)
}

fn decode_gap_varint_payload(data: &[u8]) -> Result<Vec<u64>> {
    let (count, mut pos) = read_count(data)?;
    let mut neighbors = Vec::with_capacity(count);
    let mut previous = 0u64;
    for idx in 0..count {
        let (gap, read) = decode_varint_u64(&data[pos..])?;
        pos += read;
        let value = if idx == 0 {
            gap
        } else {
            previous
                .checked_add(gap)
                .ok_or_else(|| anyhow::anyhow!("gap-varint neighbor id overflow"))?
        };
        neighbors.push(value);
        previous = value;
    }
    if pos != data.len() {
        bail!("gap-varint neighbor payload has trailing bytes");
    }
    Ok(neighbors)
}

fn read_count(data: &[u8]) -> Result<(usize, usize)> {
    ensure_remaining(
        data,
        0,
        RAW_HEADER_LEN,
        "neighbor payload shorter than count header",
    )?;
    let count = u32::from_le_bytes(data[..RAW_HEADER_LEN].try_into()?) as usize;
    Ok((count, RAW_HEADER_LEN))
}

fn encode_varint_u64(buf: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        buf.push((value as u8) | 0x80);
        value >>= 7;
    }
    buf.push(value as u8);
}

fn decode_varint_u64(data: &[u8]) -> Result<(u64, usize)> {
    let mut result = 0u64;
    for (idx, byte) in data.iter().copied().enumerate().take(10) {
        let value = (byte & 0x7f) as u64;
        result |= value << (idx * 7);
        if byte & 0x80 == 0 {
            return Ok((result, idx + 1));
        }
    }
    bail!("incomplete or overlong topology varint")
}

fn make_profile(
    compression: TopologyCompression,
    degree: usize,
    raw_bytes: usize,
    encoded_bytes: usize,
    rejection: Option<TopologyCodecRejection>,
) -> TopologyCodecProfile {
    TopologyCodecProfile {
        compression,
        degree,
        raw_bytes,
        encoded_bytes,
        compression_ratio: if raw_bytes == 0 || encoded_bytes == 0 {
            0.0
        } else {
            raw_bytes as f64 / encoded_bytes as f64
        },
        rejection,
    }
}

fn raw_payload_bytes(degree: usize) -> Result<usize> {
    degree
        .checked_mul(8)
        .ok_or_else(|| anyhow::anyhow!("raw neighbor byte count overflow"))
}

fn is_non_decreasing(values: &[u64]) -> bool {
    values.windows(2).all(|pair| pair[0] <= pair[1])
}

fn aggregate_compression(encoded_lists: &[EncodedNeighborList]) -> TopologyCompression {
    let mut compressed = BTreeSet::new();
    let mut has_raw = false;
    for list in encoded_lists {
        if list.profile.should_use_compressed() {
            compressed.insert(list.profile.compression);
        } else {
            has_raw = true;
        }
    }

    match (compressed.len(), has_raw) {
        (0, _) => TopologyCompression::RawU64,
        (1, false) => *compressed.iter().next().expect("one compressed codec"),
        _ => TopologyCompression::MixedExact,
    }
}

fn aggregate_fallback_reason(encoded_lists: &[EncodedNeighborList]) -> Option<String> {
    let raw_fallbacks = encoded_lists
        .iter()
        .filter(|list| !list.profile.should_use_compressed())
        .count();
    if raw_fallbacks == 0 {
        return None;
    }

    let small_degree = encoded_lists
        .iter()
        .filter(|list| {
            matches!(
                list.profile.rejection,
                Some(TopologyCodecRejection::DegreeBelowRawCutoff { .. })
            )
        })
        .count();
    let unsorted = encoded_lists
        .iter()
        .filter(|list| {
            matches!(
                list.profile.rejection,
                Some(TopologyCodecRejection::UnsortedNeighbors)
            )
        })
        .count();
    let ratio = encoded_lists
        .iter()
        .filter(|list| {
            matches!(
                list.profile.rejection,
                Some(TopologyCodecRejection::RatioBelowThreshold { .. })
            )
        })
        .count();

    Some(format!(
        "{raw_fallbacks} raw list fallbacks: small_degree={small_degree}, unsorted={unsorted}, ratio={ratio}"
    ))
}

fn ensure_remaining(data: &[u8], pos: usize, len: usize, message: &'static str) -> Result<()> {
    let end = pos
        .checked_add(len)
        .ok_or_else(|| anyhow::anyhow!("topology payload offset overflow"))?;
    if end > data.len() {
        bail!(message);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn gap_varint_roundtrips_sorted_neighbors_with_duplicates() {
        let neighbors = vec![2, 3, 3, 5, 8, 13, 21, 21, 34];
        let encoded = encode_neighbor_list(
            &neighbors,
            TopologyCodecOptions {
                raw_degree_cutoff: 0,
                min_compression_ratio: 1.0,
            },
        )
        .unwrap();

        assert_eq!(encoded.profile.compression, TopologyCompression::GapVarint);
        assert_eq!(decode_neighbor_list(&encoded).unwrap(), neighbors);
    }

    #[test]
    fn gap_varint_selected_for_clustered_sorted_neighbors() {
        let neighbors: Vec<u64> = (10..1034).collect();
        let encoded = encode_neighbor_list(&neighbors, TopologyCodecOptions::default()).unwrap();

        assert!(
            encoded.profile.should_use_compressed(),
            "{:?}",
            encoded.profile
        );
        assert_eq!(encoded.profile.compression, TopologyCompression::GapVarint);
        assert!(encoded.profile.compression_ratio > 4.0);
        assert_eq!(decode_neighbor_list(&encoded).unwrap(), neighbors);
    }

    #[test]
    fn raw_selected_for_hot_small_degree_lists() {
        let neighbors = vec![7, 9, 12];
        let encoded = encode_neighbor_list(&neighbors, TopologyCodecOptions::default()).unwrap();

        assert_eq!(encoded.profile.compression, TopologyCompression::RawU64);
        assert_eq!(
            encoded.profile.rejection,
            Some(TopologyCodecRejection::DegreeBelowRawCutoff {
                degree: 3,
                cutoff: 4
            })
        );
        assert_eq!(decode_neighbor_list(&encoded).unwrap(), neighbors);
    }

    #[test]
    fn raw_selected_for_unsorted_neighbors() {
        let neighbors = vec![9, 7, 12, 1];
        let encoded = encode_neighbor_list(
            &neighbors,
            TopologyCodecOptions {
                raw_degree_cutoff: 0,
                ..TopologyCodecOptions::default()
            },
        )
        .unwrap();

        assert_eq!(encoded.profile.compression, TopologyCompression::RawU64);
        assert_eq!(
            encoded.profile.rejection,
            Some(TopologyCodecRejection::UnsortedNeighbors)
        );
        assert_eq!(decode_neighbor_list(&encoded).unwrap(), neighbors);
    }

    #[test]
    fn raw_selected_when_gap_varint_expands_payload() {
        let mut neighbors = Vec::new();
        let mut value = 0u64;
        for _ in 0..64 {
            value += 1u64 << 56;
            neighbors.push(value);
        }

        let encoded = encode_neighbor_list(
            &neighbors,
            TopologyCodecOptions {
                raw_degree_cutoff: 0,
                min_compression_ratio: 1.10,
            },
        )
        .unwrap();

        assert_eq!(encoded.profile.compression, TopologyCompression::RawU64);
        assert!(matches!(
            encoded.profile.rejection,
            Some(TopologyCodecRejection::RatioBelowThreshold { .. })
        ));
        assert_eq!(decode_neighbor_list(&encoded).unwrap(), neighbors);
    }

    #[test]
    fn metadata_records_epoch_ordering_codec_and_fallback() {
        let neighbors: Vec<u64> = (1..32).collect();
        let encoded = encode_neighbor_list(
            &neighbors,
            TopologyCodecOptions {
                raw_degree_cutoff: 0,
                min_compression_ratio: 1.0,
            },
        )
        .unwrap();

        let metadata = metadata_for_neighbor_list(
            "g1",
            TopologyEpoch(7),
            TopologyDirection::Outgoing,
            TopologyVertexOrdering::SourceSorted,
            Some("KNOWS".to_string()),
            &encoded.profile,
        );

        assert_eq!(metadata.topology_epoch, TopologyEpoch(7));
        assert_eq!(metadata.direction, TopologyDirection::Outgoing);
        assert_eq!(
            metadata.vertex_ordering,
            TopologyVertexOrdering::SourceSorted
        );
        assert_eq!(metadata.compression, TopologyCompression::GapVarint);
        assert_eq!(metadata.edge_label, Some("KNOWS".to_string()));
        assert!(metadata.is_compressed());
        assert!(metadata.fallback_reason.is_none());
        assert!(metadata.compression_ratio() > 1.0);
        assert!(!metadata.descriptor.write_authoritative);
        assert!(metadata.descriptor.rebuildable);
    }

    #[test]
    fn topology_projection_roundtrips_mixed_raw_and_compressed_lists() {
        let adjacency = vec![
            vec![1, 2],
            (10..266).collect::<Vec<_>>(),
            vec![9, 7, 12],
            Vec::new(),
        ];

        let projection = encode_topology_projection(
            "g1",
            TopologyEpoch(9),
            TopologyDirection::Outgoing,
            TopologyVertexOrdering::SourceSorted,
            Some("KNOWS".to_string()),
            &adjacency,
            TopologyCodecOptions::default(),
        )
        .unwrap();

        assert_eq!(projection.vertex_count(), adjacency.len());
        assert!(projection.compressed_list_count() > 0);
        assert!(projection.raw_fallback_count() > 0);
        assert_eq!(
            projection.metadata.compression,
            TopologyCompression::MixedExact
        );
        assert_eq!(projection.metadata.topology_epoch, TopologyEpoch(9));
        assert_eq!(projection.metadata.edge_count, 261);
        assert!(
            projection
                .metadata
                .fallback_reason
                .as_deref()
                .unwrap()
                .contains("raw list fallbacks")
        );

        for (vertex, expected) in adjacency.iter().enumerate() {
            assert_eq!(projection.neighbors(vertex).unwrap(), *expected);
        }
    }

    #[test]
    fn traversal_sees_same_neighbors_from_raw_and_compressed_projections() {
        let adjacency = traversal_fixture();
        let raw_projection = encode_topology_projection(
            "g1",
            TopologyEpoch(11),
            TopologyDirection::Outgoing,
            TopologyVertexOrdering::Natural,
            None,
            &adjacency,
            TopologyCodecOptions {
                raw_degree_cutoff: usize::MAX,
                min_compression_ratio: 1.10,
            },
        )
        .unwrap();
        let compressed_projection = encode_topology_projection(
            "g1",
            TopologyEpoch(11),
            TopologyDirection::Outgoing,
            TopologyVertexOrdering::SourceSorted,
            None,
            &adjacency,
            TopologyCodecOptions {
                raw_degree_cutoff: 0,
                min_compression_ratio: 1.0,
            },
        )
        .unwrap();

        assert_eq!(
            raw_projection.metadata.compression,
            TopologyCompression::RawU64
        );
        assert!(compressed_projection.metadata.is_compressed());
        assert_eq!(
            bfs(&raw_projection, 0).unwrap(),
            bfs(&compressed_projection, 0).unwrap()
        );
        assert_eq!(
            bfs(&compressed_projection, 0).unwrap(),
            vec![0, 1, 2, 3, 4, 5, 6]
        );
    }

    fn traversal_fixture() -> Vec<Vec<u64>> {
        vec![
            vec![1, 2, 3, 4, 5],
            vec![6],
            vec![6],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ]
    }

    fn bfs(projection: &EncodedTopologyProjection, start: u64) -> Result<Vec<u64>> {
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        let mut order = Vec::new();

        visited.insert(start);
        queue.push_back(start);

        while let Some(vertex) = queue.pop_front() {
            order.push(vertex);
            for neighbor in projection.neighbors(vertex as usize)? {
                if visited.insert(neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }

        Ok(order)
    }
}
