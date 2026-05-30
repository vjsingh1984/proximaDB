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

#![allow(unused_imports)] // helpers brought in for follow-up commits

use arrow_flight::Ticket;
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
