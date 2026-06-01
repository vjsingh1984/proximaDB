//! Arrow Flight contract gate for the Rust **connectors** that speak Flight
//! (today: Trino + DuckDB-bulk).
//!
//! Unlike the OpenAPI gate (which validates against the YAML spec), the
//! Flight contract is the union of:
//!
//!   - The `arrow-flight` crate's generated `FlightService` proto trait
//!     (pinned via `Cargo.toml`).
//!   - Custom JSON-encoded ticket shapes ProximaDB stamps into
//!     `Ticket.ticket`, e.g. [`ArrowFileTicket`] at
//!     `src/network/arrow_ipc/file_export.rs:639`.
//!   - `FlightDescriptor` path patterns the server's routing logic at
//!     `src/network/arrow_ipc/service.rs:313-342` recognizes
//!     (`["relational", table_fqn]`, `["vectors", collection_id]`, …).
//!
//! Per-method TDD tests for live Flight calls land in C7 (Trino pilot).
//! This file's smoke gate just proves the ticket-shape contract surfaces
//! we'll validate against.

use arrow_flight::{FlightDescriptor, Ticket};
use proximadb::network::arrow_ipc::file_export::ArrowFileTicket;

// ---------------------------------------------------------------------------
// Smoke test — proves the ArrowFileTicket detector recognizes a known-good
// JSON blob. Per-shape contract tests (descriptor patterns, ticket round
// trip) land in C7.
// ---------------------------------------------------------------------------

#[test]
fn helpers_arrow_file_ticket_detector_recognizes_known_shape() {
    let raw = br#"{"type":"arrow_file","collection_id":"c1","file_path":"/tmp/x.arrow"}"#;
    let ticket = Ticket {
        ticket: raw.to_vec().into(),
    };
    assert!(ArrowFileTicket::is_arrow_file_ticket(&ticket));
}

#[test]
fn helpers_arrow_file_ticket_detector_rejects_other_shapes() {
    let raw = br#"{"type":"sql","statement":"SELECT 1"}"#;
    let ticket = Ticket {
        ticket: raw.to_vec().into(),
    };
    assert!(!ArrowFileTicket::is_arrow_file_ticket(&ticket));
}

// ---------------------------------------------------------------------------
// ArrowFileTicket roundtrip — the JSON shape Trino's DoGet codepath needs
// when the server returns split tickets from GetFlightInfo.
// ---------------------------------------------------------------------------

#[test]
fn arrow_file_ticket_json_roundtrips() {
    let original = ArrowFileTicket {
        ticket_type: "arrow_file".to_string(),
        collection_id: "trino_col".to_string(),
        file_path: "/data/segment-001.arrow".to_string(),
        compression: None,
    };
    let bytes = serde_json::to_vec(&original).expect("serialize");
    let ticket = Ticket {
        ticket: bytes.into(),
    };
    assert!(ArrowFileTicket::is_arrow_file_ticket(&ticket));
    let parsed = ArrowFileTicket::from_ticket(&ticket).expect("parse");
    assert_eq!(parsed.collection_id, "trino_col");
    assert_eq!(parsed.file_path, "/data/segment-001.arrow");
}

// ---------------------------------------------------------------------------
// FlightDescriptor path patterns. The server at
// `src/network/arrow_ipc/service.rs:313-342` recognizes two `path` shapes
// when routing a GetSchema / GetFlightInfo request:
//
//   ["relational" | "table" | "sql", <table_fqn>]
//   ["vectors", <collection_id>]
//
// Trino's metadata calls (flight_get_table_schema, flight_get_splits) must
// build descriptors in one of these shapes. The contract gate constructs
// each shape and asserts the proto-level descriptor matches expectation.
// ---------------------------------------------------------------------------

#[test]
fn flight_descriptor_relational_path_shape() {
    let desc = FlightDescriptor::new_path(vec!["relational".into(), "tenant1.users".into()]);
    assert_eq!(
        desc.path,
        vec!["relational".to_string(), "tenant1.users".to_string()]
    );
    // First element must be one of the model-router prefixes the server recognizes.
    let head = desc.path.first().map(String::as_str);
    assert!(matches!(head, Some("relational")));
}

#[test]
fn flight_descriptor_vectors_path_shape() {
    let desc = FlightDescriptor::new_path(vec!["vectors".into(), "embeddings".into()]);
    assert_eq!(
        desc.path,
        vec!["vectors".to_string(), "embeddings".to_string()]
    );
    let head = desc.path.first().map(String::as_str);
    assert!(matches!(head, Some("vectors")));
}
