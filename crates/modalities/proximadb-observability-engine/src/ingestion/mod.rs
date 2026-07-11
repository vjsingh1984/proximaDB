//! Observability ingestion modules extracted to the engine crate (Slice 2).
//!
//! The ingestion *facade* (`ObservabilityIngester`) and the format adapters
//! remain in the root `src/observability/ingestion/` (the OTLP adapter couples
//! to the `ObservabilityService` facade) and re-export these foundation-pure
//! modules.
//!
//! Modules here:
//! - **`buffer`** — `RingBuffer` for ingestion backpressure.
//! - **`parser`** — `LogParser` multi-format log parsing.

pub mod buffer;
pub mod parser;
