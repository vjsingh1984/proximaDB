//! Hand-written prost-derived wire types for the multi-phase ranking
//! pipeline (R-7c.4a).
//!
//! Source of truth: `proto/proximadb/v1/ranking.proto`. These Rust
//! mirrors stay hand-written until the next proto regeneration pass —
//! when that lands, the contents move to `src/proto/proximadb.v1.ranking.rs`
//! and this file becomes a re-export shim.
//!
//! Wire compatibility note: tag numbers below MUST match
//! `ranking.proto`. Any field renumbering breaks deployed clients.

use prost::Message;

/// One per-feature contribution to a ranking score.
#[derive(Clone, PartialEq, Message)]
pub struct ScoreComponent {
    #[prost(string, tag = "1")]
    pub name: ::prost::alloc::string::String,
    #[prost(double, tag = "2")]
    pub value: f64,
    #[prost(double, tag = "3")]
    pub weight: f64,
    #[prost(double, tag = "4")]
    pub contribution: f64,
}

/// Multi-component score. `phase`: 0=FIRST, 1=SECOND, 2=GLOBAL.
#[derive(Clone, PartialEq, Message)]
pub struct ScoreVector {
    #[prost(float, tag = "1")]
    pub primary: f32,
    #[prost(uint32, tag = "2")]
    pub phase: u32,
    #[prost(message, repeated, tag = "3")]
    pub components: ::prost::alloc::vec::Vec<ScoreComponent>,
}

/// Per-phase override knobs.
#[derive(Clone, PartialEq, Message)]
pub struct PhaseOverride {
    #[prost(uint32, optional, tag = "1")]
    pub rerank_count: ::core::option::Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub batch_size: ::core::option::Option<u32>,
}

/// Bundle of phase overrides on top of the resolved profile.
#[derive(Clone, PartialEq, Message)]
pub struct RankOverrides {
    #[prost(message, optional, tag = "1")]
    pub second_phase: ::core::option::Option<PhaseOverride>,
    #[prost(message, optional, tag = "2")]
    pub global_phase: ::core::option::Option<PhaseOverride>,
}

/// Rank pipeline request — wire mirror of REST `RankSearchRequest`.
#[derive(Clone, PartialEq, Message)]
pub struct RankSearchRequest {
    #[prost(string, tag = "1")]
    pub collection: ::prost::alloc::string::String,
    #[prost(float, repeated, tag = "2")]
    pub query_vector: ::prost::alloc::vec::Vec<f32>,
    #[prost(string, optional, tag = "3")]
    pub query_text: ::core::option::Option<::prost::alloc::string::String>,
    #[prost(uint32, tag = "4")]
    pub k: u32,
    #[prost(string, optional, tag = "5")]
    pub rank_profile: ::core::option::Option<::prost::alloc::string::String>,
    #[prost(message, optional, tag = "6")]
    pub rank_overrides: ::core::option::Option<RankOverrides>,
}

/// One scored hit on the wire.
#[derive(Clone, PartialEq, Message)]
pub struct ScoredHit {
    #[prost(string, tag = "1")]
    pub id: ::prost::alloc::string::String,
    #[prost(float, tag = "2")]
    pub score: f32,
    #[prost(message, optional, tag = "3")]
    pub score_vector: ::core::option::Option<ScoreVector>,
    #[prost(map = "string, double", tag = "4")]
    pub match_features: ::std::collections::HashMap<::prost::alloc::string::String, f64>,
    #[prost(map = "string, double", tag = "5")]
    pub summary_features: ::std::collections::HashMap<::prost::alloc::string::String, f64>,
}

/// Rank pipeline response.
#[derive(Clone, PartialEq, Message)]
pub struct RankSearchResponse {
    #[prost(message, repeated, tag = "1")]
    pub hits: ::prost::alloc::vec::Vec<ScoredHit>,
    #[prost(bool, tag = "2")]
    pub phase_truncated: bool,
    #[prost(string, optional, tag = "3")]
    pub rank_profile: ::core::option::Option<::prost::alloc::string::String>,
    #[prost(uint32, optional, tag = "4")]
    pub rank_profile_version: ::core::option::Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn score_component_round_trips_through_prost() {
        let c = ScoreComponent {
            name: "bm25(title)".into(),
            value: 12.4,
            weight: 0.4,
            contribution: 4.96,
        };
        let bytes = c.encode_to_vec();
        let back = ScoreComponent::decode(bytes.as_slice()).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn score_vector_round_trips_with_components() {
        let sv = ScoreVector {
            primary: 0.876,
            phase: 2,
            components: vec![
                ScoreComponent {
                    name: "bm25(title)".into(),
                    value: 12.4,
                    weight: 0.4,
                    contribution: 4.96,
                },
                ScoreComponent {
                    name: "model(rerank-v3)".into(),
                    value: 0.87,
                    weight: 1.0,
                    contribution: 0.87,
                },
            ],
        };
        let bytes = sv.encode_to_vec();
        let back = ScoreVector::decode(bytes.as_slice()).unwrap();
        assert_eq!(sv, back);
    }

    #[test]
    fn score_vector_empty_components_round_trips() {
        let sv = ScoreVector {
            primary: 0.5,
            phase: 0,
            components: Vec::new(),
        };
        let bytes = sv.encode_to_vec();
        let back = ScoreVector::decode(bytes.as_slice()).unwrap();
        assert_eq!(sv, back);
        assert!(back.components.is_empty());
    }

    #[test]
    fn rank_search_request_round_trips_minimal() {
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![0.1, 0.2, 0.3],
            query_text: None,
            k: 10,
            rank_profile: None,
            rank_overrides: None,
        };
        let bytes = req.encode_to_vec();
        let back = RankSearchRequest::decode(bytes.as_slice()).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn rank_search_request_round_trips_full() {
        let req = RankSearchRequest {
            collection: "docs".into(),
            query_vector: vec![0.1, 0.2, 0.3, 0.4],
            query_text: Some("laptop computer".into()),
            k: 50,
            rank_profile: Some("semantic_plus_ce".into()),
            rank_overrides: Some(RankOverrides {
                second_phase: Some(PhaseOverride {
                    rerank_count: Some(200),
                    batch_size: Some(32),
                }),
                global_phase: Some(PhaseOverride {
                    rerank_count: Some(50),
                    batch_size: None,
                }),
            }),
        };
        let bytes = req.encode_to_vec();
        let back = RankSearchRequest::decode(bytes.as_slice()).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn scored_hit_round_trips_with_features() {
        let mut match_features = HashMap::new();
        match_features.insert("bm25(title)".into(), 12.4);
        match_features.insert("closeness(embedding)".into(), 0.91);
        let hit = ScoredHit {
            id: "doc:abc".into(),
            score: 0.876,
            score_vector: Some(ScoreVector {
                primary: 0.876,
                phase: 1,
                components: vec![],
            }),
            match_features,
            summary_features: HashMap::new(),
        };
        let bytes = hit.encode_to_vec();
        let back = ScoredHit::decode(bytes.as_slice()).unwrap();
        assert_eq!(hit, back);
    }

    #[test]
    fn rank_search_response_round_trips() {
        let resp = RankSearchResponse {
            hits: vec![
                ScoredHit {
                    id: "doc:1".into(),
                    score: 0.9,
                    score_vector: None,
                    match_features: HashMap::new(),
                    summary_features: HashMap::new(),
                },
                ScoredHit {
                    id: "doc:2".into(),
                    score: 0.7,
                    score_vector: None,
                    match_features: HashMap::new(),
                    summary_features: HashMap::new(),
                },
            ],
            phase_truncated: false,
            rank_profile: Some("test".into()),
            rank_profile_version: Some(7),
        };
        let bytes = resp.encode_to_vec();
        let back = RankSearchResponse::decode(bytes.as_slice()).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn phase_overrides_skip_absent_fields_on_wire() {
        // Verify prost omits absent optional fields — the encoded bytes
        // should be smaller for empty than for fully-populated.
        let empty = RankOverrides {
            second_phase: None,
            global_phase: None,
        };
        let full = RankOverrides {
            second_phase: Some(PhaseOverride {
                rerank_count: Some(100),
                batch_size: Some(32),
            }),
            global_phase: Some(PhaseOverride {
                rerank_count: Some(50),
                batch_size: Some(16),
            }),
        };
        assert!(empty.encode_to_vec().len() < full.encode_to_vec().len());
    }

    #[test]
    fn wire_tag_numbers_are_stable() {
        // Smoke test: encoding a known message yields a payload that
        // begins with the expected tag-byte sequence. If anyone
        // renumbers a field, this test trips and the change is caught
        // before it breaks deployed clients.
        let sv = ScoreVector {
            primary: 1.0,
            phase: 2,
            components: vec![],
        };
        let bytes = sv.encode_to_vec();
        // tag=1 field=primary (float): wire type 5 (fixed32), tag byte = 0x0D
        assert_eq!(
            bytes[0], 0x0D,
            "ScoreVector tag 1 (primary) must encode as wire byte 0x0D"
        );
        // After 4 fixed32 bytes, tag=2 field=phase (varint): wire type 0, tag byte = 0x10
        assert_eq!(
            bytes[5], 0x10,
            "ScoreVector tag 2 (phase) must encode as wire byte 0x10"
        );
    }

    #[test]
    fn empty_request_decodes_to_default() {
        // Empty bytes decode to a fully-defaulted request — protobuf
        // wire-compat property (clients that send nothing get sensible
        // defaults on the server).
        let req = RankSearchRequest::decode(&[][..]).unwrap();
        assert_eq!(req.collection, "");
        assert!(req.query_vector.is_empty());
        assert!(req.query_text.is_none());
        assert_eq!(req.k, 0);
        assert!(req.rank_profile.is_none());
        assert!(req.rank_overrides.is_none());
    }
}
